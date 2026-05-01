#!/usr/bin/env python3
"""cpu_delta.py <before> <after> <budget_pct>
Compare two /proc/<pid>/stat snapshots, fail if delta exceeds pre-SP#5 baseline + budget."""
import sys
before = open(sys.argv[1]).read().split()
after  = open(sys.argv[2]).read().split()
b_total = int(before[13]) + int(before[14])
a_total = int(after [13]) + int(after [14])
hz = 100  # CLK_TCK on this kernel
delta_secs = (a_total - b_total) / hz
window_secs = 15
pct = 100.0 * delta_secs / window_secs
budget = float(sys.argv[3])
pre_sp5_baseline = 11.5  # measured on this device pre-SP#5
limit = pre_sp5_baseline + budget
if pct > limit:
    print(f"FAIL: CPU {pct:.2f}% > {limit:.2f}%"); sys.exit(1)
print(f"OK: CPU {pct:.2f}% (limit {limit:.2f}%)")
