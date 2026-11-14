"""Sample files for the Kantor screenshots (site and README). Made-up data only: a small coffee
chain, "Kopi Senja", with outlets in eight Indonesian cities.

    uv run --with openpyxl --with python-docx --with python-pptx \
        site/tools/make_samples.py OUT_DIR

Writes
- sales.xlsx: quarterly sales per city (Rp million) with a colour scale, a highlight rule for
  cities below target, data bars, a bar chart beside the data and a best-sellers table;
- report.docx: a quarterly review with headings, a table, footnotes, tracked changes
  (insertions and deletions), a comment and one misspelt word; page 2 is long enough that one
  Page Up from its end shows the bottom of page 1 (see shots.sh);
- review.pptx: a four-slide deck; slide 3 has a column chart beside three key figures.
"""
import datetime
import sys
from pathlib import Path

out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)

CITIES = ["Jakarta", "Bandung", "Surabaya", "Yogyakarta", "Semarang", "Medan", "Makassar",
          "Denpasar"]
# Rp million per quarter, and the year's target.
SALES = [
    (412.5, 438.2, 451.9, 497.3, 1700),
    (186.3, 179.8, 201.4, 214.7, 760),
    (238.9, 251.2, 217.6, 268.1, 1050),
    (151.4, 163.0, 172.8, 189.5, 640),
    (118.2, 124.6, 131.3, 139.9, 540),
    (121.7, 118.4, 126.9, 131.2, 520),
    (94.2, 101.6, 108.3, 117.8, 400),
    (132.6, 149.1, 171.5, 158.9, 580),
]

# ---------- Sheets ----------
from openpyxl import Workbook
from openpyxl.chart import BarChart, Reference
from openpyxl.formatting.rule import CellIsRule, ColorScaleRule, DataBarRule
from openpyxl.styles import Alignment, Border, Font, PatternFill, Side

NAVY, BLUE_LT = "1F4E78", "DDEBF7"


def solid(c):
    return PatternFill("solid", fgColor=c)


wb = Workbook()
ws = wb.active
ws.title = "Sales 2026"
# Widths are multiples of 4 characters (36 px), so column edges fall on whole pixels at
# 125 % scaling and merged cells show no seam.
for col, w in zip("ABCDEFGHI", [12, 8, 8, 8, 8, 8, 8, 8, 4]):
    ws.column_dimensions[col].width = w
ws.row_dimensions[1].height = 30

ws.merge_cells("A1:H1")
ws["A1"] = "Kopi Senja · sales by city, 2026"
ws["A1"].font = Font(size=15, bold=True, color="FFFFFF")
ws["A1"].fill = solid(NAVY)
ws["A1"].alignment = Alignment(horizontal="center", vertical="center")
ws.merge_cells("A2:E2")
ws["A2"] = "Rp million, net of tax"
ws["A2"].font = Font(italic=True)
ws.merge_cells("F2:H2")
ws["F2"] = datetime.date(2026, 11, 14)
ws["F2"].number_format = "d mmm yyyy"
ws["F2"].font = Font(italic=True)
ws["F2"].alignment = Alignment(horizontal="right")

thin = Side(style="thin", color="BFBFBF")
medium = Side(style="medium", color=NAVY)
for i, h in enumerate(["City", "Q1", "Q2", "Q3", "Q4", "Total", "Target", "vs target"], 1):
    c = ws.cell(row=4, column=i, value=h)
    c.font = Font(bold=True, color=NAVY)
    c.fill = solid(BLUE_LT)
    c.border = Border(top=medium, bottom=medium)
    c.alignment = Alignment(horizontal="left" if i == 1 else "center", vertical="center")
ws.row_dimensions[4].height = 22

