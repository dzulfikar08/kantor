#!/usr/bin/env python3
"""Write the Strict Open XML fixtures (CC0, synthetic) in tests/corpus/strict/
from Transitional corpus files, the way Office's "Strict Open XML" Save As
writes them:

* every Transitional namespace and relationship type becomes its Strict URI
  (http://purl.oclc.org/ooxml/...), and the root says conformance="strict";
* dates.xlsx (from xlsx/04_dates.xlsx): the two date constants become ISO
  8601 date cells (t="d");
* report.docx (from docx/report.docx): page sizes, margins and indents are
  written with units ("1in", "90pt"), font sizes in points, and paragraph
  alignment and indents with start/end;
* deck.pptx (from pptx/deck.pptx): DrawingML and PresentationML percentages
  are written as "35%" (themes, gradients, view proportions).

Read by crates/{sheets,docs,slides}_engine/tests/strict.rs, which compare
each with its Transitional original, and by tools/fidelity (office_scenarios),
which validates the Transitional file Kantor saves. Python standard library
only, deterministic output.

    python3 scripts/corpus/gen_strict.py [--check]
"""
import io
import re
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / "tests" / "corpus"
OUT = CORPUS / "strict"

T = "http://schemas.openxmlformats.org/"
S = "http://purl.oclc.org/ooxml/"


def strict_uri(m):
    area, name = m.group(1), m.group(2)
    if area == "package":
        if name == "relationships/metadata/thumbnail":
            return S + "officeDocument/relationships/metadata/thumbnail"
        return m.group(0)  # OPC: the same in Strict
    if area == "officeDocument" and name.startswith("relationships"):
        name = name.replace("extended-properties", "extendedProperties")
        name = name.replace("custom-properties", "customProperties")
        return S + "officeDocument/" + name
    name = {"extended-properties": "extendedProperties",
            "custom-properties": "customProperties"}.get(name, name)
    return S + area + "/" + name


URI = re.compile(r"http://schemas\.openxmlformats\.org/(\w+)/2006/([\w/-]+)")


def pct(n, scale):
    v = int(n) / scale
    return f"{v:g}%"


def drawing_percentages(xml):
    # Attributes that are ST_Percentage-family in DrawingML/PresentationML.
    xml = re.sub(
        r'(<a:(?:alpha|alphaMod|alphaOff|lumMod|lumOff|tint|shade|satMod|satOff|hueMod)'
        r' val=")(-?\d+)(")',
        lambda m: m.group(1) + pct(m.group(2), 1000) + m.group(3), xml)
    xml = re.sub(r'(<a:gs pos=")(\d+)(")',
                 lambda m: m.group(1) + pct(m.group(2), 1000) + m.group(3), xml)
    xml = re.sub(r'(<a:fillToRect)((?: [lrtb]="-?\d+")+)',
                 lambda m: m.group(1) + re.sub(
                     r'="(-?\d+)"', lambda n: f'="{pct(n.group(1), 1000)}"', m.group(2)),
                 xml)
    xml = re.sub(r'(<p:restored(?:Left|Top) sz=")(\d+)(")',
                 lambda m: m.group(1) + pct(m.group(2), 1000) + m.group(3), xml)
    return xml


def twips(n):
    n = int(n)
    if n % 1440 == 0:
        return f"{n // 1440}in"
    if n % 20 == 0:
        return f"{n // 20}pt"
    return str(n)


def word_measures(xml):
    def attrs(m):
        return m.group(1) + re.sub(
            r'(w:(?:top|right|bottom|left|start|end|header|footer|w|h|hanging|firstLine)=")(\d+)(")',
            lambda a: a.group(1) + twips(a.group(2)) + a.group(3), m.group(2))
    xml = re.sub(r'(<w:(?:pgMar|pgSz|ind))((?: [\w:]+="[^"]*")*)', attrs, xml)
    xml = re.sub(r'(<w:sz(?:Cs)? w:val=")(\d+)(")',
                 lambda m: m.group(1) + f"{int(m.group(2)) / 2:g}pt" + m.group(3), xml)
    xml = re.sub(r'(<w:jc w:val=")left(")', r"\1start\2", xml)
    xml = re.sub(r'(<w:jc w:val=")right(")', r"\1end\2", xml)
    xml = re.sub(r'(<w:ind[^>]*?) w:left=', r"\1 w:start=", xml)
    xml = re.sub(r'(<w:ind[^>]*?) w:right=', r"\1 w:end=", xml)
    return xml


DATES = {"B1": "2025-08-17", "B2": "2025-12-31"}


def date_cells(xml):
    for ref, iso in DATES.items():
        xml, n = re.subn(
            rf'<c r="{ref}" s="(\d+)"><v>\d+</v></c>',
            rf'<c r="{ref}" s="\1" t="d"><v>{iso}</v></c>', xml)
        assert n == 1, ref
    return xml


def root_conformance(xml, root):
    xml, n = re.subn(rf"(<{root}\b)", r'\1 conformance="strict"', xml, count=1)
    return xml


def convert(src, kind):
    out = io.BytesIO()
    with zipfile.ZipFile(src) as zin, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as zout:
        for info in zin.infolist():
            data = zin.read(info.filename)
            name = info.filename
            if name.endswith((".xml", ".rels")):
                xml = data.decode("utf-8")
                xml = URI.sub(strict_uri, xml)
                if kind == "xlsx":
                    if name == "xl/workbook.xml":
                        xml = root_conformance(xml, "workbook")
                    if name == "xl/worksheets/sheet1.xml":
                        xml = date_cells(xml)
                if kind == "docx":
                    if name == "word/document.xml":
                        xml = xml.replace("<w:document ", '<w:document w:conformance="strict" ', 1)
                    if name.startswith("word/") and name.count("/") == 1:
                        xml = word_measures(xml)
                if kind == "pptx":
                    if name == "ppt/presentation.xml":
                        xml = root_conformance(xml, "p:presentation")
                    xml = drawing_percentages(xml)
                if kind != "pptx" and "theme" in name:
                    xml = drawing_percentages(xml)
                data = xml.encode("utf-8")
            zi = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            zi.compress_type = zipfile.ZIP_DEFLATED
            zi.external_attr = 0o644 << 16
            zout.writestr(zi, data)
    return out.getvalue()


FILES = [
    ("xlsx/04_dates.xlsx", "dates.xlsx", "xlsx"),
    ("docx/report.docx", "report.docx", "docx"),
    ("pptx/deck.pptx", "deck.pptx", "pptx"),
]


def parts(data):
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        return [(i.filename, z.read(i)) for i in z.infolist()]


def main():
    check = "--check" in sys.argv
    OUT.mkdir(exist_ok=True)
    stale = []
    for src, name, kind in FILES:
        data = convert(CORPUS / src, kind)
        path = OUT / name
        if check:
            # Parts and their contents, not bytes: another zlib build
            # compresses differently.
            if not path.exists() or parts(path.read_bytes()) != parts(data):
                stale.append(name)
        else:
            path.write_bytes(data)
            print(f"wrote {path.relative_to(ROOT)} ({len(data)} bytes)")
    if stale:
        sys.exit(f"stale: {', '.join(stale)} (run scripts/corpus/gen_strict.py)")


if __name__ == "__main__":
    main()
