#!/usr/bin/env python3
"""Controlled vocals/rest experiments. Own weights, raw gain, frozen evaluation."""
import argparse
import importlib.util
import json
import math
from pathlib import Path
import random
import shutil
import time

import numpy as np
import soundfile as sf
import torch

spec = importlib.util.spec_from_file_location("separator", Path(__file__).with_name("soundroom-separator.py"))
base = importlib.util.module_from_spec(spec); spec.loader.exec_module(base)
ROOT, RATE = base.ROOT, base.RATE
stream_spec = importlib.util.spec_from_file_location("mixtures", Path(__file__).with_name("vocal-mixture-stream.py"))
stream = importlib.util.module_from_spec(stream_spec); stream_spec.loader.exec_module(stream)

# Internal research gates, declared before the generalization runs. These are
# minimums for considering listening, not an industry benchmark or launch bar.
GATES = dict(minimumVocalSDR=6., minimumReferenceGain=.89, maximumReferenceGain=1.12,
             maximumAbsentVocalEnergyFraction=.01, humanListeningRequired=True)


def fidelity_loss(prediction, target, gain_weight=1.):
    loss = base.complex_loss(prediction, target)
    energy = target.abs().square().sum(dim=(2, 3, 4))
    gain = (prediction * target.conj()).real.sum(dim=(2, 3, 4)) / energy.clamp_min(1e-8)
    active = energy > target.sum(1).abs().square().sum(dim=(1, 2, 3))[:, None] * 1e-5
    gain_error = ((gain - 1).square() * active).sum() / active.sum().clamp_min(1)
    return loss + gain_weight * gain_error


def binary(target):
    return torch.stack([target[:-1].sum(0), target[-1]])


def checkpoint(model, step, output, **metadata):
    record = dict(schema=1, architecture="soundroom-complex-unet-v1", classes=base.BINARY_CLASSES,
                  width=model.width, state=model.state_dict(), sampleRate=RATE, fft=base.FFT, hop=base.HOP,
                  inferenceFrames=getattr(model, "inference_frames", RATE * 3),
                  padContext=getattr(model, "pad_context", False), step=step, **metadata)
    temporary = output.with_suffix(".pt.part"); torch.save(record, temporary); temporary.replace(output)


def diagnostic(args):
    torch.manual_seed(args.seed)
    manifest = base.read_corpus(args.corpus)
    target, source = None, None
    for record in manifest["scenes"]:
        if record["split"] != "train": continue
        audio = binary(base.load_scene(args.corpus, record, manifest["classes"]))
        candidate = audio[..., :int(args.seconds * RATE)]
        if (candidate.square().mean(dim=(1, 2)) > 1e-5).all():
            target, source = candidate, record; break
    if target is None: raise ValueError("No active two-source training fixture")
    args.output.mkdir(parents=True, exist_ok=True)
    spectrum = base.spectrum(target)[None]; mixture = spectrum.sum(1)
    ratio = spectrum[:, 1] / torch.where(mixture.abs() > 1e-8, mixture, torch.ones_like(mixture) * 1e-8)
    bounded = torch.complex(ratio.real.clamp(-.5, 1.5), ratio.imag.clamp(-.5, .5)) * mixture
    oracle = torch.stack([mixture - bounded, bounded], dim=1)
    oracle_audio = base.waveform(oracle[0], target.shape[-1])
    oracle_metrics = base.metrics(oracle_audio, target, target.sum(0), base.BINARY_CLASSES)
    model = base.Separator(width=args.width, classes=base.BINARY_CLASSES, complex_masks=True)
    optimizer = torch.optim.Adam(model.parameters(), lr=.002)
    start = time.monotonic(); history = []
    for step in range(1, args.steps + 1):
        model.train(); prediction = model(mixture)
        loss = fidelity_loss(prediction, spectrum, args.gain_weight)
        optimizer.zero_grad(); loss.backward(); torch.nn.utils.clip_grad_norm_(model.parameters(), 5); optimizer.step()
        if step == 1 or step % 25 == 0 or step == args.steps:
            model.eval()
            with torch.no_grad(): result = base.waveform(model(mixture)[0], target.shape[-1])
            measures = base.metrics(result, target, target.sum(0), base.BINARY_CLASSES)
            row = dict(step=step, loss=float(loss), elapsedSeconds=time.monotonic() - start, metrics=measures)
            history.append(row)
            print(json.dumps(row), flush=True)
    vocal = history[-1]["metrics"][1]
    passed = .89 <= vocal["referenceGain"] <= 1.12 and vocal["sdr"] >= min(12., oracle_metrics[1]["sdr"] - 3)
    report = dict(task="Fixed training-clip overfit diagnostic, not generalization", source=source,
                  width=args.width, parameters=sum(p.numel() for p in model.parameters()), steps=args.steps,
                  gainWeight=args.gain_weight, contextSeconds=target.shape[-1] / RATE,
                  boundedOracle=oracle_metrics, history=history, gainAndReconstructionGatePassed=passed,
                  sourceSHA256=base.digest(Path(__file__)), helperSHA256=base.digest(Path(base.__file__)))
    (args.output / "diagnostic.json").write_text(json.dumps(report, indent=2) + "\n")
    checkpoint(model, args.steps, args.output / "diagnostic.pt", diagnosticOnly=True)
    sf.write(args.output / "mixture.wav", target.sum(0).T.numpy(), RATE, subtype="FLOAT")
    for i, name in enumerate(base.BINARY_CLASSES):
        sf.write(args.output / f"{name}-reference.wav", target[i].T.numpy(), RATE, subtype="FLOAT")
        sf.write(args.output / f"{name}-estimated.wav", result[i].T.numpy(), RATE, subtype="FLOAT")


