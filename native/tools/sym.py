#!/usr/bin/env python3
"""Remplace les adresses x86 du journal d'un jeu natif par le nom de fonction. sym.py <exe> [journal]"""
import bisect
import re
import subprocess
import sys

exe = sys.argv[1]
nm = subprocess.run(["nm", "-n", exe], capture_output=True, text=True, errors="replace").stdout
addrs, names = [], []
for line in nm.splitlines():
    f = line.split()
    if len(f) == 3 and f[1] in "tT":
        addrs.append(int(f[0], 16))
        names.append(f[2][1:] if f[2].startswith("_") else f[2])
text = open(sys.argv[2], errors="replace").read() if len(sys.argv) > 2 else sys.stdin.read()


def sub(m):
    v = int(m.group(0), 16)
    if not (0x10000000 <= v < 0x11000000):
        return m.group(0)
    i = bisect.bisect_right(addrs, v) - 1
    return "%s+0x%x" % (names[i], v - addrs[i]) if i >= 0 else m.group(0)


print(re.sub(r"\b1[0-9a-f]{7}\b", sub, text))
