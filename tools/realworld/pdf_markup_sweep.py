#!/usr/bin/env python3
"""Annotate and fill real-world PDFs through Kantor and check the results.

For every PDF in DIR, `pdf_markup_probe` (tools/fidelity) adds a highlight on
real text, a sticky note, a text box, an ink drawing and an image stamp, fills
every AcroForm field, and saves as the viewer's Save does (`saved.pdf`) and
with stamps flattened as Save As → Flatten does (`flat.pdf`), reading both back
with Kantor. This script then checks each result with `qpdf --check`, reopens
it with poppler (`pdfinfo`, page count), renders the annotated page with
poppler before and after and checks every annotation is visible where it was
placed (and Kantor's own render too), and compares the encryption with
`qpdf --show-encryption`.

    python3 -I tools/realworld/pdf_markup_sweep.py --probe target/release/pdf_markup_probe \
        --stamp stamp.png --out OUT ~/.cache/realworld/pdf/*.pdf

Writes OUT/results.csv and OUT/summary.md.
"""

import argparse
import csv
import json
import os
import resource
import subprocess
import sys
from pathlib import Path


def run(cmd, timeout=120, mem_gb=None):
    def limit():
        if mem_gb:
            b = int(mem_gb * 1024**3)
            resource.setrlimit(resource.RLIMIT_AS, (b, b))

    try:
        p = subprocess.run(
            cmd, capture_output=True, text=True, timeout=timeout, preexec_fn=limit
        )
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return None, "", "timeout"


def read_ppm(path):
    data = Path(path).read_bytes()
    fields, i = [], 0
    while len(fields) < 4:
        while data[i : i + 1].isspace():
            i += 1
        if data[i : i + 1] == b"#":
            while data[i : i + 1] != b"\n":
                i += 1
            continue
        s = i
        while not data[i : i + 1].isspace():
            i += 1
        fields.append(data[s:i].decode())
    i += 1
    w, h = int(fields[1]), int(fields[2])
    return w, h, data[i : i + w * h * 3]


def changed_share(a, b, rect, pad=2):
    """Share of pixels in `rect` (px) whose colour differs by more than 24."""
    (wa, ha, pa), (wb, hb, pb) = a, b
    w, h = min(wa, wb), min(ha, hb)
    x0, y0 = max(0, int(rect[0]) - pad), max(0, int(rect[1]) - pad)
    x1, y1 = min(w, int(rect[2]) + pad + 1), min(h, int(rect[3]) + pad + 1)
    if x1 <= x0 or y1 <= y0:
        return None
    n = diff = 0
    for y in range(y0, y1):
        ra, rb = (y * wa + x0) * 3, (y * wb + x0) * 3
        sa, sb = pa[ra : ra + (x1 - x0) * 3], pb[rb : rb + (x1 - x0) * 3]
        for x in range(0, len(sa), 3):
            n += 1
            if max(abs(sa[x] - sb[x]), abs(sa[x + 1] - sb[x + 1]), abs(sa[x + 2] - sb[x + 2])) > 24:
                diff += 1
    return diff / n if n else None


def outside_share(a, b, rects, pad=6):
    """Share of pixels outside all `rects` that changed (collateral damage)."""
    (wa, ha, pa), (wb, hb, pb) = a, b
    if (wa, ha) != (wb, hb):
        return 1.0
    inside = bytearray(wa * ha)
    for r in rects:
        for y in range(max(0, int(r[1]) - pad), min(ha, int(r[3]) + pad + 1)):
            s, e = max(0, int(r[0]) - pad), min(wa, int(r[2]) + pad + 1)
            inside[y * wa + s : y * wa + e] = b"\x01" * max(0, e - s)
    diff = 0
    for i in range(wa * ha):
        if inside[i]:
            continue
        j = i * 3
        if pa[j] != pb[j] or pa[j + 1] != pb[j + 1] or pa[j + 2] != pb[j + 2]:
            if max(abs(pa[j] - pb[j]), abs(pa[j + 1] - pb[j + 1]), abs(pa[j + 2] - pb[j + 2])) > 24:
                diff += 1
    return diff / (wa * ha)


def poppler_page(pdf, page, dpi, out_prefix):
    code, _, err = run(
        ["pdftoppm", "-f", str(page + 1), "-l", str(page + 1), "-r", str(dpi),
         "-cropbox", "-singlefile", str(pdf), str(out_prefix)]
    )
    p = Path(str(out_prefix) + ".ppm")
    return read_ppm(p) if code == 0 and p.exists() else None


