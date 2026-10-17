"""Builds a Soundroom sampled instrument from a library without SFZ mappings
(VCSL, VSCO 2 CE): note, dynamic layer and round robin come from file names.

uv run --no-project build_from_names.py <instrument-id> <out root> <converter> <scratch> <spec.json>

spec: {"repo": "owner/repo", "tree": "<git tree json>", "sets": [{"prefix": "...",
  "pattern": "regex with (?P<note>) and optional (?P<layer>) (?P<rr>)",
  "layers": ["v1", "v2", ...]  # soft→loud; omitted = one layer
  "keys": [lo, hi]  # optional range of this set (after octave correction)
}], "robins": 2, "loop": false, "maxSeconds": 30, "release": 0.3}

Octave numbering differs between libraries, so the octave is corrected from
each note's detected pitch (median offset per set).
"""

import json
import math
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor

NOTE = {"c": 0, "d": 2, "e": 4, "f": 5, "g": 7, "a": 9, "b": 11}


def named_key(name: str) -> int:
    m = re.fullmatch(r"([A-Ga-g])([#b]?)(-?\d+)", name)
    if not m:
        raise ValueError(name)
    return (
        (int(m.group(3)) + 1) * 12 + NOTE[m.group(1).lower()] + {"#": 1, "b": -1, "": 0}[m.group(2)]
    )


def fetch(url: str) -> bytes:
    error = None
    for _ in range(3):
        try:
            return urllib.request.urlopen(url, timeout=300).read()
        except Exception as e:
            error = e
    raise RuntimeError(f"download failed: {url}") from error


def main():
    instrument, out_root, converter, scratch_root, spec_path = sys.argv[1:6]
    spec = json.load(open(spec_path))
    tree_path = os.path.join(os.environ.get("SOUNDROOM_TREES", "."), spec["tree"])
    tree = [e for e in json.load(open(tree_path))["tree"] if e["type"] == "blob"]
    robins = spec.get("robins", 2)
    picked = []  # (set index, path, note, layer index, rr)
    for si, s in enumerate(spec["sets"]):
        layers = s.get("layers")
        for e in tree:
            path = e["path"]
            if not path.startswith(s["prefix"]) or not path.lower().endswith(".wav"):
                continue
            m = re.search(s["pattern"], path.split("/")[-1])
            if not m:
                continue
            layer = 0
            if layers:
                if m.group("layer") not in layers:
                    continue
                layer = layers.index(m.group("layer"))
            rr = int(m.groupdict().get("rr") or 1)
            if rr > robins:
                continue
            picked.append((si, path, named_key(m.group("note")), layer, rr))
    # Spread velocity over the layers each set actually has.
    for si in range(len(spec["sets"])):
        present = sorted({p[3] for p in picked if p[0] == si})
        picked = [(a, b, c, present.index(d) if a == si else d, e) for a, b, c, d, e in picked]
        spec["sets"][si]["present"] = len(present)
    base = f"https://raw.githubusercontent.com/{spec['repo']}/master/"
    os.makedirs(scratch_root, exist_ok=True)
    scratch = tempfile.mkdtemp(prefix=f"soundroom-{instrument}-", dir=scratch_root)
    out_dir = os.path.join(out_root, instrument)
    staging = out_dir + ".building"
    shutil.rmtree(staging, ignore_errors=True)
    os.makedirs(staging)
    try:

        def download(item):
            local = os.path.join(scratch, f"{picked.index(item):04d}.wav")
            open(local, "wb").write(fetch(base + urllib.parse.quote(item[1])))
            return local

        with ThreadPoolExecutor(8) as pool:
            locals_ = list(pool.map(download, picked))
        raw = sum(os.path.getsize(p) for p in locals_)
        jobs = [
            {
                "input": local_path,
                "output": os.path.join(staging, f"{i:04d}.wav"),
                "loop": spec.get("loop", False),
                "maxSeconds": spec.get("maxSeconds", 30),
            }
            for i, local_path in enumerate(locals_)
        ]
        jobs_path, results_path = (
            os.path.join(scratch, "jobs.json"),
            os.path.join(scratch, "results.json"),
        )
        json.dump(jobs, open(jobs_path, "w"))
        subprocess.run([converter, jobs_path, results_path], check=True)
        results = {os.path.basename(r["output"]): r for r in json.load(open(results_path))}
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    # Octave correction per set from detected pitch.
    offsets = {}
    for si in range(len(spec["sets"])):
        deltas = []
        for i, (s_i, _, key, _, _) in enumerate(picked):
            r = results.get(f"{i:04d}.wav", {})
            if s_i == si and r.get("pitch"):
                midi = 69 + 12 * math.log2(r["pitch"] / 440)
                deltas.append(round((midi - key) / 12) * 12)
        offsets[si] = int(statistics.median(deltas)) if deltas else 0
    entries = []
    for i, (si, path, key, layer, rr) in enumerate(picked):
        r = results.get(f"{i:04d}.wav")
        if not r or "error" in r:
            continue
        key += offsets[si]
        lo_hi = spec["sets"][si].get("keys")
        if lo_hi and not (lo_hi[0] <= key <= lo_hi[1]):
            continue
        layers = spec["sets"][si].get("present") or 1
        entries.append(
            {
                "file": f"{i:04d}.wav",
                "centre": key,
                "layer": layer,
                "layers": layers,
                "robin": rr - 1,
                "gain": r["gain"],
                "loopStart": r["loopStart"],
                "loopEnd": r["loopEnd"],
            }
        )
    # Key zones: halfway between neighbouring sampled notes.
    centres = sorted({e["centre"] for e in entries})
    for e in entries:
        k = centres.index(e["centre"])
        e["lokey"] = 0 if k == 0 else (centres[k - 1] + e["centre"]) // 2 + 1
        e["hikey"] = 127 if k == len(centres) - 1 else (e["centre"] + centres[k + 1]) // 2
        n = e.pop("layers")
        e["lovel"] = 1 + (127 * e["layer"]) // n
        e["hivel"] = (127 * (e.pop("layer") + 1)) // n
        e["tune"] = 0.0
    for f in os.listdir(staging):
        if f.endswith(".wav") and f not in {e["file"] for e in entries}:
            os.remove(os.path.join(staging, f))
    manifest = {
        "version": 1,
        "instrument": instrument,
        "source": f"https://github.com/{spec['repo']}",
        "release": spec.get("release", 0.3),
        "regions": entries,
    }
    json.dump(manifest, open(os.path.join(staging, "manifest.json"), "w"), indent=1)
    shutil.rmtree(out_dir, ignore_errors=True)
    os.rename(staging, out_dir)
    stored = sum(os.path.getsize(os.path.join(out_dir, f)) for f in os.listdir(out_dir))
    print(
        f"{instrument}: {len(entries)} regions, {len(centres)} notes {centres[0]}–{centres[-1]}, octave offsets {offsets}; downloaded {raw / 1e6:.0f} MB, stored {stored / 1e6:.0f} MB"
    )


if __name__ == "__main__":
    main()
