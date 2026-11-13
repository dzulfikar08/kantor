#!/usr/bin/env python3
"""Catch Office files Kantor writes that Excel, Word or PowerPoint would
open with "We found a problem with some content".

1. `office_scenarios` (tools/fidelity) opens, edits and saves every file under
   tests/corpus/** and writes files that use what Kantor itself makes.
2. Every saved file and every original is validated with Microsoft's Open XML
   SDK (tools/ooxml-validate, Microsoft 365 file format). Errors in a saved
   file that its original doesn't have are Kantor's.
3. With --soffice, every file is converted to PDF with LibreOffice headless;
   a saved file that doesn't convert when its original does is Kantor's too.

Errors listed in tools/fidelity/allowlist.tsv (known false positives of the
SDK, each with a reason) don't count. Exit status 1 when anything else does.

    python3 tools/fidelity/office_check.py \
        --scenarios target/release/office_scenarios \
        --validator 'dotnet tools/ooxml-validate/bin/OoxmlValidate.dll' \
        [--soffice soffice] --out fidelity-office
"""

import argparse
import collections
import fnmatch
import json
import os
import re
import shlex
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def load_allowlist(path):
    rules = []
    if not path.exists():
        return rules
    for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        cols = line.split("\t")
        if len(cols) != 5 or not cols[4].strip():
            sys.exit(f"{path}:{n}: want 5 tab-separated columns with a reason")
        scenario, part, err_id, desc, reason = cols
        rules.append(
            {
                "scenario": scenario,
                "part": part,
                "id": err_id,
                "desc": re.compile(desc),
                "reason": reason,
                "used": 0,
            }
        )
    return rules


def allowed(rules, scenario, err):
    for r in rules:
        if (
            fnmatch.fnmatch(scenario, r["scenario"])
            and fnmatch.fnmatch(err["part"] or "", r["part"])
            and (r["id"] == "*" or r["id"] == err["id"])
            and r["desc"].search(err["description"])
        ):
            r["used"] += 1
            return True
    return False


def key(err):
    return (err["part"], err["id"], err["description"])


def validate(validator, files):
    out = {}
    files = list(files)
    for i in range(0, len(files), 100):
        chunk = files[i : i + 100]
        res = subprocess.run(
            shlex.split(validator) + chunk,
            capture_output=True,
            text=True,
            check=True,
        )
        for line in res.stdout.splitlines():
            d = json.loads(line)
            out[d["file"]] = d
    return out


def convert(soffice, files, outdir, profile):
    """PDF path per file (None when LibreOffice made none). Files go in one
    batch; the ones without a PDF are tried again one by one (a crash ends
    the batch)."""
    outdir.mkdir(parents=True, exist_ok=True)

    def run(batch):
        for f in batch:
            pdf = outdir / (Path(f).stem + ".pdf")
            if pdf.exists():
                pdf.unlink()
        try:
            subprocess.run(
                [
                    *shlex.split(soffice),
                    f"-env:UserInstallation={profile.as_uri()}",
                    "--headless",
                    "--norestore",
                    "--convert-to",
                    "pdf",
                    "--outdir",
                    str(outdir),
                    *batch,
                ],
                capture_output=True,
                timeout=120 + 30 * len(batch),
                check=False,
            )
        except subprocess.TimeoutExpired:
            pass

    def ok(f):
        pdf = outdir / (Path(f).stem + ".pdf")
        return pdf.exists() and pdf.read_bytes()[:5] == b"%PDF-"

    files = list(files)
    run(files)
    for f in [f for f in files if not ok(f)]:
        run([f])
    return {f: (outdir / (Path(f).stem + ".pdf")) if ok(f) else None for f in files}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scenarios", required=True, help="office_scenarios binary")
    ap.add_argument("--validator", required=True, help="command running OoxmlValidate")
    ap.add_argument("--soffice", help="LibreOffice command (skip the check if absent)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--allowlist", default=str(ROOT / "tools/fidelity/allowlist.tsv"))
    args = ap.parse_args()

    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    subprocess.run([args.scenarios, str(ROOT), str(out)], check=True)

    problems = []
    rows = []
    for line in (out / "manifest.tsv").read_text(encoding="utf-8").splitlines():
        cols = line.split("\t")
        if cols[0] == "ERROR":
            problems.append(f"scenario `{cols[1]}` failed: {cols[2]}")
            continue
        saved, original, scenario = cols
        rows.append((saved, original or None, scenario))

    originals = sorted({o for _, o, _ in rows if o})
    report = validate(args.validator, originals + [s for s, _, _ in rows])
    rules = load_allowlist(Path(args.allowlist))

    lines = ["# Office file validation", ""]
    new_total = 0
    allowed_total = 0
    for saved, original, scenario in rows:
        s = report[saved]
        if s["open_error"]:
            problems.append(f"`{scenario}`: the SDK can't open it: {s['open_error']}")
            continue
        before = collections.Counter()
        if original:
            o = report[original]
            before.update(key(e) for e in o["errors"])
        new = []
        for e in s["errors"]:
            if before[key(e)] > 0:
                before[key(e)] -= 1
                continue
            if allowed(rules, scenario, e):
                allowed_total += 1
                continue
            new.append(e)
        new_total += len(new)
        for e in new:
            problems.append(
                f"`{scenario}` {e['part']} {e['path']}: {e['id']}: {e['description']}"
            )

    if args.soffice:
        profile = out / "lo-profile"
        pdf_o = convert(args.soffice, originals, out / "pdf/original", profile)
        pdf_s = convert(args.soffice, [s for s, _, _ in rows], out / "pdf/saved", profile)
        for saved, original, scenario in rows:
            if pdf_s[saved] is None and (original is None or pdf_o[original] is not None):
                problems.append(f"`{scenario}`: LibreOffice can't convert it to PDF")
        lo_failed_orig = [o for o in originals if pdf_o[o] is None]
        lines.append(
            f"LibreOffice: {sum(1 for v in pdf_s.values() if v)} of {len(pdf_s)} saved files "
            f"converted to PDF (originals that don't: {len(lo_failed_orig)})."
        )
    else:
        lines.append("LibreOffice: not run.")

    lines.append(
        f"Open XML SDK: {len(rows)} saved files, {len(originals)} originals; "
        f"{new_total} new errors, {allowed_total} allowlisted."
    )
    lines.append("")
    for r in rules:
        if r["used"] == 0:
            lines.append(
                f"- unused allowlist entry: {r['scenario']} {r['part']} {r['id']} "
                f"(remove it if the error is gone)"
            )
    if problems:
        lines.append("## Problems Kantor introduced")
        lines.append("")
        lines.extend(f"- {p}" for p in problems)
    else:
        lines.append("No problems introduced by Kantor.")
    text = "\n".join(lines) + "\n"
    (out / "report.md").write_text(text, encoding="utf-8")
    (out / "validation.json").write_text(json.dumps(report, indent=1), encoding="utf-8")
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as f:
            f.write(text)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
