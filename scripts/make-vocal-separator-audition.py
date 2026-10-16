#!/usr/bin/env python3
"""Create a local, blinded A/B listening page for locked research checkpoints."""
import argparse
import html
import importlib.util
import json
from pathlib import Path
import random

import soundfile as sf
import torch

spec = importlib.util.spec_from_file_location("experiments", Path(__file__).with_name("vocal-separator-experiments.py"))
experiments = importlib.util.module_from_spec(spec); spec.loader.exec_module(experiments)
base, stream = experiments.base, experiments.stream


def player(label, path):
    return f'<label>{html.escape(label)}<audio controls preload="none" aria-label="{html.escape(label)}" src="{html.escape(path, quote=True)}"></audio></label>'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--checkpoint", type=Path, required=True)
    parser.add_argument("--baseline", type=Path, default=base.ROOT / "build/Training/models/soundroom-complex-v2/best.pt")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--independent", type=Path, default=base.ROOT / "build/Training/vocadito-evaluation")
    parser.add_argument("--bank", type=Path, default=base.ROOT / "build/Training/vocal-bank-v3")
    parser.add_argument("--voices", type=Path, default=base.ROOT / "build/Training/vocal-voices-v3")
    parser.add_argument("--data", type=Path, default=base.ROOT / "build/Training/data")
    parser.add_argument("--recipe-seed", type=int, default=20261019)
    args = parser.parse_args(); torch.set_num_threads(2)
    if args.output.exists(): raise ValueError("Use a fresh audition directory")
    dataset = experiments.data_stream(args)
    voices = dataset.independent(args.independent)
    models = dict(candidate=base.load_model(args.checkpoint), baseline=base.load_model(args.baseline))
    if models["candidate"].classes != base.BINARY_CLASSES or models["baseline"].classes != base.ALL_CLASSES:
        raise ValueError("Expected a binary candidate and six-source previous baseline")
    args.output.mkdir(parents=True)
    cards, records = [], []
    cases = [(f"Mixed vocal {i+1}", i, "mixed", voices[i]) for i in range(3)]
    cases += [("Instrumental only", 41, "no_vocals", None),
              ("Voice-like instrument, no human vocal", 43, "hard_negative", voices[3]),
              ("Solo vocal", 48, "solo_vocals", voices[0]),
              ("Voice against a voice-like instrument", 51, "hard", voices[4])]
    for ci, (title, index, category, external) in enumerate(cases):
        target, recipe = dataset.sample("test", index, external=external, category="hard" if category == "hard_negative" else category)
        if category == "hard_negative": target[1].zero_(); recipe["pairedAblation"] = "Human vocal removed from hard recipe"
        mixture = target.sum(0); estimates = {}
        with torch.no_grad():
            for name, model in models.items():
                prediction = base.predict(model, mixture)
                estimates[name] = experiments.binary(prediction) if model.classes == base.ALL_CLASSES else prediction
        order = ["candidate", "baseline"]; random.Random(f"blinded-audio/20261021/{ci}").shuffle(order)
        audios = dict(mixture=mixture, reference=target[1], **{label:estimates[name][1] for label, name in zip(["A", "B"], order)})
        # A common factor for this entire comparison prevents PCM playback clipping.
        # No individual loudness normalization or gain correction; metrics use raw floats.
        playback_gain = min(1., .95 / max(1e-7, max(float(a.abs().max()) for a in audios.values())))
        folder = args.output / f"{ci:02}"; folder.mkdir()
        for label, audio in audios.items(): sf.write(folder / f"{label}.wav", (audio * playback_gain).T.numpy(), base.RATE, subtype="PCM_16")
        metrics = {name:base.metrics(result, target, mixture, base.BINARY_CLASSES) for name, result in estimates.items()}
        records.append(dict(title=title, recipe=recipe, playbackGain=playback_gain, identities=dict(zip(["A", "B"], order)), rawMetrics=metrics))
        audio = "".join(player(label, f"{ci:02}/{filename}.wav") for label, filename in [("Mixture", "mixture"), ("Vocal reference", "reference"), ("Vocal estimate A", "A"), ("Vocal estimate B", "B")])
        identities = "; ".join(f"{label}: {name}" for label, name in zip(["A", "B"], order))
        cards.append(f'<section aria-labelledby="case{ci}"><h2 id="case{ci}">{ci+1}. {html.escape(title)}</h2>{audio}<details><summary>Reveal model identities</summary><p>{html.escape(identities)}</p></details></section>')
    report = dict(schema=1, listeningCompleted=False, checkpointSHA256=base.digest(args.checkpoint), baselineSHA256=base.digest(args.baseline),
                  independentManifestSHA256=base.digest(args.independent / "sources.json"), dataFingerprint=dataset.fingerprint, examples=records,
                  sourceSHA256=experiments.snapshot(args.output))
    report["auditionSourceSHA256"] = base.digest(Path(__file__))
    (args.output / "executed-source" / Path(__file__).name).write_bytes(Path(__file__).read_bytes())
    (args.output / "listening.json").write_text(json.dumps(report, indent=2) + "\n")
    attribution = (args.independent / "ATTRIBUTION.txt").read_text()
    attribution += "\n" + (args.bank / "ATTRIBUTION.txt").read_text()
    for name, record in dataset.sources["datasets"].items():
        attribution += f"\n{name}: {record['credit']}\n{record['license']} · {record['page']}\n"
    (args.output / "ATTRIBUTION.txt").write_text(attribution)
    document = '''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Soundroom vocal separator A/B</title><style>
:root{color-scheme:light dark;font:17px system-ui;line-height:1.5}body{max-width:960px;margin:auto;padding:24px}
section{border:1px solid #888;border-radius:12px;padding:20px;margin:24px 0}h1{line-height:1.2}h2{font-size:1.2rem}
label{display:inline-block;width:min(100%,420px);margin:10px 12px 10px 0}audio{display:block;width:100%;margin-top:6px}
summary{cursor:pointer;padding:12px 0}a:focus-visible,summary:focus-visible,audio:focus-visible{outline:3px solid #c77d00;outline-offset:4px}
</style><main><h1>Vocal separator: blinded A/B</h1>
<p>Listen to the mixture and reference, then compare A and B. Check missing words, vocal body, cymbal or instrument leakage, warbling and clicks. In instrumental-only cases, the ideal vocal estimate is silence. A/B identity changes between examples.</p>
<p>Keep playback volume fixed. Each case uses the same playback headroom for all four files; estimates retain their relative gain. Scores in <a href="listening.json">listening.json</a> use unadjusted floating-point predictions. These seven four-second excerpts are a listening aid, not a complete quality benchmark.</p>
<p>Use the audio controls with a keyboard or screen reader. Human listening has not been marked complete. <a href="ATTRIBUTION.txt">Dataset and instrument credits</a>.</p>'''
    (args.output / "index.html").write_text(document + "".join(cards) + "</main></html>\n")
    print(args.output / "index.html")


if __name__ == "__main__": main()
