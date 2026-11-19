#!/usr/bin/env python3
"""Check that every file type Kantor opens is registered with each OS (CI).

The types are the extensions `AppKind::for_path` (crates/kantor) opens.

- macOS: the Info.plist scripts/macos-info-plist.sh writes. Finder's Open
  With, double-clicks and Dock drops go by the UTIs a bundle declares; each
  type needs its UTI, an import when macOS doesn't know it, and
  LSHandlerRank Alternate (Kantor never takes over the default apps).
- Linux: packaging/kantor.desktop lists every type's MIME type and passes
  all selected files (Exec=kantor %F); kantor-sheets.desktop the spreadsheets.
- Windows: packaging/windows/kantor.iss lists every extension under
  OpenWithProgids and SupportedTypes, opening "%1".

    python3 scripts/check-file-types.py
"""

import plistlib
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Extension -> the UTI macOS (or Office/LibreOffice) knows it by.
EXPECTED = {
    "xlsx": "org.openxmlformats.spreadsheetml.sheet",
    "xlsm": "org.openxmlformats.spreadsheetml.sheet.macroenabled",
    "xls": "com.microsoft.excel.xls",
    "xlt": "com.microsoft.excel.xlt",
    "xlsb": "com.microsoft.excel.sheet.binary.macroenabled",
    "ods": "org.oasis-open.opendocument.spreadsheet",
    "csv": "public.comma-separated-values-text",
    "tsv": "public.tab-separated-values-text",
    "docx": "org.openxmlformats.wordprocessingml.document",
    "rtf": "public.rtf",
    "doc": "com.microsoft.word.doc",
    "dot": "com.microsoft.word.dot",
    "pptx": "org.openxmlformats.presentationml.presentation",
    "ppt": "com.microsoft.powerpoint.ppt",
    "pps": "com.microsoft.powerpoint.pps",
    "pot": "com.microsoft.powerpoint.pot",
}
# Declared by macOS itself: no import needed.
SYSTEM = {
    "org.openxmlformats.spreadsheetml.sheet",
    "com.microsoft.excel.xls",
    "public.comma-separated-values-text",
    "org.openxmlformats.wordprocessingml.document",
    "public.rtf",
    "com.microsoft.word.doc",
    "org.openxmlformats.presentationml.presentation",
    "com.microsoft.powerpoint.ppt",
}


def for_path_extensions():
    """The extensions AppKind::for_path opens."""
    src = (ROOT / "crates/kantor/src/workspace/mod.rs").read_text()
    body = src[src.index("pub fn for_path") :]
    body = body[: body.index("_ => None")]
    exts = set()
    for line in body.splitlines():
        line = line.strip()
        if "=>" in line and line.startswith('"'):
            exts.update(p.strip().strip('"') for p in line.split("=>")[0].split("|"))
    return exts


# Extension -> MIME type (shared-mime-info's name) in the .desktop files.
MIME = {
    "xlsx": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "xlsm": "application/vnd.ms-excel.sheet.macroEnabled.12",
    "xls": "application/vnd.ms-excel",
    "xlt": "application/vnd.ms-excel",
    "xlsb": "application/vnd.ms-excel.sheet.binary.macroEnabled.12",
    "ods": "application/vnd.oasis.opendocument.spreadsheet",
    "csv": "text/csv",
    "tsv": "text/tab-separated-values",
    "docx": "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "rtf": "application/rtf",
    "doc": "application/msword",
    "dot": "application/msword-template",
    "pptx": "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "ppt": "application/vnd.ms-powerpoint",
    "pps": "application/vnd.ms-powerpoint",
    "pot": "application/vnd.ms-powerpoint",
}
SHEETS = {"xlsx", "xlsm", "xls", "xlt", "xlsb", "ods", "csv", "tsv"}


def desktop_entry(path):
    entry = {}
    for line in path.read_text().splitlines():
        if "=" in line and not line.startswith("#"):
            k, v = line.split("=", 1)
            entry[k.strip()] = v.strip()
    return entry


