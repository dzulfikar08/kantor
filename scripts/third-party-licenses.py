#!/usr/bin/env python3
"""Write THIRD_PARTY_LICENSES.txt: the licence texts of every crate linked into
the `kantor` binary on this platform (normal and build dependencies), which their
licences (MIT, Apache-2.0, BSD, ...) require to ship with the binaries.

    python3 scripts/third-party-licenses.py [OUT]     # default: THIRD_PARTY_LICENSES.txt

Needs the crates' sources locally (run after `cargo build` / `cargo fetch`).
Identical licence files are printed once, with every crate that uses them.
Kantor's own crates are skipped (AGPL-3.0, see LICENSE); vendored copies of
other projects under vendor/ are included.
"""

import json
import os
import re
import subprocess
import sys
from collections import OrderedDict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NAMES = re.compile(r"^(licen[cs]e|copying|notice|copyright|unlicense|authors)([-_.].*)?$", re.I)


def run(*args):
    return subprocess.run(args, cwd=ROOT, check=True, capture_output=True, text=True, encoding="utf-8").stdout


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "THIRD_PARTY_LICENSES.txt")
    meta = json.loads(run("cargo", "metadata", "--format-version", "1", "--locked"))
    by_id = {p["id"]: p for p in meta["packages"]}
    members = set(meta["workspace_members"])

    # The packages in `kantor`'s normal + build graph for this host.
    tree = run("cargo", "tree", "--locked", "-p", "kantor", "-e", "normal,build",
               "--prefix", "none", "--format", "{p}")
    wanted = set()
    for line in tree.splitlines():
        m = re.match(r"^(\S+) v(\S+)", line.strip())
        if m:
            wanted.add((m.group(1), m.group(2)))

    pkgs = []
    for p in meta["packages"]:
        if (p["name"], p["version"]) not in wanted:
            continue
        own = p["id"] in members and not os.path.relpath(p["manifest_path"], ROOT).startswith("vendor")
        if own:
            continue
        pkgs.append(p)
    pkgs.sort(key=lambda p: (p["name"], p["version"]))

    texts = OrderedDict()  # normalised text -> (text, [crate labels])
    missing = []
    for p in pkgs:
        label = f'{p["name"]} {p["version"]}'
        d = os.path.dirname(p["manifest_path"])
        files = sorted(f for f in os.listdir(d) if NAMES.match(f) and os.path.isfile(os.path.join(d, f)))
        found = False
        for f in files:
            if f.lower().startswith("authors"):
                continue
            try:
                t = open(os.path.join(d, f), encoding="utf-8", errors="replace").read().strip()
            except OSError:
                continue
            if not t:
                continue
            key = " ".join(t.split()).lower()
            texts.setdefault(key, (t, []))[1].append(f"{label} ({f})")
            found = True
        if not found:
            missing.append(p)

    with open(out, "w", encoding="utf-8") as w:
        w.write("Third-party software in Kantor\n==============================\n\n")
        w.write("Kantor is licensed under AGPL-3.0-or-later (see LICENSE). It includes the\n")
        w.write("following open-source software, under the licences below. NOTICE lists code\n")
        w.write("adapted into Kantor's own crates (e.g. from GridCraft).\n\n")
        w.write("Crates and their licences\n-------------------------\n\n")
        for p in pkgs:
            repo = p.get("repository") or ""
            w.write(f'{p["name"]} {p["version"]}: {p.get("license") or "see licence file"}'
                    f'{"  " + repo if repo else ""}\n')
        for i, (t, users) in enumerate(texts.values(), 1):
            w.write(f"\n\n{'=' * 78}\nLicence text {i}, used by:\n")
            for u in users:
                w.write(f"  {u}\n")
            w.write(f"{'-' * 78}\n\n{t}\n")
        if missing:
            w.write(f"\n\n{'=' * 78}\nCrates that ship no licence file. Their licence is given in their\n"
                    "manifest; the standard text of each licence named here is included above\n"
                    "(from other crates under the same licence), with these authors as holders:\n\n")
            for p in missing:
                authors = ", ".join(p.get("authors") or []) or "the crate's authors"
                w.write(f'  {p["name"]} {p["version"]}: {p.get("license") or p.get("license_file") or "unknown"}'
                        f' (copyright {authors})\n')
    print(f"wrote {out}: {len(pkgs)} crates, {len(texts)} distinct licence texts, "
          f"{len(missing)} without a licence file")


if __name__ == "__main__":
    main()
