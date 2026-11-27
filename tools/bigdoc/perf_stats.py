#!/usr/bin/env python3
"""Sum up a Kantor log written with KANTOR_PERF=1 (stderr).

    python3 -I tools/bigdoc/perf_stats.py LOG

Prints the open timings, then the key→paint latencies (p50/p95/max) per
step of the run, the steps being the `[rig] mark NAME` lines the driver
(tools/bigdoc/README.md) writes into the log, the scroll benchmark
results, memory lines, and the layout phases' mean times.
"""

import re
import sys
from collections import defaultdict


def pct(v):
    v = sorted(v)
    p = lambda q: v[min(len(v) - 1, len(v) * q // 100)]
    return f"p50 {p(50):8.1f}  p95 {p(95):8.1f}  max {v[-1]:8.1f} ms  (n={len(v)})"


def main():
    lines = open(sys.argv[1], errors="replace").read().splitlines()
    step = "start"
    steps = defaultdict(list)
    order = []
    phases = defaultdict(list)
    for l in lines:
        if l.startswith("[docs] open") or l.startswith("[docs] bench") or l.startswith("[rig] mem"):
            print(l)
        m = re.match(r"\[rig\] mark (\S+)", l)
        if m:
            step = m.group(1)
            continue
        m = re.match(r"\[docs\] input (.*)→paint\s+([\d.]+) ms", l)
        if m:
            if step not in steps:
                order.append(step)
            steps[step].append(float(m.group(2)))
        m = re.match(r"\[perf\] (.*?)\s+([\d.]+) ms", l)
        if m:
            phases[m.group(1).strip()].append(float(m.group(2)))
    for s in order:
        print(f"{s:12} {pct(steps[s])}")
    print("phases (mean ms, count):")
    for k, v in sorted(phases.items(), key=lambda kv: -sum(kv[1]) / len(kv[1]))[:16]:
        print(f"  {k:32} {sum(v) / len(v):8.1f}  x{len(v)}")


if __name__ == "__main__":
    main()
