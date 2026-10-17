#!/usr/bin/env python3
"""Soundroom's offline, local-only Demucs bridge.

Only --prepare-models uses the network. Normal inference loads an allowlisted
provider model from the local repository after verifying its recorded SHA-256.
The provider's htdemucs_ft bag gives each source weight 1 in exactly one model;
loading that source's model produces the same selected stem with lower memory.
Provenance: https://github.com/facebookresearch/demucs/blob/main/demucs/remote/
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import math
import os
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

RATE = 48000
SEGMENT_SECONDS = 7.8
MAX_FRAMES = RATE * 60 * 120
MAX_MODEL_BYTES = 1024 * 1024 * 1024
MODEL_ROOT = "https://dl.fbaipublicfiles.com/demucs/hybrid_transformer/"
PROVIDER = "https://github.com/facebookresearch/demucs"
WEIGHTS_LICENSE_SOURCE = "https://github.com/facebookresearch/demucs/issues/327"
WEIGHTS_USAGE_STATEMENT = "Provided only for scientific purposes (upstream author statement)."
WEIGHTS_NOTICE = f"""Demucs code and pretrained weights have separate licensing.
DEMUCS-LICENSE.txt contains the MIT license for the code, not the weights.
{WEIGHTS_USAGE_STATEMENT}
Source: {WEIGHTS_LICENSE_SOURCE}
The statement predates HTDemucs v4. No later upstream grant clearing these
v4 weights for general/commercial use has been verified by this project.
Converting weights or downloading them separately does not establish permission.
Reviewed 2026-10-15; see Soundroom's THIRD_PARTY_NOTICES.md.
"""
EXPECTED_PACKAGES = {
    "demucs": "4.0.1",
    "torch": "2.0.1",
    "torchaudio": "2.0.2",
    "numpy": "1.26.4",
    "soundfile": "0.12.1",
}
MODEL_FILES = {
    "955717e8": "955717e8-8726e21a.th",
    "f7e0c4bc": "f7e0c4bc-ba3fe64a.th",
    "d12395a8": "d12395a8-e57c48e6.th",
    "92cfc3b6": "92cfc3b6-ef3bcb9c.th",
    "04573f0d": "04573f0d-f3cf25b2.th",
    "5c90dfd2": "5c90dfd2-34c22ccb.th",
}
TARGETS = ("vocals", "drums", "bass", "other", "guitar", "piano")
QUALITY_MODELS = {
    "vocals": "04573f0d",
    "drums": "f7e0c4bc",
    "bass": "d12395a8",
    "other": "92cfc3b6",
}
MODELS = {
    "quality": {"name": "htdemucs_ft", "experimental": False, "targets": tuple(QUALITY_MODELS)},
    "standard": {"name": "htdemucs", "experimental": False, "targets": tuple(QUALITY_MODELS)},
    "extended": {"name": "htdemucs_6s", "experimental": True, "targets": TARGETS},
}
_TORCH_THREADS_SET = False
MIT_LICENSE = """MIT License

Copyright (c) Meta Platforms, Inc. and affiliates.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""


class WorkerError(RuntimeError):
    pass


def emit(kind: str, **values) -> None:
    print(json.dumps({"type": kind, **values}, allow_nan=False, separators=(",", ":")), flush=True)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_manifest(folder: Path) -> dict:
    path = folder / "manifest.json"
    if not path.is_file() or path.stat().st_size > 1024 * 1024:
        return {"schema": 1, "provider": PROVIDER, "files": {}}
    try:
        manifest = json.loads(path.read_text())
        if (
            manifest.get("schema") == 1
            and manifest.get("provider") == PROVIDER
            and isinstance(manifest.get("files"), dict)
        ):
            return manifest
    except (OSError, ValueError, AttributeError):
        pass
    return {"schema": 1, "provider": PROVIDER, "files": {}}


def checksum_prefix(signature: str) -> str:
    return MODEL_FILES[signature].split("-", 1)[1].removesuffix(".th")


