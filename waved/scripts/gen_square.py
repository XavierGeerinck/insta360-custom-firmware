#!/usr/bin/env python3
"""Full-scale 1 kHz square wave — limiter stress test."""
import sys, struct
dur = float(sys.argv[1])
sr = 48_000
n = int(sr * dur)
period = sr // 1000
buf = []
for i in range(n):
    s = 32767 if (i // (period // 2)) % 2 == 0 else -32767
    buf.append(struct.pack("<hh", s, s))
sys.stdout.buffer.write(b"".join(buf))
