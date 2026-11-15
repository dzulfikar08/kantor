#!/usr/bin/env python3
"""Run real-world Office files through Kantor and sort what goes wrong.

    python3 -I tools/realworld/sweep.py \
        --probe ~/.cache/rw-target/release/realworld_probe \
        --validator 'dotnet ~/.cache/rwtools/validator/OoxmlValidate.dll' \
        [--manifest ~/.cache/realworld/manifest.tsv] [--out ~/.cache/realworld-out/run1] \
        [--timeout 120] [--mem-gb 4] [--jobs 3] [--only xlsx,docx]

For every file in the manifest (tools/realworld/fetch.py writes it),
`realworld_probe` (tools/fidelity/src/realworld.rs) opens it, saves it to its
Office format, opens the saved file again and exports a PDF, under a timeout
and an address-space cap. The saved .xlsx/.docx/.pptx files are then checked
with the Open XML SDK (tools/ooxml-validate); errors the original already
has don't count.

Writes OUT/results.csv (one row per file) and OUT/summary.md (counts by
failure kind, then every failure with its message). Each file gets one
`kind`, the first of: crash, timeout, panic, open_error, save_error,
reopen_error, pdf_error, invalid_save, lost_content, ok. Docs and Slides
files whose reopened copy lays out to another page count than the original
get `pages_changed` (listed in the summary; it doesn't change `kind`).
"""

import argparse
import collections
import csv
import json
import os
import resource
import shlex
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

OOXML = {"xlsx", "xlsm", "docx", "docm", "pptx", "pptm"}
KINDS = [
    "crash", "timeout", "panic", "open_error", "save_error", "reopen_error",
    "pdf_error", "invalid_save", "lost_content", "ok",
]


def load_manifest(path, only):
    rows = []
    with open(path, encoding="utf-8") as f:
        for r in csv.DictReader(f, delimiter="\t"):
            if only and r["format"] not in only:
                continue
            rows.append(r)
    return rows


def probe(args, row, n):
    work = Path(args.out) / "work" / f"{n:04d}"
    work.mkdir(parents=True, exist_ok=True)
    cap = args.mem_gb * 1024**3

    def limits():
        resource.setrlimit(resource.RLIMIT_AS, (cap, cap))
        resource.setrlimit(resource.RLIMIT_CORE, (0, 0))

    start = time.monotonic()
    try:
        p = subprocess.run(
            [args.probe, row["path"], str(work)],
            capture_output=True, text=True, timeout=args.timeout, preexec_fn=limits,
            env={**os.environ, "RUST_BACKTRACE": "0"},
        )
    except subprocess.TimeoutExpired:
        return {"kind": "timeout", "error": f"over {args.timeout} s",
                "ms": int((time.monotonic() - start) * 1000)}, work
    ms = int((time.monotonic() - start) * 1000)
    line = next((l for l in p.stdout.splitlines() if l.startswith("{")), None)
    if p.returncode != 0 or line is None:
        err = (p.stderr.strip().splitlines() or [f"exit {p.returncode}"])[-1]
        if "memory allocation" in p.stderr or p.returncode == -6 and "alloc" in p.stderr:
            err = f"out of memory ({args.mem_gb} GB cap): {err}"
        return {"kind": "crash", "error": f"exit {p.returncode}: {err}"[:300], "ms": ms}, work
    rep = json.loads(line)
    rep["ms"] = ms
    return rep, work


def classify(rep):
    if "kind" in rep:
        return rep["kind"], rep.get("error", "")
    for stage, kind in [("open", "open_error"), ("save", "save_error"),
                        ("reopen", "reopen_error"), ("pdf", "pdf_error"),
                        ("pdf_after", "pdf_error")]:
        st = rep.get(stage)
        if st and st.get("error"):
            e = st["error"]
            if e.startswith("panic:"):
                return "panic", f"{stage}: {e}"
            return kind, e
    return None, ""


