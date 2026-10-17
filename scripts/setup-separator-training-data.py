#!/usr/bin/env python3
"""Fetch a small, reproducible CC-BY pilot, without entire multi-GB archives.

Only explicit invocation uses the network. No models or executable code are
downloaded. Zip entry CRCs are verified; retained source bytes get SHA-256s.
"""

import argparse
import hashlib
import io
import json
import random
import re
import sys
import urllib.error
import urllib.request
import zipfile
from pathlib import Path, PurePosixPath

SOURCES = {
    "vocalset": {
        "url": "https://zenodo.org/records/1442513/files/VocalSet11.zip?download=1",
        "page": "https://zenodo.org/records/1442513",
        "license": "CC-BY-4.0",
        "credit": "Julia Wilkins, Prem Seetharaman, Alison Wahl, Bryan Pardo. VocalSet: A Singing Voice Dataset, ISMIR 2018.",
    },
    "babyslakh": {
        # v1 has a ZIP suitable for bounded range reads; v2 is a tar.gz.
        "url": "https://zenodo.org/records/4603844/files/babyslakh_16k.zip?download=1",
        "page": "https://zenodo.org/records/4603844",
        "license": "CC-BY-4.0",
        "version": "v1 pilot; group duplicate MIDI/compositions before splitting",
        "credit": "Ethan Manilow, Gordon Wichern, Prem Seetharaman, Jonathan Le Roux. Cutting Music Source Separation Some Slakh, WASPAA 2019.",
        "licensePage": "https://www.slakh.com/",
    },
}


class RemoteZip(io.RawIOBase):
    """Strict bounded HTTP range reader. Never fall back to an entire archive."""

    def __init__(self, url, byte_budget=256 * 1024 * 1024):
        self.url, self.position, self.transferred = url, 0, 0
        self.budget = byte_budget
        tail, first, total = self._request("bytes=-65557", 65557)
        self.size, self.tail_start, self.tail = total, first, tail

    def _request(self, value, limit):
        request = urllib.request.Request(
            self.url,
            headers={
                "Range": value,
                "Accept-Encoding": "identity",
                "User-Agent": "Soundroom-TrainingPilot/1",
            },
        )
        with urllib.request.urlopen(request, timeout=45) as response:
            if response.status != 206 or not response.geturl().startswith("https://"):
                raise RuntimeError(
                    "Dataset host did not honor secure range requests; refusing a full archive download"
                )
            match = re.fullmatch(
                r"bytes (\d+)-(\d+)/(\d+)", response.headers.get("Content-Range", "")
            )
            if not match:
                raise RuntimeError("Dataset host returned an invalid Content-Range")
            start, end, total = map(int, match.groups())
            if start > end or end >= total or end - start + 1 > limit:
                raise RuntimeError("Dataset range exceeds the requested bound")
            if self.transferred + end - start + 1 > self.budget:
                raise RuntimeError(
                    "Pilot download budget exceeded; reduce the requested sample counts"
                )
            data = response.read(limit + 1)
            if len(data) != end - start + 1:
                raise RuntimeError("Dataset returned truncated or oversized bytes")
            self.transferred += len(data)
            return data, start, total

    def readable(self):
        return True

    def seekable(self):
        return True

    def tell(self):
        return self.position

    def seek(self, offset, whence=0):
        position = (
            offset
            if whence == 0
            else self.position + offset
            if whence == 1
            else self.size + offset
            if whence == 2
            else -1
        )
        if position < 0:
            raise ValueError("Invalid archive seek")
        self.position = position
        return position

    def read(self, size=-1):
        size = (
            self.size - self.position if size < 0 else min(size, max(0, self.size - self.position))
        )
        if size > 16 * 1024 * 1024:
            raise RuntimeError("Archive read exceeds pilot memory bound")
        if size == 0:
            return b""
        start = self.position
        if start >= self.tail_start:
            data = self.tail[start - self.tail_start : start - self.tail_start + size]
        else:
            data, received_start, total = self._request(f"bytes={start}-{start + size - 1}", size)
            if received_start != start or total != self.size or len(data) != size:
                raise RuntimeError("Archive changed during range download")
        self.position += len(data)
        return data


