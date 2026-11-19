#!/usr/bin/env python3
"""Run real-world PDFs through Kantor's PDF viewer engine and page tools.

    python3 -I tools/realworld/pdf_sweep.py \
        --probe ~/.cache/rw-target/release/pdf_probe \
        [--manifest ~/.cache/realworld/pdf-manifest.tsv] [--out ~/.cache/realworld-out/pdf] \
        [--pages 50] [--dpi 36] [--timeout 300] [--mem-gb 8] [--jobs 2]

The files come from `tools/realworld/fetch.py PLAN --manifest pdf-manifest.tsv`.
For each one, poppler's `pdftoppm -cropbox` renders the first `--pages` pages
(the reference), then `pdf_probe` (tools/fidelity/src/pdf_probe.rs) opens the
file with pdf_engine, renders the same pages and scores each against poppler,
extracts and searches the text, reads the outline and links, and does the
page-tool round trip (page 1 turned, last page deleted, saved with
kantor_pdf). The saved file is checked with `qpdf --check` and reopened with
poppler (`pdfinfo`, page 1 rendered).

Needs poppler-utils and qpdf. Writes OUT/results.csv (one row per file) and
OUT/summary.md: each file gets one `kind`, the first of crash, timeout,
open_error (Kantor refused a file poppler opens), password (both need a
password), unreadable (neither opens it), render_error, save_error,
qpdf_error, reopen_error, render_diff (a page scores over --bad %), ok. Pages
that differ keep Kantor's render as OUT/<file>/k-<n>.ppm beside poppler's
ref/p-<n>.ppm, to look at.
"""

import argparse
import csv
import json
import os
import resource
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

KINDS = ["crash", "timeout", "open_error", "password", "unreadable", "render_error",
         "save_error", "qpdf_error", "reopen_error", "render_diff", "ok"]


def run(cmd, timeout, mem_gb=None):
    def cap():
        if mem_gb:
            lim = mem_gb << 30
            resource.setrlimit(resource.RLIMIT_AS, (lim, lim))
    t = time.perf_counter()
    try:
        p = subprocess.run(cmd, capture_output=True, timeout=timeout, preexec_fn=cap)
    except subprocess.TimeoutExpired:
        return None, (time.perf_counter() - t) * 1000
    return p, (time.perf_counter() - t) * 1000


def poppler_pages(path):
    p, _ = run(["pdfinfo", str(path)], 60)
    if p is None or p.returncode != 0:
        err = (p.stderr.decode(errors="replace") if p else "timeout").strip()
        return None, err
    for line in p.stdout.decode(errors="replace").splitlines():
        if line.startswith("Pages:"):
            return int(line.split()[1]), ""
    return None, "no page count"