def verified_model(folder: Path, signature: str, manifest: dict, full: bool = False) -> bool:
    path = folder / MODEL_FILES[signature]
    record = manifest["files"].get(MODEL_FILES[signature], {})
    if not isinstance(record, dict) or not path.is_file() or path.is_symlink():
        return False
    digest = record.get("sha256", "")
    if (
        not isinstance(digest, str)
        or len(digest) != 64
        or not digest.startswith(checksum_prefix(signature))
    ):
        return False
    stamp = path.stat()
    if record.get("bytes") != stamp.st_size or record.get("mtime_ns") != stamp.st_mtime_ns:
        return False
    if record.get("url") != MODEL_ROOT + MODEL_FILES[signature]:
        return False
    return not full or sha256_file(path) == digest


def signature_for(model: str, target: str) -> str:
    if target not in MODELS[model]["targets"]:
        raise WorkerError(f"{target} requires the experimental six-source model")
    if model == "quality":
        return QUALITY_MODELS[target]
    return "955717e8" if model == "standard" else "5c90dfd2"


def runtime_status(folder: Path) -> dict:
    """No torch/demucs imports and no network or model deserialization."""
    installed = {}
    for package in EXPECTED_PACKAGES:
        try:
            installed[package] = importlib.metadata.version(package)
        except importlib.metadata.PackageNotFoundError:
            installed[package] = None
    packages_ready = all(installed[p] == v for p, v in EXPECTED_PACKAGES.items())
    manifest = load_manifest(folder)
    models = {}
    for name, info in MODELS.items():
        available = [
            target
            for target in info["targets"]
            if verified_model(folder, signature_for(name, target), manifest)
        ]
        models[name] = {
            **info,
            "available": packages_ready and len(available) == len(info["targets"]),
            "availableTargets": available if packages_ready else [],
        }
    return {
        "runtimeReady": packages_ready,
        "packages": installed,
        "expectedPackages": EXPECTED_PACKAGES,
        "models": models,
        "modelsDir": str(folder),
        "sampleRate": RATE,
        "offline": True,
        "provider": PROVIDER,
        "license": "unresolved",
        "codeLicense": "MIT",
        "weightsLicense": "unresolved",
        "weightsUsageStatement": WEIGHTS_USAGE_STATEMENT,
        "weightsLicenseSource": WEIGHTS_LICENSE_SOURCE,
        "extendedWarning": "Guitar and piano are experimental; piano commonly contains bleed and artifacts.",
    }


def atomic_text(path: Path, content: str) -> None:
    fd, name = tempfile.mkstemp(prefix=f".{path.name}-", suffix=".tmp", dir=path.parent)
    temporary = Path(name)
    try:
        with os.fdopen(fd, "w") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def download_model(
    folder: Path, signature: str, manifest: dict, opener=urllib.request.urlopen
) -> None:
    """Only allowlisted Meta URLs; verify provider hash before publishing bytes."""
    filename = MODEL_FILES[signature]
    weight_terms = {
        "license": "unresolved",
        "licenseSource": WEIGHTS_LICENSE_SOURCE,
        "usageStatement": WEIGHTS_USAGE_STATEMENT,
    }
    if verified_model(folder, signature, manifest, full=True):
        # Repair old MIT labels without downloading or changing verified bytes.
        manifest["files"][filename].update(weight_terms)
        atomic_text(folder / "manifest.json", json.dumps(manifest, indent=2) + "\n")
        emit("prepared", file=filename, cached=True)
        return
    url = MODEL_ROOT + filename
    fd, name = tempfile.mkstemp(prefix=f".{filename}-", suffix=".download", dir=folder)
    temporary = Path(name)
    try:
        digest = hashlib.sha256()
        total = 0
        request = urllib.request.Request(
            url, headers={"User-Agent": "Soundroom-SourceSeparation/1"}
        )
        with os.fdopen(fd, "wb") as output, opener(request, timeout=45) as response:
            if not response.geturl().startswith("https://"):
                raise WorkerError("Model download redirected to an insecure URL")
            content_length = response.headers.get("Content-Length")
            expected_bytes = int(content_length) if content_length is not None else None
            if expected_bytes is not None and not 0 < expected_bytes <= MAX_MODEL_BYTES:
                raise WorkerError("Provider returned an invalid model size")
            for chunk in iter(lambda: response.read(1024 * 1024), b""):
                total += len(chunk)
                if total > MAX_MODEL_BYTES:
                    raise WorkerError("Provider model exceeds the permitted download size")
                output.write(chunk)
                digest.update(chunk)
            output.flush()
            os.fsync(output.fileno())
        checksum = digest.hexdigest()
        if total == 0 or (expected_bytes is not None and total != expected_bytes):
            raise WorkerError("Incomplete model download")
        if not checksum.startswith(checksum_prefix(signature)):
            raise WorkerError(f"Provider checksum mismatch for {filename}")
        destination = folder / filename
        os.replace(temporary, destination)
        manifest["files"][filename] = {
            "url": url,
            "providerChecksumPrefix": checksum_prefix(signature),
            "sha256": checksum,
            "bytes": total,
            "mtime_ns": destination.stat().st_mtime_ns,
            "preparedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            **weight_terms,
        }
        atomic_text(folder / "manifest.json", json.dumps(manifest, indent=2) + "\n")
        emit("prepared", file=filename, cached=False, bytes=total, sha256=checksum)
    finally:
        temporary.unlink(missing_ok=True)


