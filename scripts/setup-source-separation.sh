#!/bin/bash
set -euo pipefail

# Explicit installation: this script is the only workflow that downloads Python
# packages or provider model weights. Normal Soundroom separation is offline.
script_dir="$(cd "$(dirname "$0")" && pwd)"
project_dir="$(cd "$script_dir/.." && pwd)"
runtime_dir="$project_dir/build/SeparationRuntime"
model_set="quality"
python_bin="${SOUNDROOM_SEPARATION_PYTHON:-python3.11}"

while (($#)); do
    case "$1" in
        --runtime-dir|--models|--python)
            if (($# < 2)); then printf 'Missing value for %s\n' "$1" >&2; exit 2; fi
            case "$1" in
                --runtime-dir) runtime_dir="$2" ;;
                --models) model_set="$2" ;;
                --python) python_bin="$2" ;;
            esac
            shift 2 ;;
        --help)
            printf '%s\n' "Usage: $0 [--runtime-dir PATH] [--models quality|standard|extended|all] [--python PYTHON3.11]"
            printf '%s\n' 'Installs an isolated Python runtime and downloads official Demucs weights.'
            printf '%s\n' 'Code: MIT. Weights: scientific purposes per upstream; general/commercial rights unconfirmed.'
            printf '%s\n' 'See THIRD_PARTY_NOTICES.md and https://github.com/facebookresearch/demucs/issues/327.'
            exit 0 ;;
        *) printf 'Unknown argument: %s\n' "$1" >&2; exit 2 ;;
    esac
done
case "$model_set" in quality|standard|extended|all) ;; *) printf 'Invalid model set\n' >&2; exit 2 ;; esac
printf '%s\n' 'Demucs code is MIT; pretrained weights have separate, unresolved licensing.' >&2
printf '%s\n' 'Upstream provides weights only for scientific purposes; see THIRD_PARTY_NOTICES.md.' >&2
"$python_bin" -c 'import sys; assert sys.version_info[:2] == (3, 11), "Python 3.11 is required for the pinned runtime"'
mkdir -p "$runtime_dir"
runtime_dir="$(cd "$runtime_dir" && pwd)"
"$python_bin" -c 'from pathlib import Path; import sys; Path(sys.argv[1]).unlink(missing_ok=True)' "$runtime_dir/ready.json"
if [[ ! -x "$runtime_dir/.venv/bin/python" ]]; then
    "$python_bin" -m venv "$runtime_dir/.venv"
fi
runtime_python="$runtime_dir/.venv/bin/python"
"$runtime_python" -c 'import sys; assert sys.version_info[:2] == (3, 11), "The existing runtime must use Python 3.11"'
"$runtime_python" -m pip install --disable-pip-version-check --no-cache-dir \
    'torch==2.0.1' 'torchaudio==2.0.2' 'demucs==4.0.1' 'numpy==1.26.4' 'soundfile==0.12.1'
"$runtime_python" -m pip freeze > "$runtime_dir/requirements-resolved.txt"
"$runtime_python" - "$script_dir/source-separation-worker.py" "$runtime_dir/worker.py" <<'PY'
import os
from pathlib import Path
import sys
import tempfile
source, destination = map(Path, sys.argv[1:])
fd, name = tempfile.mkstemp(prefix=".worker-", suffix=".py", dir=destination.parent)
temporary = Path(name)
try:
    with os.fdopen(fd, "wb") as output:
        output.write(source.read_bytes())
        output.flush()
        os.fsync(output.fileno())
    temporary.chmod(0o755)
    os.replace(temporary, destination)
finally:
    temporary.unlink(missing_ok=True)
PY
"$runtime_python" -I "$runtime_dir/worker.py" --prepare-models --models-dir "$runtime_dir/models" --model "$model_set" --ready-file "$runtime_dir/ready.json"
"$runtime_python" -I "$runtime_dir/worker.py" --status --models-dir "$runtime_dir/models"
printf 'Soundroom source separation is ready at %s\n' "$runtime_dir"
