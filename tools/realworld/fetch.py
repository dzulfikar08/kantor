#!/usr/bin/env python3
"""Download the real-world sweep's files (see docs/realworld-sweep.md).

    python3 -I tools/realworld/fetch.py PLAN.tsv [--dest ~/.cache/realworld]
        [--manifest NAME.tsv] [--max-mb 20]

PLAN.tsv has one file per line: `format \t url \t source site \t licence`.
Each file goes to DEST/<format>/<sha256[:10]>-<name> and gets a line in
DEST/manifest.tsv (format, path, url, site, licence, sha256, bytes). Files
already in the manifest are skipped, so a rerun only fetches what's new.
The PDF set (tools/realworld/pdf_sweep.py) keeps its own manifest
(`--manifest pdf-manifest.tsv`) so the Office sweep doesn't pick it up.

The downloads are untrusted data: they are only ever read by Kantor's
engines (tools/realworld/sweep.py), never opened, run or committed. Licences
vary, so they stay out of the repository.
"""

import argparse
import hashlib
import os
import re
import sys
import urllib.parse
import urllib.request
from pathlib import Path

MAX_BYTES = 20_000_000
ZIP = b"PK\x03\x04"
CFB = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1"
MAGIC = {
    "xlsx": ZIP, "xlsm": ZIP, "xlsb": ZIP, "docx": ZIP, "pptx": ZIP, "ods": ZIP,
    "xls": CFB, "doc": CFB, "ppt": CFB,
}
# PDF readers accept junk before the header (Acrobat: within 1024 bytes).
PDF_HEADER_WITHIN = 1024
FIELDS = ["format", "path", "url", "site", "licence", "sha256", "bytes"]


def load_manifest(path):
    rows = {}
    if path.exists():
        for line in path.read_text(encoding="utf-8").splitlines()[1:]:
            cols = line.split("\t")
            if len(cols) == len(FIELDS):
                rows[cols[2]] = dict(zip(FIELDS, cols))
    return rows


def save_manifest(path, rows):
    lines = ["\t".join(FIELDS)]
    for r in sorted(rows.values(), key=lambda r: (r["format"], r["path"])):
        lines.append("\t".join(str(r[f]) for f in FIELDS))
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def is_format(data, fmt):
    if fmt == "pdf":
        return b"%PDF-" in data[:PDF_HEADER_WITHIN + 5]
    return data.startswith(MAGIC.get(fmt, b""))


def fetch(url):
    parts = urllib.parse.urlsplit(url)
    url = parts._replace(path=urllib.parse.quote(parts.path, safe="/%")).geturl()
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0 (kantor-sweep)"})
    with urllib.request.urlopen(req, timeout=60) as r:
        data = r.read(MAX_BYTES + 1)
    if len(data) > MAX_BYTES:
        raise ValueError(f"over {MAX_BYTES // 1_000_000} MB")
    return data


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("plan")
    ap.add_argument("--dest", default=os.path.expanduser("~/.cache/realworld"))
    ap.add_argument("--manifest", default="manifest.tsv", help="file name in DEST")
    ap.add_argument("--max-mb", type=int, default=20)
    args = ap.parse_args()
    global MAX_BYTES
    MAX_BYTES = args.max_mb * 1_000_000
    dest = Path(args.dest)
    dest.mkdir(parents=True, exist_ok=True)
    manifest = dest / args.manifest
    rows = load_manifest(manifest)
    hashes = {r["sha256"] for r in rows.values()}
    plan = [l.split("\t") for l in Path(args.plan).read_text().splitlines() if l.strip()]
    for n, cols in enumerate(plan, 1):
        fmt, url = cols[0].lower(), cols[1]
        site = cols[2] if len(cols) > 2 else urllib.parse.urlparse(url).netloc
        licence = cols[3] if len(cols) > 3 else ""
        if url in rows:
            continue
        try:
            data = fetch(url)
        except Exception as e:  # noqa: BLE001 - report and go on
            print(f"[{n}/{len(plan)}] skip {url}: {e}", file=sys.stderr)
            continue
        if not is_format(data, fmt):
            print(f"[{n}/{len(plan)}] skip {url}: not a .{fmt} file", file=sys.stderr)
            continue
        sha = hashlib.sha256(data).hexdigest()
        if sha in hashes:
            continue
        name = urllib.parse.unquote(urllib.parse.urlparse(url).path.rsplit("/", 1)[-1])
        name = re.sub(r"[^A-Za-z0-9._-]+", "_", name)[-80:]
        if not name.lower().endswith("." + fmt):
            name = f"{name}.{fmt}"
        path = dest / fmt / f"{sha[:10]}-{name}"
        path.parent.mkdir(exist_ok=True)
        path.write_bytes(data)
        hashes.add(sha)
        rows[url] = dict(format=fmt, path=str(path), url=url, site=site,
                         licence=licence, sha256=sha, bytes=len(data))
        print(f"[{n}/{len(plan)}] {fmt} {len(data):>9} {name}")
        if n % 20 == 0:
            save_manifest(manifest, rows)
    save_manifest(manifest, rows)
    print(f"{len(rows)} files in {manifest}")


if __name__ == "__main__":
    main()