first = 5
for r, (name, vals) in enumerate(zip(CITIES, SALES), start=first):
    ws.cell(row=r, column=1, value=name)
    for c, v in enumerate(vals[:4], start=2):
        ws.cell(row=r, column=c, value=v).number_format = "#,##0.0"
    ws.cell(row=r, column=6, value=f"=SUM(B{r}:E{r})").number_format = "#,##0.0"
    ws.cell(row=r, column=6).font = Font(bold=True)
    ws.cell(row=r, column=7, value=vals[4]).number_format = "#,##0"
    ws.cell(row=r, column=8, value=f"=F{r}/G{r}-1").number_format = "+0.0%;-0.0%;0.0%"
    for c in range(1, 9):
        ws.cell(row=r, column=c).border = Border(bottom=thin)
last = first + len(CITIES) - 1
tot = last + 1
ws.cell(row=tot, column=1, value="Total")
for c, col in enumerate("BCDEFG", start=2):
    ws.cell(row=tot, column=c, value=f"=SUM({col}{first}:{col}{last})").number_format = "#,##0.0"
ws.cell(row=tot, column=7).number_format = "#,##0"
ws.cell(row=tot, column=8, value=f"=F{tot}/G{tot}-1").number_format = "+0.0%;-0.0%"
for c in range(1, 9):
    cell = ws.cell(row=tot, column=c)
    cell.font = Font(bold=True)
    cell.border = Border(top=Side(style="thin", color="000000"),
                         bottom=Side(style="double", color="000000"))

# Conditional formatting: a white-to-green scale on the quarters and Excel's "light red fill with
# dark red text" for cities below target. Text outside filled cells keeps the default colour, so
# it reads in the dark theme too.
ws.conditional_formatting.add(
    f"B{first}:E{last}",
    ColorScaleRule(start_type="min", start_color="FFFFFF", end_type="max", end_color="63BE7B"))
ws.conditional_formatting.add(
    f"H{first}:H{last}",
    CellIsRule(operator="lessThan", formula=["0"], fill=solid("FFC7CE"),
               font=Font(color="9C0006")))

# Best sellers with data bars.
b0 = tot + 2
ws.cell(row=b0, column=1, value="Best sellers, Q4").font = Font(bold=True)
for i, h in enumerate(["Menu item", "", "", "Cups", "Share"], 1):
    c = ws.cell(row=b0 + 1, column=i, value=h or None)
    c.font = Font(bold=True, color=NAVY)
    c.fill = solid(BLUE_LT)
    c.alignment = Alignment(horizontal="center" if i > 3 else "left")
items = [("Es kopi susu gula aren", 18420), ("Americano", 9310), ("Cappuccino", 7865),
         ("Kopi tubruk", 5240), ("Matcha latte", 4980), ("Croissant", 3655)]
for k, (name, cups) in enumerate(items):
    r = b0 + 2 + k
    ws.merge_cells(start_row=r, start_column=1, end_row=r, end_column=3)
    ws.cell(row=r, column=1, value=name)
    ws.cell(row=r, column=4, value=cups).number_format = "#,##0"
    ws.cell(row=r, column=5, value=f"=D{r}/SUM($D${b0 + 2}:$D${b0 + 1 + len(items)})")
    ws.cell(row=r, column=5).number_format = "0%"
ws.conditional_formatting.add(
    f"D{b0 + 2}:D{b0 + 1 + len(items)}",
    DataBarRule(start_type="num", start_value=0, end_type="max", color="5B9BD5"))

# A clustered bar chart beside the table: Q1 and Q4 per city, first city at the top.
ch = BarChart()
ch.type = "bar"
ch.grouping = "clustered"
ch.title = "Q1 and Q4 by city (Rp million)"
ch.add_data(Reference(ws, min_col=2, min_row=4, max_row=last), titles_from_data=True)
ch.add_data(Reference(ws, min_col=5, min_row=4, max_row=last), titles_from_data=True)
ch.set_categories(Reference(ws, min_col=1, min_row=first, max_row=last))
ch.legend.position = "b"
ch.series[0].graphicalProperties.solidFill = "9DC3E6"
ch.series[1].graphicalProperties.solidFill = "1F4E78"
ch.width, ch.height = 11.5, 7.9
ch.y_axis.number_format = "#,##0"
ch.x_axis.scaling.orientation = "maxMin"
# With the categories reversed, the value axis crosses at the first city (the
# top); "at maximum category" keeps it and its labels below the bars, as Excel
# users set it.
ch.y_axis.crosses = "max"
ch.gapWidth = 60
ws.add_chart(ch, "J4")

