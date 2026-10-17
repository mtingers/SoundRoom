# Fetch selected entries from a remote zip via range requests; write under an output dir.
import json
import os
import struct
import sys
import urllib.request
import zlib
from concurrent.futures import ThreadPoolExecutor

index, outdir, names = sys.argv[1], sys.argv[2], sys.argv[3:]
d = json.load(open(index))
url = d["url"]
by = {e["name"]: e for e in d["entries"]}


def get(start, end):
    req = urllib.request.Request(url, headers={"Range": f"bytes={start}-{end}"})
    return urllib.request.urlopen(req, timeout=300).read()


def fetch(name):
    e = by[name]
    safe = os.path.normpath(name).lstrip("/")
    assert not safe.startswith("..")
    path = os.path.join(outdir, safe)
    if os.path.exists(path) and os.path.getsize(path) == e["usize"]:
        return
    head = get(e["offset"], e["offset"] + 29)
    assert head[:4] == b"PK\x03\x04"
    nlen, xlen = struct.unpack("<HH", head[26:30])
    start = e["offset"] + 30 + nlen + xlen
    data = get(start, start + e["csize"] - 1)
    if e["method"] == 8:
        data = zlib.decompress(data, -15)
    elif e["method"] != 0:
        raise SystemExit(f"unsupported method {e['method']}")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path + ".part", "wb").write(data)
    os.replace(path + ".part", path)


with ThreadPoolExecutor(8) as pool:
    list(pool.map(fetch, names))
