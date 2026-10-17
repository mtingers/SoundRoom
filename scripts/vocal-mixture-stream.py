#!/usr/bin/env python3
"""Split-safe, deterministic online vocal/accompaniment mixtures; no mixture WAV cache."""

import argparse
import csv
import hashlib
import importlib.util
import json
import random
import subprocess
import tempfile
from pathlib import Path

import numpy as np
import soundfile as sf
import torch
from torchaudio.functional import resample


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


recordings = module("recordings", "generate-separator-recordings.py")
native = recordings.native
ROOT, RATE = native.ROOT, 48000
CATEGORIES = ["no_vocals", "no_vocals", "solo_vocals", "hard", "mixed", "mixed", "mixed", "mixed"]


def checked_path(root, relative):
    path = (root / relative).resolve()
    if not path.is_relative_to(root.resolve()):
        raise ValueError("Manifest path escapes its root")
    return path


def verify_manifest(root, name="sources.json"):
    manifest = json.loads((root / name).read_text())
    for record in manifest["files"]:
        path = checked_path(root, record["path"])
        if path.stat().st_size != record["bytes"] or native.sha(path) != record["sha256"]:
            raise ValueError(f"Source integrity failed: {path}")
    return manifest


def prepare_voices(args):
    """Cache dry + one Soundroom EQ/room/compressor variant per original performance."""
    sources = recordings.verify_sources(args.data)
    _, singers, _ = recordings.assign_groups(args.data, sources, 20261016)
    identity = dict(
        sources=native.sha(args.data / "sources.json"),
        renderer=native.sha(args.renderer),
        script=native.sha(Path(__file__)),
        splitSeed=20261016,
    )
    fingerprint = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    args.output.mkdir(parents=True, exist_ok=True)
    rows = []
    for index, record in enumerate(
        r for r in sources["files"] if r["dataset"] == "vocalset" and r["path"].endswith(".wav")
    ):
        split = next(s for s, names in singers.items() if record["group"] in names)
        folder = args.output / f"{index:03}"
        meta = folder / "voice.json"
        if meta.exists():
            row = json.loads(meta.read_text())
            if row["fingerprint"] != fingerprint:
                raise ValueError("Voice bank changed; choose a fresh output")
            for name, sha in row["sha256"].items():
                if native.sha(folder / name) != sha:
                    raise ValueError("Voice cache integrity failed")
        else:
            raw, rate = sf.read(args.data / record["path"], dtype="float32", always_2d=True)
            if raw.shape[1] not in [1, 2] or not np.isfinite(raw).all():
                raise ValueError("Invalid voice source")
            if rate != RATE:
                raw = recordings.resample_audio(raw, rate, RATE)
            if raw.shape[1] == 1:
                raw = np.repeat(raw, 2, axis=1)
            raw *= min(1, 0.7 / max(1e-8, float(np.abs(raw).max())))
            rng = random.Random("voice-effects/" + record["sha256"])
            asset = native.uid(record["path"])
            effects = []
            for kind, parameters in [
                (
                    "equalizer",
                    dict(
                        lowGain=rng.uniform(-4, 3),
                        midGain=rng.uniform(-4, 3),
                        highGain=rng.uniform(-4, 3),
                        midFrequency=rng.uniform(500, 3000),
                    ),
                ),
                (
                    "room",
                    dict(
                        decay=rng.uniform(0.2, 0.65),
                        mix=rng.uniform(0.04, 0.17),
                        predelay=rng.uniform(0, 20),
                    ),
                ),
                (
                    "compressor",
                    dict(
                        threshold=-22, ratio=rng.uniform(1.5, 3), attack=10, release=100, makeup=0
                    ),
                ),
            ]:
                effects.append(
                    dict(
                        id=native.uid(asset + kind), kind=kind, enabled=True, parameters=parameters
                    )
                )
            with tempfile.TemporaryDirectory(prefix=".voice-", dir=args.output) as temporary:
                temporary = Path(temporary)
                source = temporary / "input.wav"
                sf.write(source, raw, RATE, subtype="FLOAT")
                plan = dict(
                    assets=[dict(id=asset, name="voice", audioPath=str(source))],
                    tailSeconds=0.3,
                    session=dict(
                        version=9,
                        name="Voice augmentation",
                        masterTrack=dict(
                            id=native.MASTER, name="Master", gain=1, clips=[], realtimeEffects=[]
                        ),
                        tracks=[
                            dict(
                                id=native.uid(asset + "track"),
                                name="voice",
                                gain=1,
                                pan=0,
                                realtimeEffects=effects,
                                clips=[
                                    dict(
                                        id=native.uid(asset + "clip"),
                                        name="voice",
                                        assetID=asset,
                                        start=0,
                                        offset=0,
                                        length=len(raw),
                                        edgeProtectionDisabled=True,
                                    )
                                ],
                            )
                        ],
                    ),
                )
                job = temporary / "plan.json"
                job.write_text(json.dumps(plan))
                result = subprocess.run(
                    [
                        str(args.renderer),
                        str(job),
                        str(temporary / "voice.soundroom"),
                        str(temporary / "wet.wav"),
                        "--kits",
                        str(args.kits),
                    ],
                    capture_output=True,
                    text=True,
                    timeout=120,
                )
                if result.returncode:
                    raise RuntimeError(result.stdout + result.stderr)
                wet, rate = sf.read(temporary / "wet.wav", dtype="float32", always_2d=True)
                if rate != RATE or wet.shape[1] != 2 or not np.isfinite(wet).all():
                    raise ValueError("Invalid wet voice")
                wet *= min(1, 0.8 / max(1e-8, float(np.abs(wet).max())))
            folder.mkdir(parents=True, exist_ok=True)
            hashes = {}
            for label, audio in [("dry", raw), ("wet", wet)]:
                path = folder / f"{label}.flac"
                sf.write(path, audio, RATE, subtype="PCM_24")
                hashes[path.name] = native.sha(path)
            row = dict(
                path=folder.name,
                split=split,
                singer=record["group"],
                original=record,
                effects=effects,
                fingerprint=fingerprint,
                sha256=hashes,
            )
            meta.write_text(json.dumps(row, indent=2) + "\n")
        rows.append(row)
        print(f"Voice {index + 1}/60: {split}", flush=True)
    manifest = dict(
        schema=1,
        fingerprint=fingerprint,
        identity=identity,
        singers=singers,
        voices=rows,
        attribution=sources["datasets"]["vocalset"],
    )
    part = args.output / "bank.json.part"
    part.write_text(json.dumps(manifest, indent=2) + "\n")
    part.replace(args.output / "bank.json")