def data_stream(args):
    return stream.Mixtures(args.bank, args.voices, args.data, seed=args.recipe_seed)


def snapshot(output):
    destination = output / "executed-source"; destination.mkdir(parents=True, exist_ok=True)
    hashes = {}
    for filename in ["vocal-separator-experiments.py", "vocal-mixture-stream.py", "soundroom-separator.py",
                     "generate-separator-recordings.py", "generate-separator-corpus.py", "build-vocal-accompaniment-bank.py"]:
        path = Path(__file__).with_name(filename)
        shutil.copy2(path, destination / filename); hashes[filename] = base.digest(path)
    return hashes


def summarize(rows, reconstruction):
    active = [r for r in rows if not r["vocals"]["absent"]]
    absent = [r for r in rows if r["vocals"]["absent"]]
    mean = lambda key: float(np.mean([r["vocals"][key] for r in active])) if active else None
    gain = mean("referenceGain")
    leak = float(np.mean([r["vocalEnergyFraction"] for r in absent])) if absent else None
    sdr = mean("sdr")
    mixed = [r for r in active if not r["accompaniment"].get("absent", False)]
    # Selection sees validation only and penalizes attenuation and hallucination.
    score = sdr - 6 * abs(1 - gain) - 10 * (leak or 0) if active else -1e6
    passed = (sdr is not None and sdr >= GATES["minimumVocalSDR"] and
              GATES["minimumReferenceGain"] <= gain <= GATES["maximumReferenceGain"] and
              leak is not None and leak <= GATES["maximumAbsentVocalEnergyFraction"])
    return dict(examples=len(rows), activeVocals=len(active), absentVocals=len(absent), vocalSDR=sdr,
                vocalSiSDR=mean("siSDR"), vocalSiSDRImprovement=mean("siSDRImprovement"), referenceGain=gain,
                absentVocalEnergyFraction=leak, selectionScore=score, numericalGatePassed=passed,
                mixedVocalSiSDRImprovement=float(np.mean([r["vocals"]["siSDRImprovement"] for r in mixed])) if mixed else None,
                siImprovementNote="All-active improvement includes solo vocals where the input is already a perfect target; mixed-only improvement excludes those cases",
                reconstructionMaxError=max(reconstruction, default=0.))


