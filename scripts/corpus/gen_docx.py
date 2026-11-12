#!/usr/bin/env python3
"""Write the Docs fixtures (CC0, synthetic): tests/corpus/docx/{report,letter,
invoice,minutes}.docx, read by crates/docs_engine/tests/sections.rs and
crates/docs_pdf/tests/.

* report.docx: A4, header, footer "Page {PAGE} of {NUMPAGES}" (complex
  fields), a different first-page header, three numbered headings with
  body text over several pages, a landscape section with a wide table, then
  portrait again.
* letter.docx: a letterhead header, different odd and even footers.
* invoice.docx: a logo picture in the header, a tab-aligned block and a
  styled table.
* minutes.docx: a comment and tracked changes (an insertion, a deletion).

Regenerate with
`uv run --with python-docx==1.2.0 scripts/corpus/gen_docx.py tests/corpus/docx`.
"""
import datetime
import copy
import sys
from pathlib import Path

from docx import Document
from docx.enum.section import WD_ORIENT, WD_SECTION
from docx.enum.text import WD_BREAK
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
from docx.shared import Cm, Pt, RGBColor

out = Path(sys.argv[1] if len(sys.argv) > 1 else "tests/corpus/docx")
FIXED = datetime.datetime(2026, 11, 12, 0, 0, 0)


def save(d, name):
    d.core_properties.created = FIXED
    d.core_properties.modified = FIXED
    d.core_properties.last_modified_by = "Kantor corpus"
    d.core_properties.author = "Kantor corpus"
    d.save(out / name)

out.mkdir(parents=True, exist_ok=True)


def field(par, instr):
    """Append a complex field (PAGE, NUMPAGES) to a paragraph."""
    for kind, text in (("begin", None), ("instr", instr), ("separate", None), ("text", "1"), ("end", None)):
        r = OxmlElement("w:r")
        if kind == "instr":
            t = OxmlElement("w:instrText")
            t.set(qn("xml:space"), "preserve")
            t.text = f" {instr} "
            r.append(t)
        elif kind == "text":
            t = OxmlElement("w:t")
            t.text = text
            r.append(t)
        else:
            fc = OxmlElement("w:fldChar")
            fc.set(qn("w:fldCharType"), kind)
            r.append(fc)
        par._p.append(r)


# ── report: headers/footers, page numbers, different first page, landscape section
d = Document()
s = d.sections[0]
s.page_width, s.page_height = Cm(21), Cm(29.7)
s.different_first_page_header_footer = True
s.header.paragraphs[0].text = "PT Contoh Sejahtera — Quarterly Operations Report"
fp = s.footer.paragraphs[0]
fp.text = "Page "
field(fp, "PAGE")
fp.add_run(" of ")
field(fp, "NUMPAGES")
s.first_page_header.paragraphs[0].text = "CONFIDENTIAL"
d.add_heading("Quarterly Operations Report", 0)
d.add_paragraph("Prepared for the board, Q3 2026.")
for i in range(1, 4):
    d.add_heading(f"{i}. Section heading {i}", 1)
    for _ in range(6):
        d.add_paragraph(
            "Throughput rose across all three lines while unplanned downtime fell. "
            "Maintenance backlog was reduced by a third after the new planning cycle. " * 3
        )
new = d.add_section(WD_SECTION.NEW_PAGE)
new.orientation = WD_ORIENT.LANDSCAPE
new.page_width, new.page_height = Cm(29.7), Cm(21)
d.add_heading("Appendix A — wide table", 1)
t = d.add_table(rows=4, cols=8)
t.style = "Table Grid"
for r in range(4):
    for c in range(8):
        t.cell(r, c).text = f"R{r}C{c}"
back = d.add_section(WD_SECTION.NEW_PAGE)
back.orientation = WD_ORIENT.PORTRAIT
back.page_width, back.page_height = Cm(21), Cm(29.7)
d.add_paragraph("Back in portrait.")
save(d, "report.docx")

# ── letter: letterhead in header, odd/even footers
d = Document()
s = d.sections[0]
s.header.paragraphs[0].text = "PT Contoh Sejahtera · Jl. Sudirman 1, Jakarta"
d.settings.odd_and_even_pages_header_footer = True
s.footer.paragraphs[0].text = "odd footer"
s.even_page_footer.paragraphs[0].text = "even footer"
d.add_paragraph("Jakarta, 12 November 2026")
d.add_paragraph("Kepada Yth.\nBapak Direktur\nPT Mitra Abadi")
d.add_paragraph("Dengan hormat,")
for _ in range(14):
    d.add_paragraph("Bersama surat ini kami sampaikan penawaran harga untuk pekerjaan pemeliharaan. " * 4)
d.add_paragraph("Hormat kami,\n\n\nNama Penandatangan")
save(d, "letter.docx")

# ── invoice: logo in the header, tables, tab-aligned totals
def png(w, h, rgb):
    import struct, zlib
    def chunk(kind, data):
        c = struct.pack(">I", len(data)) + kind + data
        return c + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    row = b"\x00" + bytes(rgb) * w
    raw = zlib.compress(row * h)
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", raw) + chunk(b"IEND", b"")


import io

d = Document()
hp = d.sections[0].header.paragraphs[0]
hp.add_run().add_picture(io.BytesIO(png(40, 20, (30, 90, 160))), width=Cm(3))
hp.add_run("  PT Contoh Sejahtera")
d.add_heading("INVOICE #2026-114", 1)
p = d.add_paragraph("Bill to:\tPT Mitra Abadi\nDate:\t12/11/2026")
t = d.add_table(rows=1, cols=4)
t.style = "Light Grid Accent 1"
for c, h in enumerate(["Item", "Qty", "Price", "Amount"]):
    t.rows[0].cells[c].text = h
for i in range(6):
    row = t.add_row().cells
    row[0].text, row[1].text, row[2].text, row[3].text = f"Service {i+1}", "2", "1.500.000", "3.000.000"
d.add_paragraph("Total:\tRp 18.000.000").runs[0].bold = True
save(d, "invoice.docx")

# ── minutes: comments and tracked changes
d = Document()
d.add_heading("Meeting minutes — 10 Nov 2026", 1)
p1 = d.add_paragraph("Attendees: Ani, Budi, Citra.")
p2 = d.add_paragraph("Decision: the vendor contract is renewed for one year.")
try:
    d.add_comment(p2.runs, text="Check the price clause first.", author="Ani", initials="A")
except Exception as e:  # older python-docx
    print("no comments API:", e)
p3 = d.add_paragraph("Action: Budi sends the ")
# tracked insertion and deletion by hand
ins = OxmlElement("w:ins")
ins.set(qn("w:id"), "901")
ins.set(qn("w:author"), "Citra")
ins.set(qn("w:date"), "2026-11-10T09:00:00Z")
r = OxmlElement("w:r")
t_ = OxmlElement("w:t")
t_.text = "signed "
r.append(t_)
ins.append(r)
p3._p.append(ins)
p3.add_run("draft by Friday")
dl = OxmlElement("w:del")
dl.set(qn("w:id"), "902")
dl.set(qn("w:author"), "Citra")
dl.set(qn("w:date"), "2026-11-10T09:01:00Z")
r = OxmlElement("w:r")
dt = OxmlElement("w:delText")
dt.set(qn("xml:space"), "preserve")
dt.text = " or Monday"
r.append(dt)
dl.append(r)
p3._p.append(dl)
p3.add_run(".")
save(d, "minutes.docx")
print("docx ok")