ws.freeze_panes = "A5"
wb.save(out / "sales.xlsx")


# ---------- Docs: a quarterly review with tracked changes, a comment and footnotes ----------
from docx import Document
from docx.enum.text import WD_ALIGN_PARAGRAPH
from docx.opc.packuri import PackURI
from docx.opc.part import Part
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
from docx.shared import Cm, Pt, RGBColor

d = Document()
sec = d.sections[0]
sec.page_width, sec.page_height = Cm(21), Cm(29.7)
sec.left_margin = sec.right_margin = Cm(2.5)
sec.top_margin, sec.bottom_margin = Cm(2), Cm(2)
normal = d.styles["Normal"]
normal.font.size = Pt(11)
normal.paragraph_format.space_after = Pt(6)

STAMP = "2026-10-03T09:30:00Z"
rev_id = [100]


def run_el(text, deleted=False, sup=False):
    r = OxmlElement("w:r")
    if sup:
        rpr = OxmlElement("w:rPr")
        va = OxmlElement("w:vertAlign")
        va.set(qn("w:val"), "superscript")
        rpr.append(va)
        r.append(rpr)
    t = OxmlElement("w:delText" if deleted else "w:t")
    t.set(qn("xml:space"), "preserve")
    t.text = text
    r.append(t)
    return r


def change(kind, text, author):
    el = OxmlElement(f"w:{kind}")
    rev_id[0] += 1
    el.set(qn("w:id"), str(rev_id[0]))
    el.set(qn("w:author"), author)
    el.set(qn("w:date"), STAMP)
    el.append(run_el(text, deleted=kind == "del"))
    return el


def footnote_ref(n):
    r = OxmlElement("w:r")
    rpr = OxmlElement("w:rPr")
    va = OxmlElement("w:vertAlign")
    va.set(qn("w:val"), "superscript")
    rpr.append(va)
    r.append(rpr)
    ref = OxmlElement("w:footnoteReference")
    ref.set(qn("w:id"), str(n))
    r.append(ref)
    return r


def para(parts, style=None):
    """parts: str (plain), ("ins"|"del", text, author), ("fn", id), ("run", text) -> Run."""
    p = d.add_paragraph(style=style)
    runs = {}
    for part in parts:
        if isinstance(part, str):
            p.add_run(part)
        elif part[0] in ("ins", "del"):
            p._p.append(change(*part))
        elif part[0] == "fn":
            p._p.append(footnote_ref(part[1]))
        elif part[0] == "run":
            runs[part[1]] = p.add_run(part[1])
    return p, runs


d.add_heading("Third quarter review", level=0)
sub = d.add_paragraph()
r = sub.add_run("Kopi Senja · Rina Wulandari, operations · 3 October 2026")
r.font.color.rgb = RGBColor(0x59, 0x59, 0x59)
para(["Sales grew in seven of our eight cities this quarter. Together the outlets sold "
      "Rp 1,581 million", ("fn", 1), ", 4 percent more than in the second quarter, and the "
      "iced palm-sugar coffee is still a third of every order."])

d.add_heading("Highlights", level=1)
for t in ["Bandung and Denpasar grew fastest, by 12 and 15 percent.",
          "Surabaya fell 13 percent while the Tunjungan outlet was being renovated.",
          "Cups sold per day rose from 2,310 to 2,450 across all outlets."]:
    d.add_paragraph(t, style="List Bullet")