@torch.no_grad()
def evaluate_recipes(model, dataset, split, count=24, external=None, audition=None, legacy=False):
    model.eval(); rows, reconstruction = [], []
    count = len(external) + 16 if external else count
    for index in range(count):
        kwargs = {}
        if external:
            if index < len(external):
                kwargs = dict(external=external[index], category="mixed")
            elif index < len(external) + 8: kwargs = dict(category="no_vocals")
            else: kwargs = dict(external=external[(index - len(external) - 8) % len(external)], category="solo_vocals")
        target, recipe = dataset.sample(split, index, seconds=4., **kwargs)
        mixture = target.sum(0); prediction = base.predict(model, mixture)
        if legacy: prediction = binary(prediction)
        measures = base.metrics(prediction, target, mixture, base.BINARY_CLASSES)
        energy_fraction = float(prediction[1].square().sum() / mixture.square().sum().clamp_min(1e-10))
        rows.append(dict(recipe=recipe, accompaniment=measures[0], vocals=measures[1], vocalEnergyFraction=energy_fraction))
        reconstruction.append(float((prediction.sum(0) - mixture).abs().max()))
        if audition and (index < 4 or (external and index in [len(external), len(external) + 8])):
            folder = audition / f"{index:03}"; folder.mkdir(parents=True, exist_ok=True)
            for label, audio in [("mixture", mixture), ("vocals-reference", target[1]), ("accompaniment-reference", target[0]),
                                 ("vocals-estimated", prediction[1]), ("accompaniment-estimated", prediction[0])]:
                sf.write(folder / f"{label}.wav", audio.T.numpy(), RATE, subtype="FLOAT")
    return dict(summary=summarize(rows, reconstruction), examples=rows, split=split,
                evaluation="Independent vocadito; no checkpoint selection" if external else "VocalSet regression" if split == "test" else "Validation only")


def train_online(args):
    if (args.output / "run.json").exists() or (args.output / "best.pt").exists():
        raise ValueError("Run already exists; choose a fresh output directory")
    dataset = data_stream(args); args.output.mkdir(parents=True, exist_ok=True)
    hashes = snapshot(args.output)
    torch.manual_seed(args.seed)
    model = base.Separator(width=args.width, classes=base.BINARY_CLASSES, complex_masks=True)
    model.inference_frames = round(args.seconds * RATE); model.pad_context = True
    optimizer = torch.optim.AdamW(model.parameters(), lr=.001, weight_decay=.0001)
    setup = dict(schema=1, task="vocals vs accompaniment", initialization="random; no pretrained weights", seed=args.seed,
                 steps=args.steps, batch=args.batch, contextSeconds=args.seconds, width=args.width, gainWeight=args.gain_weight,
                 parameters=sum(p.numel() for p in model.parameters()), data=dataset.identity, dataFingerprint=dataset.fingerprint,
                 sourceSHA256=hashes, gates=GATES, selection="Full 24-recipe validation; SDR - 6*abs(1-gain) - 10*absent-energy-fraction",
                 trainingRecipeCount=args.steps * args.batch, recipeCountAvailable=4096,
                 physicalMicroBatch=1, effectiveBatch=args.batch,
                 possibleRecipeAudioHours=4096 * 4 / 3600, presentedAudioSeconds=args.steps * args.batch * args.seconds,
                 versions=dict(torch=torch.__version__, numpy=np.__version__), threads=args.threads)
    (args.output / "run.json").write_text(json.dumps(setup, indent=2) + "\n")
    best = -math.inf; history = []; start = time.monotonic()
    for step in range(1, args.steps + 1):
        model.train()
        optimizer.zero_grad(); loss_value = 0.
        for i in range(args.batch):
            target = dataset.sample("train", ((step - 1) * args.batch + i) % 4096, args.seconds)[0][None]
            target_spectrum = base.spectrum(target)
            prediction = model(target_spectrum.sum(1))
            loss = fidelity_loss(prediction, target_spectrum, args.gain_weight)
            if not torch.isfinite(loss): raise ValueError("Nonfinite training loss")
            loss_value += float(loss.detach()) / args.batch
            (loss / args.batch).backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 5); optimizer.step()
        loss = loss_value
        if step % 25 == 0 or step == 1: print(json.dumps(dict(step=step, loss=float(loss), elapsedSeconds=time.monotonic()-start)), flush=True)
        if step % args.validate_every == 0 or step == args.steps:
            report = evaluate_recipes(model, dataset, "validation")
            report.update(step=step, loss=float(loss), elapsedSeconds=time.monotonic()-start)
            history.append(report)
            if report["summary"]["selectionScore"] > best:
                best = report["summary"]["selectionScore"]
                checkpoint(model, step, args.output / "best.pt", dataFingerprint=dataset.fingerprint, trainingSourceSHA256=hashes,
                           inferenceOnly=True, validation=report["summary"])
                (args.output / "best-validation.json").write_text(json.dumps(report, indent=2) + "\n")
            (args.output / "history.json").write_text(json.dumps(history, indent=2) + "\n")
            print(json.dumps(dict(validation=report["summary"], step=step, elapsedSeconds=report["elapsedSeconds"])), flush=True)
    setup.update(elapsedSeconds=time.monotonic()-start, bestCheckpointSHA256=base.digest(args.output / "best.pt"), bestValidationScore=best,
                 status="Completed research run; no app integration; human listening pending")
    (args.output / "run.json").write_text(json.dumps(setup, indent=2) + "\n")


