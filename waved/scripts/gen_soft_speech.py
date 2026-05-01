#!/usr/bin/env python3
"""−40 dBFS multi-tone in the speech band — stand-in for soft talker."""
import sys, struct, math
dur = float(sys.argv[1])
sr = 48_000
n = int(sr * dur)
amp = 10**(-40.0/20.0)
buf = []
for i in range(n):
    t = i / sr
    s = (math.sin(2*math.pi*500*t) + math.sin(2*math.pi*1500*t) + math.sin(2*math.pi*3000*t)) / 3.0
    v = int(amp * 32767 * s)
    buf.append(struct.pack("<hh", v, v))
sys.stdout.buffer.write(b"".join(buf))
