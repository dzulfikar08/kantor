#!/usr/bin/env python3
"""Write the synthetic datasets kantor_pls's golden tests estimate.

Each dataset comes from a population model (standardized latent variables,
indicators = loading x latent + error, rounded to a Likert scale), drawn
with a fixed seed, so the files are the same on every run. They are made
here, not copied from R packages or textbooks, so they can ship with Kantor.

    python3 tools/pls/gen_data.py            # writes crates/kantor_pls/tests/data/*.csv

Datasets:
  ecsi_synth   7 constructs shaped like the ECSI mobile phone model
               (Tenenhaus et al. 2005): image, expectation, quality,
               value, satisfaction, complaints, loyalty; n = 250, 1-10.
  corp_synth   the corporate reputation model of Hair et al.'s primer:
               four formative drivers (QUAL, PERF, CSOR, ATTR), COMP and
               LIKE, single-item CUSA, CUSL; n = 344, 1-7, some values
               missing (coded -99).
  mediation    X -> M -> Y with a direct path; n = 150, 1-5, a few empty
               cells (for mean replacement).
  many         12 constructs, 3-4 indicators each, a layered network;
               n = 500, 1-7.
"""

import math
import random
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates" / "kantor_pls" / "tests" / "data"


def likert(x, lo, hi, rng):
    """A standard normal value to a lo..hi scale (centred, a little skewed)."""
    mid = (lo + hi) / 2 + 0.6
    v = round(mid + x * (hi - lo) / 4.5)
    return max(lo, min(hi, v))


def draw(spec, n, seed, scale, extra=None):
    """spec: list of (construct, kind, items, preds) in causal order.

    kind "reflective": items = [loadings]; the latent is
    sum(beta * pred) + noise. kind "formative": items = [weights]; the
    items are correlated normals and the latent is their weighted sum.
    preds: [(pred, beta)].
    """
    rng = random.Random(seed)
    lo, hi = scale
    rows = []
    for _ in range(n):
        lat = {}
        row = {}
        for name, kind, items, preds in spec:
            if kind == "formative":
                common = rng.gauss(0, 1)
                xs = [0.45 * common + math.sqrt(1 - 0.45**2) * rng.gauss(0, 1) for _ in items]
                val = sum(w * x for w, x in zip(items, xs))
                val += sum(b * lat[p] for p, b in preds)
                lat[name] = val / 1.2
                for i, x in enumerate(xs, 1):
                    row[f"{name}_{i}"] = likert(x, lo, hi, rng)
            else:
                explained = sum(b * lat[p] for p, b in preds)
                var = sum(b * b for _, b in preds)
                noise = math.sqrt(max(0.2, 1 - min(var, 0.8)))
                lat[name] = explained + noise * rng.gauss(0, 1)
                for i, l in enumerate(items, 1):
                    x = l * lat[name] + math.sqrt(max(0.05, 1 - l * l)) * rng.gauss(0, 1)
                    row[f"{name}_{i}"] = likert(x, lo, hi, rng)
        if extra:
            extra(row, rng)
        rows.append(row)
    return rows


def write(name, rows, missing=None):
    cols = list(rows[0].keys())
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / f"{name}.csv"
    with open(path, "w", newline="\n") as f:
        f.write(",".join(cols) + "\n")
        for r in rows:
            f.write(",".join("" if r[c] is None else str(r[c]) for c in cols) + "\n")
    print(f"{path.relative_to(ROOT)}: {len(rows)} rows, {len(cols)} columns", file=sys.stderr)


def ecsi():
    spec = [
        ("IMAG", "reflective", [0.75, 0.62, 0.70, 0.78, 0.66], []),
        ("EXPE", "reflective", [0.80, 0.72, 0.58], [("IMAG", 0.55)]),
        ("QUAL", "reflective", [0.82, 0.70, 0.79, 0.77, 0.68, 0.74, 0.80], [("EXPE", 0.6)]),
        ("VAL", "reflective", [0.90, 0.88], [("EXPE", 0.1), ("QUAL", 0.55)]),
        ("SAT", "reflective", [0.85, 0.82, 0.78], [("IMAG", 0.2), ("EXPE", 0.05), ("QUAL", 0.5), ("VAL", 0.2)]),
        ("COMP", "reflective", [1.0], [("SAT", 0.5)]),
        ("LOY", "reflective", [0.85, 0.40, 0.88], [("IMAG", 0.2), ("SAT", 0.5), ("COMP", 0.05)]),
    ]
    write("ecsi_synth", draw(spec, 250, 20261127, (1, 10)))


def corp():
    spec = [
        ("QUAL", "formative", [0.25, 0.1, 0.2, 0.15, 0.3, 0.05, 0.2, 0.1], []),
        ("PERF", "formative", [0.3, 0.2, 0.15, 0.1, 0.25], []),
        ("CSOR", "formative", [0.3, 0.05, 0.25, 0.2, 0.15], []),
        ("ATTR", "formative", [0.4, 0.3, 0.3], []),
        ("COMP", "reflective", [0.82, 0.80, 0.78], [("QUAL", 0.25), ("PERF", 0.3), ("CSOR", 0.05), ("ATTR", 0.1)]),
        ("LIKE", "reflective", [0.88, 0.86, 0.84], [("QUAL", 0.4), ("PERF", 0.1), ("CSOR", 0.15), ("ATTR", 0.15)]),
        ("CUSA", "reflective", [1.0], [("COMP", 0.15), ("LIKE", 0.45)]),
        ("CUSL", "reflective", [0.83, 0.90, 0.85], [("COMP", 0.0), ("LIKE", 0.35), ("CUSA", 0.5)]),
    ]
    rows = draw(spec, 344, 4344, (1, 7))
    rng = random.Random(99)
    cols = list(rows[0].keys())
    for _ in range(40):
        r = rng.randrange(len(rows))
        rows[r][rng.choice(cols)] = -99
    write("corp_synth", rows)


def mediation():
    spec = [
        ("X", "reflective", [0.8, 0.75, 0.7, 0.72], []),
        ("M", "reflective", [0.78, 0.8, 0.74, 0.69], [("X", 0.5)]),
        ("Y", "reflective", [0.81, 0.77, 0.73, 0.7], [("X", 0.2), ("M", 0.45)]),
    ]
    rows = draw(spec, 150, 715, (1, 5))
    rng = random.Random(7)
    cols = list(rows[0].keys())
    for _ in range(12):
        rows[rng.randrange(len(rows))][rng.choice(cols)] = None
    write("mediation", rows)


def many():
    names = [f"C{i:02d}" for i in range(1, 13)]
    spec = []
    rng = random.Random(12)
    for i, name in enumerate(names):
        items = [round(rng.uniform(0.6, 0.9), 2) for _ in range(3 + i % 2)]
        preds = []
        if i >= 3:
            for p in names[max(0, i - 4) : i - 1]:
                if rng.random() < 0.6:
                    preds.append((p, round(rng.uniform(0.1, 0.4), 2)))
            if not preds:
                preds.append((names[i - 2], 0.3))
        spec.append((name, "reflective", items, preds))
    write("many", draw(spec, 500, 1212, (1, 7)))


if __name__ == "__main__":
    ecsi()
    corp()
    mediation()
    many()