para(["Weekday mornings are still our busiest hours: more than half of all cups are sold before "
      "eleven, most of them to take away. Delivery orders through the apps grew to 28 percent of "
      "sales, so two outlets now have a separate pick-up counter for riders."])

d.add_heading("Sales by city", level=1)
rows = [("City", "Q2 (Rp m)", "Q3 (Rp m)", "Change")]
for name, v in zip(CITIES, SALES):
    rows.append((name, f"{v[1]:,.1f}", f"{v[2]:,.1f}", f"{(v[2] / v[1] - 1) * 100:+.1f}%"))
q2, q3 = sum(v[1] for v in SALES), sum(v[2] for v in SALES)
rows.append(("Total", f"{q2:,.1f}", f"{q3:,.1f}", f"{(q3 / q2 - 1) * 100:+.1f}%"))
tbl = d.add_table(rows=len(rows), cols=4)
tbl.style = "Light Grid Accent 1"
for ri, row in enumerate(rows):
    for ci, v in enumerate(row):
        cell = tbl.cell(ri, ci)
        cell.text = v
        if ci:
            cell.paragraphs[0].alignment = WD_ALIGN_PARAGRAPH.RIGHT
        if ri in (0, len(rows) - 1):
            for run in cell.paragraphs[0].runs:
                run.font.bold = True

d.add_heading("Plans for 2027", level=1)
_, runs = para(["We plan to open ", ("del", "two", "Budi Santoso"), ("ins", "three", "Budi Santoso"),
                ("run", " new outlets next year"), ", starting with Semarang",
                ("ins", " and Solo", "Budi Santoso"), ". Each outlet needs about Rp 850 million "
                "for the fit-out and the first three months of rent", ("fn", 2), "."])
d.add_comment(runs[" new outlets next year"], text="Can we fund three from this year's profit, "
              "or do we need the bank loan?", author="Sari Dewi", initials="SD")
para(["The new roastery in Bekasi should ", ("run", "recieve"), " its first green beans from "
      "Toraja and Gayo in November", ("del", ", if the permits are ready", "Budi Santoso"),
      ". From then on we roast for all outlets ourselves."])
d.add_heading("Next steps", level=1).paragraph_format.page_break_before = True
for t in ["Confirm the Semarang lease before 15 October.",
          "Train two head baristas for each new outlet.",
          "Move the till reports to the new spreadsheet."]:
    d.add_paragraph(t, style="List Number")

d.add_heading("Appendix: cost of a new outlet", level=1)
costs = [("Item", "Rp million"), ("Fit-out and furniture", "420"), ("Espresso machine and grinders", "180"),
         ("Deposit and first three months of rent", "190"), ("Opening stock and training", "60"),
         ("Total", "850")]
ct = d.add_table(rows=len(costs), cols=2)
ct.style = "Light Grid Accent 1"
for ri, row in enumerate(costs):
    for ci, v in enumerate(row):
        ct.cell(ri, ci).text = v
        if ci:
            ct.cell(ri, ci).paragraphs[0].alignment = WD_ALIGN_PARAGRAPH.RIGHT
d.add_paragraph()
para(["Prices are the middle of three quotes from contractors in Semarang and Solo, received in "
      "September. Rent assumes a ground-floor unit of about 60 square metres on a main road, with "
      "a two-year lease paid quarterly. The espresso machine is the same model as in our other "
      "outlets, so parts and training can be shared."])

NOTES = {
    1: "Net of tax and returns, from the till reports of 30 September.",
    2: "Estimate from the Surabaya and Denpasar openings in 2025.",
}
W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
notes = "".join(
    f'<w:footnote w:id="{n}"><w:p><w:pPr><w:spacing w:after="0"/></w:pPr>'
    f'<w:r><w:rPr><w:vertAlign w:val="superscript"/><w:sz w:val="18"/></w:rPr><w:footnoteRef/></w:r>'
    f'<w:r><w:rPr><w:sz w:val="18"/></w:rPr><w:t xml:space="preserve"> {t}</w:t></w:r></w:p></w:footnote>'
    for n, t in NOTES.items())
