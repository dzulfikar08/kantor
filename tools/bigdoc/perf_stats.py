#!/usr/bin/env python3
"""Sum up a Kantor log written with KANTOR_PERF=1 (stderr).

    python3 -I tools/bigdoc/perf_stats.py LOG [--from TEXT] [--to TEXT]

Prints the open timings, and p50/p95/max of input→paint latencies grouped
as typing (single characters, space, backspace), other keys (by name) and
wheel events, plus the slowest layout phases. --from/--to keep only the
lines between the first line containing each text (e.g. a marker echoed
into the log by the driver).
"""

import re
import sys
from collections import defaultdict


def pct(v):
    v = sorted(v)
    p = lambda q: v[min(len(v) - 1, len(v) * q // 100)]
    return f"p50 {p(50):7.1f}  p95 {p(95):7.1f}  max {v[-1]:7.1f} ms  (n={len(v)})"


def main():
    args = sys.argv[1:]
    log = args[0]
    lo = args[args.index("--from") + 1] if "--from" in args else None
    hi = args[args.index("--to") + 1] if "--to" in args else None
    lines = open(log, errors="replace").read().splitlines()
    if lo:
        lines = lines[next((i for i, l in enumerate(lines) if lo in l), 0):]
    if hi:
        lines = lines[: next((i for i, l in enumerate(lines) if hi in l), len(lines))]
    groups = defaultdict(list)
    phases = defaultdict(list)
    for l in lines:
        if l.startswith("[docs] open") or l.startswith("[docs] bench"):
            print(l)
        m = re.match(r"\[docs\] input (.*)→paint\s+([\d.]+) ms", l)
        if m:
            what, v = m.group(1), float(m.group(2))
            key = what[4:] if what.startswith("key ") else what
            if what.startswith("key ") and (len(key) == 1 or key in ("space", "backspace", "shift-" + key[-1:])):
                groups["typing"].append(v)
            else:
                groups[what].append(v)
        m = re.match(r"\[perf\] (.*?)\s+([\d.]+) ms", l)
        if m:
            phases[m.group(1).strip()].append(float(m.group(2)))
    for k, v in sorted(groups.items(), key=lambda kv: -len(kv[1])):
        print(f"{k:28} {pct(v)}")
    print("layout phases (mean ms, count):")
    for k, v in sorted(phases.items(), key=lambda kv: -sum(kv[1]) / len(kv[1]))[:20]:
        print(f"  {k:30} {sum(v) / len(v):8.1f}  x{len(v)}")


if __name__ == "__main__":
    main()
