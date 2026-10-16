#!/usr/bin/env python3
"""Fetch the 58.5 MB CC-BY-4.0 vocadito v3 archive for held-out evaluation only."""
import argparse
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import stat
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parent.parent
URL = "https://zenodo.org/records/5578807/files/vocadito.zip?download=1"
MD5 = "dea40fd18f14d899643c4ba221b33a46"
CREDIT = "Rachel M. Bittner, Katherine Pasalo, Juan José Bosch, Gabriel Meseguer-Brocal, David Rubinstein. vocadito: A dataset of solo vocals with f0, note, and lyric annotations (2021)."


def install(payload, destination):
    if hashlib.md5(payload).hexdigest() != MD5:
        raise ValueError("Archive checksum differs from the published Zenodo/mirdata checksum")
    records = []
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        if sum(info.file_size for info in archive.infolist()) > 256 * 1024 * 1024:
            raise ValueError("Unpacked archive exceeds the evaluation bound")
        for info in archive.infolist():
            name = PurePosixPath(info.filename)
            if name.is_absolute() or ".." in name.parts or "\\" in info.filename or stat.S_ISLNK(info.external_attr >> 16):
                raise ValueError("Unsafe archive member")
            if info.is_dir() or "__MACOSX" in name.parts: continue
            if info.file_size > 16 * 1024 * 1024: raise ValueError("Evaluation entry exceeds the size bound")
            data = archive.read(info)
            path = destination.joinpath(*name.parts)
            if not path.resolve().is_relative_to(destination.resolve()): raise ValueError("Cached path escapes evaluation directory")
            if path.exists() and path.read_bytes() != data: raise ValueError(f"Existing evaluation file differs: {path}")
            path.parent.mkdir(parents=True, exist_ok=True)
            part = path.with_suffix(path.suffix + ".part"); part.write_bytes(data); part.replace(path)
            records.append(dict(path=str(path.relative_to(destination)), bytes=len(data), sha256=hashlib.sha256(data).hexdigest()))
    if len([r for r in records if r["path"].lower().endswith(".wav")]) != 40:
        raise ValueError("Expected exactly forty vocadito audio excerpts")
    manifest = dict(schema=1, dataset="vocadito", use="Evaluation only; never training or checkpoint selection",
                    url=URL, page="https://zenodo.org/records/5578807", license="CC-BY-4.0", credit=CREDIT,
                    archiveMD5=MD5, archiveSHA256=hashlib.sha256(payload).hexdigest(), files=records)
    partial = destination / "sources.json.part"; partial.write_text(json.dumps(manifest, indent=2) + "\n"); partial.replace(destination / "sources.json")
    (destination / "ATTRIBUTION.txt").write_text(CREDIT + "\nCC-BY-4.0\nhttps://zenodo.org/records/5578807\n")
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "build/Training/vocadito-evaluation")
    parser.add_argument("--archive", type=Path)
    args = parser.parse_args()
    if args.archive:
        if args.archive.stat().st_size > 80 * 1024 * 1024: parser.error("Archive exceeds 80 MiB")
        payload = args.archive.read_bytes()
    else:
        request = urllib.request.Request(URL, headers={"User-Agent": "Soundroom-Evaluation/1"})
        with urllib.request.urlopen(request, timeout=60) as response:
            if not response.geturl().startswith("https://"): raise ValueError("Expected secure download")
            payload = response.read(80 * 1024 * 1024 + 1)
        if len(payload) > 80 * 1024 * 1024: raise ValueError("Download exceeds 80 MiB")
    args.output.mkdir(parents=True, exist_ok=True)
    manifest = install(payload, args.output)
    print(f"Ready: {args.output / 'sources.json'} · {len(manifest['files'])} verified files · evaluation only")


if __name__ == "__main__": main()
