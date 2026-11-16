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
* threads.docx: Word 365 threaded comments: a question with a reply (both
  on the same word), a resolved comment, with commentsExtended,
  commentsIds, commentsExtensible and people parts.
* drawings.docx: a group (`wpg:wgp`, a shape with an outer shadow, an
  ellipse and a picture in scaled child coordinates), a drawing canvas
  (`wpc:wpc`), a column chart with its values cached, a picture turned 30°,
  a text box turned 90°, a shape with a shadow, and a triangle with Tight
  wrapping (`wp:wrapPolygon`).
* diagrams.docx: a group whose shapes hold text (`wps:txbx` inside
  `wpg:wgp`, one shape turned 90°); SmartArt (data model, layout, style,
  colours and Word's drawing part), one shape showing two items; radar,
  bubble, stock and 3-D column charts.
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

# ── threads: Word 365 threaded comments (a reply, a resolved thread) with
# commentsExtended / commentsIds / commentsExtensible / people parts.
W14 = 'xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml"'
d = Document()
p = d.add_paragraph()
threads = [
    # (id, author, initials, text, paraId, parent paraId, done, durableId)
    ("0", "Ani Wijaya", "AW", "Is this the final budget?", "1A000001", None, 0, "2B000001"),
    ("1", "Budi Santoso", "BS", "Yes, approved on Monday.", "1A000002", "1A000001", 0, "2B000002"),
    ("2", "Citra Dewi", "CD", "Fix the date.", "1A000003", None, 1, "2B000003"),
]
add(p, '<w:commentRangeStart w:id="0"/><w:commentRangeStart w:id="1"/>')
add(p, '<w:r><w:t>Budget</w:t></w:r>')
add(p, '<w:commentRangeEnd w:id="0"/><w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:commentReference w:id="0"/></w:r>')
add(p, '<w:commentRangeEnd w:id="1"/><w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:commentReference w:id="1"/></w:r>')
add(p, '<w:r><w:t xml:space="preserve"> for 2027, dated </w:t></w:r>')
add(p, '<w:commentRangeStart w:id="2"/><w:r><w:t>10 Nov</w:t></w:r><w:commentRangeEnd w:id="2"/>')
add(p, '<w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:commentReference w:id="2"/></w:r><w:r><w:t>.</w:t></w:r>')
d.add_paragraph("A second paragraph without comments.")
DATE = "2026-11-10T09:00:00Z"
cx = "".join(
    f'<w:comment w:id="{i}" w:author="{a}" w:date="{DATE}" w:initials="{ini}">'
    f'<w:p w14:paraId="{pid}" w14:textId="77777777"><w:r><w:annotationRef/></w:r><w:r><w:t>{t}</w:t></w:r></w:p></w:comment>'
    for i, a, ini, t, pid, _, _, _ in threads
)
ex = "".join(
    f'<w15:commentEx w15:paraId="{pid}"' + (f' w15:paraIdParent="{par}"' if par else "") + f' w15:done="{done}"/>'
    for _, _, _, _, pid, par, done, _ in threads
)
ids = "".join(f'<w16cid:commentId w16cid:paraId="{t[4]}" w16cid:durableId="{t[7]}"/>' for t in threads)
cex = "".join(f'<w16cex:commentExtensible w16cex:durableId="{t[7]}" w16cex:dateUtc="{DATE}"/>' for t in threads)
people = "".join(
    f'<w15:person w15:author="{a}"><w15:presenceInfo w15:providerId="None" w15:userId="{a}"/></w15:person>'
    for a in ("Ani Wijaya", "Budi Santoso", "Citra Dewi")
)
MC = 'xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"'
W15 = 'xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml"'
DECL = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
WML = "application/vnd.openxmlformats-officedocument.wordprocessingml."
extra = [
    ("comments", WML + "comments+xml", "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments",
     f'<w:comments {W} {W14} {MC} mc:Ignorable="w14">{cx}</w:comments>'),
    ("commentsExtended", WML + "commentsExtended+xml", "http://schemas.microsoft.com/office/2011/relationships/commentsExtended",
     f'<w15:commentsEx {W15} {MC} mc:Ignorable="w15">{ex}</w15:commentsEx>'),
    ("commentsIds", WML + "commentsIds+xml", "http://schemas.microsoft.com/office/2016/09/relationships/commentsIds",
     f'<w16cid:commentsIds xmlns:w16cid="http://schemas.microsoft.com/office/word/2016/wordml/cid" {MC} mc:Ignorable="w16cid">{ids}</w16cid:commentsIds>'),
    ("commentsExtensible", WML + "commentsExtensible+xml", "http://schemas.microsoft.com/office/2018/08/relationships/commentsExtensible",
     f'<w16cex:commentsExtensible xmlns:w16cex="http://schemas.microsoft.com/office/word/2018/wordml/cex" {MC} mc:Ignorable="w16cex">{cex}</w16cex:commentsExtensible>'),
    ("people", WML + "people+xml", "http://schemas.microsoft.com/office/2011/relationships/people",
     f'<w15:people {W15} {MC} mc:Ignorable="w15">{people}</w15:people>'),
]
for name, ct, rel, xml in extra:
    part = Part(PackURI(f"/word/{name}.xml"), ct, (DECL + xml).encode(), d.part.package)
    d.part.relate_to(part, rel)
save(d, "threads.docx")

# ── drawings: groups, a canvas, a chart, turned objects, a shadow, Tight
# wrap following a polygon.
NS_WPG = "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup"
NS_WPC = "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas"
NS_C = "http://schemas.openxmlformats.org/drawingml/2006/chart"
NS_DRAW = NS_ALL + f' xmlns:wpg="{NS_WPG}" xmlns:wpc="{NS_WPC}" xmlns:c="{NS_C}"'


def add_draw(par, xml):
    holder = parse_xml(f"<w:p {W} {NS_DRAW}>{xml}</w:p>")
    for child in list(holder):
        par._p.append(child)


def sp(prst, x, y, cx, cy, fill, line="404040", rot=0, effect=""):
    return (
        f'<wps:wsp><wps:cNvSpPr/><wps:spPr><a:xfrm rot="{rot}"><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
        f'<a:prstGeom prst="{prst}"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="{fill}"/></a:solidFill>'
        f'<a:ln w="12700"><a:solidFill><a:srgbClr val="{line}"/></a:solidFill></a:ln>{effect}</wps:spPr>'
        f'<wps:bodyPr/></wps:wsp>'
    )


def pic(rid, x, y, cx, cy, rot=0):
    return (
        f'<pic:pic><pic:nvPicPr><pic:cNvPr id="0" name="p.png"/><pic:cNvPicPr/></pic:nvPicPr>'
        f'<pic:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>'
        f'<pic:spPr><a:xfrm rot="{rot}"><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
        f'<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>'
    )


SHADOW = ('<a:effectLst><a:outerShdw blurRad="50800" dist="38100" dir="2700000" algn="tl" rotWithShape="0">'
          '<a:srgbClr val="000000"><a:alpha val="40000"/></a:srgbClr></a:outerShdw></a:effectLst>')
d = Document()
rid_red, _ = d.part.get_or_add_image(io.BytesIO(png(80, 40, (200, 60, 50))))
rid_teal, _ = d.part.get_or_add_image(io.BytesIO(png(60, 60, (30, 150, 150))))
d.add_heading("Drawings", 1)
# A group: a rectangle with a shadow, an ellipse and a picture, in child
# coordinates twice the group's size.
p = d.add_paragraph()
G = 200 * EMU
group = (
    f'<a:graphic><a:graphicData uri="{NS_WPG}"><wpg:wgp><wpg:cNvGrpSpPr/><wpg:grpSpPr><a:xfrm>'
    f'<a:off x="0" y="0"/><a:ext cx="{G}" cy="{G // 2}"/><a:chOff x="0" y="0"/><a:chExt cx="{2 * G}" cy="{G}"/>'
    f'</a:xfrm></wpg:grpSpPr>'
    + sp("rect", 0, 0, G, G, "BDD7EE", effect=SHADOW)
    + sp("ellipse", G, 0, G, G // 2, "F8CBAD")
    + pic(rid_red, G, G // 2, G, G // 2)
    + '</wpg:wgp></a:graphicData></a:graphic>'
)
add_draw(p, drawing(anchor(201, "Group 1", pos("H", "margin", "<wp:align>right</wp:align>"),
                            pos("V", "paragraph", "<wp:posOffset>0</wp:posOffset>"),
                            G, G // 2, '<wp:wrapSquare wrapText="bothSides"/>', group, 251660000)))
p.add_run(LOREM * 3)
# A drawing canvas with a rounded rectangle and a picture.
p = d.add_paragraph()
C = 260 * EMU
canvas = (
    f'<a:graphic><a:graphicData uri="{NS_WPC}"><wpc:wpc><wpc:bg><a:solidFill><a:srgbClr val="F2F2F2"/></a:solidFill></wpc:bg>'
    f'<wpc:whole/>'
    + sp("roundRect", 20 * EMU, 20 * EMU, 120 * EMU, 60 * EMU, "C6E0B4")
    + pic(rid_teal, 160 * EMU, 20 * EMU, 60 * EMU, 60 * EMU)
    + '</wpc:wpc></a:graphicData></a:graphic>'
)
add_draw(p, drawing(anchor(202, "Canvas 1", pos("H", "column", "<wp:align>left</wp:align>"),
                            pos("V", "paragraph", "<wp:posOffset>0</wp:posOffset>"),
                            C, 100 * EMU, '<wp:wrapTopAndBottom/>', canvas, 251660001)))
p.add_run(LOREM)
# A chart (clustered columns, values cached in the part).
p = d.add_paragraph()
chart_xml = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    f'<c:chartSpace xmlns:c="{NS_C}" xmlns:a="{NS_A}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
    '<c:chart><c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>Sales</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title>'
    '<c:autoTitleDeleted val="0"/><c:plotArea><c:layout/><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/>'
    '<c:varyColors val="0"/>'
    + "".join(
        f'<c:ser><c:idx val="{i}"/><c:order val="{i}"/><c:tx><c:strRef><c:f>Sheet1!${col}$1</c:f><c:strCache><c:ptCount val="1"/>'
        f'<c:pt idx="0"><c:v>{name}</c:v></c:pt></c:strCache></c:strRef></c:tx>'
        '<c:cat><c:strRef><c:f>Sheet1!$A$2:$A$4</c:f><c:strCache><c:ptCount val="3"/>'
        '<c:pt idx="0"><c:v>Q1</c:v></c:pt><c:pt idx="1"><c:v>Q2</c:v></c:pt><c:pt idx="2"><c:v>Q3</c:v></c:pt></c:strCache></c:strRef></c:cat>'
        f'<c:val><c:numRef><c:f>Sheet1!${col}$2:${col}$4</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="3"/>'
        + "".join(f'<c:pt idx="{k}"><c:v>{v}</c:v></c:pt>' for k, v in enumerate(vals))
        + '</c:numCache></c:numRef></c:val></c:ser>'
        for i, (col, name, vals) in enumerate([("B", "North", (4, 6, 5)), ("C", "South", (3, 5, 7))])
    )
    + '<c:gapWidth val="150"/><c:axId val="1001"/><c:axId val="1002"/></c:barChart>'
    '<c:catAx><c:axId val="1001"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/>'
    '<c:crossAx val="1002"/></c:catAx><c:valAx><c:axId val="1002"/><c:scaling><c:orientation val="minMax"/></c:scaling>'
    '<c:delete val="0"/><c:axPos val="l"/><c:majorGridlines/><c:title><c:tx><c:rich><a:bodyPr rot="-5400000"/><a:p><a:r><a:t>Units</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title>'
    '<c:numFmt formatCode="General" sourceLinked="1"/><c:crossAx val="1001"/></c:valAx></c:plotArea>'
    '<c:legend><c:legendPos val="b"/><c:overlay val="0"/></c:legend><c:plotVisOnly val="1"/></c:chart></c:chartSpace>'
)
chart_part = Part(PackURI("/word/charts/chart1.xml"),
                  "application/vnd.openxmlformats-officedocument.drawingml.chart+xml",
                  chart_xml.encode(), d.part.package)
rid_chart = d.part.relate_to(chart_part, "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart")
CW, CH = 320 * EMU, 200 * EMU
chart = (f'<a:graphic><a:graphicData uri="{NS_C}"><c:chart r:id="{rid_chart}"/></a:graphicData></a:graphic>')
add_draw(p, drawing(anchor(203, "Chart 1", pos("H", "column", "<wp:align>center</wp:align>"),
                            pos("V", "paragraph", "<wp:posOffset>0</wp:posOffset>"),
                            CW, CH, '<wp:wrapTopAndBottom/>', chart, 251660002)))
p.add_run(LOREM)
# A picture turned 30°, a text box turned 90° and a shape with a shadow.
p = d.add_paragraph()
turned_pic = (f'<a:graphic><a:graphicData uri="{NS_PIC}">' + pic(rid_red, 0, 0, 120 * EMU, 60 * EMU, rot=1800000)
              + '</a:graphicData></a:graphic>')
add_draw(p, drawing(anchor(204, "Turned picture", pos("H", "margin", "<wp:posOffset>0</wp:posOffset>"),
                            pos("V", "paragraph", f"<wp:posOffset>{10 * EMU}</wp:posOffset>"),
                            120 * EMU, 60 * EMU, '<wp:wrapSquare wrapText="bothSides"/>', turned_pic, 251660003)))
tb = (
    f'<a:graphic><a:graphicData uri="{NS_WPS}"><wps:wsp><wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm rot="5400000">'
    f'<a:off x="0" y="0"/><a:ext cx="{160 * EMU}" cy="{40 * EMU}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>'
    '<a:solidFill><a:srgbClr val="FFF2CC"/></a:solidFill><a:ln w="9525"><a:solidFill><a:srgbClr val="BF9000"/></a:solidFill></a:ln></wps:spPr>'
    '<wps:txbx><w:txbxContent><w:p><w:r><w:t>Turned text box</w:t></w:r></w:p></w:txbxContent></wps:txbx>'
    '<wps:bodyPr rot="0" vert="horz" lIns="91440" tIns="45720" rIns="91440" bIns="45720" anchor="t"/></wps:wsp></a:graphicData></a:graphic>'
)
add_draw(p, drawing(anchor(205, "Turned text box", pos("H", "margin", "<wp:align>right</wp:align>"),
                            pos("V", "paragraph", f"<wp:posOffset>{60 * EMU}</wp:posOffset>"),
                            160 * EMU, 40 * EMU, '<wp:wrapNone/>', tb, 251660004)))
shadowed = (f'<a:graphic><a:graphicData uri="{NS_WPS}">' + sp("rect", 0, 0, 100 * EMU, 50 * EMU, "DEEBF7", effect=SHADOW)
            + '</a:graphicData></a:graphic>')
add_draw(p, drawing(anchor(206, "Shadowed shape", pos("H", "column", "<wp:align>center</wp:align>"),
                            pos("V", "paragraph", f"<wp:posOffset>{130 * EMU}</wp:posOffset>"),
                            100 * EMU, 50 * EMU, '<wp:wrapNone/>', shadowed, 251660005)))
p.add_run(LOREM * 3)
# Tight wrapping around a triangle's polygon.
p = d.add_paragraph()
tri = (f'<a:graphic><a:graphicData uri="{NS_WPS}">' + sp("triangle", 0, 0, 160 * EMU, 160 * EMU, "FFE699")
       + '</a:graphicData></a:graphic>')
poly = ('<wp:wrapTight wrapText="bothSides"><wp:wrapPolygon edited="0"><wp:start x="10800" y="0"/>'
        '<wp:lineTo x="21600" y="21600"/><wp:lineTo x="0" y="21600"/><wp:lineTo x="10800" y="0"/></wp:wrapPolygon></wp:wrapTight>')
add_draw(p, drawing(anchor(207, "Tight triangle", pos("H", "column", "<wp:align>center</wp:align>"),
                            pos("V", "paragraph", "<wp:posOffset>0</wp:posOffset>"),
                            160 * EMU, 160 * EMU, poly, tri, 251660006)))
p.add_run(LOREM * 5)
save(d, "drawings.docx")

# ── diagrams: text in grouped shapes, SmartArt, more chart types.
def sp_text(prst, x, y, cx, cy, fill, text, rot=0, anchor_v="ctr"):
    paras = "".join(f'<w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t>{t}</w:t></w:r></w:p>' for t in text)
    return (
        f'<wps:wsp><wps:cNvSpPr/><wps:spPr><a:xfrm rot="{rot}"><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
        f'<a:prstGeom prst="{prst}"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="{fill}"/></a:solidFill>'
        f'<a:ln w="12700"><a:solidFill><a:srgbClr val="404040"/></a:solidFill></a:ln></wps:spPr>'
        f'<wps:txbx><w:txbxContent>{paras}</w:txbxContent></wps:txbx>'
        f'<wps:bodyPr rot="0" vert="horz" lIns="91440" tIns="45720" rIns="91440" bIns="45720" anchor="{anchor_v}"/></wps:wsp>'
    )


d = Document()
d.add_heading("Diagrams", 1)
# A group of two labelled boxes and an arrow, in child coordinates twice
# the group's size; the second box is turned 90°.
p = d.add_paragraph()
G = 300 * EMU
group = (
    f'<a:graphic><a:graphicData uri="{NS_WPG}"><wpg:wgp><wpg:cNvGrpSpPr/><wpg:grpSpPr><a:xfrm>'
    f'<a:off x="0" y="0"/><a:ext cx="{G}" cy="{G // 3}"/><a:chOff x="0" y="0"/><a:chExt cx="{2 * G}" cy="{2 * G // 3}"/>'
    f'</a:xfrm></wpg:grpSpPr>'
    + sp_text("roundRect", 0, 0, 2 * G // 5, 2 * G // 3, "BDD7EE", ["Plan", "first"])
    + sp("rightArrow", 2 * G // 5, G // 4, G // 5, G // 6, "A9D18E")
    + sp_text("rect", 3 * G // 5 + G // 6, 2 * G // 15, 2 * G // 3, 2 * G // 5, "F8CBAD", ["Do"], rot=5400000)
    + '</wpg:wgp></a:graphicData></a:graphic>'
)
add_draw(p, drawing(anchor(301, "Labelled group", pos("H", "column", "<wp:align>left</wp:align>"),
                            pos("V", "paragraph", "<wp:posOffset>0</wp:posOffset>"),
                            G, G // 3, '<wp:wrapTopAndBottom/>', group, 251661000)))
p.add_run(LOREM)

# SmartArt, by hand (as gen_pptx_groups.py): a data model, layout, style,
# colours and Word's pre-drawn shapes: three boxes, the middle one an
# empty placeholder item, and a list shape showing two items.
DGM = "http://schemas.openxmlformats.org/drawingml/2006/diagram"
DSP = "http://schemas.microsoft.com/office/drawing/2008/diagram"
FW, FH = 400 * EMU, 160 * EMU
doc_id = "{00000000-0000-0000-0000-000000000001}"
items = [  # (id, text, parent, placeholder)
    ("{A0000000-0000-0000-0000-000000000001}", "Plan", doc_id, False),
    ("{A0000000-0000-0000-0000-000000000002}", "", doc_id, True),
    ("{A0000000-0000-0000-0000-000000000003}", "Ship", doc_id, False),
    ("{A0000000-0000-0000-0000-000000000004}", "Steps", doc_id, False),
    ("{A0000000-0000-0000-0000-000000000005}", "One", "{A0000000-0000-0000-0000-000000000004}", False),
    ("{A0000000-0000-0000-0000-000000000006}", "Two", "{A0000000-0000-0000-0000-000000000004}", False),
]
bw = FW // 5
drawn = [  # (presentation item, data item it shows, paragraphs, box)
    ("{B0000000-0000-0000-0000-000000000001}", items[0][0], ["Plan"], (0, 0, bw, FH // 2)),
    ("{B0000000-0000-0000-0000-000000000002}", items[1][0], [""], (int(bw * 1.25), 0, bw, FH // 2)),
    ("{B0000000-0000-0000-0000-000000000003}", items[2][0], ["Ship"], (int(bw * 2.5), 0, bw, FH // 2)),
    ("{B0000000-0000-0000-0000-000000000004}", items[3][0], ["Steps"], (int(bw * 3.75), 0, bw, FH // 4)),
    ("{B0000000-0000-0000-0000-000000000005}", items[3][0], ["One", "Two"], (int(bw * 3.75), FH // 4, bw, FH // 2)),
]


def dgm_paras(texts, extra=""):
    out = ""
    for t in texts:
        if t:
            out += f'<a:p><a:r><a:rPr lang="en-US"{extra}/><a:t>{t}</a:t></a:r></a:p>'
        else:
            out += '<a:p><a:endParaRPr lang="en-US"/></a:p>'
    return out


pts = (
    f'<dgm:pt modelId="{doc_id}" type="doc"><dgm:prSet loTypeId="urn:microsoft.com/office/officeart/2005/8/layout/default" '
    'loCatId="list" qsTypeId="urn:microsoft.com/office/officeart/2005/8/quickstyle/simple1" qsCatId="simple" '
    'csTypeId="urn:microsoft.com/office/officeart/2005/8/colors/accent1_2" csCatId="accent1"/><dgm:spPr/>'
    f'<dgm:t><a:bodyPr/><a:lstStyle/>{dgm_paras([""])}</dgm:t></dgm:pt>'
)
for mid, text, _, ph in items:
    pr = '<dgm:prSet phldrT="[Text]" phldr="1"/>' if ph else '<dgm:prSet phldrT="[Text]"/>'
    pts += f'<dgm:pt modelId="{mid}">{pr}<dgm:spPr/><dgm:t><a:bodyPr/><a:lstStyle/>{dgm_paras([text])}</dgm:t></dgm:pt>'
for i, (pid, assoc, _, _) in enumerate(drawn):
    pts += (
        f'<dgm:pt modelId="{pid}" type="pres"><dgm:prSet presAssocID="{assoc}" presName="node" '
        f'presStyleLbl="node1" presStyleIdx="{i}" presStyleCnt="{len(drawn)}"/><dgm:spPr/></dgm:pt>'
    )
cxns = ""
order = {}
for n, (mid, _, parent, _) in enumerate(items):
    k = order.get(parent, 0)
    order[parent] = k + 1
    cxns += f'<dgm:cxn modelId="{{C0000000-0000-0000-0000-00000000000{n}}}" srcId="{parent}" destId="{mid}" srcOrd="{k}" destOrd="0"/>'
for n, (pid, assoc, _, _) in enumerate(drawn):
    cxns += (
        f'<dgm:cxn modelId="{{D0000000-0000-0000-0000-00000000000{n}}}" type="presOf" srcId="{assoc}" '
        f'destId="{pid}" srcOrd="0" destOrd="0"/>'
    )
sps = ""
for pid, _, texts, (x, y, cx, cy) in drawn:
    ins = 40000
    sps += (
        f'<dsp:sp modelId="{pid}"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr>'
        f'<dsp:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
        '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="accent1"/></a:solidFill>'
        '<a:ln w="12700"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></dsp:spPr>'
        '<dsp:style><a:lnRef idx="2"><a:scrgbClr r="0" g="0" b="0"/></a:lnRef><a:fillRef idx="1"><a:scrgbClr r="0" g="0" b="0"/></a:fillRef>'
        '<a:effectRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></dsp:style>'
        '<dsp:txBody><a:bodyPr spcFirstLastPara="0" vert="horz" wrap="square" lIns="38100" tIns="38100" rIns="38100" bIns="38100" '
        'numCol="1" spcCol="1270" anchor="ctr" anchorCtr="0"><a:noAutofit/></a:bodyPr><a:lstStyle/>'
        + dgm_paras(texts, ' sz="1400" kern="1200"').replace("<a:p>", '<a:p><a:pPr algn="ctr"/>')
        + f'</dsp:txBody><dsp:txXfrm><a:off x="{x + ins}" y="{y + ins}"/><a:ext cx="{cx - 2 * ins}" cy="{cy - 2 * ins}"/></dsp:txXfrm></dsp:sp>'
    )
dgm_drawing = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dsp:drawing xmlns:dgm="{DGM}" xmlns:dsp="{DSP}" xmlns:a="{NS_A}">'
    '<dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/>'
    f"{sps}</dsp:spTree></dsp:drawing>"
)
dgm_layout = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dgm:layoutDef xmlns:dgm="{DGM}" xmlns:a="{NS_A}" '
    'uniqueId="urn:microsoft.com/office/officeart/2005/8/layout/default"><dgm:title val=""/><dgm:desc val=""/>'
    '<dgm:catLst><dgm:cat type="list" pri="400"/></dgm:catLst><dgm:layoutNode name="diagram"><dgm:alg type="snake"/>'
    '<dgm:shape/><dgm:presOf/></dgm:layoutNode></dgm:layoutDef>'
)
dgm_style = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dgm:styleDef xmlns:dgm="{DGM}" xmlns:a="{NS_A}" '
    'uniqueId="urn:microsoft.com/office/officeart/2005/8/quickstyle/simple1"><dgm:title val=""/><dgm:desc val=""/>'
    '<dgm:styleLbl name="node1"/></dgm:styleDef>'
)
dgm_colors = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dgm:colorsDef xmlns:dgm="{DGM}" xmlns:a="{NS_A}" '
    'uniqueId="urn:microsoft.com/office/officeart/2005/8/colors/accent1_2"><dgm:title val=""/><dgm:desc val=""/></dgm:colorsDef>'
)
CT_DGM = "application/vnd.openxmlformats-officedocument.drawingml."
REL_DGM = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/"


def dgm_part(name, ct, xml, rel):
    part = Part(PackURI(f"/word/diagrams/{name}"), ct, xml.encode(), d.part.package)
    return d.part.relate_to(part, rel)


rid_dr = dgm_part("drawing1.xml", "application/vnd.ms-office.drawingml.diagramDrawing+xml", dgm_drawing,
                  "http://schemas.microsoft.com/office/2007/relationships/diagramDrawing")
dgm_data = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dgm:dataModel xmlns:dgm="{DGM}" xmlns:a="{NS_A}">'
    f"<dgm:ptLst>{pts}</dgm:ptLst><dgm:cxnLst>{cxns}</dgm:cxnLst><dgm:bg/><dgm:whole/>"
    f'<dgm:extLst><a:ext uri="http://schemas.microsoft.com/office/drawing/2008/diagram"><dsp:dataModelExt xmlns:dsp="{DSP}" '
    f'relId="{rid_dr}" minVer="http://schemas.openxmlformats.org/drawingml/2006/diagram"/></a:ext></dgm:extLst></dgm:dataModel>'
)
rid_dm = dgm_part("data1.xml", CT_DGM + "diagramData+xml", dgm_data, REL_DGM + "diagramData")
rid_lo = dgm_part("layout1.xml", CT_DGM + "diagramLayout+xml", dgm_layout, REL_DGM + "diagramLayout")
rid_qs = dgm_part("quickStyle1.xml", CT_DGM + "diagramStyle+xml", dgm_style, REL_DGM + "diagramQuickStyle")
rid_cs = dgm_part("colors1.xml", CT_DGM + "diagramColors+xml", dgm_colors, REL_DGM + "diagramColors")
p = d.add_paragraph()
smartart = (
    f'<a:graphic><a:graphicData uri="{DGM}"><dgm:relIds xmlns:dgm="{DGM}" r:dm="{rid_dm}" r:lo="{rid_lo}" '
    f'r:qs="{rid_qs}" r:cs="{rid_cs}"/></a:graphicData></a:graphic>'
)
add_draw(p, drawing(anchor(302, "Diagram 1", pos("H", "column", "<wp:align>center</wp:align>"),
                            pos("V", "paragraph", "<wp:posOffset>0</wp:posOffset>"),
                            FW, FH, '<wp:wrapTopAndBottom/>', smartart, 251661001)))
p.add_run(LOREM)


# More chart types: radar, bubble, stock (high-low-close) and 3-D columns.
def cat_ser(i, name, vals, extra=""):
    cats = "".join(f'<c:pt idx="{k}"><c:v>{c}</c:v></c:pt>' for k, c in enumerate(["Mon", "Tue", "Wed", "Thu"][:len(vals)]))
    pts = "".join(f'<c:pt idx="{k}"><c:v>{v}</c:v></c:pt>' for k, v in enumerate(vals))
    return (
        f'<c:ser><c:idx val="{i}"/><c:order val="{i}"/><c:tx><c:v>{name}</c:v></c:tx>{extra}'
        f'<c:cat><c:strLit><c:ptCount val="{len(vals)}"/>{cats}</c:strLit></c:cat>'
        f'<c:val><c:numLit><c:formatCode>General</c:formatCode><c:ptCount val="{len(vals)}"/>{pts}</c:numLit></c:val></c:ser>'
    )


def bubble_ser(i, name, xs, ys, sizes):
    lit = lambda vs: "".join(f'<c:pt idx="{k}"><c:v>{v}</c:v></c:pt>' for k, v in enumerate(vs))
    n = len(xs)
    return (
        f'<c:ser><c:idx val="{i}"/><c:order val="{i}"/><c:tx><c:v>{name}</c:v></c:tx><c:invertIfNegative val="0"/>'
        f'<c:xVal><c:numLit><c:ptCount val="{n}"/>{lit(xs)}</c:numLit></c:xVal>'
        f'<c:yVal><c:numLit><c:ptCount val="{n}"/>{lit(ys)}</c:numLit></c:yVal>'
        f'<c:bubbleSize><c:numLit><c:ptCount val="{n}"/>{lit(sizes)}</c:numLit></c:bubbleSize><c:bubble3D val="0"/></c:ser>'
    )


AXES = ('<c:catAx><c:axId val="2001"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/>'
        '<c:crossAx val="2002"/></c:catAx><c:valAx><c:axId val="2002"/><c:scaling><c:orientation val="minMax"/></c:scaling>'
        '<c:delete val="0"/><c:axPos val="l"/><c:majorGridlines/><c:numFmt formatCode="General" sourceLinked="0"/><c:crossAx val="2001"/></c:valAx>')
VAL_AXES = ('<c:valAx><c:axId val="2001"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/>'
            '<c:crossAx val="2002"/></c:valAx><c:valAx><c:axId val="2002"/><c:scaling><c:orientation val="minMax"/></c:scaling>'
            '<c:delete val="0"/><c:axPos val="l"/><c:majorGridlines/><c:crossAx val="2001"/></c:valAx>')
AX_IDS = '<c:axId val="2001"/><c:axId val="2002"/>'
more_charts = [
    ("Radar chart", "Radar", '<c:radarChart><c:radarStyle val="marker"/><c:varyColors val="0"/>'
     + cat_ser(0, "Team A", (4, 3, 5, 2)) + cat_ser(1, "Team B", (2, 5, 3, 4)) + AX_IDS + '</c:radarChart>' + AXES),
    ("Bubble chart", "Bubbles", '<c:bubbleChart><c:varyColors val="0"/>'
     + bubble_ser(0, "Plants", (1, 2, 3), (3, 1, 2), (5, 10, 3)) + '<c:bubbleScale val="100"/>' + AX_IDS + '</c:bubbleChart>' + VAL_AXES),
    ("Stock chart", "Prices", '<c:stockChart>'
     + cat_ser(0, "High", (12, 14, 13, 15)) + cat_ser(1, "Low", (8, 9, 10, 11)) + cat_ser(2, "Close", (10, 13, 11, 14))
     + '<c:hiLowLines/>' + AX_IDS + '</c:stockChart>' + AXES),
    ("3-D column chart", "Output", '<c:bar3DChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/>'
     + cat_ser(0, "Line 1", (5, 7, 6)) + '<c:gapWidth val="150"/><c:shape val="box"/>' + AX_IDS + '<c:axId val="0"/></c:bar3DChart>' + AXES),
]
for n, (name, title, plot) in enumerate(more_charts):
    view3d = '<c:view3D><c:rotX val="15"/><c:rotY val="20"/><c:rAngAx val="1"/></c:view3D>' if "3D" in plot else ""
    xml = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        f'<c:chartSpace xmlns:c="{NS_C}" xmlns:a="{NS_A}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
        f'<c:chart><c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>{title}</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title>'
        f'<c:autoTitleDeleted val="0"/>{view3d}<c:plotArea><c:layout/>{plot}</c:plotArea>'
        '<c:legend><c:legendPos val="r"/><c:overlay val="0"/></c:legend><c:plotVisOnly val="1"/></c:chart></c:chartSpace>'
    )
    part = Part(PackURI(f"/word/charts/chart{n + 1}.xml"),
                "application/vnd.openxmlformats-officedocument.drawingml.chart+xml", xml.encode(), d.part.package)
    rid = d.part.relate_to(part, "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart")
    p = d.add_paragraph()
    frame = f'<a:graphic><a:graphicData uri="{NS_C}"><c:chart r:id="{rid}"/></a:graphicData></a:graphic>'
    add_draw(p, drawing(anchor(310 + n, name, pos("H", "column", "<wp:align>center</wp:align>"),
                                pos("V", "paragraph", "<wp:posOffset>0</wp:posOffset>"),
                                300 * EMU, 180 * EMU, '<wp:wrapTopAndBottom/>', frame, 251661010 + n)))
    p.add_run(f"{name}.")
save(d, "diagrams.docx")
print("docx ok")