def check_linux(exts, errors):
    for name, wanted, exec_ in [
        ("kantor.desktop", exts, "kantor %F"),
        ("kantor-sheets.desktop", exts & SHEETS, "kantor --sheets %F"),
    ]:
        entry = desktop_entry(ROOT / "packaging" / name)
        if entry.get("Exec") != exec_:
            errors.append(f"{name}: Exec must be `{exec_}` (several files at once)")
        types = set(filter(None, entry.get("MimeType", "").split(";")))
        for ext in sorted(wanted):
            mime = MIME.get(ext)
            if mime is None:
                errors.append(f".{ext} has no MIME type here (add it to MIME)")
            elif mime not in types:
                errors.append(f"{name}: MimeType misses {mime} (.{ext})")


def check_windows(exts, errors):
    iss = (ROOT / "packaging/windows/kantor.iss").read_text()
    for ext in sorted(exts):
        if f'"Software\\Classes\\.{ext}\\OpenWithProgids"' not in iss:
            errors.append(f"kantor.iss: .{ext} has no OpenWithProgids entry")
        if f'SupportedTypes"; ValueType: string; ValueName: ".{ext}"' not in iss:
            errors.append(f"kantor.iss: .{ext} isn't in Applications\\kantor.exe\\SupportedTypes")
        if f'FileAssociations"; ValueType: string; ValueName: ".{ext}"' not in iss:
            errors.append(f"kantor.iss: .{ext} isn't in Capabilities\\FileAssociations")
    if '""{app}\\kantor.exe"" ""%1""' not in iss:
        errors.append('kantor.iss: open commands must pass "%1"')


def main():
    out = subprocess.run(
        [str(ROOT / "scripts/macos-info-plist.sh"), "9.9.9"],
        check=True,
        capture_output=True,
    ).stdout
    plist = plistlib.loads(out)
    errors = []

    if plist.get("CFBundleShortVersionString") != "9.9.9":
        errors.append("version not filled in")
    declared = {}
    for doc in plist["CFBundleDocumentTypes"]:
        name = doc.get("CFBundleTypeName")
        if doc.get("LSHandlerRank") != "Alternate":
            errors.append(f"{name}: LSHandlerRank must be Alternate (don't take over defaults)")
        if doc.get("CFBundleTypeRole") not in ("Editor", "Viewer"):
            errors.append(f"{name}: bad CFBundleTypeRole")
        utis = doc.get("LSItemContentTypes") or []
        if not utis:
            errors.append(f"{name}: no LSItemContentTypes")
        for u in utis:
            declared[u] = name
    imported = {}
    for t in plist.get("UTImportedTypeDeclarations", []):
        uti = t["UTTypeIdentifier"]
        tags = t.get("UTTypeTagSpecification", {})
        imported[uti] = [e.lower() for e in tags.get("public.filename-extension", [])]
        if not t.get("UTTypeConformsTo"):
            errors.append(f"{uti}: no UTTypeConformsTo")

    exts = for_path_extensions()
    if not exts:
        errors.append("couldn't read AppKind::for_path")
    for ext in sorted(exts):
        uti = EXPECTED.get(ext)
        if uti is None:
            errors.append(f".{ext} opens in Kantor but has no UTI here (add it to EXPECTED and the plist)")
            continue
        if uti not in declared:
            errors.append(f".{ext}: {uti} not in CFBundleDocumentTypes")
        if uti not in SYSTEM and ext not in imported.get(uti, []):
            errors.append(f".{ext}: {uti} needs a UTImportedTypeDeclarations entry with that extension")

    check_linux(exts, errors)
    check_windows(exts, errors)

    if errors:
        print("File type check failed:", *errors, sep="\n  ", file=sys.stderr)
        return 1
    print(
        f"File types OK: {len(exts)} extensions; macOS {len(declared)} UTIs "
        f"({len(imported)} imported), Linux and Windows entries"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
