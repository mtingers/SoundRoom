#!/usr/bin/env python3
"""Combine verified VocalSet/BabySlakh with original Soundroom stems, via native inserts.

Splits singers and source song identifiers/exact MIDI duplicates before effects
or crop variations. Synthesized Slakh choir is `other`, never human vocals.
"""

import argparse
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
import yaml
from torchaudio.functional import resample

spec = importlib.util.spec_from_file_location(
    "native_corpus", Path(__file__).with_name("generate-separator-corpus.py")
)
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)
ROOT, RATE, CLASSES = native.ROOT, 48000, native.CLASSES + ["other", "vocals"]


def resample_audio(audio, original, target):
    tensor = torch.from_numpy(np.ascontiguousarray(audio.T))
    return resample(tensor, original, target).numpy().T.copy()


def verify_sources(root):
    manifest = json.loads((root / "sources.json").read_text())
    for record in manifest["files"]:
        path = (root / record["path"]).resolve()
        if not path.is_relative_to(root.resolve()) or record["license"] != "CC-BY-4.0":
            raise ValueError("Unexpected source path/license")
        if path.stat().st_size != record["bytes"] or native.sha(path) != record["sha256"]:
            raise ValueError(f"Source failed manifest integrity: {path}")
    return manifest


def assign_groups(root, manifest, seed):
    voices, songs = {}, {}
    for record in manifest["files"]:
        if record["dataset"] == "vocalset" and record["path"].lower().endswith(".wav"):
            voices.setdefault(record["group"], []).append(record)
        if record["dataset"] == "babyslakh" and record["path"].endswith("metadata.yaml"):
            metadata = yaml.safe_load((root / record["path"]).read_text())
            midi = next(
                r
                for r in manifest["files"]
                if r["dataset"] == "babyslakh"
                and r["group"] == record["group"]
                and r["path"].endswith("all_src.mid")
            )
            origin = Path(metadata["lmd_midi_dir"]).parent.name
            songs[record["group"]] = dict(
                metadata=metadata, identifiers={"song:" + origin, "midi:" + midi["sha256"]}
            )
    if len(voices) < 10 or len(songs) < 3:
        raise ValueError("Pilot needs at least ten singers and three song groups")
    # Connected components join same original song or identical MIDI bytes.
    components = []
    for group, song in sorted(songs.items()):
        members, keys = {group}, set(song["identifiers"])
        merged = True
        while merged:
            merged = False
            for old_members, old_keys in list(components):
                if keys & old_keys:
                    components.remove((old_members, old_keys))
                    members |= old_members
                    keys |= old_keys
                    merged = True
        components.append((members, keys))
    if len(components) < 3:
        raise ValueError("Not enough distinct compositions after duplicate grouping")
    rng = random.Random(f"source-splits/{seed}")
    names = sorted(voices)
    rng.shuffle(names)
    n = max(1, int(len(names) * 0.15))
    singers = dict(train=names[: -2 * n], validation=names[-2 * n : -n], test=names[-n:])
    rng.shuffle(components)
    for i, (members, keys) in enumerate(components):
        split = ["train", "validation", "test"][i % 3]
        identity = "slakh:" + hashlib.sha256("|".join(sorted(keys)).encode()).hexdigest()
        for group in members:
            songs[group].update(split=split, identity=identity)
    return voices, singers, songs


def read_audio(path, frames, rng, offset=None):
    info = sf.info(path)
    needed = int(frames * info.samplerate / RATE)
    first = offset if offset is not None else rng.randrange(max(1, info.frames - needed + 1))
    audio, rate = sf.read(path, start=first, frames=needed, dtype="float32", always_2d=True)
    if audio.shape[1] not in [1, 2] or not np.isfinite(audio).all():
        raise ValueError("Unexpected recording format")
    if rate != RATE:
        audio = resample_audio(audio, rate, RATE)
    if audio.shape[1] == 1:
        audio = np.repeat(audio, 2, axis=1)
    audio = np.pad(audio[:frames], ((0, max(0, frames - len(audio))), (0, 0)))
    return audio, first


