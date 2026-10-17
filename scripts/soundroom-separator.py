#!/usr/bin/env python3
"""Own separation research models: a four-instrument baseline and six-source extension.

No Demucs code or weights. Offline, CPU-compatible, 48 kHz stereo. Default v0
uses positive masks and mixture phase. Optional complex masks support six-source
corpora including human vocals and other instruments. Research CLI only; each
checkpoint declares its trained sources. No production quality is asserted.
"""

import argparse
import hashlib
import json
import math
import random
import time
from pathlib import Path

import numpy as np
import soundfile as sf
import torch
from torch import nn
from torch.nn import functional as F

ROOT = Path(__file__).resolve().parent.parent
CLASSES = ["drums", "bass", "guitar", "piano"]
ALL_CLASSES = CLASSES + ["other", "vocals"]
BINARY_CLASSES = ["accompaniment", "vocals"]
RATE, FFT, HOP = 48000, 2048, 512


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class Block(nn.Module):
    def __init__(self, inputs, outputs):
        super().__init__()
        self.layers = nn.Sequential(
            nn.Conv2d(inputs, outputs, 3, padding=1),
            nn.GroupNorm(4, outputs),
            nn.SiLU(),
            nn.Conv2d(outputs, outputs, 3, padding=1),
            nn.GroupNorm(4, outputs),
            nn.SiLU(),
        )

    def forward(self, x):
        return self.layers(x)


class Separator(nn.Module):
    def __init__(self, width=8, classes=None, complex_masks=False):
        super().__init__()
        self.classes = list(classes or CLASSES)
        self.width = width
        self.complex_masks = complex_masks
        self.down1, self.down2, self.down3 = (
            Block(7, width),
            Block(width, width * 2),
            Block(width * 2, width * 4),
        )
        self.bottom = Block(width * 4, width * 8)
        self.up3, self.up2, self.up1 = (
            Block(width * 12, width * 4),
            Block(width * 6, width * 2),
            Block(width * 3, width),
        )
        self.output = nn.Conv2d(width, len(self.classes) * 2 * (3 if complex_masks else 1), 1)

    def forward(self, mixture):
        # [batch, stereo, frequency, time] complex spectrum.
        magnitude = mixture.abs()
        scale = magnitude.mean(dim=(1, 2, 3), keepdim=True).clamp_min(1e-5)
        phase = mixture[:, 0] * mixture[:, 1].conj()
        phase = phase / (magnitude[:, 0] * magnitude[:, 1]).clamp_min(1e-8)
        frequency = torch.linspace(0, 1, magnitude.shape[2], device=mixture.device)[
            None, None, :, None
        ]
        frequency = frequency.expand(mixture.shape[0], 1, -1, mixture.shape[3])
        features = torch.cat(
            [
                torch.log1p(magnitude / scale),
                (
                    (magnitude[:, :1] - magnitude[:, 1:])
                    / magnitude.sum(1, keepdim=True).clamp_min(1e-8)
                ),
                phase.real[:, None],
                phase.imag[:, None],
                frequency,
                frequency.sqrt(),
            ],
            dim=1,
        )
        height, width = features.shape[-2:]
        x = F.pad(features, (0, -width % 8, 0, -height % 8), mode="replicate")
        a = self.down1(x)
        b = self.down2(F.avg_pool2d(a, 2))
        c = self.down3(F.avg_pool2d(b, 2))
        x = self.bottom(F.avg_pool2d(c, 2))
        for skip, block in [(c, self.up3), (b, self.up2), (a, self.up1)]:
            x = block(
                torch.cat(
                    [
                        F.interpolate(
                            x, size=skip.shape[-2:], mode="bilinear", align_corners=False
                        ),
                        skip,
                    ],
                    dim=1,
                )
            )
        logits = self.output(x)[..., :height, :width]
        if self.complex_masks:
            logits = logits.reshape(mixture.shape[0], len(self.classes), 2, 3, height, width)
            masks = logits[:, :, :, 0].softmax(dim=1)
            correction = logits[:, :, :, 1:].tanh() * 0.5
            correction = correction - correction.mean(dim=1, keepdim=True)
            coefficients = torch.complex(masks + correction[:, :, :, 0], correction[:, :, :, 1])
            return coefficients * mixture[:, None]
        logits = logits.reshape(mixture.shape[0], len(self.classes), 2, height, width)
        return logits.softmax(dim=1) * mixture[:, None]


