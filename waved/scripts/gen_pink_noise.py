#!/usr/bin/env python3
"""Generate `dur` seconds of pink noise as raw S16LE stereo 48 kHz to stdout."""
import sys, struct, random
dur = float(sys.argv[1])
sr = 48_000
n = int(sr * dur)
buf = []
state = [random.uniform(-1, 1) for _ in range(8)]
for i in range(n):
    for k in range(len(state)):
        if i % (1 << k) == 0:
            state[k] = random.uniform(-1, 1)
    pink = sum(state) / len(state)
    s = int(0.3 * 32767 * pink)
    buf.append(struct.pack("<hh", s, s))
sys.stdout.buffer.write(b"".join(buf))
