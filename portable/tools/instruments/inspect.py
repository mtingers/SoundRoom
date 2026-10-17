"""Summarize the playable note regions of local SFZ files."""

import collections
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
import sfz

for path in sys.argv[1:]:
    regions = sfz.parse(
        open(path, encoding="utf8", errors="replace").read(), include=lambda name: ""
    )
    notes = sfz.note_regions(regions)
    vel = sorted({(r["lovel"], r["hivel"]) for r in notes})
    keys = sorted({r["centre"] for r in notes})
    print(
        f"{os.path.basename(path)}: {len(regions)} regions -> {len(notes)} notes; centres {keys[:3]}..{keys[-3:]} ({len(keys)}); velocity zones {vel}; robins {collections.Counter(r['robin'] for r in notes)}; release {sorted({r['release'] for r in notes})}"
    )
    print("   e.g.", notes[0])
