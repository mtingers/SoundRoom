#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ $# -ne 1 || ! -x "$1" ]]; then
    echo 'Usage: scripts/build-portable-probe.sh /absolute/path/to/soundroom_server-library-test-binary' >&2
    exit 2
fi
case "$1" in
    /*) ;;
    *)
        echo 'The test binary path must be absolute.' >&2
        exit 2
        ;;
esac
task_app="$task_root/build/SoundroomPortableProbe.app"
mkdir -p "$task_app/Contents/MacOS"
python3 - "$task_root" "$task_app" "$1" <<'PY'
import pathlib, plistlib, sys

root, app, binary = map(pathlib.Path, sys.argv[1:])
configuration = {
    "CFBundleIdentifier": "com.soundroom.portable-probe",
    "CFBundleExecutable": "SoundroomPortableProbe",
    "CFBundleName": "SoundroomPortableProbe",
    "CFBundlePackageType": "APPL",
    "LSMinimumSystemVersion": "14.0",
    "LSUIElement": True,
    "SoundroomPortableDiagnostic": {
        "testBinary": str(binary),
        "testSelector": "service::request_acceptance::verified_wss_stop_while_large_authored_candidate_is_blocked",
        "tlsDirectory": str(root / "build/CrossPlatformBaseline/web-tls"),
        "output": str(root / "build/QA/portable-live-ordered-control-returned-session.log"),
    },
}
(app / "Contents/Info.plist").write_bytes(plistlib.dumps(configuration))
PY
clang -fobjc-arc -Wall -Wextra -Werror -framework AppKit -framework Foundation \
    "$task_root/scripts/NativePortableDiagnostic.m" -o "$task_app/Contents/MacOS/SoundroomPortableProbe"
codesign --force --sign - --identifier com.soundroom.portable-probe "$task_app"
echo "Prepared $task_app; launch it through Finder and inspect the QA log/JSON result."