def prepare_models(folder: Path, model: str) -> None:
    folder.mkdir(parents=True, exist_ok=True)
    # Prevent two explicit installers from losing each other's manifest entries.
    import fcntl

    with (folder / ".prepare.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        manifest = load_manifest(folder)
        requested = MODELS if model == "all" else (model,)
        signatures = {
            signature_for(name, target) for name in requested for target in MODELS[name]["targets"]
        }
        for signature in sorted(signatures):
            download_model(folder, signature, manifest)
        atomic_text(folder / "DEMUCS-LICENSE.txt", MIT_LICENSE)
        atomic_text(folder / "DEMUCS-WEIGHTS-NOTICE.txt", WEIGHTS_NOTICE)
        emit("ready", **runtime_status(folder))


def triangle_weights(length: int) -> list[float]:
    """Positive triangular overlap-add weights, including both endpoint samples."""
    if length < 2:
        return [1.0] * length
    half = length // 2
    peak = max(half, length - half)
    return [min(i + 1, length - i) / peak for i in range(length)]


def overlap_layout(frames: int, segment: int):
    stride = segment * 3 // 4
    if frames < 0 or segment < 4:
        raise WorkerError("Invalid streaming layout")
    for start in range(0, frames, stride):
        yield start, min(segment, frames - start), min(stride, frames - start)


def scratch_path(output: Path) -> Path:
    # The parent Rust job owns this exact name and can remove it after SIGKILL,
    # which cannot execute Python's finally block. UUID outputs avoid collisions.
    return output.with_name(f".{output.name}.separation.tmp")


def read_stereo(stream, frames, np):
    block = stream.read(frames, dtype="float32", always_2d=True)
    if block.shape[1] == 1:
        block = np.repeat(block, 2, axis=1)
    if not np.isfinite(block).all():
        raise WorkerError("Source audio contains non-finite samples")
    return block


def normalization(stream, np):
    # Parallel-variance update avoids cancellation for quiet audio with DC.
    count, mean, m2, channel_power = 0, 0.0, 0.0, 0.0
    while True:
        block = read_stereo(stream, RATE * 4, np)
        if not len(block):
            break
        reference = block.astype("float64").mean(axis=1)
        n = len(reference)
        block_mean = float(reference.mean())
        block_m2 = float(np.square(reference - block_mean).sum())
        delta = block_mean - mean
        m2 += block_m2 + delta * delta * count * n / (count + n)
        mean += delta * n / (count + n)
        count += n
        channel_power += float(np.square(block.astype("float64")).sum()) * 0.5
    stream.seek(0)
    if count == 0:
        return 0.0, 0.0, 0.0
    std = math.sqrt(max(0.0, m2 / count))
    rms = math.sqrt(max(0.0, channel_power / count))
    # Anti-phase stereo has a silent mono reference, while carrying real music.
    return mean, std if std > rms * 1e-4 else rms, rms


def render(args, model_loader=None, predictor=None) -> None:
    """Write bounded overlapping chunks; preserve source gain and exact frame count.

    Injectable model/predictor callbacks are for integration tests with a small
    known linear model. Production always uses local verified Demucs weights.
    """
    ready = runtime_status(args.models_dir)
    if not ready["runtimeReady"]:
        raise WorkerError(
            "Source separation runtime is not installed. Run scripts/setup-source-separation.sh."
        )
    import numpy as np
    import soundfile as sf
    import torch
    import torchaudio

    if args.input.resolve() == args.output.resolve():
        raise WorkerError("Source and output paths must be different")
    if args.output.exists():
        raise WorkerError("Output already exists")
    global _TORCH_THREADS_SET
    if not _TORCH_THREADS_SET:
        torch.set_num_threads(max(1, min(4, os.cpu_count() or 1)))
        torch.set_num_interop_threads(1)
        _TORCH_THREADS_SET = True
    torch.manual_seed(0)
    signature = signature_for(args.model, args.target)
    manifest = load_manifest(args.models_dir)
    if model_loader is None and not verified_model(args.models_dir, signature, manifest, full=True):
        raise WorkerError(
            "Selected model is not installed or failed checksum verification. Run the setup script."
        )
    # get_model(repo=...) cannot download. Reject any unexpected .th files and
    # YAML bags before deserialization to keep the repository unambiguous.
    if model_loader is None:
        allowed = set(MODEL_FILES.values())
        for path in args.models_dir.iterdir():
            if path.suffix in (".th", ".yaml", ".yml") and path.name not in allowed:
                raise WorkerError(f"Unexpected model repository entry: {path.name}")

    temporary = scratch_path(args.output)
    # Exclusive creation keeps unrelated/pre-existing scratch files intact.
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.close(fd)
    try:
        with sf.SoundFile(args.input) as source:
            if (
                source.samplerate != RATE
                or source.channels not in (1, 2)
                or source.format not in ("WAV", "WAVEX")
            ):
                raise WorkerError("Expected a mono/stereo 48 kHz WAV source")
            frames = source.frames
            if not 0 <= frames <= MAX_FRAMES:
                raise WorkerError("Source exceeds the two-hour processing limit")
            emit("progress", progress=0.0, phase="Analyzing source")
            mean, std, rms = normalization(source, np)
            with sf.SoundFile(
                temporary, "w", samplerate=RATE, channels=2, format="WAV", subtype="FLOAT"
            ) as output:
                if rms <= 1e-10 or frames == 0:
                    # Silence needs no model prediction. Remove still retains
                    # the original (including tiny sub-threshold float values).
                    remaining = frames
                    while remaining:
                        n = min(remaining, RATE * 4)
                        dry = read_stereo(source, n, np)
                        if len(dry) != n:
                            raise WorkerError("Source changed while processing")
                        output.write(
                            dry if args.operation == "remove" else np.zeros((n, 2), dtype="float32")
                        )
                        remaining -= n
                else:
                    if model_loader is None:
                        from demucs.apply import apply_model
                        from demucs.pretrained import get_model

                        model_loader = lambda: get_model(signature, repo=args.models_dir)
                        predictor = lambda model, audio: apply_model(
                            model,
                            audio[None],
                            shifts=0,
                            split=False,
                            progress=False,
                            device="cpu",
                            num_workers=0,
                            segment=SEGMENT_SECONDS,
                        )[0, model.sources.index(args.target)]
                    model = model_loader().cpu().eval()
                    if (
                        model.audio_channels != 2
                        or model.samplerate != 44100
                        or args.target not in model.sources
                    ):
                        raise WorkerError("Provider model has unexpected input/source metadata")
                    to_model = torchaudio.transforms.Resample(
                        RATE,
                        model.samplerate,
                        lowpass_filter_width=64,
                        rolloff=0.95,
                        resampling_method="sinc_interp_kaiser",
                        beta=14.0,
                    )
                    from_model = torchaudio.transforms.Resample(
                        model.samplerate,
                        RATE,
                        lowpass_filter_width=64,
                        rolloff=0.95,
                        resampling_method="sinc_interp_kaiser",
                        beta=14.0,
                    )
                    segment = round(SEGMENT_SECONDS * RATE)
                    pending = np.zeros((segment, 2), dtype="float32")
                    weight_sum = np.zeros(segment, dtype="float32")
                    weights = np.asarray(triangle_weights(segment), dtype="float32")
                    last_progress = -1
                    with torch.inference_mode():
                        for start, n, writable in overlap_layout(frames, segment):
                            source.seek(start)
                            original = read_stereo(source, n, np)
                            if len(original) != n:
                                raise WorkerError("Source changed while processing")
                            audio = torch.from_numpy(original.T.copy())
                            converted = to_model((audio - mean) / std)
                            prediction = predictor(model, converted)
                            if (
                                prediction.shape != converted.shape
                                or not torch.isfinite(prediction).all()
                            ):
                                raise WorkerError("Model returned invalid source audio")
                            selected = from_model(prediction * std + mean)
                            if selected.shape[-1] < n:
                                selected = torch.nn.functional.pad(
                                    selected, (0, n - selected.shape[-1])
                                )
                            selected = selected[:, :n].T.cpu().numpy()
                            if not np.isfinite(selected).all():
                                raise WorkerError("Resampled source contains non-finite audio")
                            pending[:n] += selected * weights[:n, None]
                            weight_sum[:n] += weights[:n]
                            if np.any(weight_sum[:writable] <= 0):
                                raise WorkerError("Invalid overlap normalization")
                            stem = pending[:writable] / weight_sum[:writable, None]
                            source.seek(start)
                            dry = read_stereo(source, writable, np)
                            result = dry - stem if args.operation == "remove" else stem
                            if not np.isfinite(result).all():
                                raise WorkerError("Processed source contains non-finite audio")
                            output.write(result)
                            pending[:-writable] = pending[writable:].copy()
                            pending[-writable:] = 0
                            weight_sum[:-writable] = weight_sum[writable:].copy()
                            weight_sum[-writable:] = 0
                            progress = min(99, int((start + writable) / frames * 100))
                            if progress != last_progress:
                                emit(
                                    "progress",
                                    progress=progress / 100.0,
                                    phase="Separating sources",
                                )
                                last_progress = progress
                output.flush()
                if output.frames != frames:
                    raise WorkerError("Processed audio frame count changed")
        with temporary.open("rb") as finished:
            os.fsync(finished.fileno())
        # Exclusive publication leaves a concurrently created output intact.
        os.link(temporary, args.output)
        emit(
            "complete",
            progress=1.0,
            frames=frames,
            sampleRate=RATE,
            channels=2,
            target=args.target,
            model=MODELS[args.model]["name"],
            operation=args.operation,
        )
    finally:
        temporary.unlink(missing_ok=True)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group()
    action.add_argument("--status", action="store_true")
    action.add_argument("--prepare-models", action="store_true")
    parser.add_argument("--models-dir", type=Path, required=True)
    parser.add_argument(
        "--ready-file",
        type=Path,
        help="Atomically publish local readiness after explicit preparation",
    )
    parser.add_argument("--model", choices=(*MODELS, "all"), default="quality")
    parser.add_argument("--target", choices=TARGETS, default="vocals")
    parser.add_argument("--operation", choices=("extract", "remove"), default="extract")
    parser.add_argument("--input", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.status:
            emit("status", **runtime_status(args.models_dir))
        elif args.prepare_models:
            prepare_models(args.models_dir, args.model)
            if args.ready_file:
                ready = runtime_status(args.models_dir)
                if not ready["runtimeReady"] or not any(
                    model["available"] for model in ready["models"].values()
                ):
                    raise WorkerError("Installed separation runtime did not pass readiness checks")
                atomic_text(
                    args.ready_file,
                    json.dumps(
                        {
                            "schema": 1,
                            **ready,
                            "message": "Local separation models are ready.",
                        },
                        indent=2,
                    )
                    + "\n",
                )
        else:
            if not args.input or not args.output or args.model == "all":
                raise WorkerError("Inference requires --input, --output and a single --model")
            render(args)
        return 0
    except (WorkerError, OSError, ValueError, ImportError, RuntimeError) as error:
        emit("error", message=str(error)[:512])
        return 1


if __name__ == "__main__":
    sys.exit(main())
