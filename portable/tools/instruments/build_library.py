"""Builds one Soundroom sampled instrument from an open SFZ library on GitHub.

uv run --no-project build_library.py <instrument-id> <github owner/repo> <sfz path in repo>
    <out root> <converter binary> [--loop] [--robins N] [--release SECONDS] [--branch master]

Only the mapped note samples are downloaded (into a scratch folder that is
deleted afterwards); they are converted by `build_instrument` and described
in <out root>/<instrument-id>/manifest.json.
"""

import argparse
import json
import os
import posixpath
import shutil
import subprocess
import sys
import tempfile
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sfz  # noqa: E402  (sibling module)


def fetch(url: str, retries: int = 3) -> bytes:
    error = None
    for _ in range(retries):
        try:
            return urllib.request.urlopen(url, timeout=300).read()
        except Exception as e:
            error = e
    raise RuntimeError(f"download failed: {url}") from error


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("instrument")
    ap.add_argument("repo")
    ap.add_argument("sfz_path")
    ap.add_argument("out_root")
    ap.add_argument("converter")
    ap.add_argument("--loop", action="store_true")
    ap.add_argument("--robins", type=int, default=2)
    ap.add_argument("--release", type=float)
    ap.add_argument("--branch", default="master")
    ap.add_argument("--scratch", default=None)
    ap.add_argument(
        "--local",
        default=None,
        help="read the library from this folder instead of GitHub (repo is then only a label)",
    )
    ap.add_argument(
        "--sample-dir",
        default=None,
        help="repo folder that sample paths are relative to (default: the SFZ's folder)",
    )
    ap.add_argument("--min-key", type=int, default=0)
    ap.add_argument("--max-key", type=int, default=127)
    a = ap.parse_args()
    base = f"https://raw.githubusercontent.com/{a.repo}/{a.branch}/"
    url = lambda rel: base + urllib.parse.quote(rel)
    if a.local:
        url = lambda rel: "file://" + urllib.parse.quote(
            os.path.join(os.path.abspath(a.local), rel)
        )
    sfz_dir = posixpath.dirname(a.sfz_path)
    include = lambda name: fetch(
        url(posixpath.normpath(posixpath.join(sfz_dir, name.replace("\\", "/"))))
    ).decode("utf8", "replace")
    regions = sfz.parse(fetch(url(a.sfz_path)).decode("utf8", "replace"), include=include)
    notes = [
        n
        for n in sfz.note_regions(regions, max_robins=a.robins)
        if a.min_key <= n["centre"] <= a.max_key
    ]
    for n in notes:
        n["source"] = posixpath.normpath(
            posixpath.join(a.sample_dir if a.sample_dir is not None else sfz_dir, n["sample"])
        )
    sources = sorted({n["source"] for n in notes})
    out_dir = os.path.join(a.out_root, a.instrument)
    staging = out_dir + ".building"
    shutil.rmtree(staging, ignore_errors=True)
    os.makedirs(staging)
    if a.scratch:
        os.makedirs(a.scratch, exist_ok=True)
    scratch = tempfile.mkdtemp(prefix=f"soundroom-{a.instrument}-", dir=a.scratch)
    try:

        def download(rel):
            path = os.path.join(scratch, rel.replace("/", "__"))
            open(path, "wb").write(fetch(url(rel)))
            return rel, path

        with ThreadPoolExecutor(8) as pool:
            local = dict(pool.map(download, sources))
        raw_bytes = sum(os.path.getsize(p) for p in local.values())
        names = {rel: f"{i:03d}.wav" for i, rel in enumerate(sources)}
        jobs = [
            {"input": local[rel], "output": os.path.join(staging, names[rel]), "loop": a.loop}
            for rel in sources
        ]
        jobs_path, results_path = (
            os.path.join(scratch, "jobs.json"),
            os.path.join(scratch, "results.json"),
        )
        json.dump(jobs, open(jobs_path, "w"))
        subprocess.run([a.converter, jobs_path, results_path], check=True)
        results = {os.path.basename(r["output"]): r for r in json.load(open(results_path))}
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    # Some libraries name octaves differently (e.g. a cello's low C as C1):
    # correct the mapping from the pitch detected in each converted note.
    import math
    import statistics

    deltas = []
    for n in notes:
        r = results.get(names[n["source"]], {})
        if r.get("pitch"):
            deltas.append(round((69 + 12 * math.log2(r["pitch"] / 440) - n["centre"]) / 12) * 12)
    octave = int(statistics.median(deltas)) if deltas else 0
    regions_out = []
    for n in notes:
        n["centre"] += octave
        n["lokey"] = max(0, min(127, n["lokey"] + octave))
        n["hikey"] = max(0, min(127, n["hikey"] + octave))
        r = results.get(names[n["source"]])
        if not r or "error" in r:
            continue
        regions_out.append(
            {
                "file": names[n["source"]],
                "lokey": n["lokey"],
                "hikey": n["hikey"],
                "centre": n["centre"],
                "lovel": n["lovel"],
                "hivel": n["hivel"],
                "robin": n["robin"],
                "tune": n["tune"],
                "gain": r["gain"] * 10 ** (n["volume"] / 20),
                "loopStart": r["loopStart"],
                "loopEnd": r["loopEnd"],
            }
        )
    release = a.release if a.release is not None else max(n["release"] for n in notes)
    manifest = {
        "version": 1,
        "instrument": a.instrument,
        "source": a.repo if a.local else f"https://github.com/{a.repo}",
        "release": release,
        "regions": regions_out,
    }
    json.dump(manifest, open(os.path.join(staging, "manifest.json"), "w"), indent=1)
    shutil.rmtree(out_dir, ignore_errors=True)
    os.rename(staging, out_dir)
    stored = sum(os.path.getsize(os.path.join(out_dir, f)) for f in os.listdir(out_dir))
    errors = [r for r in results.values() if "error" in r]
    print(
        f"{a.instrument}: octave correction {octave:+d}; {len(regions_out)} regions from {len(sources)} samples; downloaded {raw_bytes / 1e6:.0f} MB, stored {stored / 1e6:.0f} MB; errors {len(errors)}"
    )


if __name__ == "__main__":
    main()
