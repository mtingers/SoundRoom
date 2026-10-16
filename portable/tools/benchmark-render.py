#!/usr/bin/env python3
"""Measure the real portable offline worker with a long sequential arrangement.

This includes file I/O and is never callback deadline/hardware qualification.
Build first: cargo build --release --locked -p soundroom-server
"""
import argparse
import json
import math
from pathlib import Path
try:
    import resource
except ImportError:
    resource = None
import shutil
import subprocess
import sys
import tempfile
import time
import uuid

parser = argparse.ArgumentParser()
parser.add_argument("--case", default="room-dense")
parser.add_argument("--seconds", type=float, default=180)
parser.add_argument("--binary", type=Path)
args = parser.parse_args()
if not math.isfinite(args.seconds) or not 0 < args.seconds <= 3600:
    parser.error("Duration must be positive and at most one hour")
root = Path(__file__).resolve().parents[1]
source = root / "fixtures" / (args.case + ".soundroom")
if not source.is_dir():
    parser.error("Unknown fixture")
binary = args.binary or root / "target" / "release" / "soundroom-server"
if sys.platform == "win32" and args.binary is None:
    binary = binary.with_suffix(".exe")
frames = round(args.seconds * 48000)
with tempfile.TemporaryDirectory(prefix="soundroom-render-benchmark-") as temporary:
    directory = Path(temporary)
    project = directory / "Benchmark.soundroom"
    shutil.copytree(source, project)
    document = json.loads((project / "session.json").read_text())
    for track_index, track in enumerate(document["tracks"]):
        template = track["clips"][0]
        length = template["length"]
        clips = []
        for index, start in enumerate(range(0, frames, length)):
            clip = dict(template)
            clip["id"] = str(uuid.UUID(int=(track_index + 1) * (1 << 64) + index + 1))
            clip["start"] = start
            clip["length"] = min(length, frames - start)
            clips.append(clip)
        track["clips"] = clips
    (project / "session.json").write_text(json.dumps(document))
    started = time.perf_counter()
    result = subprocess.run([str(binary), "render", str(project), str(directory / "mix.wav")],
                            check=True, capture_output=True, text=True)
    elapsed = time.perf_counter() - started
    # macOS reports bytes; Linux reports KiB. Windows resource module is unavailable.
    peak = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss if resource else None
    peak_mib = peak / (1024 * 1024 if sys.platform == "darwin" else 1024) if peak is not None else None
    print(json.dumps({"case": args.case, "audio_seconds": frames / 48000,
                      "clips": sum(len(t["clips"]) for t in document["tracks"]),
                      "worker_seconds": elapsed, "audio_seconds_per_worker_second": frames / 48000 / elapsed,
                      "child_peak_resident_mib": peak_mib,
                      "scope": "offline worker including file I/O; not callback/hardware qualification"}, indent=2))
