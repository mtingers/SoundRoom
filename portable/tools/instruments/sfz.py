"""Minimal SFZ reader for the open instrument libraries Soundroom converts.

Supports: comments, <control> default_path, <global>/<master>/<group>/<region>
inheritance, #define $VAR value, #include "file" (resolved through a fetch
callback), note names, and the opcodes Soundroom maps (key ranges, velocity
ranges, pitch centre, tune, volume, round robins, trigger, loops).
"""
import re

NOTE = {"c": 0, "d": 2, "e": 4, "f": 5, "g": 7, "a": 9, "b": 11}


def key(value):
    value = value.strip().lower()
    if re.fullmatch(r"-?\d+", value):
        return int(value)
    m = re.fullmatch(r"([a-g])([#b]?)(-?\d+)", value)
    if not m:
        raise ValueError(f"bad key {value!r}")
    n = NOTE[m.group(1)] + {"#": 1, "b": -1, "": 0}[m.group(2)]
    return (int(m.group(3)) + 1) * 12 + n


def parse(text, include, defines=None):
    """Regions as dicts of opcodes (inherited values merged)."""
    defines = dict(defines or {})

    def expand(src, depth=0):
        out = []
        for line in src.splitlines():
            line = re.sub(r"//.*", "", line)
            m = re.match(r"\s*#define\s+(\$\w+)\s+(.*)", line)
            if m:
                defines[m.group(1)] = m.group(2).strip()
                continue
            m = re.match(r'\s*#include\s+"([^"]+)"', line)
            if m and depth < 8:
                out.append(expand(include(m.group(1)), depth + 1))
                continue
            for name in sorted(defines, key=len, reverse=True):
                line = line.replace(name, defines[name])
            out.append(line)
        return "\n".join(out)

    text = expand(text)
    tokens = re.split(r"(<\w+>)", text)
    levels = {"control": {}, "global": {}, "master": {}, "group": {}}
    current, regions = None, []
    order = ["global", "master", "group"]

    def opcodes(body):
        found = {}
        # sample= may contain spaces; it runs to the next opcode or line end.
        for m in re.finditer(r"(\w+)=(.*?)(?=\s+\w+=|\s*$)", body, re.M):
            found[m.group(1)] = m.group(2).strip()
        return found

    for token in tokens:
        header = re.fullmatch(r"<(\w+)>", token)
        if header:
            current = header.group(1)
            if current in order:
                for lower in order[order.index(current):]:
                    levels[lower] = {}
            if current == "region":
                regions.append({})
            continue
        if current is None:
            continue
        values = opcodes(token)
        if current == "region":
            merged = {}
            for level in order:
                merged.update(levels[level])
            merged.update(values)
            regions[-1] = {**merged, **regions[-1], **values}
        elif current in levels:
            levels[current].update(values)
    prefix = levels["control"].get("default_path", "")
    for r in regions:
        if "sample" in r:
            r["sample"] = (prefix + r["sample"]).replace("\\", "/")
    return [r for r in regions if "sample" in r]


def note_regions(regions, max_robins=2, exclude=("noise", "harmonic", "release", "fret", "slide")):
    """Playable note regions: attack-triggered, not noises; at most
    `max_robins` round robins per key/velocity zone."""
    out = []
    for r in regions:
        if r.get("trigger", "attack") != "attack":
            continue
        if any(word in r["sample"].lower() for word in exclude):
            continue
        lo = key(r.get("lokey", r.get("key", "0")))
        hi = key(r.get("hikey", r.get("key", "127")))
        centre = key(r.get("pitch_keycenter", r.get("key", str(lo))))
        if centre < 12 or centre > 115:
            continue
        robin = int(r.get("seq_position", "1"))
        lorand = float(r.get("lorand", "0"))
        if "lorand" in r or "hirand" in r:
            robin = 1 + int(round(lorand * 5))
        out.append({
            "sample": r["sample"], "lokey": lo, "hikey": hi, "centre": centre,
            "lovel": int(r.get("lovel", "1")), "hivel": int(r.get("hivel", "127")),
            "robin": robin, "tune": float(r.get("tune", "0")),
            "volume": float(r.get("volume", "0")),
            "release": float(r.get("ampeg_release", "0.25")),
        })
    zones = {}
    for r in sorted(out, key=lambda r: r["robin"]):
        zone = (r["lokey"], r["hikey"], r["lovel"], r["hivel"])
        zones.setdefault(zone, [])
        if len(zones[zone]) < max_robins:
            zones[zone].append(r)
    picked = [r for rs in zones.values() for r in rs]
    for zone in zones.values():
        for i, r in enumerate(zone):
            r["robin"] = i
    return picked
