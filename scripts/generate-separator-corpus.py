#!/usr/bin/env python3
"""Original seeded scores -> Soundroom's native renderer -> additive wet stems.

Research pilot, four instrument families; no vocals or pretrained model weights.
Requires numpy/soundfile (the optional separation venv already supplies these).
"""

import argparse
import hashlib
import json
import random
import subprocess
import tempfile
import uuid
from pathlib import Path

import numpy as np
import soundfile as sf

ROOT = Path(__file__).resolve().parent.parent
CLASSES = ["drums", "bass", "guitar", "piano"]
MASTER = "E3E58100-5125-4E6F-B985-10AA11000001"


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def uid(key):
    return str(uuid.uuid5(uuid.NAMESPACE_URL, "soundroom-training-v1/" + key))


def make_plan(split, index, seed):
    key = f"{seed}/{split}/{index}"
    rng = random.Random(key)
    bpm = rng.uniform(85, 155)
    root = rng.randrange(12)
    scale = rng.choice([[0, 2, 4, 5, 7, 9, 11], [0, 2, 3, 5, 7, 8, 10]])
    assets, tracks = [], []
    # Entire score is generated inside its split, before effects/pitch variations.
    # Preset combinations differ; underlying sample banks are shared and disclosed.
    instruments = {
        "train": ["pickedBass", "electricGuitar", "studioPiano"],
        "validation": ["uprightBass", "acousticGuitar", "uprightPiano"],
        "test": ["pickedBass", "acousticGuitar", "uprightPiano"],
    }[split]
    kit = rng.choice(
        {
            "train": ["studioNatural", "machine808", "jazzMaple", "studioTight"],
            "validation": ["studioVintage", "machine909"],
            "test": ["studioRoom", "funkTight"],
        }[split]
    )
    for ci, family in enumerate(CLASSES):
        asset = uid(f"{key}/{family}/asset")
        if ci == 0:
            steps = []
            for voice, indices in [
                ("kick", [0, rng.choice([6, 8, 10])]),
                ("snare", [4, 12]),
                ("closedHat", list(range(0, 16, rng.choice([1, 2, 4])))),
            ]:
                for position in indices:
                    steps.append(
                        dict(
                            voice=voice,
                            index=position,
                            velocity=rng.uniform(0.45, 0.9),
                            pan=rng.uniform(-0.2, 0.2),
                            probability=1,
                        )
                    )
            job = dict(
                id=asset,
                name=family,
                drums=dict(kit=kit, bpm=bpm, bars=1, stepsPerBeat=4, masterGain=0.5, steps=steps),
            )
        else:
            notes = []
            spacing = rng.choice([0.25, 0.5, 1.0])
            for ni in range(int(4 / spacing)):
                if rng.random() < 0.15:
                    continue
                beat = ni * spacing + rng.uniform(0, 0.025)
                base = (36 if ci == 1 else rng.choice([48, 60])) + root
                chord = [0, 2, 4] if ci > 1 and rng.random() < 0.3 else [rng.randrange(7)]
                for degree in chord:
                    notes.append(
                        dict(
                            id=uid(f"{key}/{family}/{ni}/{degree}"),
                            pitch=base + scale[degree],
                            velocity=rng.uniform(0.4, 0.85),
                            startBeat=beat,
                            durationBeats=min(4 - beat, spacing * rng.uniform(0.45, 1.4)),
                        )
                    )
            # Native pitch variation changes note pitches, avoiding resampling artifacts.
            pattern = dict(
                notes=notes,
                instrument=instruments[ci - 1],
                bpm=bpm,
                beats=4,
                baseGain=0.4,
                fineTune=rng.uniform(-8, 8),
                release=rng.uniform(0.04, 0.3),
            )
            job = dict(id=asset, name=family, pattern=pattern)
        assets.append(job)
        effects = []

        def effect(kind, parameters):
            effects.append(
                dict(
                    id=uid(f"{key}/{family}/fx/{kind}"),
                    kind=kind,
                    enabled=True,
                    parameters=parameters,
                )
            )

        if rng.random() < 0.65:
            effect(
                "equalizer",
                dict(
                    lowGain=rng.uniform(-4, 4),
                    midGain=rng.uniform(-5, 4),
                    midFrequency=rng.uniform(300, 4000),
                    highGain=rng.uniform(-4, 3),
                ),
            )
        if rng.random() < 0.4:
            effect(
                "room",
                dict(
                    mix=rng.uniform(0.03, 0.2),
                    decay=rng.uniform(0.2, 0.9),
                    predelay=rng.uniform(0, 25),
                ),
            )
        if ci > 0 and rng.random() < 0.2:
            effect(
                "chorus",
                dict(
                    mix=rng.uniform(0.05, 0.2), depth=rng.uniform(0.5, 4), rate=rng.uniform(0.2, 2)
                ),
            )
        if rng.random() < 0.2:
            effect("warmth", dict(drive=rng.uniform(1, 6), mix=rng.uniform(0.1, 0.4), output=-3))
        tracks.append(
            dict(
                id=uid(f"{key}/{family}/track"),
                name=family,
                gain=0.6,
                pan=rng.uniform(-0.7, 0.7),
                realtimeEffects=effects,
                clips=[
                    dict(
                        id=uid(f"{key}/{family}/clip"),
                        assetID=asset,
                        name=family,
                        start=0,
                        offset=0,
                        length=1,
                        edgeProtectionDisabled=True,
                    )
                ],
            )
        )
    return dict(
        session=dict(
            version=9,
            name=f"Original separator pilot {key}",
            timing=dict(tempo=bpm, beatsPerBar=4, beatUnit=4),
            tracks=tracks,
            masterTrack=dict(id=MASTER, name="Master", gain=1, clips=[], realtimeEffects=[]),
        ),
        assets=assets,
        tailSeconds=0.5,
    )