def evaluate_command(args):
    dataset = data_stream(args)
    model = base.load_model(args.checkpoint)
    if model.classes not in [base.ALL_CLASSES, base.BINARY_CLASSES]: raise ValueError("Model has no vocal output")
    external = dataset.independent(args.independent) if args.independent else None
    args.output.mkdir(parents=True, exist_ok=True)
    if (args.output / "evaluation.json").exists(): raise ValueError("Evaluation exists; use a fresh output directory")
    report = evaluate_recipes(model, dataset, "test", count=24, external=external, audition=args.output / "audio", legacy=model.classes == base.ALL_CLASSES)
    report.update(checkpointSHA256=base.digest(args.checkpoint), checkpoint=str(args.checkpoint), dataFingerprint=dataset.fingerprint,
                  sourceSHA256=snapshot(args.output), gates=GATES, listeningCompleted=False)
    if external:
        report.update(independentManifestSHA256=base.digest(args.independent / "sources.json"),
                      independentClips=len(external), independentSingers=len({r["singer"] for r in external}))
    (args.output / "evaluation.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report["summary"]), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--threads", type=int, default=4)
    commands = parser.add_subparsers(dest="command", required=True)
    diag = commands.add_parser("diagnostic")
    diag.add_argument("--corpus", type=Path, default=ROOT / "build/Training/recorded-pilot-v1")
    diag.add_argument("--output", type=Path, required=True)
    diag.add_argument("--steps", type=int, default=300)
    diag.add_argument("--seconds", type=float, default=2.)
    diag.add_argument("--width", type=int, choices=[8, 12, 16, 24, 32], default=8)
    diag.add_argument("--gain-weight", type=float, default=1.)
    diag.add_argument("--seed", type=int, default=20261017)
    online = commands.add_parser("train", help="Paired online data; select using gain-aware validation only")
    online.add_argument("--output", type=Path, required=True)
    online.add_argument("--steps", type=int, default=400)
    online.add_argument("--batch", type=int, default=2)
    online.add_argument("--seconds", type=float, choices=[1., 4.], default=4.)
    online.add_argument("--width", type=int, choices=[8, 16], default=8)
    online.add_argument("--gain-weight", type=float, default=1.)
    online.add_argument("--validate-every", type=int, default=100)
    online.add_argument("--seed", type=int, default=20261020)
    evaluation = commands.add_parser("evaluate", help="Locked checkpoint regression or independent evaluation")
    evaluation.add_argument("--checkpoint", type=Path, required=True)
    evaluation.add_argument("--output", type=Path, required=True)
    evaluation.add_argument("--independent", type=Path, help="Verified vocadito evaluation directory")
    for command in [online, evaluation]:
        command.add_argument("--bank", type=Path, default=ROOT / "build/Training/vocal-bank-v3")
        command.add_argument("--voices", type=Path, default=ROOT / "build/Training/vocal-voices-v3")
        command.add_argument("--data", type=Path, default=ROOT / "build/Training/data")
        command.add_argument("--recipe-seed", type=int, default=20261019)
    args = parser.parse_args()
    if not 1 <= args.threads <= 8 or not 1 <= getattr(args, "steps", 1) <= 10000 or not .1 <= getattr(args, "seconds", 1) <= 8:
        parser.error("Invalid bounded experiment settings")
    if args.command == "train" and (not 1 <= args.batch <= 8 or args.steps * args.batch > 4096 or args.validate_every < 1):
        parser.error("Batch 1–8, at most 4096 recipes and a positive validation interval required")
    torch.set_num_threads(args.threads)
    if args.command == "diagnostic": diagnostic(args)
    elif args.command == "train": train_online(args)
    else: evaluate_command(args)


if __name__ == "__main__": main()