blob = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<w:footnotes {W}>'
        '<w:footnote w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="0"/></w:pPr>'
        '<w:r><w:separator/></w:r></w:p></w:footnote>'
        '<w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:pPr><w:spacing w:after="0"/>'
        '</w:pPr><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>'
        f'{notes}</w:footnotes>').encode()
fn_part = Part(PackURI("/word/footnotes.xml"),
               "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml",
               blob, d.part.package)
d.part.relate_to(fn_part,
                 "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes")
d.save(out / "report.docx")

# ---------- Slides: a short review deck ----------
from pptx import Presentation
from pptx.chart.data import CategoryChartData
from pptx.dml.color import RGBColor as PRGB
from pptx.enum.chart import XL_CHART_TYPE, XL_LEGEND_POSITION
from pptx.enum.shapes import MSO_SHAPE
from pptx.enum.text import PP_ALIGN
from pptx.util import Inches, Pt as PPt

prs = Presentation()
prs.slide_width, prs.slide_height = Inches(13.333), Inches(7.5)
INK = PRGB(0x1F, 0x23, 0x2E)
GREY = PRGB(0x59, 0x5F, 0x6B)
WHITE = PRGB(0xFF, 0xFF, 0xFF)
C1, C2, C3, C4 = (PRGB(0x1F, 0x4E, 0x78), PRGB(0x2F, 0x80, 0xED),
                  PRGB(0x9D, 0xC3, 0xE6), PRGB(0xF2, 0x99, 0x4A))


def gradient(shape, a, b, angle=0):
    shape.fill.gradient()
    shape.fill.gradient_angle = angle
    st = shape.fill.gradient_stops
    st[0].color.rgb, st[1].color.rgb = a, b
    shape.line.fill.background()


def text(shape, s, size, color=INK, bold=False, align=None):
    tf = shape.text_frame
    tf.word_wrap = True
    tf.text = s
    p = tf.paragraphs[0]
    if align is not None:
        p.alignment = align
    r = p.runs[0]
    r.font.size, r.font.bold, r.font.color.rgb = PPt(size), bold, color
    return tf


def title(slide, s, sub=None):
    t = slide.shapes.add_textbox(Inches(0.7), Inches(0.45), Inches(11.5), Inches(0.9))
    text(t, s, 36, INK, True)
    if sub:
        p = t.text_frame.add_paragraph()
        p.text = sub
        p.runs[0].font.size, p.runs[0].font.color.rgb = PPt(18), GREY
    bar = slide.shapes.add_shape(MSO_SHAPE.RECTANGLE, Inches(0.72), Inches(1.75), Inches(0.9),
                                 Inches(0.07))
    bar.fill.solid(); bar.fill.fore_color.rgb = C4; bar.line.fill.background()


# 1: title on a gradient
s = prs.slides.add_slide(prs.slide_layouts[6])
bg = s.shapes.add_shape(MSO_SHAPE.RECTANGLE, 0, 0, prs.slide_width, prs.slide_height)
gradient(bg, C1, C2, 45)
t = s.shapes.add_textbox(Inches(0.9), Inches(2.5), Inches(11), Inches(1.4))
text(t, "Kopi Senja 2026", 60, WHITE, True)
p = t.text_frame.add_paragraph()
p.text = "Year in review and plans for 2027"
p.runs[0].font.size, p.runs[0].font.color.rgb = PPt(26), PRGB(0xDD, 0xEB, 0xF7)
arrow = s.shapes.add_shape(MSO_SHAPE.RIGHT_ARROW, Inches(0.9), Inches(4.7), Inches(2.4), Inches(0.7))
arrow.fill.solid(); arrow.fill.fore_color.rgb = C4; arrow.line.fill.background()