def probe_file(row, args, out):
    src = Path(row["path"])
    work = out / src.stem
    shutil.rmtree(work, ignore_errors=True)
    ref = work / "ref"
    ref.mkdir(parents=True)
    res = {"file": src.name, "site": row["site"], "bytes": row["bytes"]}
    pages, perr = poppler_pages(src)
    res["poppler_pages"] = pages if pages is not None else ""
    res["poppler_error"] = perr[:200]
    if pages:
        last = min(pages, args.pages) if args.pages else pages
        p, ms = run(["pdftoppm", "-r", str(args.dpi), "-cropbox", "-f", "1", "-l", str(last),
                     str(src), str(ref / "p")], args.timeout)
        res["poppler_ms"] = round(ms)
        # pdftoppm pads page numbers to the page count's width: p-001.ppm.
        for f in ref.glob("p-*.ppm"):
            f.rename(ref / f"p-{int(f.stem[2:])}.ppm")
    cmd = [args.probe, str(src), str(work), "--pages", str(args.pages),
           "--ref", str(ref), "--dpi", str(args.dpi)]
    p, ms = run(cmd, args.timeout, args.mem_gb)
    res["probe_ms"] = round(ms)
    if p is None:
        res["kind"] = "timeout"
        return res
    try:
        j = json.loads(p.stdout.decode(errors="replace").strip().splitlines()[-1])
    except Exception:  # noqa: BLE001
        res["kind"] = "crash"
        res["error"] = (p.stderr.decode(errors="replace").strip().splitlines() or ["?"])[-1][:300]
        return res
    res["peak_rss_mb"] = round(j.get("peak_rss_mb") or 0)
    res["view_rss_mb"] = round(j.get("view_rss_mb") or 0)
    res["open_ms"] = round(j["open"]["ms"], 1)
    open_err = j["open"]["error"]
    res["pages"] = j.get("pages", "")
    if open_err:
        res["error"] = open_err
        if "password" in open_err:
            res["kind"] = "password" if pages is None else "open_error"
        else:
            res["kind"] = "unreadable" if pages is None else "open_error"
        return res
    pg = j.get("page", [])
    times = [x["render"]["ms"] for x in pg]
    res["rendered"] = len(pg)
    res["render_med_ms"] = round(sorted(times)[len(times) // 2], 1) if times else ""
    res["render_max_ms"] = round(max(times), 1) if times else ""
    res["chars"] = j.get("chars", 0)
    res["hits"] = j.get("hits", 0)
    res["query"] = j.get("query") or ""
    res["outline"] = j.get("outline_items", 0)
    res["links"] = j.get("links", 0)
    errs = [f"p{i + 1}: {x['render']['error']}" for i, x in enumerate(pg) if x["render"]["error"]]
    errs += [f"p{i + 1} text: {x['text']['error']}" for i, x in enumerate(pg) if x["text"]["error"]]
    diffs = [(i + 1, x["diff"]) for i, x in enumerate(pg) if isinstance(x.get("diff"), dict)]
    bad = [(n, d["bad"]) for n, d in diffs if d.get("bad") is not None]
    res["diff_max"] = max((b for _, b in bad), default="")
    res["diff_mean"] = round(sum(b for _, b in bad) / len(bad), 2) if bad else ""
    res["diff_pages"] = " ".join(f"{n}:{b}" for n, b in bad if b > args.bad)
    res["size_off"] = max((d.get("size_off", 0) for _, d in diffs), default="")
    res["pages_differ"] = "" if pages is None or pages == res["pages"] else f"{pages} vs {res['pages']}"
    # poppler's text, to tell "no text layer" from "text not found".
    if pages:
        last = min(pages, args.pages) if args.pages else pages
        t, _ = run(["pdftotext", "-f", "1", "-l", str(last), str(src), "-"], 120)
        res["poppler_chars"] = len("".join(t.stdout.decode(errors="replace").split())) if t else ""
    if errs:
        res["kind"], res["error"] = "render_error", "; ".join(errs)[:300]
        return res
    save_err = j.get("save", {}).get("error")
    res["save_ms"] = round(j.get("save", {}).get("ms", 0))
    if save_err:
        res["kind"], res["error"] = "save_error", save_err[:300]
        return res
    saved = work / "saved.pdf"
    q, _ = run(["qpdf", "--check", str(saved)], 300)
    qout = (q.stdout + q.stderr).decode(errors="replace") if q else "timeout"
    res["qpdf"] = "ok" if q and q.returncode == 0 else ("warnings" if q and q.returncode == 3 else "error")
    if res["qpdf"] != "ok":
        res["qpdf_msg"] = " | ".join(l for l in qout.splitlines() if "WARNING" in l or "error" in l.lower())[:300]
    want = (res["pages"] - 1) if res["pages"] > 1 else res["pages"]
    after, aerr = poppler_pages(saved)
    res["saved_pages_poppler"] = after if after is not None else ""
    res["saved_pages_kantor"] = j.get("pages_after", "")
    r1, _ = run(["pdftoppm", "-r", "18", "-f", "1", "-l", "1", str(saved), str(work / "saved")], 120)
    reopen_err = j.get("reopen", {}).get("error") or (j.get("render_after") or {}).get("error")
    if res["qpdf"] == "error":
        res["kind"], res["error"] = "qpdf_error", res.get("qpdf_msg", "")
    elif after != want or res["saved_pages_kantor"] != want or reopen_err or not r1 or r1.returncode:
        res["kind"] = "reopen_error"
        res["error"] = f"poppler {after} ({aerr}) kantor {res['saved_pages_kantor']} want {want} {reopen_err or ''}"[:300]
    elif res["diff_pages"]:
        res["kind"] = "render_diff"
    else:
        res["kind"] = "ok"
    if res["kind"] in ("ok", "render_diff") and not args.keep:
        saved.unlink(missing_ok=True)
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--probe", required=True)
    ap.add_argument("--manifest", default=os.path.expanduser("~/.cache/realworld/pdf-manifest.tsv"))
    ap.add_argument("--out", default=os.path.expanduser("~/.cache/realworld-out/pdf"))
    ap.add_argument("--pages", type=int, default=50, help="pages to render, 0 = all")
    ap.add_argument("--dpi", type=int, default=36)
    ap.add_argument("--bad", type=float, default=2.0, help="%% of differing cells that flags a page")
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--mem-gb", type=int, default=8)
    ap.add_argument("--jobs", type=int, default=2)
    ap.add_argument("--keep", action="store_true", help="keep every saved file")
    args = ap.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    with open(args.manifest, encoding="utf-8") as f:
        rows = [r for r in csv.DictReader(f, delimiter="\t") if r["format"] == "pdf"]
    results = []
    with ThreadPoolExecutor(args.jobs) as ex:
        for n, res in enumerate(ex.map(lambda r: probe_file(r, args, out), rows), 1):
            results.append(res)
            print(f"[{n}/{len(rows)}] {res['file']}: {res['kind']} {res.get('probe_ms', '')} ms "
                  f"{res.get('error', '') or res.get('diff_pages', '')}", flush=True)
    cols = []
    for r in results:
        cols += [k for k in r if k not in cols]
    with open(out / "results.csv", "w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=cols)
        w.writeheader()
        w.writerows(results)
    counts = {k: sum(r["kind"] == k for r in results) for k in KINDS}
    lines = ["# PDF sweep", "", f"{len(results)} files.", "",
             " | ".join(f"{k} {v}" for k, v in counts.items() if v), ""]
    for k in KINDS[:-1]:
        bad = [r for r in results if r["kind"] == k]
        if bad:
            lines += [f"## {k} ({len(bad)})", ""]
            lines += [f"- {r['file']}: {r.get('error') or r.get('diff_pages', '')}" for r in bad]
            lines.append("")
    slow = sorted(results, key=lambda r: -float(r.get("render_max_ms") or 0))[:10]
    lines += ["## Slowest page", ""]
    lines += [f"- {r.get('render_max_ms')} ms (median {r.get('render_med_ms')}) {r['file']}" for r in slow]
    (out / "summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(" ".join(f"{k}={v}" for k, v in counts.items() if v))


if __name__ == "__main__":
    sys.exit(main())
