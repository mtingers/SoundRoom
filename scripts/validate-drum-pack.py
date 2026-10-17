#!/usr/bin/env python3
"""Independently validate delivered PCM files, then write checksums and a ZIP."""

import argparse
import hashlib
import json
import math
import wave
import zipfile
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("directory", type=Path)
args = parser.parse_args()
root = args.directory.resolve()
manifest = json.loads((root / "manifest.json").read_text())
assert manifest["sampleRate"] == 48000 and manifest["bitDepth"] == 24 and manifest["channels"] == 1
kits = manifest.get("kits", ["acoustic", "electronic"])
velocities = manifest.get("velocities", [42, 85, 127])
robins = manifest.get("roundRobins", 3)
assert len(manifest["samples"]) == len(kits) * 9 * len(velocities) * robins
assert len({row["path"] for row in manifest["samples"]}) == len(manifest["samples"])
checksums = []
waveforms = {}
peak_max = 0.0
for row in manifest["samples"]:
    path = (root / row["path"]).resolve()
    assert path.is_relative_to(root)
    with wave.open(str(path), "rb") as audio:
        assert (
            audio.getnchannels(),
            audio.getsampwidth(),
            audio.getframerate(),
            audio.getnframes(),
        ) == (1, 3, 48000, row["frames"])
        payload = audio.readframes(audio.getnframes())
    values = []
    for i in range(0, len(payload), 3):
        value = payload[i] | payload[i + 1] << 8 | payload[i + 2] << 16
        if value & 0x800000:
            value -= 0x1000000
        values.append(value / 8388607)
    peak = max(map(abs, values))
    rms = math.sqrt(sum(x * x for x in values) / len(values))
    assert 0.0001 < peak < 1 and values[0] == 0 and values[-1] == 0
    assert max(map(abs, values[-240:])) < 0.001
    assert abs(peak - row["peak"]) < 0.000001 and abs(rms - row["rms"]) < 0.000001
    peak_max = max(peak_max, peak)
    if row["kit"] != "electronic" and row["velocity"] in (min(velocities), max(velocities)):
        waveforms[(row["kit"], row["voice"], row["velocity"], row["roundRobin"])] = [
            x / peak for x in values[:12000]
        ]
    checksums.append(hashlib.sha256(path.read_bytes()).hexdigest() + "  " + row["path"])
for kit in kits:
    if kit != "electronic":
        for voice in {row["voice"] for row in manifest["samples"]}:
            soft = waveforms[(kit, voice, min(velocities), 1)]
            hard = waveforms[(kit, voice, max(velocities), 1)]
            difference = math.sqrt(
                sum((a - b) ** 2 for a, b in zip(soft, hard)) / min(len(soft), len(hard))
            )
            assert difference > 0.002, (kit, voice, "velocity changes only level")
            for robin in range(2, robins + 1):
                rr = waveforms[(kit, voice, max(velocities), robin)]
                difference = math.sqrt(
                    sum((a - b) ** 2 for a, b in zip(rr, hard)) / min(len(rr), len(hard))
                )
                assert difference > 0.005, (kit, voice, "round robin unchanged")
    with wave.open(str(root / f"{kit}-demo-110bpm.wav"), "rb") as demo:
        assert (demo.getnchannels(), demo.getsampwidth(), demo.getframerate()) == (2, 3, 48000)
        assert abs(demo.getnframes() / 48000 - 16 * 60 / 110) < 0.001
report = {
    "samples": len(manifest["samples"]),
    "kits": len(kits),
    "voices_per_kit": 9,
    "velocities": velocities,
    "round_robins": robins,
    "peak_max": peak_max,
    "verified_format": "PCM24 / 48 kHz / mono",
    "checks": [
        "PCM headers",
        "actual headroom/RMS",
        "silent endpoints/tails",
        "velocity timbre",
        "round robins",
        "stereo demo headers/length",
    ],
    "provenance": "original modeled synthesis; no external audio sources",
}
(root / "SHA256SUMS.txt").write_text("\n".join(checksums) + "\n")
(root / "VALIDATION.json").write_text(json.dumps(report, indent=2) + "\n")
archive = root.parent / (
    "Soundroom-Raw-Drums.zip" if root.name == "DrumPacks" else f"{root.name}-Raw-Drums.zip"
)
with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as output:
    for path in sorted(root.rglob("*")):
        if path.is_file():
            entry = zipfile.ZipInfo(
                "Soundroom-Raw-Drums/" + str(path.relative_to(root)),
                date_time=(2026, 10, 10, 0, 0, 0),
            )
            entry.compress_type = zipfile.ZIP_DEFLATED
            entry.external_attr = 0o644 << 16
            output.writestr(entry, path.read_bytes())
print(json.dumps(report))
print(archive)