def retain(archive, member, destination, dataset, group, records):
    info = archive.getinfo(member)
    if info.file_size > 32 * 1024 * 1024 or info.is_dir():
        raise RuntimeError(f"Pilot member is too large or not a file: {member}")
    relative = PurePosixPath(member)
    if relative.is_absolute() or ".." in relative.parts or "\\" in member:
        raise RuntimeError("Unsafe dataset entry name")
    if (info.external_attr >> 16) & 0o170000 == 0o120000:
        raise RuntimeError("Dataset symbolic links are not permitted")
    path = destination / dataset / Path(*relative.parts)
    if path.exists():
        data = path.read_bytes()
        import zlib

        if len(data) != info.file_size or zlib.crc32(data) & 0xFFFFFFFF != info.CRC:
            raise RuntimeError(f"Cached source failed integrity checks: {path}")
    else:
        # ZipFile.read() may request a whole compressed entry in one HTTP read,
        # larger than RemoteZip's bound. Bounded blocks also retain CRC checking.
        buffer = io.BytesIO()
        with archive.open(info) as entry:
            while block := entry.read(4 * 1024 * 1024):
                buffer.write(block)
        data = buffer.getvalue()
        path.parent.mkdir(parents=True, exist_ok=True)
        partial = path.with_suffix(path.suffix + ".part")
        partial.write_bytes(data)
        partial.replace(path)
    records.append(
        {
            "dataset": dataset,
            "group": group,
            "path": str(path.relative_to(destination)),
            "archiveMember": member,
            "bytes": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
            "license": SOURCES[dataset]["license"],
        }
    )


def select_vocals(names, per_singer):
    groups = {}
    for name in names:
        match = re.search(r"(?:^|/)(female\d+|male\d+)(?:/|_)", name, re.I)
        if match and name.lower().endswith(".wav") and "__MACOSX" not in name:
            groups.setdefault(match.group(1).lower(), []).append(name)
    if len(groups) < 4:
        raise RuntimeError(
            "Unexpected VocalSet archive layout: cannot establish singer-disjoint splits"
        )
    selected = []
    for group, members in sorted(groups.items()):
        members = sorted(members)
        random.Random("soundroom-vocalset-1-" + group).shuffle(members)
        selected.extend((name, group) for name in members[:per_singer])
    return selected


def select_slakh(names, tracks):
    groups = {}
    for name in names:
        match = re.search(r"(?:^|/)(Track\d+)(?:/|$)", name)
        if match and "__MACOSX" not in name:
            groups.setdefault(match.group(1), []).append(name)
    if len(groups) < 6:
        raise RuntimeError("Unexpected BabySlakh archive layout")
    chosen = sorted(groups)
    random.Random(41015).shuffle(chosen)
    selected = []
    for group in chosen[:tracks]:
        for name in sorted(groups[group]):
            audio = name.lower().endswith(".wav") and "/stems/" in name.lower()
            metadata = name.lower().endswith(("metadata.yaml", "all_src.mid"))
            if audio or metadata:
                selected.append((name, group))
    return selected


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "build/Training/data",
    )
    parser.add_argument("--vocal-files-per-singer", type=int, default=3)
    parser.add_argument("--slakh-tracks", type=int, default=6)
    parser.add_argument("--dataset", choices=["all", *SOURCES], default="all")
    args = parser.parse_args()
    if not 1 <= args.vocal_files_per_singer <= 20 or not 6 <= args.slakh_tracks <= 20:
        parser.error("Use 1–20 files per singer and 6–20 Slakh tracks")
    args.output.mkdir(parents=True, exist_ok=True)
    manifest_path = args.output / "sources.json"
    previous = (
        json.loads(manifest_path.read_text())
        if manifest_path.exists()
        else {"schema": 1, "datasets": SOURCES, "files": []}
    )
    records = [
        record
        for record in previous["files"]
        if args.dataset != "all" and record["dataset"] != args.dataset
    ]
    for dataset in SOURCES if args.dataset == "all" else [args.dataset]:
        print(
            f"Fetching {dataset} pilot · {SOURCES[dataset]['license']} · {SOURCES[dataset]['page']}",
            flush=True,
        )
        # Six complete Slakh songs can exceed 256 MiB; retain a bounded 512 MiB
        # transfer allowance rather than failing near the end of the default pilot.
        with (
            RemoteZip(SOURCES[dataset]["url"], byte_budget=512 * 1024 * 1024) as source,
            zipfile.ZipFile(source) as archive,
        ):
            members = (
                select_vocals(archive.namelist(), args.vocal_files_per_singer)
                if dataset == "vocalset"
                else select_slakh(archive.namelist(), args.slakh_tracks)
            )
            for member, group in members:
                retain(archive, member, args.output, dataset, group, records)
                print(f"  {dataset}/{group}: {PurePosixPath(member).name}", flush=True)
            print(f"Transferred {source.transferred / 1024**2:.1f} MiB", flush=True)
    manifest = {"schema": 1, "datasets": SOURCES, "files": sorted(records, key=lambda r: r["path"])}
    partial = manifest_path.with_suffix(".json.part")
    partial.write_text(json.dumps(manifest, indent=2) + "\n")
    partial.replace(manifest_path)
    (args.output / "ATTRIBUTION.txt").write_text(
        "\n\n".join(
            f"{name}: {info['credit']}\n{info['license']} · {info['page']}"
            for name, info in SOURCES.items()
        )
        + "\n"
    )
    print(f"Ready: {manifest_path}", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, zipfile.BadZipFile, urllib.error.URLError) as error:
        print(
            f"Dataset setup failed: {error}\nNo complete source manifest was published. Cached verified entries can be reused on retry.",
            file=sys.stderr,
        )
        sys.exit(1)
