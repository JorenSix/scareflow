#!/usr/bin/env python3
"""Builds a show file for a Linux box from showui.toml in a Scareflow checkout.

Drops the macOS output device names, so sounds use the default output, and adds
an OSC-in block per audio file: /pumpkin/scare/<name> plays a one-shot from
scare/, /pumpkin/bg/<name> starts a loop from bg-music/ and /pumpkin/bg/stop
stops the loop.

Usage: generate-osc-show.py . out/showui.toml
"""
import re, sys, tomllib
from pathlib import Path

ext, out = Path(sys.argv[1]), Path(sys.argv[2])
text = (ext / "showui.toml").read_text()
text = re.sub(r'^device = ".*"\n', "", text, flags=re.M)
next_id = max(n["id"] for n in tomllib.loads(text)["node"]) + 1

nodes, wires = [], []
def node(x, y, **kv):
    global next_id
    nid = next_id; next_id += 1
    body = "\n".join(f"{k} = {v!r}".replace("'", '"') for k, v in kv.items())
    nodes.append(f"[[node]]\nid = {nid}\npos = [{x:.1f}, {y:.1f}]\n{body}\n")
    return nid
def wire(a, b):
    wires.append(f"[[wire]]\nfrom = [{a[0]}, {a[1]}]\nto = [{b[0]}, {b[1]}]\n")

audio = {".wav", ".mp3", ".ogg", ".flac", ".m4a"}
y = 900.0
for f in sorted(p for p in (ext / "scare").iterdir() if p.suffix in audio):
    o = node(-900, y, block="osc_in", address=f"/pumpkin/scare/{f.stem}", args=0)
    s = node(-550, y, block="sound", file=f"scare/{f.name}", volume=0.9)
    wire((o, 0), (s, 0)); y += 130
bgs = []
for f in sorted(p for p in (ext / "bg-music").iterdir() if p.suffix in audio):
    o = node(-900, y, block="osc_in", address=f"/pumpkin/bg/{f.stem}", args=0)
    b = node(-550, y, block="background", file=f"bg-music/{f.name}", volume=0.5)
    wire((o, 0), (b, 0)); bgs.append(b); y += 130
stop = node(-1250, y, block="osc_in", address="/pumpkin/bg/stop", args=0)
for b in bgs:
    wire((stop, 0), (b, 1))

out.write_text(text.rstrip() + "\n\n" + "\n".join(nodes) + "\n" + "\n".join(wires))
tomllib.loads(out.read_text())
print(f"{len(nodes)} nodes, {len(wires)} wires -> {out}")