def family(stem):
    program = stem["program_num"]
    if stem.get("is_drum") or program == 128:
        return "drums"
    if 0 <= program <= 7:
        return "piano"
    if 24 <= program <= 31:
        return "guitar"
    if 32 <= program <= 39:
        return "bass"
    return "other"


def render_scene(
    output,
    relative,
    split,
    inputs,
    sources,
    groups,
    voice_record,
    data,
    rng,
    renderer,
    kits,
    seed,
    fingerprint,
):
    destination = output / relative
    if (destination / "scene.json").exists():
        record = json.loads((destination / "scene.json").read_text())
        if record["seed"] != seed or record.get("generationFingerprint") != fingerprint:
            raise ValueError(
                "Cached scene uses different generation provenance; choose a fresh --output directory"
            )
        for name, digest in record["sha256"].items():
            if native.sha(destination / name) != digest:
                raise ValueError("Cached stem failed integrity")
        return record
    frames = inputs.shape[1]
    for _ in range(20):
        voice, voice_start = read_audio(data / voice_record["path"], frames, rng)
        if float(np.mean(voice**2)) > 1e-6:
            break
    # Small speed/pitch variation, with formants moving too; not a formant-preserving product effect.
    semitones = rng.choice([-1, 0, 0, 0, 1])
    if semitones:
        voice = resample_audio(voice, 53 if semitones > 0 else 47, 50)
        voice = np.pad(voice[:frames], ((0, max(0, frames - len(voice))), (0, 0)))
    inputs[5] = voice
    sources.append(
        dict(
            **voice_record,
            offsetFrames=voice_start,
            pitchSemitones=semitones,
            pitchMethod="small speed resample; formants move; pad/crop to common duration",
        )
    )
    groups.append("vocalset:" + voice_record["group"])
    for ci in range(6):
        rms = float(np.sqrt(np.mean(inputs[ci] ** 2)))
        if rms > 1e-7:
            inputs[ci] *= 10 ** (rng.uniform(-29, -21) / 20) / rms
        if rng.random() < 0.12:
            inputs[ci] *= 0
    if not inputs.any():
        raise ValueError("All sources are silent")
    inputs *= min(
        1, 0.75 / max(1e-7, float(np.max(np.abs(inputs.sum(0)))), float(np.max(np.abs(inputs))))
    )
    with tempfile.TemporaryDirectory(prefix=".wet-", dir=output) as temporary:
        temporary = Path(temporary)
        assets, tracks, input_hashes = [], [], {}
        for ci, label in enumerate(CLASSES):
            path = temporary / f"{label}.wav"
            sf.write(path, inputs[ci], RATE, subtype="FLOAT")
            input_hashes[label] = native.sha(path)
            asset = native.uid(f"real/{seed}/{relative}/{label}/asset")
            assets.append(dict(id=asset, name=label, audioPath=str(path)))
            effects = []

            def effect(kind, parameters):
                effects.append(
                    dict(
                        id=native.uid(f"real/{seed}/{relative}/{label}/{kind}"),
                        kind=kind,
                        enabled=True,
                        parameters=parameters,
                    )
                )

            if rng.random() < 0.6:
                effect(
                    "equalizer",
                    dict(
                        lowGain=rng.uniform(-3, 3),
                        midGain=rng.uniform(-3, 3),
                        highGain=rng.uniform(-3, 3),
                        midFrequency=rng.uniform(400, 3500),
                    ),
                )
            if rng.random() < 0.4:
                effect(
                    "room",
                    dict(
                        decay=rng.uniform(0.2, 0.8),
                        mix=rng.uniform(0.02, 0.18),
                        predelay=rng.uniform(0, 20),
                    ),
                )
            if label == "vocals" and rng.random() < 0.4:
                effect(
                    "compressor",
                    dict(
                        threshold=-22, ratio=rng.uniform(1.5, 3), attack=10, release=100, makeup=0
                    ),
                )
            tracks.append(
                dict(
                    id=native.uid(f"real/{seed}/{relative}/{label}/track"),
                    name=label,
                    gain=1,
                    pan=rng.uniform(-0.6, 0.6),
                    realtimeEffects=effects,
                    clips=[
                        dict(
                            id=native.uid(f"real/{seed}/{relative}/{label}/clip"),
                            assetID=asset,
                            name=label,
                            start=0,
                            offset=0,
                            length=frames,
                            edgeProtectionDisabled=True,
                        )
                    ],
                )
            )
        plan = dict(
            session=dict(
                version=9,
                name=f"Recorded training {relative}",
                tracks=tracks,
                masterTrack=dict(
                    id=native.MASTER, name="Master", clips=[], gain=1, realtimeEffects=[]
                ),
            ),
            assets=assets,
            tailSeconds=0.3,
        )
        plan_path = temporary / "plan.json"
        plan_path.write_text(json.dumps(plan))
        result = subprocess.run(
            [
                str(renderer),
                str(plan_path),
                str(temporary / "wet.soundroom"),
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
        if result.returncode:
            raise RuntimeError(result.stdout + result.stderr)
        wet = np.stack(
            [
                sf.read(temporary / "stems" / f"{i:02}.wav", dtype="float32", always_2d=True)[0]
                for i in range(6)
            ]
        )
        mix = sf.read(temporary / "mix.wav", dtype="float32", always_2d=True)[0]
        error = float(np.max(np.abs(mix - wet.sum(0))))
        if not np.isfinite(wet).all() or error > 1e-5:
            raise ValueError("Native wet stems are not additive/aligned")
        wet *= min(
            1, 0.85 / max(1e-7, float(np.max(np.abs(wet.sum(0)))), float(np.max(np.abs(wet))))
        )
        destination.mkdir(parents=True, exist_ok=True)
        hashes = {}
        for ci, label in enumerate(CLASSES):
            path = destination / f"{label}.flac"
            sf.write(path, wet[ci], RATE, subtype="PCM_24")
            hashes[path.name] = native.sha(path)
        # Preserve reproducible input digests/roles instead of stale temp paths.
        for job in plan["assets"]:
            job["audioPath"] = job["name"] + "-input.wav"
        record = dict(
            schema=1,
            split=split,
            seed=seed,
            generationFingerprint=fingerprint,
            scoreID=native.uid(f"recorded/{seed}/{relative}"),
            classes=CLASSES,
            sampleRate=RATE,
            frames=wet.shape[1],
            sha256=hashes,
            sourceGroups=sorted(set(groups)),
            sources=sources,
            inputSHA256=input_hashes,
            nativeMixMaxError=error,
            plan=plan,
        )
        (destination / "scene.json").write_text(json.dumps(record, indent=2) + "\n")
        return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", type=Path, default=ROOT / "build/Training/data")
    parser.add_argument(
        "--native", type=Path, default=ROOT / "build/Training/soundroom-pilot-v2-base"
    )
    parser.add_argument("--output", type=Path, default=ROOT / "build/Training/recorded-pilot-v1")
    parser.add_argument("--seed", type=int, default=20261016)
    parser.add_argument("--slakh-segments", type=int, default=4)
    parser.add_argument("--native-limit", type=int, default=10000)
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
    if not 1 <= args.slakh_segments <= 100 or not 1 <= args.native_limit <= 10000:
        parser.error("Invalid pilot bounds")
    sources = verify_sources(args.data)
    voices, singers, songs = assign_groups(args.data, sources, args.seed)
    args.output.mkdir(parents=True, exist_ok=True)
    base = json.loads((args.native / "corpus.json").read_text())
    fingerprint = hashlib.sha256(
        json.dumps(
            dict(
                seed=args.seed,
                source=native.sha(args.data / "sources.json"),
                native=native.sha(args.native / "corpus.json"),
                renderer=native.sha(args.renderer),
                generator=native.sha(Path(__file__)),
            ),
            sort_keys=True,
        ).encode()
    ).hexdigest()
    scenes = []

    def retain(relative, split, inputs, provenance, groups):
        rng = random.Random(f"wet/{args.seed}/{relative}")
        singer = rng.choice(singers[split])
        voice_record = rng.choice(voices[singer])
        record = render_scene(
            args.output,
            relative,
            split,
            inputs,
            provenance,
            groups,
            voice_record,
            args.data,
            rng,
            args.renderer,
            args.kits,
            args.seed,
            fingerprint,
        )
        scenes.append(
            dict(
                path=relative,
                split=split,
                scoreID=record["scoreID"],
                sourceGroups=record["sourceGroups"],
                metadataSHA256=native.sha(args.output / relative / "scene.json"),
            )
        )
        print(f"{relative}: {record['frames'] / RATE:.2f}s", flush=True)

    for split in ["train", "validation", "test"]:
        for i, scene in enumerate(r for r in base["scenes"] if r["split"] == split):
            if i >= args.native_limit:
                break
            inputs = np.zeros((6, RATE * 3, 2), dtype=np.float32)
            metadata = json.loads((args.native / scene["path"] / "scene.json").read_text())
            provenance = []
            if native.sha(args.native / scene["path"] / "scene.json") != scene["metadataSHA256"]:
                raise ValueError("Native metadata integrity failed")
            for ci, label in enumerate(native.CLASSES):
                path = args.native / scene["path"] / f"{label}.flac"
                if native.sha(path) != metadata["sha256"][path.name]:
                    raise ValueError("Native source integrity failed")
                audio, _ = read_audio(path, RATE * 3, random.Random(0), offset=0)
                inputs[ci] = audio
                provenance.append(
                    dict(
                        dataset="soundroom",
                        path=str(path.relative_to(ROOT)),
                        sha256=native.sha(path),
                    )
                )
            retain(
                f"{split}/native-{i:04}",
                split,
                inputs,
                provenance,
                ["soundroom:" + scene["scoreID"]],
            )
    for group, song in sorted(songs.items()):
        records = [
            r
            for r in sources["files"]
            if r["dataset"] == "babyslakh"
            and r["group"] == group
            and "/stems/" in r["path"]
            and r["path"].endswith(".wav")
        ]
        for index in range(args.slakh_segments):
            rng = random.Random(f"slakh/{args.seed}/{group}/{index}")
            infos = [sf.info(args.data / r["path"]) for r in records]
            durations = [info.frames / info.samplerate for info in infos]
            if not durations or max(durations) - min(durations) > 1 / 16000:
                raise ValueError("Slakh stem lengths differ")
            for _ in range(20):
                seconds = rng.uniform(0, max(0, min(durations) - 3))
                inputs = np.zeros((6, RATE * 3, 2), dtype=np.float32)
                for record, info in zip(records, infos):
                    label = family(song["metadata"]["stems"][Path(record["path"]).stem])
                    audio, _ = read_audio(
                        args.data / record["path"],
                        RATE * 3,
                        rng,
                        offset=int(seconds * info.samplerate),
                    )
                    inputs[CLASSES.index(label)] += audio
                if float(np.mean(inputs**2)) > 1e-6:
                    break
            provenance = [dict(**r, segmentStartSeconds=seconds) for r in records]
            retain(
                f"{song['split']}/{group}-{index:03}",
                song["split"],
                inputs,
                provenance,
                [song["identity"]],
            )
    manifest = dict(
        schema=1,
        seed=args.seed,
        classes=CLASSES,
        sampleRate=RATE,
        scenes=scenes,
        sourceManifestSHA256=native.sha(args.data / "sources.json"),
        nativeCorpusSHA256=native.sha(args.native / "corpus.json"),
        rendererSHA256=native.sha(args.renderer),
        generatorSHA256=native.sha(Path(__file__)),
        singerSplits=singers,
        compositionSplits={
            group: dict(split=song["split"], identity=song["identity"])
            for group, song in songs.items()
        },
        voicesTrained=True,
        attribution=sources["datasets"],
        nativeAttribution=base["attribution"],
        limitations="Pilot only; VocalSet exercises, Slakh synthesized at 16 kHz; shared native banks; song identity/exact-MIDI grouping does not detect all near duplicates.",
    )
    (args.output / "ATTRIBUTION.txt").write_text(
        base["attribution"]
        + "\n\n"
        + "\n\n".join(
            f"{name}: {record['credit']}\n{record['license']} · {record['page']}"
            for name, record in sources["datasets"].items()
        )
        + "\n"
    )
    partial = args.output / "corpus.json.part"
    partial.write_text(json.dumps(manifest, indent=2) + "\n")
    partial.replace(args.output / "corpus.json")
    print(f"Ready: {args.output / 'corpus.json'}", flush=True)


if __name__ == "__main__":
    main()
