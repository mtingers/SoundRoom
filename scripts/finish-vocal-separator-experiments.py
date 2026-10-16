#!/usr/bin/env python3
"""Lock a validation-only candidate choice, then evaluate it and build an audition."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
CANDIDATES = ["short-small", "long-small", "long-wide"]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT / "build/Training/experiments/vocals-next")
    parser.add_argument("--independent", type=Path, default=ROOT / "build/Training/vocadito-evaluation")
    parser.add_argument("--select-only", action="store_true")
    args = parser.parse_args(); candidates = []
    plan_path = args.root / "experiment-plan.json"
    if not plan_path.exists(): plan_path.write_bytes(Path(__file__).with_name("vocal-separator-plan.json").read_bytes())
    plan = json.loads(plan_path.read_text())
    for name in CANDIDATES:
        folder = args.root / name; run = json.loads((folder / "run.json").read_text())
        validation = json.loads((folder / "best-validation.json").read_text())
        history = json.loads((folder / "history.json").read_text())
        if not run.get("status", "").startswith("Completed") or history[-1]["step"] != run["steps"]:
            raise ValueError(f"Incomplete candidate: {name}")
        if run["steps"] != plan["steps"] or run["effectiveBatch"] != plan["effectiveBatch"] or run["seed"] != plan["seed"] or run["gates"] != plan["gates"]:
            raise ValueError("Candidate differs from the frozen experiment plan")
        width, seconds = {"short-small": (8, 1.), "long-small": (8, 4.), "long-wide": (16, 4.)}[name]
        if run["width"] != width or run["contextSeconds"] != seconds or run["data"]["seed"] != plan["recipeSeed"]:
            raise ValueError("Candidate architecture or recipe seed differs from the plan")
        if validation["split"] != "validation" or validation["summary"]["examples"] != plan["validationRecipes"]:
            raise ValueError("Checkpoint selection did not use the declared validation set")
        if sha(folder / "best.pt") != run["bestCheckpointSHA256"]: raise ValueError("Checkpoint changed")
        if validation["summary"]["selectionScore"] != max(h["summary"]["selectionScore"] for h in history):
            raise ValueError("Checkpoint is not the validation winner")
        candidates.append(dict(name=name, run=run, step=validation["step"], validation=validation["summary"]))
    for key in ["steps", "effectiveBatch", "physicalMicroBatch", "seed", "dataFingerprint", "sourceSHA256"]:
        if any(c["run"][key] != candidates[0]["run"][key] for c in candidates): raise ValueError(f"Unpaired comparison: {key}")
    selected = max(candidates, key=lambda c:c["validation"]["selectionScore"])
    selection = dict(schema=1, selected=selected["name"], checkpointSHA256=selected["run"]["bestCheckpointSHA256"],
                     reason="Highest gain-aware validation score; no test results consulted", candidates=candidates,
                     experimentPlanSHA256=sha(args.root / "experiment-plan.json"),
                     independentManifestSHA256=sha(args.independent / "sources.json"), selectorSHA256=sha(Path(__file__)))
    lock = args.root / "selection.json"
    if lock.exists():
        if json.loads(lock.read_text()) != selection: raise ValueError("Locked choice changed; do not select using test results")
    else: lock.write_text(json.dumps(selection, indent=2) + "\n")
    print(f"Locked validation winner: {selected['name']}", flush=True)
    if args.select_only: return
    script = Path(__file__).with_name("vocal-separator-experiments.py")
    selected_checkpoint = args.root / selected["name"] / "best.pt"
    baseline = ROOT / "build/Training/models/soundroom-complex-v2/best.pt"
    for name, checkpoint, independent in [("regression", selected_checkpoint, False), ("independent", selected_checkpoint, True),
                                           ("previous-independent", baseline, True)]:
        output = args.root / name
        if (output / "evaluation.json").exists():
            existing = json.loads((output / "evaluation.json").read_text())
            if existing["checkpointSHA256"] != sha(checkpoint): raise ValueError("Existing evaluation checkpoint changed")
            if existing["dataFingerprint"] != selected["run"]["dataFingerprint"]: raise ValueError("Evaluation recipes/data changed since validation")
            if independent and existing["independentManifestSHA256"] != selection["independentManifestSHA256"]:
                raise ValueError("Existing independent evaluation dataset changed")
            continue
        command = [sys.executable, "-B", str(script), "--threads", "2", "evaluate", "--checkpoint", str(checkpoint), "--output", str(output)]
        if independent: command += ["--independent", str(args.independent)]
        subprocess.run(command, check=True)
        if json.loads((output / "evaluation.json").read_text())["dataFingerprint"] != selected["run"]["dataFingerprint"]:
            raise ValueError("Evaluation recipes/data changed since validation")
    audition = args.root / "audition"
    if not (audition / "index.html").exists():
        subprocess.run([sys.executable, "-B", str(Path(__file__).with_name("make-vocal-separator-audition.py")),
                        "--checkpoint", str(selected_checkpoint), "--baseline", str(baseline), "--independent", str(args.independent),
                        "--output", str(audition)], check=True)
    metrics = json.loads((args.root / "independent/evaluation.json").read_text())
    status = dict(selected=selected["name"], checkpointSHA256=selection["checkpointSHA256"],
                  numericalGatePassed=metrics["summary"]["numericalGatePassed"], humanListeningCompleted=False,
                  appIntegration="Deferred: numerical and human listening gates must pass", audition=str(audition / "index.html"))
    (args.root / "STATUS.json").write_text(json.dumps(status, indent=2) + "\n")
    (args.root / "executed-selector.py").write_bytes(Path(__file__).read_bytes())
    print(json.dumps(status, indent=2), flush=True)


if __name__ == "__main__": main()