def render_scene(output, split, index, seed, renderer, kits, fingerprint):
    destination = output / split / f"{index:04}"
    if (destination / "scene.json").exists():
        record = json.loads((destination / "scene.json").read_text())
        if (
            record["seed"] != seed
            or record["plan"] != make_plan(split, index, seed)
            or record.get("generationFingerprint") != fingerprint
        ):
            raise RuntimeError(
                "Cached scene uses different generation provenance; choose a fresh --output directory"
            )
        for name, digest in record["sha256"].items():
            if sha(destination / name) != digest:
                raise RuntimeError(f"Cached stem failed verification: {destination / name}")
        return record
    plan = make_plan(split, index, seed)
    with tempfile.TemporaryDirectory(prefix=".render-", dir=output) as temporary:
        temporary = Path(temporary)
        plan_path = temporary / "plan.json"
        plan_path.write_text(json.dumps(plan))
        result = subprocess.run(
            [
                str(renderer),
                str(plan_path),
                str(temporary / "source.soundroom"),
                str(temporary / "mix.wav"),
                "--kits",
                str(kits),
                "--stems",
                str(temporary / "stems"),
            ],
            capture_output=True,
            text=True,
            timeout=120,
        )
        if result.returncode or "not installed" in result.stderr:
            raise RuntimeError(result.stdout + result.stderr)
        stems = []
        for ci in range(4):
            audio, rate = sf.read(
                temporary / "stems" / f"{ci:02}.wav", dtype="float32", always_2d=True
            )
            if rate != 48000 or audio.shape[1] != 2 or not np.isfinite(audio).all():
                raise RuntimeError("Native renderer returned invalid audio")
            stems.append(audio)
        stems = np.stack(stems)
        mixture, _ = sf.read(temporary / "mix.wav", dtype="float32", always_2d=True)
        error = float(np.max(np.abs(stems.sum(0) - mixture)))
        if error > 1e-5:
            raise RuntimeError(f"Master/stem alignment mismatch: {error}")
        rng = random.Random(f"levels/{seed}/{split}/{index}")
        # Family balancing and negative examples. Effects remain in target stems.
        active = [rng.random() > 0.15 for _ in CLASSES]
        active[rng.randrange(4)] = True
        for ci in range(4):
            rms = float(np.sqrt(np.mean(stems[ci] ** 2)))
            if rms < 1e-7:
                raise RuntimeError(f"Silent generated source: {CLASSES[ci]}")
            stems[ci] *= (10 ** (rng.uniform(-29, -21) / 20) / rms) if active[ci] else 0
        stems *= min(1, 0.85 / max(1e-7, float(np.max(np.abs(stems.sum(0))))))
        destination.mkdir(parents=True, exist_ok=True)
        hashes = {}
        for ci, family in enumerate(CLASSES):
            path = destination / f"{family}.flac"
            sf.write(path, stems[ci], 48000, subtype="PCM_24")
            hashes[path.name] = sha(path)
        record = dict(
            schema=1,
            split=split,
            scoreID=uid(f"score/{seed}/{split}/{index}"),
            seed=seed,
            generationFingerprint=fingerprint,
            sampleRate=48000,
            frames=stems.shape[1],
            classes=CLASSES,
            active=active,
            nativeMixMaxError=error,
            sha256=hashes,
            plan=plan,
        )
        (destination / "scene.json").write_text(json.dumps(record, indent=2) + "\n")
        return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "build/Training/soundroom-pilot-v1")
    parser.add_argument("--train", type=int, default=80)
    parser.add_argument("--validation", type=int, default=16)
    parser.add_argument("--test", type=int, default=16)
    parser.add_argument("--seed", type=int, default=20261015)
    parser.add_argument(
        "--renderer",
        type=Path,
        default=ROOT / "portable/target/release/examples/render_arrangement",
    )
    parser.add_argument(
        "--kits",
        type=Path,
        default=Path.home() / "Library/Application Support/Soundroom/RustServer/Kits",
    )
    args = parser.parse_args()
    if any(n < 1 or n > 10000 for n in (args.train, args.validation, args.test)):
        parser.error("Each split needs 1–10000 independent scenes")
    args.output.mkdir(parents=True, exist_ok=True)
    fingerprint = hashlib.sha256(
        json.dumps(
            dict(seed=args.seed, renderer=sha(args.renderer), generator=sha(Path(__file__))),
            sort_keys=True,
        ).encode()
    ).hexdigest()
    records = []
    for split in ("train", "validation", "test"):
        for index in range(getattr(args, split)):
            record = render_scene(
                args.output, split, index, args.seed, args.renderer, args.kits, fingerprint
            )
            records.append(
                dict(
                    path=f"{split}/{index:04}",
                    scoreID=record["scoreID"],
                    split=split,
                    metadataSHA256=sha(args.output / split / f"{index:04}" / "scene.json"),
                )
            )
            print(
                f"{split} {index + 1}/{getattr(args, split)}: {record['frames'] / 48000:.2f}s",
                flush=True,
            )
    manifest = dict(
        schema=1,
        seed=args.seed,
        sampleRate=48000,
        classes=CLASSES,
        scenes=records,
        rendererSHA256=sha(args.renderer),
        voicesTrained=False,
        limitations="Synthetic short scores; shared source sample banks. Not a real-music benchmark.",
        attribution="Soundroom original scores/DSP; CC0 instrument samples from Versilian VCSL/VSCO and Karoryfer; DRSKit samples CC-BY-4.0, Drum samples provided by DrumGizmo.org. See THIRD_PARTY_NOTICES.md.",
    )
    partial = args.output / "corpus.json.part"
    partial.write_text(json.dumps(manifest, indent=2) + "\n")
    partial.replace(args.output / "corpus.json")
    print(f"Ready: {args.output / 'corpus.json'}", flush=True)


if __name__ == "__main__":
    main()