def pages_poppler(pdf):
    code, out, err = run(["pdfinfo", str(pdf)])
    if code != 0:
        return None, err.strip().splitlines()[-1:] or ["pdfinfo failed"]
    for line in out.splitlines():
        if line.startswith("Pages:"):
            return int(line.split()[1]), None
    return None, ["no page count"]


def words(pdf):
    """Every word of the document's text (poppler), as a multiset."""
    from collections import Counter

    code, out, _ = run(["pdftotext", "-q", str(pdf), "-"], timeout=300)
    return Counter(out.split()) if code == 0 else None


def lost_words(before, pdf):
    """How many of the original's words the saved file's text lacks."""
    after = words(pdf)
    if before is None or after is None:
        return None
    # The text box's own words can run into the page's ("textfor").
    glued = [w for w in after if any(k in w for k in ("Kantor", "sweep", "text", "box"))]
    lost = before - after
    return sum(n for w, n in lost.items() if not any(w in g for g in glued))


def encryption(pdf):
    code, out, err = run(["qpdf", "--show-encryption", str(pdf)])
    lines = [l for l in out.splitlines() if not l.startswith("User password")]
    return " | ".join(lines[:3]) if lines else err.strip()[:120]


def qpdf_check(pdf):
    code, out, err = run(["qpdf", "--check", str(pdf)])
    if code == 0:
        return "ok", ""
    if code == 3:
        return "warning", (err or out).strip().splitlines()[0][:160]
    return "error", (err or out).strip().splitlines()[0][:160] if (err or out).strip() else f"exit {code}"