# 2: three steps
s = prs.slides.add_slide(prs.slide_layouts[6])
title(s, "Agenda")
for i, lbl in enumerate(["Sales by city", "Best sellers", "Three new outlets"]):
    sh = s.shapes.add_shape(MSO_SHAPE.CHEVRON, Inches(0.7 + i * 4.0), Inches(2.6), Inches(3.8),
                            Inches(1.3))
    gradient(sh, C1 if i == 0 else C2, C2 if i == 0 else C3, 0)
    text(sh, f"{i + 1}", 32, WHITE, True, PP_ALIGN.CENTER)
    tb = s.shapes.add_textbox(Inches(0.9 + i * 4.0), Inches(4.2), Inches(3.4), Inches(1))
    text(tb, lbl, 22)

# 3: key figures beside a chart
s = prs.slides.add_slide(prs.slide_layouts[6])
title(s, "Sales grew every quarter", "Sales per city, Rp million")
q1 = sum(v[0] for v in SALES)
q4 = sum(v[3] for v in SALES)
figs = [(f"{q4 + q1 + sum(v[1] + v[2] for v in SALES):,.0f}", "Rp million in 2026"),
        (f"+{(q4 / q1 - 1) * 100:.0f}%", "Q4 against Q1"),
        ("3", "new outlets in 2027")]
for i, (big, small) in enumerate(figs):
    y = Inches(2.25 + i * 1.6)
    card = s.shapes.add_shape(MSO_SHAPE.ROUNDED_RECTANGLE, Inches(0.7), y, Inches(3.3),
                              Inches(1.35))
    card.adjustments[0] = 0.12
    card.fill.solid(); card.fill.fore_color.rgb = PRGB(0xEE, 0xF4, 0xFB)
    card.line.fill.background()
    tf = text(card, big, 34, C1, True, PP_ALIGN.LEFT)
    pp = tf.add_paragraph()
    pp.text = small
    pp.runs[0].font.size, pp.runs[0].font.color.rgb = PPt(15), GREY
    tf.margin_left = Inches(0.3)
cd = CategoryChartData()
cd.categories = CITIES
for qi in range(4):
    cd.add_series(f"Q{qi + 1}", [v[qi] for v in SALES])
gf = s.shapes.add_chart(XL_CHART_TYPE.COLUMN_CLUSTERED, Inches(4.4), Inches(2.1),
                        Inches(8.4), Inches(4.9), cd)
ch = gf.chart
ch.has_legend = True
ch.legend.position = XL_LEGEND_POSITION.TOP
ch.legend.include_in_layout = False
ch.legend.font.size = PPt(14)
ch.category_axis.tick_labels.font.size = PPt(12)
ch.value_axis.tick_labels.font.size = PPt(12)
ch.value_axis.has_major_gridlines = True
ch.value_axis.major_gridlines.format.line.color.rgb = PRGB(0xD9, 0xD9, 0xD9)
ch.plots[0].gap_width = 60
for ser, col in zip(ch.series, [C3, PRGB(0x5B, 0x9B, 0xD5), C2, C1]):
    ser.format.fill.solid(); ser.format.fill.fore_color.rgb = col
s.notes_slide.notes_text_frame.text = "Start with Jakarta, then Yogyakarta, which grew fastest."

# 4: closing
s = prs.slides.add_slide(prs.slide_layouts[6])
bg = s.shapes.add_shape(MSO_SHAPE.RECTANGLE, 0, 0, prs.slide_width, prs.slide_height)
gradient(bg, C1, C2, 90)
t = s.shapes.add_textbox(Inches(0.9), Inches(3.0), Inches(11), Inches(1.4))
text(t, "Terima kasih", 54, WHITE, True)
prs.save(out / "review.pptx")
print("ok")