class Mixtures:
    def __init__(self, bank, voices, data, seed=20261019):
        self.bank, self.voices_root, self.data, self.seed = bank, voices, data, seed
        self.manifest = json.loads((bank / "bank.json").read_text())
        self.voice_manifest = json.loads((voices / "bank.json").read_text())
        self.sources = recordings.verify_sources(data)
        _, singers, self.songs = recordings.assign_groups(data, self.sources, 20261016)
        if singers != self.voice_manifest["singers"]:
            raise ValueError("Voice split changed")
        ids = set()
        for row in self.manifest["scenes"]:
            if row["split"] not in singers or row["scoreID"] in ids:
                raise ValueError("Invalid bank split/duplicate score")
            ids.add(row["scoreID"])
            folder = checked_path(bank, row["path"])
            if native.sha(folder / "bank.json") != row["metadataSHA256"]:
                raise ValueError("Native metadata changed")
            meta = json.loads((folder / "bank.json").read_text())
            if meta["split"] != row["split"] or meta["humanVocals"]:
                raise ValueError("Native source role changed")
            for name, sha in meta["sha256"].items():
                if native.sha(checked_path(folder, name)) != sha:
                    raise ValueError("Native audio changed")
        for row in self.voice_manifest["voices"]:
            if row["singer"] not in singers[row["split"]]:
                raise ValueError("Singer leakage in voice bank")
            for name, sha in row["sha256"].items():
                if native.sha(checked_path(voices, row["path"] + "/" + name)) != sha:
                    raise ValueError("Voice audio changed")
        self.identity = dict(
            native=native.sha(bank / "bank.json"),
            voices=native.sha(voices / "bank.json"),
            sources=native.sha(data / "sources.json"),
            seed=seed,
            generator=native.sha(Path(__file__)),
            singerSplits=singers,
            songSplits={
                g: dict(split=s["split"], identity=s["identity"]) for g, s in self.songs.items()
            },
        )
        self.fingerprint = hashlib.sha256(
            json.dumps(self.identity, sort_keys=True).encode()
        ).hexdigest()

    def read(self, path, seconds, offset):
        info = sf.info(path)
        # Offset is a fraction of the available four-second crop range, shared by
        # every aligned part and by the 1s/4s context comparison.
        start = int(max(0, info.frames - 4 * info.samplerate) * offset)
        audio, rate = sf.read(
            path,
            start=start,
            frames=int(seconds * info.samplerate),
            dtype="float32",
            always_2d=True,
        )
        if audio.shape[1] not in [1, 2] or not np.isfinite(audio).all():
            raise ValueError("Invalid mixture source")
        tensor = torch.from_numpy(audio.T.copy())
        if rate != RATE:
            tensor = resample(tensor, rate, RATE)
        if tensor.shape[0] == 1:
            tensor = tensor.repeat(2, 1)
        length = round(seconds * RATE)
        return torch.nn.functional.pad(tensor[:, :length], (0, max(0, length - tensor.shape[-1])))

    @staticmethod
    def level(audio, db, pan=0):
        rms = audio.square().mean().sqrt()
        audio = audio * (10 ** (db / 20) / rms.clamp_min(1e-7))
        return audio * torch.tensor([min(1.0, 1 - pan), min(1.0, 1 + pan)])[:, None]

    def sample(self, split, index, seconds=4.0, external=None, category=None):
        if split not in ["train", "validation", "test"] or not 0.1 <= seconds <= 8:
            raise ValueError("Invalid recipe")
        if external and split != "test":
            raise ValueError("Independent voices are evaluation only")
        rng = random.Random(f"online/{self.seed}/{split}/{index}")
        category = category or CATEGORIES[index % len(CATEGORIES)]
        if category not in CATEGORIES:
            raise ValueError("Unknown mixture category")
        requested_seconds = seconds
        # Generate the same four-second recipe before cropping for the short
        # context run: levels, crop offsets and augmentations stay paired.
        seconds = max(4.0, seconds)
        accompaniment = torch.zeros(2, round(seconds * RATE))
        provenance = []
        if category != "solo_vocals":
            use_slakh = category != "hard" and index % 2 == 0
            if use_slakh:
                group = rng.choice(sorted(g for g, s in self.songs.items() if s["split"] == split))
                records = [
                    r
                    for r in self.sources["files"]
                    if r["dataset"] == "babyslakh"
                    and r["group"] == group
                    and "/stems/" in r["path"]
                    and r["path"].endswith(".wav")
                ]
                offset = rng.random()
                for record in records:
                    accompaniment += self.read(self.data / record["path"], seconds, offset)
                provenance.append(self.songs[group]["identity"])
            else:
                row = rng.choice([r for r in self.manifest["scenes"] if r["split"] == split])
                offset = rng.random()
                parts = [4] if category == "hard" else rng.sample(range(5), rng.randint(1, 5))
                for part in parts:
                    audio = self.read(self.bank / row["path"] / f"part{part}.flac", seconds, offset)
                    accompaniment += self.level(
                        audio, rng.uniform(-30, -23), rng.uniform(-0.5, 0.5)
                    )
                provenance.append("soundroom:" + row["scoreID"])
            accompaniment = self.level(accompaniment, rng.uniform(-25, -19))
        voice = torch.zeros_like(accompaniment)
        if category != "no_vocals":
            if external:
                path = external["path"]
                provenance.append("vocadito:" + external["singer"])
            else:
                row = rng.choice([r for r in self.voice_manifest["voices"] if r["split"] == split])
                path = self.voices_root / row["path"] / rng.choice(["dry.flac", "wet.flac"])
                provenance.append("vocalset:" + row["singer"])
            voice = self.read(path, seconds, rng.random())
            # Speed/pitch augmentation moves formants too; it does not add singers.
            pitch = rng.choice([47, 50, 50, 50, 53]) if split == "train" else 50
            if pitch != 50:
                voice = resample(voice, pitch, 50)
                voice = torch.nn.functional.pad(
                    voice[:, : accompaniment.shape[-1]],
                    (0, max(0, accompaniment.shape[-1] - voice.shape[-1])),
                )
            voice = self.level(voice, rng.uniform(-27, -19), rng.uniform(-0.25, 0.25))
        target = torch.stack([accompaniment, voice])
        target *= min(
            1.0, 0.85 / max(1e-7, float(target.abs().max()), float(target.sum(0).abs().max()))
        )
        if not torch.isfinite(target).all():
            raise ValueError("Nonfinite mixture")
        return target[..., : round(requested_seconds * RATE)], dict(
            split=split,
            index=index,
            category=category,
            sourceGroups=provenance,
            recipeSeed=f"online/{self.seed}/{split}/{index}",
            contextSeconds=requested_seconds,
        )

    @staticmethod
    def independent(root):
        verify_manifest(root)
        with (root / "vocadito_metadata.csv").open() as file:
            metadata = list(csv.DictReader(file))
        return [
            dict(
                path=root / "Audio" / f"vocadito_{r['track_id']}.wav",
                singer=r["singer_id"],
                track=r["track_id"],
                language=r["language"],
            )
            for r in metadata
        ]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", type=Path, default=ROOT / "build/Training/data")
    parser.add_argument("--output", type=Path, default=ROOT / "build/Training/vocal-voices-v3")
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
    torch.set_num_threads(2)
    prepare_voices(args)


if __name__ == "__main__":
    main()