def check(res, name, orig_pdf, out, page, added, before_pop, before_k, dpi, filled, orig_pages, orig_enc,
          orig_words):
    """Check one result (`saved` or `flat`); returns a dict of columns."""
    r = res.get(name) or {}
    row = {}
    save_err = (r.get("save") or {}).get("error")
    pdf = out / f"{name}.pdf"
    if save_err or not pdf.exists():
        row[f"{name}_kind"] = "save_error"
        row[f"{name}_error"] = save_err or "no file"
        return row
    q, qmsg = qpdf_check(pdf)
    pages, perr = pages_poppler(pdf)
    rb = r.get("readback") or {}
    row.update({
        f"{name}_qpdf": q,
        f"{name}_qpdf_msg": qmsg,
        f"{name}_pages": pages,
        f"{name}_incremental": r.get("incremental"),
        f"{name}_prefix": r.get("prefix_ok"),
        f"{name}_missing": ";".join(rb.get("missing", [])) if isinstance(rb, dict) else "",
        f"{name}_wrong": ";".join(rb.get("wrong", []))[:200] if isinstance(rb, dict) else "",
        f"{name}_fields_differ": rb.get("fields_differ") if isinstance(rb, dict) else None,
        f"{name}_field_diffs": ";".join(rb.get("field_diffs", []))[:200] if isinstance(rb, dict) else "",
        f"{name}_xfa": rb.get("xfa") if isinstance(rb, dict) else None,
        f"{name}_enc": encryption(pdf),
        f"{name}_lost_words": lost_words(orig_words, pdf),
    })
    after_pop = poppler_page(pdf, page, dpi, out / f"p-{name}")
    kp = out / f"k-{name}.ppm"
    after_k = read_ppm(kp) if kp.exists() else None
    unseen_p, unseen_k = [], []
    for a in added:
        if name == "flat" and a["kind"] == "stamp":
            pass  # drawn into the page now: must still show
        if before_pop and after_pop:
            s = changed_share(before_pop, after_pop, a["px"])
            if s is not None and s < 0.03:
                unseen_p.append(f"{a['kind']}:{s:.3f}")
        if before_k and after_k:
            s = changed_share(before_k, after_k, a["px"])
            if s is not None and s < 0.03:
                unseen_k.append(f"{a['kind']}:{s:.3f}")
    row[f"{name}_unseen_poppler"] = ";".join(unseen_p)
    row[f"{name}_unseen_kantor"] = ";".join(unseen_k)
    if before_pop and after_pop and not filled:
        row[f"{name}_outside"] = round(outside_share(before_pop, after_pop, [a["px"] for a in added]) * 100, 3)
    kind = "ok"
    if q == "error":
        kind = "invalid"
    elif pages is None:
        kind = "poppler_reopen_error"
        row[f"{name}_error"] = ";".join(perr or [])
    elif pages != orig_pages:
        kind = "pages_changed"
    elif (r.get("reopen") or {}).get("error"):
        kind = "kantor_reopen_error"
        row[f"{name}_error"] = r["reopen"]["error"]
    elif row[f"{name}_lost_words"] and not filled:  # a fill replaces old values
        kind = "lost_text"
    elif row[f"{name}_missing"] or row[f"{name}_wrong"]:
        kind = "readback"
    elif row[f"{name}_fields_differ"]:
        kind = "fields_differ"
    elif not orig_enc.startswith("qpdf:") and (
            ("not encrypted" in orig_enc) != ("not encrypted" in row[f"{name}_enc"])):
        # qpdf can't read some damaged originals at all ("unable to find page tree"):
        # their encryption is unknown, not "encrypted".
        kind = "encryption_changed"
    elif unseen_p or unseen_k:
        kind = "not_visible"
    elif row.get(f"{name}_outside", 0) and row[f"{name}_outside"] > 0.5:
        kind = "collateral"
    row[f"{name}_kind"] = kind
    return row


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--probe", required=True)
    ap.add_argument("--stamp", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--dpi", type=float, default=54)
    ap.add_argument("--timeout", type=int, default=180)
    ap.add_argument("--mem-gb", type=float, default=4)
    ap.add_argument("files", nargs="+")
    a = ap.parse_args()
    root = Path(os.path.expanduser(a.out))
    root.mkdir(parents=True, exist_ok=True)
    rows = []
    for i, f in enumerate(sorted(a.files)):
        f = Path(f)
        out = root / f.stem[:40]
        out.mkdir(exist_ok=True)
        code, stdout, stderr = run(
            [a.probe, str(f), str(out), "--stamp", a.stamp, "--dpi", str(a.dpi)],
            timeout=a.timeout, mem_gb=a.mem_gb,
        )
        row = {"file": f.name}
        try:
            res = json.loads(stdout.strip().splitlines()[-1])
        except Exception:
            row["kind"] = "timeout" if code is None else "crash"
            row["error"] = (stderr.strip().splitlines() or [""])[-1][:200]
            rows.append(row)
            print(f"[{i+1}/{len(a.files)}] {f.name}: {row['kind']} {row['error']}", flush=True)
            continue
        open_err = (res.get("open") or {}).get("error") or (res.get("load") or {}).get("error")
        orig_pages, _ = pages_poppler(f)
        row.update({
            "pages": orig_pages,
            "page": res.get("page"),
            "word": res.get("word"),
            "signed": json.dumps(res.get("signatures")),
            "encrypted": res.get("encrypted"),
            "added": len(res.get("added") or []),
            "annots_before": res.get("annots_before"),
            "fields": (res.get("form") or {}).get("fields"),
            "filled": (res.get("form") or {}).get("filled"),
            "xfa": (res.get("form") or {}).get("xfa"),
            "xfa_only": (res.get("form") or {}).get("xfa_only"),
            "orig_enc": encryption(f),
        })
        if open_err:
            row["kind"] = "open_error"
            row["error"] = open_err[:200]
        elif res.get("saved") is None:
            row["kind"] = "refused"
        else:
            page = res["page"]
            before_pop = poppler_page(f, page, a.dpi, out / "p-before")
            kb = out / "k-before.ppm"
            before_k = read_ppm(kb) if kb.exists() else None
            added = res.get("added") or []
            orig_words = words(f)
            for name in ("saved", "flat"):
                row.update(check(res, name, f, out, page, added, before_pop, before_k, a.dpi,
                                 bool(row["filled"]), orig_pages, row["orig_enc"], orig_words))
            kinds = [row.get("saved_kind"), row.get("flat_kind")]
            row["kind"] = next((k for k in kinds if k != "ok"), "ok")
        rows.append(row)
        print(f"[{i+1}/{len(a.files)}] {f.name}: {row['kind']} saved={row.get('saved_kind')} "
              f"flat={row.get('flat_kind')} {row.get('error','')}", flush=True)
    cols = []
    for r in rows:
        for k in r:
            if k not in cols:
                cols.append(k)
    with open(root / "results.csv", "w", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=cols)
        w.writeheader()
        w.writerows(rows)
    from collections import Counter
    c = Counter(r["kind"] for r in rows)
    with open(root / "summary.md", "w") as fh:
        fh.write(f"# PDF annotate + fill sweep\n\n{len(rows)} files: "
                 + ", ".join(f"{k} {v}" for k, v in c.most_common()) + "\n\n")
        for r in rows:
            if r["kind"] != "ok":
                fh.write(f"- {r['file']}: {r['kind']} {r.get('error','')} "
                         f"saved={r.get('saved_kind')} flat={r.get('flat_kind')} "
                         f"{r.get('saved_unseen_poppler','')} {r.get('saved_wrong','')} "
                         f"{r.get('saved_field_diffs','')}\n")
    print(" ".join(f"{k}={v}" for k, v in c.most_common()))


if __name__ == "__main__":
    sys.exit(main())