def spectrum(audio):
    shape = audio.shape
    return torch.stft(
        audio.reshape(-1, shape[-1]),
        FFT,
        HOP,
        window=torch.hann_window(FFT),
        return_complex=True,
        pad_mode="constant",
    ).reshape(*shape[:-1], FFT // 2 + 1, -1)


def waveform(spec, length):
    shape = spec.shape
    return torch.istft(
        spec.reshape(-1, *shape[-2:]), FFT, HOP, window=torch.hann_window(FFT), length=length
    ).reshape(*shape[:-2], length)


def complex_loss(prediction, target):
    magnitude = target.abs().clamp_min(1e-4)
    normalization = magnitude.pow(0.3).mean(dim=(2, 3, 4)).clamp_min(0.1)
    # Compress signals before differencing, rather than using a sparsity loss
    # on the residual that gives weak amplitude supervision to quiet stems.
    compressed_target = target * magnitude.pow(-0.7)
    compressed_prediction = prediction * prediction.abs().clamp_min(1e-4).pow(-0.7)
    complex_error = (compressed_prediction - compressed_target).abs().mean(dim=(2, 3, 4))
    magnitude_error = (
        (prediction.abs().clamp_min(1e-8).pow(0.3) - target.abs().clamp_min(1e-8).pow(0.3))
        .abs()
        .mean(dim=(2, 3, 4))
    )
    # A proper energy error is essential: compressed L1 alone can prefer a
    # sparse allocation (all mixture in one stem) and suppress quiet targets.
    error_energy = (prediction - target).abs().square().sum(dim=(2, 3, 4))
    target_energy = target.abs().square().sum(dim=(2, 3, 4))
    mixture_energy = target.sum(1).abs().square().sum(dim=(1, 2, 3))[:, None]
    relative_error = error_energy / (target_energy + 0.1 * mixture_energy).clamp_min(1e-8)
    return relative_error.mean() + 0.1 * ((complex_error + magnitude_error) / normalization).mean()


def load_scene(root, record, classes=None):
    directory = root / record["path"]
    if digest(directory / "scene.json") != record["metadataSHA256"]:
        raise ValueError("Corpus scene metadata failed integrity checks")
    metadata = json.loads((directory / "scene.json").read_text())
    stems = []
    for family in classes or CLASSES:
        path = directory / f"{family}.flac"
        if digest(path) != metadata["sha256"][path.name]:
            raise ValueError(f"Source integrity check failed: {path}")
        audio, rate = sf.read(path, dtype="float32", always_2d=True)
        if rate != RATE or audio.shape != (metadata["frames"], 2) or not np.isfinite(audio).all():
            raise ValueError("Corpus source format/alignment is invalid")
        stems.append(torch.from_numpy(audio.T.copy()))
    return torch.stack(stems)


def read_corpus(root):
    manifest = json.loads((root / "corpus.json").read_text())
    if (
        manifest["classes"] not in [CLASSES, ALL_CLASSES, BINARY_CLASSES]
        or manifest["sampleRate"] != RATE
    ):
        raise ValueError("Unsupported corpus format/classes")
    scores = set()
    group_splits = {}
    for record in manifest["scenes"]:
        if record["scoreID"] in scores or record["split"] not in ("train", "validation", "test"):
            raise ValueError("Duplicate score or invalid corpus split")
        scores.add(record["scoreID"])
        for group in record.get("sourceGroups", []):
            if group in group_splits and group_splits[group] != record["split"]:
                raise ValueError("Singer/composition leakage across corpus splits")
            group_splits[group] = record["split"]
        path = (root / record["path"]).resolve()
        if not path.is_relative_to(root.resolve()):
            raise ValueError("Corpus path escapes root")
    return manifest


def metrics(prediction, target, mixture, classes=None):
    rows = []
    for index, family in enumerate(classes or CLASSES):
        y, p, x = (v.double().flatten() for v in (target[index], prediction[index], mixture))
        energy = y.square().sum()
        if energy < 1e-10:
            rows.append(
                dict(
                    source=family,
                    absent=True,
                    leakageDBFS=10 * math.log10(max(1e-12, float(p.square().mean()))),
                )
            )
            continue

        def si(v):
            v, reference = v - v.mean(), y - y.mean()
            projected = reference * ((v @ reference) / reference.square().sum().clamp_min(1e-20))
            return float(
                10
                * torch.log10(
                    (projected.square().sum() + 1e-20) / ((v - projected).square().sum() + 1e-20)
                )
            )

        def sdr(v):
            return float(10 * torch.log10(energy / (v - y).square().sum().clamp_min(1e-20)))

        rows.append(
            dict(
                source=family,
                absent=False,
                siSDR=si(p),
                siSDRImprovement=si(p) - si(x),
                sdr=sdr(p),
                sdrImprovement=sdr(p) - sdr(x),
                referenceGain=float((p @ y) / energy),
            )
        )
    return rows


@torch.no_grad()
def predict(model, audio, chunk_frames=None):
    # Half-overlap windows give bounded network memory and sample-exact boundaries.
    frames = audio.shape[-1]
    chunk_frames = chunk_frames or getattr(model, "inference_frames", RATE * 3)
    if chunk_frames < 2:
        raise ValueError("Inference window must contain at least two frames")
    output = torch.zeros(len(model.classes), 2, frames)
    weight = torch.zeros(frames)
    for start in range(0, frames, chunk_frames // 2):
        end = min(frames, start + chunk_frames)
        segment = audio[:, start:end]
        if getattr(model, "pad_context", False):
            segment = F.pad(segment, (0, chunk_frames - segment.shape[-1]))
        prediction = waveform(model(spectrum(segment)[None])[0], segment.shape[-1])[
            ..., : end - start
        ]
        size = end - start
        window = (
            torch.ones(1)
            if size == 1
            else torch.hann_window(2 * size, periodic=False)[size // 2 : size // 2 + size]
        )
        output[:, :, start:end] += prediction * window
        weight[start:end] += window
        if end == frames:
            break
    return output / weight.clamp_min(1e-8)


@torch.no_grad()
def evaluate(model, root, manifest, split, limit=None, audition=None):
    if model.classes != manifest["classes"]:
        raise ValueError("Checkpoint and evaluation corpus classes differ")
    model.eval()
    records = [record for record in manifest["scenes"] if record["split"] == split]
    if limit:
        records = records[:limit]
    rows, reconstruction = [], []
    for i, record in enumerate(records):
        target = load_scene(root, record, manifest["classes"])
        mixture = target.sum(0)
        prediction = predict(model, mixture)
        reconstruction.append(float((prediction.sum(0) - mixture).abs().max()))
        rows.extend(
            dict(scene=record["path"], **row)
            for row in metrics(prediction, target, mixture, model.classes)
        )
        if audition and i < 2:
            directory = audition / record["path"].replace("/", "-")
            directory.mkdir(parents=True, exist_ok=True)
            sf.write(directory / "mixture.wav", mixture.T.numpy(), RATE, subtype="FLOAT")
            for ci, family in enumerate(model.classes):
                sf.write(
                    directory / f"{family}-reference.wav",
                    target[ci].T.numpy(),
                    RATE,
                    subtype="FLOAT",
                )
                sf.write(
                    directory / f"{family}-estimated.wav",
                    prediction[ci].T.numpy(),
                    RATE,
                    subtype="FLOAT",
                )
    aggregate = {}
    for family in model.classes:
        active = [row for row in rows if row["source"] == family and not row["absent"]]
        absent = [row for row in rows if row["source"] == family and row["absent"]]
        aggregate[family] = dict(
            activeExamples=len(active),
            absentExamples=len(absent),
            **{
                key: float(np.mean([row[key] for row in active])) if active else None
                for key in ["siSDR", "siSDRImprovement", "sdr", "sdrImprovement", "referenceGain"]
            },
            absentLeakageDBFS=float(np.mean([row["leakageDBFS"] for row in absent]))
            if absent
            else None,
        )
    improvements = [row["siSDRImprovement"] for row in rows if not row["absent"]]
    return dict(
        split=split,
        scenes=len(records),
        bySource=aggregate,
        meanSiSDRImprovement=float(np.mean(improvements)) if improvements else None,
        reconstructionMaxError=max(reconstruction, default=0),
        examples=rows,
    )


def train(args):
    if args.steps < 1 or args.steps > 100000 or args.batch < 1 or args.batch > 8:
        raise ValueError("Use 1–100000 steps, batch 1–8")
    torch.manual_seed(args.seed)
    rng = random.Random(args.seed)
    manifest = read_corpus(args.corpus)
    training = [r for r in manifest["scenes"] if r["split"] == "train"]
    origins = {}
    for record in training:
        origin = (
            "slakh"
            if any(g.startswith("slakh:") for g in record.get("sourceGroups", []))
            else "soundroom"
        )
        origins.setdefault(origin, []).append(record)
    if not training or not any(r["split"] == "validation" for r in manifest["scenes"]):
        raise ValueError("Training requires independent train and validation scores")
    model = Separator(classes=manifest["classes"], complex_masks=args.complex_masks)
    architecture = "soundroom-complex-unet-v1" if args.complex_masks else "soundroom-mask-unet-v0"
    initialization = None
    if args.initialize:
        previous = load_model(args.initialize)
        if previous.complex_masks or not args.complex_masks:
            raise ValueError("Initialization supports the own magnitude baseline -> complex model")
        state = model.state_dict()
        for name, value in previous.state_dict().items():
            if (
                not name.startswith("output.")
                and name in state
                and state[name].shape == value.shape
            ):
                state[name] = value
        model.load_state_dict(state)
        with torch.no_grad():
            model.output.weight.zero_()
            model.output.bias.zero_()
            # New classes and phase outputs start symmetrically. The earlier
            # four-class head is biased toward its small synthetic domain.
        initialization = dict(
            path=str(args.initialize), sha256=digest(args.initialize), classes=previous.classes
        )
    optimizer = torch.optim.AdamW(model.parameters(), lr=0.001, weight_decay=0.0001)
    args.output.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).read_bytes()
    (args.output / "training-source.py").write_bytes(source)
    source_hash = hashlib.sha256(source).hexdigest()
    best = -math.inf
    start = time.monotonic()
    history = []
    cache = {}
    for step in range(1, args.steps + 1):
        model.train()
        batch = []
        for _ in range(args.batch):
            pool = rng.choice(list(origins.values())) if len(origins) > 1 else training
            record = rng.choice(pool)
            if record["path"] not in cache:
                if len(cache) >= 12:
                    cache.pop(next(iter(cache)))
                cache[record["path"]] = load_scene(args.corpus, record, manifest["classes"])
            audio = cache[record["path"]]
            length = RATE  # One second crops, all stems and channels share crop.
            # Release padding contains silence. Prefer active crops; retain 10%
            # unrestricted crops so silence is still represented honestly.
            prefer_active = rng.random() < 0.9
            for _attempt in range(12):
                offset = rng.randrange(max(1, audio.shape[-1] - length + 1))
                if (
                    not prefer_active
                    or float(audio[:, :, offset : offset + length].square().mean()) > 1e-6
                ):
                    break
            crop = audio[:, :, offset : offset + length].clone()
            crop = F.pad(crop, (0, max(0, length - crop.shape[-1])))
            # Recombination after split, with gain, polarity and source absence.
            gains = torch.tensor(
                [
                    10 ** rng.uniform(-0.25, 0.25) * rng.choice([-1, 1]) * (rng.random() > 0.1)
                    for _ in manifest["classes"]
                ]
            )
            if not gains.any():
                gains[0] = 1
            batch.append(crop * gains[:, None, None])
        target = spectrum(torch.stack(batch))
        mixture = target.sum(1)
        prediction = model(mixture)
        # Power-ratio teacher + compressed spectral error. No dry/wet mismatch.
        power = target.abs().square()
        ideal = power / power.sum(1, keepdim=True).clamp_min(1e-12)
        mask_error = (
            (prediction - ideal * mixture[:, None]).abs()
            * mixture.abs()[:, None].clamp_min(1e-6).pow(-0.7)
        ).mean()
        spectral_error = (
            (prediction.abs().clamp_min(1e-8).pow(0.3) - target.abs().clamp_min(1e-8).pow(0.3))
            .abs()
            .mean()
        )
        if args.complex_masks:
            loss = complex_loss(prediction, target) + 0.1 * mask_error
        else:
            loss = mask_error + spectral_error
        if not torch.isfinite(loss):
            raise RuntimeError("Nonfinite training loss")
        optimizer.zero_grad()
        loss.backward()
        nn.utils.clip_grad_norm_(model.parameters(), 5)
        optimizer.step()
        if step % 10 == 0 or step == 1:
            print(
                f"step {step}/{args.steps} loss={float(loss):.5f} elapsed={time.monotonic() - start:.1f}s",
                flush=True,
            )
        if step % args.validate_every == 0 or step == args.steps:
            result = evaluate(
                model, args.corpus, manifest, "validation", limit=args.validation_limit or None
            )
            score = result["meanSiSDRImprovement"]
            history.append(dict(step=step, loss=float(loss), validation=result))
            print(f"validation SI-SDR improvement {score:.3f} dB", flush=True)
            if score > best:
                best = score
                checkpoint = dict(
                    schema=1,
                    architecture=architecture,
                    state=model.state_dict(),
                    classes=model.classes,
                    sampleRate=RATE,
                    fft=FFT,
                    hop=HOP,
                    step=step,
                    seed=args.seed,
                    trainingSourceSHA256=source_hash,
                    ownCheckpointInitialization=initialization,
                    corpusSHA256=digest(args.corpus / "corpus.json"),
                    vocalsTrained="vocals" in model.classes,
                )
                temporary = args.output / "best.pt.part"
                torch.save(checkpoint, temporary)
                temporary.replace(args.output / "best.pt")
    report = dict(
        architecture=architecture,
        parameters=sum(p.numel() for p in model.parameters()),
        seed=args.seed,
        steps=args.steps,
        batch=args.batch,
        elapsedSeconds=time.monotonic() - start,
        torchVersion=torch.__version__,
        threads=torch.get_num_threads(),
        corpusSHA256=digest(args.corpus / "corpus.json"),
        trainingSourceSHA256=source_hash,
        history=history,
        ownCheckpointInitialization=initialization,
        originSampling={key: len(value) for key, value in origins.items()},
        selection=f"Best mean validation SI-SDR improvement, limit {args.validation_limit or 'all'}. Test not used.",
    )
    (args.output / "training.json").write_text(json.dumps(report, indent=2) + "\n")


def load_model(path):
    checkpoint = torch.load(path, map_location="cpu", weights_only=True)
    if checkpoint.get("architecture") not in [
        "soundroom-mask-unet-v0",
        "soundroom-complex-unet-v1",
    ] or checkpoint.get("classes") not in [CLASSES, ALL_CLASSES, BINARY_CLASSES]:
        raise ValueError("Unsupported checkpoint")
    width = checkpoint.get("width", 8)
    if width not in [8, 12, 16, 24, 32]:
        raise ValueError("Unsupported model width")
    model = Separator(
        width=width,
        classes=checkpoint["classes"],
        complex_masks=checkpoint["architecture"] == "soundroom-complex-unet-v1",
    )
    model.inference_frames = checkpoint.get("inferenceFrames", RATE * 3)
    model.pad_context = checkpoint.get("padContext", False)
    if not isinstance(model.inference_frames, int) or not 2 <= model.inference_frames <= RATE * 16:
        raise ValueError("Unsupported inference context")
    model.load_state_dict(checkpoint["state"], strict=True)
    model.eval()
    return model


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--threads", type=int, default=4)
    commands = parser.add_subparsers(dest="command", required=True)
    training = commands.add_parser("train")
    training.add_argument("--corpus", type=Path, default=ROOT / "build/Training/soundroom-pilot-v1")
    training.add_argument(
        "--output", type=Path, default=ROOT / "build/Training/models/soundroom-mask-v0"
    )
    training.add_argument("--steps", type=int, default=300)
    training.add_argument("--batch", type=int, default=2)
    training.add_argument("--validate-every", type=int, default=50)
    training.add_argument("--seed", type=int, default=20261015)
    training.add_argument("--complex-masks", action="store_true")
    training.add_argument(
        "--initialize",
        type=Path,
        help="Own earlier magnitude checkpoint; never auto-downloads weights",
    )
    training.add_argument(
        "--validation-limit", type=int, default=4, help="0 uses the complete validation set"
    )
    evaluation = commands.add_parser("evaluate")
    evaluation.add_argument("--corpus", type=Path, required=True)
    evaluation.add_argument("--checkpoint", type=Path, required=True)
    evaluation.add_argument("--split", choices=["validation", "test"], default="test")
    evaluation.add_argument("--output", type=Path, required=True)
    separation = commands.add_parser("separate")
    separation.add_argument("input", type=Path)
    separation.add_argument("output", type=Path)
    separation.add_argument("--checkpoint", type=Path, required=True)
    separation.add_argument("--source", choices=ALL_CLASSES + ["accompaniment"], required=True)
    separation.add_argument("--remove", action="store_true")
    args = parser.parse_args()
    if not 1 <= args.threads <= 16:
        parser.error("Use 1–16 CPU threads")
    torch.set_num_threads(args.threads)
    if args.command == "train":
        if args.validate_every < 1 or args.validation_limit < 0:
            parser.error("Validation interval must be positive")
        train(args)
    elif args.command == "evaluate":
        args.output.mkdir(parents=True, exist_ok=True)
        report = evaluate(
            load_model(args.checkpoint),
            args.corpus,
            read_corpus(args.corpus),
            args.split,
            audition=args.output / "audition",
        )
        report["checkpointSHA256"] = digest(args.checkpoint)
        (args.output / "evaluation.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({k: v for k, v in report.items() if k != "examples"}, indent=2))
    else:
        if args.input.resolve() == args.output.resolve() or args.output.exists():
            parser.error("Output must be a new file")
        info = sf.info(args.input)
        if info.frames > RATE * 600:
            parser.error("Input is longer than ten minutes")
        audio, rate = sf.read(args.input, dtype="float32", always_2d=True)
        if (
            rate != RATE
            or audio.shape[1] not in [1, 2]
            or not 0 < len(audio) <= RATE * 600
            or not np.isfinite(audio).all()
        ):
            parser.error("Input must be finite 48 kHz mono/stereo, at most 10 minutes")
        if audio.shape[1] == 1:
            audio = np.repeat(audio, 2, axis=1)
        original = torch.from_numpy(audio.T.copy())
        model = load_model(args.checkpoint)
        if args.source not in model.classes:
            parser.error("This checkpoint was not trained for that source")
        selected = predict(model, original)[model.classes.index(args.source)]
        result = original - selected if args.remove else selected
        if not torch.isfinite(result).all():
            raise RuntimeError("Nonfinite separation result")
        sf.write(args.output, result.T.numpy(), RATE, subtype="FLOAT")
        print(
            f"Wrote {args.output}: {len(audio)} frames, 48 kHz stereo; experimental {args.source}"
        )


if __name__ == "__main__":
    main()
