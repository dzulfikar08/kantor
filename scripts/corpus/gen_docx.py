#!/usr/bin/env python3
"""Write the Docs fixtures (CC0, synthetic): tests/corpus/docx/{report,letter,
invoice,minutes,toc}.docx, read by crates/docs_engine/tests/sections.rs and
crates/docs_pdf/tests/.

* report.docx: A4, header, footer "Page {PAGE} of {NUMPAGES}" (complex
  fields), a different first-page header, three numbered headings with
  body text over several pages, a landscape section with a wide table, then
  portrait again.
* letter.docx: a letterhead header, different odd and even footers.
* invoice.docx: a logo picture in the header, a tab-aligned block and a
  styled table.
* minutes.docx: a comment and tracked changes (an insertion, a deletion).
* toc.docx: a table of contents (TOC field result, TOC 1/TOC 2 styles with a
  right dot-leader tab) and lines with bar, centre, decimal and leader tabs.
* fields.docx: fields as Word leaves them, with results that are out of date
  on purpose: a table of contents in its content control (hyperlinked
  entries with PAGEREF fields, one heading missing), headings with and
  without `_Toc` bookmarks across a page break, SEQ captions, REF and
  PAGEREF cross-references, DATE with a picture, a TITLE simple field, a
  MERGEFIELD with no result, a HYPERLINK field and an external hyperlink.
* floats.docx: floating objects (`wp:anchor`): a picture with square
  wrapping at the right of the column, a cropped picture placed from the
  page with top-and-bottom wrapping, a rounded-rectangle text box (Word's
  `mc:AlternateContent` with a VML fallback) at a percentage of the page,
  an ellipse behind the text, an arrow in front of it, one aligned to the
  line's character, and a second section continuing past a page.
* tracked.docx: tracked changes as Word saves them, beyond text: a run made
  bold (`w:rPrChange`), a paragraph centred (`w:pPrChange`), a table with a
  deleted row and an inserted row (`w:trPr/w:del`, `w:trPr/w:ins`), an
  insertion in the header and a deletion in a footnote.
* notes.docx: a footnote too long for its page (Word's continuation
  separator and notice), short footnotes, and endnotes placed at the end
  of each section (`w:endnotePr/w:pos w:val="sectEnd"`) in two sections.

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

# ── toc: a table of contents as Word leaves it (a TOC field whose result
# lines use the TOC 1 / TOC 2 styles: a right tab with dot leaders at the
# right margin), then lines with centre, decimal, bar and hyphen-leader tabs.
def tabs_el(*stops):
    tabs = OxmlElement("w:tabs")
    for val, pos, leader in stops:
        t = OxmlElement("w:tab")
        t.set(qn("w:val"), val)
        if leader:
            t.set(qn("w:leader"), leader)
        t.set(qn("w:pos"), str(pos))
        tabs.append(t)
    return tabs


def fld(par, kind, instr=None):
    r = OxmlElement("w:r")
    if instr:
        t = OxmlElement("w:instrText")
        t.set(qn("xml:space"), "preserve")
        t.text = f" {instr} "
        r.append(t)
    else:
        fc = OxmlElement("w:fldChar")
        fc.set(qn("w:fldCharType"), kind)
        r.append(fc)
    par._p.append(r)


d = Document()
sec = d.sections[0]
sec.page_width, sec.page_height = Cm(21), Cm(29.7)
sec.left_margin = sec.right_margin = Cm(2.54)
right = 9026  # the A4 text width with 2.54 cm margins, in twips
for level in (1, 2):
    st = d.styles.add_style(f"TOC {level}", 1)
    st.base_style = d.styles["Normal"]
    st.paragraph_format.space_after = Pt(4)
    st.paragraph_format.left_indent = Cm(0.5 * (level - 1))
    st.element.get_or_add_pPr().insert(0, tabs_el(("right", right, "dot")))
d.add_heading("Contents", 1)
toc = [
    (1, "1. Introduction", "1"),
    (2, "1.1 Purpose of this report", "1"),
    (2, "1.2 Scope", "2"),
    (1, "2. Findings", "3"),
    (2, "2.1 Sales by city", "4"),
    (2, "2.2 Costs", "7"),
    (1, "3. Recommendations", "11"),
    (1, "Appendix A. Data sources", "15"),
]
for i, (level, title, page) in enumerate(toc):
    par = d.add_paragraph(style=f"TOC {level}")
    if i == 0:
        fld(par, "begin")
        fld(par, None, 'TOC \\o "1-3" \\h \\z \\u')
        fld(par, "separate")
    par.add_run(f"{title}\t{page}")
    if i == len(toc) - 1:
        fld(par, "end")
d.add_heading("Price list", 1)
for text in ["Item\tUnit\tPrice", "Coffee beans 1 kg\tbag\t185.50", "Paper cups\tbox\t42.75", "Delivery\ttrip\t1250.00"]:
    par = d.add_paragraph(text)
    par._p.get_or_add_pPr().append(
        tabs_el(("bar", 4300, None), ("center", 5200, None), ("decimal", 8000, None))
    )
par = d.add_paragraph("Signature\t")
par._p.get_or_add_pPr().append(tabs_el(("left", 6000, "underscore")))
par = d.add_paragraph("Chapter one\tpage 3")
par._p.get_or_add_pPr().append(tabs_el(("right", right, "hyphen")))
save(d, "toc.docx")

# ── fields: what F9 / Update Table work on (results deliberately stale).
from docx.oxml import parse_xml
from docx.oxml.ns import nsdecls
from docx.opc.constants import RELATIONSHIP_TYPE as RT

W = nsdecls("w")


def runs(*parts):
    """Run XML for ("t", text) / ("tab",) / ("begin", instr) / ("sep",) /
    ("end",) parts."""
    out = []
    for p in parts:
        if p[0] == "t":
            out.append(f'<w:r><w:t xml:space="preserve">{p[1]}</w:t></w:r>')
        elif p[0] == "tab":
            out.append("<w:r><w:tab/></w:r>")
        elif p[0] == "begin":
            out.append('<w:r><w:fldChar w:fldCharType="begin"/></w:r>')
            out.append(f'<w:r><w:instrText xml:space="preserve"> {p[1]} </w:instrText></w:r>')
        elif p[0] == "sep":
            out.append('<w:r><w:fldChar w:fldCharType="separate"/></w:r>')
        elif p[0] == "end":
            out.append('<w:r><w:fldChar w:fldCharType="end"/></w:r>')
    return "".join(out)


def add(par, xml):
    """Append paragraph content (several elements) to a paragraph."""
    holder = parse_xml(f"<w:p {W} {nsdecls('r')}>{xml}</w:p>")
    for child in list(holder):
        par._p.append(child)


def toc_link(anchor, title, page):
    inner = runs(("t", title), ("tab",), ("begin", f"PAGEREF {anchor} \\h"), ("sep",), ("t", page), ("end",))
    return f'<w:hyperlink w:anchor="{anchor}" w:history="1">{inner}</w:hyperlink>'


d = Document()
d.core_properties.title = "Laporan Kuartal"
sec = d.sections[0]
sec.page_width, sec.page_height = Cm(21), Cm(29.7)
sec.left_margin = sec.right_margin = Cm(2.54)
for level in (1, 2):
    st = d.styles.add_style(f"TOC {level}", 1)
    st.base_style = d.styles["Normal"]
    st.paragraph_format.space_after = Pt(4)
    st.paragraph_format.left_indent = Cm(0.5 * (level - 1))
    st.element.get_or_add_pPr().insert(0, tabs_el(("right", right, "dot")))
toc_instr = 'TOC \\o "1-3" \\h \\z \\u'
sdt = parse_xml(
    f'<w:sdt {W}><w:sdtPr><w:docPartObj><w:docPartGallery w:val="Table of Contents"/>'
    "<w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtEndPr/><w:sdtContent>"
    "<w:p><w:r><w:t>Contents</w:t></w:r></w:p>"
    '<w:p><w:pPr><w:pStyle w:val="TOC1"/></w:pPr>'
    + runs(("begin", toc_instr), ("sep",))
    + toc_link("_Toc100000001", "Introduction", "4")
    + "</w:p>"
    '<w:p><w:pPr><w:pStyle w:val="TOC2"/></w:pPr>'
    + toc_link("_Toc100000002", "Scope", "9")
    + "</w:p>"
    "<w:p>" + runs(("end",)) + "</w:p>"
    "</w:sdtContent></w:sdt>"
)
d.element.body.insert(0, sdt)


def heading(text, level, bookmark=None, bid=None):
    h = d.add_heading(text, level)
    if bookmark:
        h._p.insert(1, parse_xml(f'<w:bookmarkStart {W} w:id="{bid}" w:name="{bookmark}"/>'))
        h._p.append(parse_xml(f'<w:bookmarkEnd {W} w:id="{bid}"/>'))
    return h


heading("Introduction", 1, "_Toc100000001", 1)
d.add_paragraph("This report covers the third quarter.")
heading("Scope", 2, "_Toc100000002", 2)
p = d.add_paragraph()
add(p, runs(("t", "Prepared on "), ("begin", 'DATE \\@ "d MMMM yyyy"'), ("sep",), ("t", "1 January 2020"), ("end",), ("t", " for ")))
add(p, f'<w:fldSimple w:instr=" TITLE \\* MERGEFORMAT "><w:r><w:t>Old title</w:t></w:r></w:fldSimple>')
add(p, runs(("t", ". Dear "), ("begin", "MERGEFIELD FirstName"), ("end",), ("t", ",")))
d.add_paragraph("End of part one.").runs[0].add_break(WD_BREAK.PAGE)
heading("Results", 1)
p = d.add_paragraph(style="Caption")
add(p, '<w:bookmarkStart w:id="3" w:name="_Ref1"/>')
add(p, runs(("t", "Table "), ("begin", "SEQ Table \\* ARABIC"), ("sep",), ("t", "5"), ("end",)))
add(p, '<w:bookmarkEnd w:id="3"/>')
add(p, runs(("t", ": Sales by city")))
p = d.add_paragraph(style="Caption")
add(p, runs(("t", "Table "), ("begin", "SEQ Table \\* ARABIC"), ("sep",), ("t", "9"), ("end",), ("t", ": Costs")))
p = d.add_paragraph()
add(p, runs(
    ("t", "See "), ("begin", "REF _Ref1 \\h"), ("sep",), ("t", "Table 7"), ("end",),
    ("t", " on page "), ("begin", "PAGEREF _Ref1 \\h"), ("sep",), ("t", "3"), ("end",),
    ("t", ", the "), ("begin", 'HYPERLINK "https://kantor.paparanai.id"'), ("sep",), ("t", "Kantor site"), ("end",),
    ("t", " and the "),
))
rid = d.part.relate_to("https://example.com/report", RT.HYPERLINK, is_external=True)
add(p, f'<w:hyperlink r:id="{rid}" w:history="1"><w:r><w:t>online copy</w:t></w:r></w:hyperlink>')
add(p, runs(("t", ".")))
save(d, "fields.docx")

# ── floats: wp:anchor pictures, shapes and text boxes.
NS_WP = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
NS_A = "http://schemas.openxmlformats.org/drawingml/2006/main"
NS_PIC = "http://schemas.openxmlformats.org/drawingml/2006/picture"
NS_WPS = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
NS_MC = "http://schemas.openxmlformats.org/markup-compatibility/2006"
NS_V = "urn:schemas-microsoft-com:vml"
NS_WP14 = "http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing"
EMU = 9525  # per px
NS_ALL = (
    f'xmlns:wp="{NS_WP}" xmlns:a="{NS_A}" xmlns:pic="{NS_PIC}" xmlns:wps="{NS_WPS}" '
    f'xmlns:mc="{NS_MC}" xmlns:v="{NS_V}" xmlns:wp14="{NS_WP14}" '
    'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
)


def anchor(doc_id, name, pos_h, pos_v, cx, cy, wrap, graphic, z, behind=0, dist=(0, 0, 114300, 114300)):
    t, b, l, r = dist
    return (
        f'<wp:anchor distT="{t}" distB="{b}" distL="{l}" distR="{r}" simplePos="0" '
        f'relativeHeight="{z}" behindDoc="{behind}" locked="0" layoutInCell="1" allowOverlap="1">'
        f'<wp:simplePos x="0" y="0"/>{pos_h}{pos_v}<wp:extent cx="{cx}" cy="{cy}"/>'
        f'<wp:effectExtent l="0" t="0" r="0" b="0"/>{wrap}<wp:docPr id="{doc_id}" name="{name}"/>'
        f'<wp:cNvGraphicFramePr/>{graphic}</wp:anchor>'
    )


def pos(axis, rel, inner):
    return f'<wp:position{axis} relativeFrom="{rel}">{inner}</wp:position{axis}>'


def picture(rid, cx, cy, crop=""):
    return (
        f'<a:graphic><a:graphicData uri="{NS_PIC}"><pic:pic><pic:nvPicPr><pic:cNvPr id="0" name="p.png"/>'
        f'<pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{rid}"/>{crop}<a:stretch><a:fillRect/>'
        f'</a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/>'
        f'</a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>'
    )


def shape(prst, cx, cy, fill, line, text=None, extra=""):
    body = ""
    if text:
        paras = "".join(f'<w:p><w:r><w:t xml:space="preserve">{t}</w:t></w:r></w:p>' for t in text)
        body = f'<wps:txbx><w:txbxContent>{paras}</w:txbxContent></wps:txbx>'
    fill_xml = f'<a:solidFill><a:srgbClr val="{fill}"/></a:solidFill>' if fill else "<a:noFill/>"
    return (
        f'<a:graphic><a:graphicData uri="{NS_WPS}"><wps:wsp><wps:cNvSpPr/><wps:spPr><a:xfrm>'
        f'<a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="{prst}"><a:avLst/>'
        f'</a:prstGeom>{fill_xml}<a:ln w="19050"><a:solidFill><a:srgbClr val="{line}"/></a:solidFill>{extra}</a:ln>'
        f'</wps:spPr>{body}<wps:bodyPr rot="0" vert="horz" lIns="91440" tIns="45720" rIns="91440" bIns="45720" '
        f'anchor="t"/></wps:wsp></a:graphicData></a:graphic>'
    )


def drawing(inner):
    return f'<w:r><w:drawing>{inner}</w:drawing></w:r>'


def alternate(inner, fallback):
    return (
        f'<w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing>{inner}</w:drawing>'
        f'</mc:Choice><mc:Fallback>{fallback}</mc:Fallback></mc:AlternateContent></w:r>'
    )


def add_ns(par, xml):
    holder = parse_xml(f"<w:p {W} {NS_ALL}>{xml}</w:p>")
    for child in list(holder):
        par._p.append(child)


LOREM = (
    "Kantor lays out floating objects the way Word does: pictures, shapes and text boxes "
    "anchored to a paragraph, positioned from the page, the margins or the paragraph, with "
    "the text wrapping around them. "
)
d = Document()
sec = d.sections[0]
sec.page_width, sec.page_height = Cm(21), Cm(29.7)
sec.left_margin = sec.right_margin = Cm(2.54)
sec.top_margin = sec.bottom_margin = Cm(2.54)
# Pictures: python-docx adds the parts; their relationship ids go in the anchors.
rid_blue, _ = d.part.get_or_add_image(io.BytesIO(png(80, 60, (40, 110, 200))))
rid_green, _ = d.part.get_or_add_image(io.BytesIO(png(60, 60, (60, 170, 90))))
d.add_heading("Floating objects", 1)
p = d.add_paragraph()
add_ns(p, drawing(anchor(
    101, "Square picture",
    pos("H", "margin", "<wp:align>right</wp:align>"),
    pos("V", "paragraph", "<wp:posOffset>0</wp:posOffset>"),
    160 * EMU, 120 * EMU, '<wp:wrapSquare wrapText="bothSides"/>',
    picture(rid_blue, 160 * EMU, 120 * EMU), 251659264,
)))
p.add_run(LOREM * 4)
p = d.add_paragraph()
add_ns(p, drawing(anchor(
    102, "Cropped picture",
    pos("H", "page", "<wp:posOffset>1143000</wp:posOffset>"),
    pos("V", "page", f"<wp:posOffset>{600 * EMU}</wp:posOffset>"),
    120 * EMU, 80 * EMU, "<wp:wrapTopAndBottom/>",
    picture(rid_green, 120 * EMU, 80 * EMU, '<a:srcRect l="25000" t="0" r="25000" b="0"/>'), 251659265,
)))
p.add_run(LOREM * 2)
p = d.add_paragraph()
fallback = '<w:pict><v:roundrect style="width:180pt;height:72pt" arcsize="10923f"><v:textbox><w:txbxContent><w:p><w:r><w:t>Text box</w:t></w:r></w:p></w:txbxContent></v:textbox></v:roundrect></w:pict>'
add_ns(p, alternate(anchor(
    103, "Text Box 1",
    '<mc:AlternateContent><mc:Choice Requires="wp14"><wp:positionH relativeFrom="page">'
    '<wp14:pctPosHOffset>55000</wp14:pctPosHOffset></wp:positionH></mc:Choice><mc:Fallback>'
    '<wp:positionH relativeFrom="page"><wp:posOffset>4157345</wp:posOffset></wp:positionH>'
    '</mc:Fallback></mc:AlternateContent>',
    pos("V", "paragraph", f"<wp:posOffset>{20 * EMU}</wp:posOffset>"),
    240 * EMU, 96 * EMU, '<wp:wrapSquare wrapText="largest"/>',
    shape("roundRect", 240 * EMU, 96 * EMU, "FFF4D6", "C08A00", ["Text box", "with two paragraphs"]),
    251659266,
), fallback))
p.add_run(LOREM * 3)
p = d.add_paragraph()
add_ns(p, drawing(anchor(
    104, "Ellipse behind",
    pos("H", "column", "<wp:align>center</wp:align>"),
    pos("V", "line", "<wp:posOffset>0</wp:posOffset>"),
    300 * EMU, 60 * EMU, "<wp:wrapNone/>",
    shape("ellipse", 300 * EMU, 60 * EMU, "DDEBFF", "4472C4"), 251659200, behind=1,
)))
add_ns(p, drawing(anchor(
    105, "Arrow",
    pos("H", "character", "<wp:posOffset>0</wp:posOffset>"),
    pos("V", "line", f"<wp:posOffset>{30 * EMU}</wp:posOffset>"),
    150 * EMU, 0, "<wp:wrapNone/>",
    shape("straightConnector1", 150 * EMU, 0, None, "C00000", extra='<a:tailEnd type="triangle"/>'),
    251659300,
)))
p.add_run(LOREM * 2)
for _ in range(8):
    d.add_paragraph(LOREM * 2)
save(d, "floats.docx")

# ── notes: a long footnote that continues, endnotes at each section's end.
d = Document()
sec = d.sections[0]
sec.page_width, sec.page_height = Cm(21), Cm(29.7)
sec.left_margin = sec.right_margin = Cm(2.54)
fn_part_xml = None


def footnote_ref(par, kind, nid):
    add(par, f'<w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:{kind}Reference w:id="{nid}"/></w:r>')


notes = {"footnote": [], "endnote": []}


def note(kind, nid, paras):
    body = "".join(
        f'<w:p><w:pPr><w:pStyle w:val="{kind.capitalize()}Text"/></w:pPr>'
        + (f'<w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:{kind}Ref/></w:r>' if i == 0 else "")
        + f'<w:r><w:t xml:space="preserve"> {t}</w:t></w:r></w:p>'
        for i, t in enumerate(paras)
    )
    notes[kind].append(f'<w:{kind} w:id="{nid}">{body}</w:{kind}>')


d.add_heading("Notes", 1)
for i in range(30):
    par = d.add_paragraph("Body text before the long note. " * 6)
par = d.add_paragraph("This sentence has a long footnote.")
footnote_ref(par, "footnote", 1)
note("footnote", 1, [f"Line {k + 1} of a footnote that is much too long for the rest of its page, so Word carries it on." for k in range(40)])
par = d.add_paragraph("A short footnote follows.")
footnote_ref(par, "footnote", 2)
note("footnote", 2, ["A short footnote."])
par = d.add_paragraph("And an endnote in the first section.")
footnote_ref(par, "endnote", 1)
note("endnote", 1, ["The first section's endnote."])
d.add_section(WD_SECTION.NEW_PAGE)
d.add_heading("Second section", 1)
par = d.add_paragraph("An endnote in the second section.")
footnote_ref(par, "endnote", 2)
note("endnote", 2, ["The second section's endnote."])


def sep(kind):
    return (
        f'<w:{kind} w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr>'
        f'<w:r><w:separator/></w:r></w:p></w:{kind}>'
        f'<w:{kind} w:type="continuationSeparator" w:id="0"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr>'
        f'<w:r><w:continuationSeparator/></w:r></w:p></w:{kind}>'
        f'<w:{kind} w:type="continuationNotice" w:id="1"><w:p><w:pPr><w:jc w:val="right"/></w:pPr>'
        f'<w:r><w:t>Continued on the next page</w:t></w:r></w:p></w:{kind}>'
    )


from docx.opc.part import Part
from docx.opc.packuri import PackURI

CT = "application/vnd.openxmlformats-officedocument.wordprocessingml.{}s+xml"
for kind in ("footnote", "endnote"):
    # Notes ids 1.. are content; the special ones get -1, 0, 1 → renumber.
    xml = (
        f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<w:{kind}s {W}>'
        + sep(kind).replace('w:id="-1"', 'w:id="-2"').replace('w:id="0"', 'w:id="-1"').replace('w:id="1"', 'w:id="0"')
        + "".join(notes[kind])
        + f"</w:{kind}s>"
    )
    part = Part(PackURI(f"/word/{kind}s.xml"), CT.format(kind), xml.encode(), d.part.package)
    d.part.relate_to(part, f"http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}s")
# Settings: the special notes, and endnotes at each section's end.
settings = d.settings.element
fpr = parse_xml(f'<w:footnotePr {W}><w:footnote w:id="-2"/><w:footnote w:id="-1"/><w:footnote w:id="0"/></w:footnotePr>')
epr = parse_xml(f'<w:endnotePr {W}><w:pos w:val="sectEnd"/><w:endnote w:id="-2"/><w:endnote w:id="-1"/><w:endnote w:id="0"/></w:endnotePr>')
compat = settings.find(qn("w:compat"))
compat.addprevious(fpr)
compat.addprevious(epr)
save(d, "notes.docx")

# ── tracked: formatting, table rows, header and footnote changes by "Citra".
REV = 'w:author="Citra" w:date="2026-11-10T09:00:00Z"'
d = Document()
p = d.add_paragraph()
add(p, f'<w:r><w:rPr><w:b/><w:rPrChange w:id="1" {REV}><w:rPr/></w:rPrChange></w:rPr><w:t>Budget</w:t></w:r>')
add(p, '<w:r><w:t xml:space="preserve"> for 2027.</w:t></w:r>')
p = d.add_paragraph("Approved by the board.")
p._p.get_or_add_pPr().append(parse_xml(f'<w:jc {W} w:val="center"/>'))
p._p.get_or_add_pPr().append(parse_xml(f'<w:pPrChange {W} w:id="2" {REV}><w:pPr/></w:pPrChange>'))
t = d.add_table(rows=3, cols=2)
for r, (a, b) in enumerate([("Item", "Cost"), ("Venue", "Rp 5.000.000"), ("Catering", "Rp 3.000.000")]):
    for c, text in enumerate((a, b)):
        cell = t.cell(r, c)
        cell.paragraphs[0].text = ""
        kind = {1: "del", 2: "ins"}.get(r)
        tag = "delText" if kind == "del" else "t"
        run = f'<w:r><w:{tag} xml:space="preserve">{text}</w:{tag}></w:r>'
        add(cell.paragraphs[0], f'<w:{kind} w:id="{10 + r * 2 + c}" {REV}>{run}</w:{kind}>' if kind else run)
    if r:
        tr = t.rows[r]._tr
        trpr = tr.get_or_add_trPr()
        trpr.append(parse_xml(f'<w:{"del" if r == 1 else "ins"} {W} w:id="{20 + r}" {REV}/>'))
d.add_paragraph("After the table.")
hp = d.sections[0].header.paragraphs[0]
add(hp, f'<w:ins w:id="30" {REV}><w:r><w:t xml:space="preserve">Draft </w:t></w:r></w:ins>')
hp.add_run("Budget memo")
par = d.add_paragraph("A sentence with a footnote.")
footnote_ref(par, "footnote", 1)
xml = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<w:footnotes {W}>'
    + sep("footnote").replace('w:id="-1"', 'w:id="-2"').replace('w:id="0"', 'w:id="-1"').replace('w:id="1"', 'w:id="0"')
    + '<w:footnote w:id="1"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr>'
    + '<w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:footnoteRef/></w:r>'
    + '<w:r><w:t xml:space="preserve"> Prices include tax</w:t></w:r>'
    + f'<w:del w:id="31" {REV}><w:r><w:delText xml:space="preserve"> and delivery</w:delText></w:r></w:del>'
    + '<w:r><w:t>.</w:t></w:r></w:p></w:footnote></w:footnotes>'
)
part = Part(PackURI("/word/footnotes.xml"), CT.format("footnote"), xml.encode(), d.part.package)
d.part.relate_to(part, "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes")
settings = d.settings.element
settings.find(qn("w:compat")).addprevious(
    parse_xml(f'<w:footnotePr {W}><w:footnote w:id="-2"/><w:footnote w:id="-1"/><w:footnote w:id="0"/></w:footnotePr>')
)
save(d, "tracked.docx")
print("docx ok")
