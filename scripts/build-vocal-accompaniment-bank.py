#!/usr/bin/env python3
"""Render reusable original four-bar instrument stems for online vocal mixtures."""

import argparse
import copy
import hashlib
import importlib.util
import json
import random
import subprocess
import tempfile
from pathlib import Path

import numpy as np
import soundfile as sf

spec = importlib.util.spec_from_file_location(
    "native", Path(__file__).with_name("generate-separator-corpus.py")
)
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)


def long_plan(split, index, seed):
    plan = native.make_plan(split, index, seed)
    rng = random.Random(f"long/{seed}/{split}/{index}")
    for job in plan["assets"]:
        if "drums" in job:
            pattern = job["drums"]
            pattern["bars"] = 4
            pattern["steps"] = [
                dict(
                    step,
                    index=step["index"] + 16 * bar,
                    velocity=step["velocity"] * rng.uniform(0.85, 1),
                )
                for bar in range(4)
                for step in pattern["steps"]
            ]
        else:
            pattern = job["pattern"]
            pattern["beats"] = 16
            pattern["baseGain"] = 0.25
            pattern["notes"] = [
                dict(
                    note,
                    id=native.uid(f"long/{seed}/{split}/{index}/{job['id']}/{bar}/{ni}"),
                    startBeat=note["startBeat"] + 4 * bar,
                    pitch=min(127, note["pitch"] + [0, 5, 7, 3][bar]),
                    velocity=note["velocity"] * rng.uniform(0.8, 1),
                )
                for bar in range(4)
                for ni, note in enumerate(pattern["notes"])
            ]
    # Voice-like nonhuman timbres are deliberate negative examples.
    distractor = copy.deepcopy(plan["assets"][-1])
    distractor["id"] = native.uid(f"distractor/{seed}/{split}/{index}")
    distractor["name"] = "Non-vocal distractor"
    distractor["pattern"]["instrument"] = rng.choice(
        ["woodenFlute", "soloCello", "stringEnsemble", "tonewheelOrgan", "vocalEnsemble"]
    )
    track = copy.deepcopy(plan["session"]["tracks"][-1])
    track["id"] = native.uid(distractor["id"] + "/track")
    track["name"] = distractor["name"]
    track["clips"][0].update(id=native.uid(distractor["id"] + "/clip"), assetID=distractor["id"])
    plan["assets"].append(distractor)
    plan["session"]["tracks"].append(track)
    return plan


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=native.ROOT / "build/Training/vocal-bank-v3")
    parser.add_argument("--train", type=int, default=64)
    parser.add_argument("--validation", type=int, default=16)
    parser.add_argument("--test", type=int, default=16)
    parser.add_argument("--seed", type=int, default=20261018)
    parser.add_argument(
        "--renderer",
        type=Path,
        default=native.ROOT / "portable/target/release/examples/render_arrangement",
    )
    parser.add_argument(
        "--kits",
        type=Path,
        default=Path.home() / "Library/Application Support/Soundroom/RustServer/Kits",
    )
    args = parser.parse_args()
    if any(not 1 <= n <= 10000 for n in [args.train, args.validation, args.test]):
        parser.error("Invalid bank counts")
    args.output.mkdir(parents=True, exist_ok=True)
    identity = dict(
        seed=args.seed,
        renderer=native.sha(args.renderer),
        generator=native.sha(Path(__file__)),
        helper=native.sha(Path(native.__file__)),
    )
    fingerprint = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    records = []
    for split in ["train", "validation", "test"]:
        for index in range(getattr(args, split)):
            directory = args.output / split / f"{index:04}"
            metadata = directory / "bank.json"
            plan = long_plan(split, index, args.seed)
            if metadata.exists():
                record = json.loads(metadata.read_text())
                if record["generationFingerprint"] != fingerprint:
                    raise ValueError("Bank provenance changed; choose fresh output")
                for name, digest in record["sha256"].items():
                    if native.sha(directory / name) != digest:
                        raise ValueError("Cached bank stem changed")
            else:
                with tempfile.TemporaryDirectory(prefix=".bank-", dir=args.output) as temporary:
                    temporary = Path(temporary)
                    job = temporary / "plan.json"
                    job.write_text(json.dumps(plan))
                    process = subprocess.run(
                        [
                            str(args.renderer),
                            str(job),
                            str(temporary / "source.soundroom"),
                            str(temporary / "mix.wav"),
                            "--kits",
                            str(args.kits),
                            "--stems",
                            str(temporary / "stems"),
                        ],
                        capture_output=True,
                        text=True,
                        timeout=120,
                    )
                    if process.returncode or "not installed" in process.stderr:
                        raise RuntimeError(process.stdout + process.stderr)
                    stems = np.stack(
                        [
                            sf.read(
                                temporary / "stems" / f"{i:02}.wav", dtype="float32", always_2d=True
                            )[0]
                            for i in range(5)
                        ]
                    )
                    if not np.isfinite(stems).all():
                        raise ValueError("Nonfinite bank audio")
                    stems *= min(1, 0.8 / max(1e-7, float(np.max(np.abs(stems)))))
                    directory.mkdir(parents=True, exist_ok=True)
                    hashes = {}
                    for i in range(5):
                        path = directory / f"part{i}.flac"
                        sf.write(path, stems[i], 48000, subtype="PCM_24")
                        hashes[path.name] = native.sha(path)
                    record = dict(
                        schema=1,
                        split=split,
                        scoreID=native.uid(f"vocal-bank/{args.seed}/{split}/{index}"),
                        sampleRate=48000,
                        frames=stems.shape[1],
                        generationFingerprint=fingerprint,
                        humanVocals=False,
                        sha256=hashes,
                        plan=plan,
                    )
                    metadata.write_text(json.dumps(record, indent=2) + "\n")
            records.append(
                dict(
                    path=f"{split}/{index:04}",
                    split=split,
                    scoreID=record["scoreID"],
                    metadataSHA256=native.sha(metadata),
                )
            )
            print(
                f"{split} {index + 1}/{getattr(args, split)}: {record['frames'] / 48000:.1f}s",
                flush=True,
            )
    manifest = dict(
        schema=1,
        fingerprint=fingerprint,
        sources=identity,
        scenes=records,
        attribution="Soundroom original scores and DSP; CC0 VCSL/VSCO/Karoryfer instruments. DRSKit CC-BY-4.0: Drum samples provided by DrumGizmo.org. See THIRD_PARTY_NOTICES.md.",
    )
    (args.output / "bank.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (args.output / "ATTRIBUTION.txt").write_text(manifest["attribution"] + "\n")
    print(f"Ready: {args.output / 'bank.json'}", flush=True)


if __name__ == "__main__":
    main()