def lost(rep):
    """What the round trip lost, as a short string ("" = nothing)."""
    b, a, d = rep.get("before"), rep.get("after"), rep.get("diff") or {}
    if not b or not a:
        return ""
    bits = []
    for k in ("sheets", "cells", "formulas", "paragraphs", "slides", "shapes", "text_chars"):
        if k in b and b[k] != a.get(k):
            bits.append(f"{k} {b[k]}->{a.get(k)}")
    if d.get("changed"):
        ex = d.get("examples") or ([d["example"]] if d.get("example") else [])
        bits.append(f"{d['changed']} changed, e.g. {json.dumps(ex[:2], ensure_ascii=False)[:200]}")
    if rep.get("sheet_names_changed"):
        bits.append("sheet names changed")
    return "; ".join(bits)


def pages_changed(rep):
    """True when the saved-and-reopened copy lays out to another page count
    (Docs and Slides; the probe doesn't print the reopened workbook)."""
    a, b = rep.get("pages"), rep.get("pages_after")
    return a is not None and b is not None and a != b


def validate(validator, files):
    out = {}
    files = [str(f) for f in files]
    cmd = [os.path.expanduser(a) for a in shlex.split(validator)]

    def run(chunk, timeout):
        try:
            p = subprocess.run(cmd + chunk, capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            return False
        for line in p.stdout.splitlines():
            if line.startswith("{"):
                v = json.loads(line)
                out[v["file"]] = v
        return True

    # Batches for speed; a batch that hangs (the SDK can take minutes on a
    # sheet with millions of cells) is redone one file at a time.
    for i in range(0, len(files), 20):
        chunk = files[i:i + 20]
        if not run(chunk, 600):
            for f in chunk:
                if not run([f], 180):
                    out[f] = {"file": f, "open_error": None, "errors": [], "timeout": True}
    return out


def err_key(e):
    return (e.get("part"), e.get("id"), e.get("description"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--probe", required=True)
    ap.add_argument("--validator")
    ap.add_argument("--manifest", default=os.path.expanduser("~/.cache/realworld/manifest.tsv"))
    ap.add_argument("--out", default=os.path.expanduser("~/.cache/realworld-out/run"))
    ap.add_argument("--timeout", type=int, default=120)
    ap.add_argument("--mem-gb", type=int, default=4)
    ap.add_argument("--jobs", type=int, default=3)
    ap.add_argument("--only", default="")
    ap.add_argument("--keep", action="store_true", help="keep saved files and PDFs")
    args = ap.parse_args()
    only = set(filter(None, args.only.split(",")))
    rows = load_manifest(args.manifest, only)
    out = Path(args.out)
    if (out / "work").exists():
        shutil.rmtree(out / "work")
    out.mkdir(parents=True, exist_ok=True)

    def one(item):
        n, row = item
        rep, work = probe(args, row, n)
        print(f"[{n + 1}/{len(rows)}] {row['format']} {Path(row['path']).name}: "
              f"{classify(rep)[0] or 'ok?'} {rep.get('ms')} ms", flush=True)
        return row, rep, work

    with ThreadPoolExecutor(args.jobs) as pool:
        results = list(pool.map(one, enumerate(rows)))

    # Validate saved Office files and their originals.
    checks = {}
    if args.validator:
        todo = []
        for row, rep, work in results:
            saved = work / f"saved.{rep.get('app', '')}"
            if saved.exists() and saved.suffix[1:] in OOXML:
                todo.append(saved)
                if row["format"] in OOXML:
                    todo.append(Path(row["path"]))
        checks = validate(args.validator, todo)
        print(f"validated {len(checks)} of {len(todo)} files", flush=True)

    table = []
    for row, rep, work in results:
        kind, error = classify(rep)
        new_errors = []
        saved = work / f"saved.{rep.get('app', '')}"
        v = checks.get(str(saved))
        if v:
            orig = checks.get(row["path"]) or {"errors": [], "open_error": None}
            have = {err_key(e) for e in orig["errors"]}
            if v.get("open_error") and orig.get("open_error"):
                new_errors = []  # the SDK can't open the original either
            elif v.get("open_error"):
                new_errors = [{"id": "open", "part": "", "description": v["open_error"]}]
            else:
                new_errors = [e for e in v["errors"] if err_key(e) not in have]
        loss = lost(rep)
        if kind is None:
            kind = "invalid_save" if new_errors else ("lost_content" if loss else "ok")
            if kind == "invalid_save":
                e = new_errors[0]
                error = f"{len(new_errors)} new: {e.get('part')} {e.get('id')}: {e.get('description')}"
            elif kind == "lost_content":
                error = loss
        table.append({
            "format": row["format"],
            "file": Path(row["path"]).name,
            "site": row["site"],
            "bytes": row["bytes"],
            "kind": kind,
            "error": error.replace("\n", " ")[:400],
            "ms": rep.get("ms"),
            "open_ms": (rep.get("open") or {}).get("ms"),
            "save_ms": (rep.get("save") or {}).get("ms"),
            "pdf_ms": (rep.get("pdf") or {}).get("ms"),
            "pages": rep.get("pages"),
            "pages_after": rep.get("pages_after"),
            "pages_changed": pages_changed(rep),
            "new_validator_errors": len(new_errors),
            "validator_timeout": bool(v and v.get("timeout")),
            "lost": loss[:300],
            "before": json.dumps(rep.get("before")) if rep.get("before") else "",
            "after": json.dumps(rep.get("after")) if rep.get("after") else "",
            "import_notes": " | ".join(rep.get("import_notes") or [])[:300],
        })
        if not args.keep:
            shutil.rmtree(work, ignore_errors=True)

    with open(out / "results.csv", "w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=list(table[0].keys()))
        w.writeheader()
        w.writerows(table)
    write_summary(out / "summary.md", table)
    counts = collections.Counter(t["kind"] for t in table)
    print(" ".join(f"{k}={counts[k]}" for k in KINDS if counts[k]))


def write_summary(path, table):
    counts = collections.Counter(t["kind"] for t in table)
    by_fmt = collections.defaultdict(collections.Counter)
    for t in table:
        by_fmt[t["format"]][t["kind"]] += 1
    lines = ["# Real-world sweep", "", f"{len(table)} files.", "",
             "| format | files | " + " | ".join(KINDS) + " |",
             "|---|---:|" + "---:|" * len(KINDS)]
    for fmt in sorted(by_fmt):
        c = by_fmt[fmt]
        lines.append(f"| {fmt} | {sum(c.values())} | " + " | ".join(str(c[k] or "") for k in KINDS) + " |")
    lines.append("| **all** | " + str(len(table)) + " | "
                 + " | ".join(str(counts[k] or "") for k in KINDS) + " |")
    slow = sorted((t for t in table if t["ms"]), key=lambda t: -t["ms"])[:10]
    lines += ["", "## Slowest", ""]
    lines += [f"- {t['ms']} ms {t['format']} {t['file']} ({int(t['bytes']) // 1024} KB)" for t in slow]
    moved = [t for t in table if t["pages_changed"]]
    if moved:
        lines += ["", f"## Page count changed by the round trip ({len(moved)})", ""]
        lines += [f"- `{t['format']}` {t['file']} ({t['site']}): {t['pages']} -> {t['pages_after']} pages"
                  for t in moved]
    for kind in KINDS[:-1]:
        group = [t for t in table if t["kind"] == kind]
        if not group:
            continue
        lines += ["", f"## {kind} ({len(group)})", ""]
        for t in sorted(group, key=lambda t: t["error"]):
            lines.append(f"- `{t['format']}` {t['file']} ({t['site']}): {t['error']}")
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


if __name__ == "__main__":
    sys.exit(main())
