"""Generic sample files for the Kantor screenshots (site and README). Made-up data only.

    uv run --with openpyxl --with python-docx --with python-pptx --with pillow \
        site/tools/make_samples.py OUT_DIR

Writes sales.xlsx (formatted workbook: merged title, fills, borders, Rupiah and percent formats),
report.docx (headings, lists, a table and a picture) and review.pptx (gradient title slide,
chevrons and arrows, a table and a column chart).
"""
import datetime
import sys
from pathlib import Path

out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)

# ---------- Sheets: quarterly sales in rupiah ----------
from openpyxl import Workbook
from openpyxl.styles import Alignment, Border, Font, PatternFill, Side

NAVY, BLUE_LT, BAND, GREEN = "1F4E78", "DDEBF7", "F3F7FB", "2E7D32"


def solid(c):
    return PatternFill("solid", fgColor=c)


wb = Workbook()
ws = wb.active
ws.title = "Sales 2026"
ws.sheet_view.showGridLines = True
for col, w in zip("ABCDEFGH", [12, 16, 16, 16, 16, 20, 20, 12]):
    ws.column_dimensions[col].width = w
ws.row_dimensions[1].height = 30

ws.merge_cells("A1:H1")
ws["A1"] = "Quarterly sales by region, 2026"
ws["A1"].font = Font(name="Calibri", size=16, bold=True, color="FFFFFF")
ws["A1"].fill = solid(NAVY)
ws["A1"].alignment = Alignment(horizontal="center", vertical="center")
ws.merge_cells("A2:F2")
ws["A2"] = "Amounts in rupiah. Draft for the quarterly review."
ws["A2"].font = Font(italic=True, color="595959")
ws["G2"] = datetime.date(2026, 11, 10)
ws["G2"].number_format = "[$-421]d mmmm yyyy"
ws["G2"].font = Font(italic=True, color="595959")
ws["G2"].alignment = Alignment(horizontal="right")
# Widths are multiples of 4 characters (36 px), so column edges fall on whole pixels at
# 125 % scaling and merged cells show no seam. A2:F2 and G2 get a fill; H2 stays empty.
for c in range(1, 8):
    ws.cell(row=2, column=c).fill = solid("F2F2F2")

thin = Side(style="thin", color="A6A6A6")
medium = Side(style="medium", color=NAVY)
head = ["Region", "Q1", "Q2", "Q3", "Q4", "Total", "Target", "vs target"]
for i, h in enumerate(head, start=1):
    c = ws.cell(row=4, column=i, value=h)
    c.font = Font(bold=True, color=NAVY)
    c.fill = solid(BLUE_LT)
    c.border = Border(top=medium, bottom=medium, left=thin, right=thin)
    c.alignment = Alignment(horizontal="center", vertical="center")
ws.row_dimensions[4].height = 22

RP = '[$Rp-421]#,##0'
rows = [
    ("Jakarta", 412_500_000, 438_200_000, 451_900_000, 497_300_000, 1_700_000_000),
    ("Bandung", 186_300_000, 179_800_000, 201_400_000, 214_700_000, 760_000_000),
    ("Surabaya", 238_900_000, 251_200_000, 247_600_000, 268_100_000, 1_050_000_000),
    ("Medan", 121_700_000, 118_400_000, 126_900_000, 131_200_000, 520_000_000),
    ("Makassar", 94_200_000, 101_600_000, 108_300_000, 117_800_000, 400_000_000),
    ("Denpasar", 132_600_000, 149_100_000, 171_500_000, 158_900_000, 580_000_000),
]
first = 5
for r, (name, *vals) in enumerate(rows, start=first):
    ws.cell(row=r, column=1, value=name)
    for c, v in enumerate(vals[:4], start=2):
        ws.cell(row=r, column=c, value=v)
    ws.cell(row=r, column=6, value=f"=SUM(B{r}:E{r})").font = Font(bold=True)
    ws.cell(row=r, column=7, value=vals[4])
    ws.cell(row=r, column=8, value=f"=F{r}/G{r}-1")
    for c in range(1, 9):
        cell = ws.cell(row=r, column=c)
        cell.border = Border(left=thin, right=thin, top=thin, bottom=thin)
        if 2 <= c <= 7:
            cell.number_format = RP
        cell.fill = solid(BAND if (r - first) % 2 else "FFFFFF")
    ws.cell(row=r, column=8).number_format = '[Color10]+0.0%;[Red]-0.0%;0.0%'
last = first + len(rows) - 1
tot = last + 1
ws.cell(row=tot, column=1, value="Total")
for c, col in enumerate("BCDEFG", start=2):
    ws.cell(row=tot, column=c, value=f"=SUM({col}{first}:{col}{last})").number_format = RP
ws.cell(row=tot, column=8, value=f"=F{tot}/G{tot}-1").number_format = '+0.0%;[Red]-0.0%'
double = Side(style="double", color="000000")
for c in range(1, 9):
    cell = ws.cell(row=tot, column=c)
    cell.font = Font(bold=True)
    cell.fill = solid("FFFFFF")
    cell.border = Border(top=Side(style="thin", color="000000"), bottom=double)

ws.cell(row=tot + 2, column=1, value="Share of total").font = Font(bold=True, color=NAVY)
ws.cell(row=tot + 2, column=1).fill = solid(BLUE_LT)
for c, col in enumerate("BCDE", start=2):
    cell = ws.cell(row=tot + 2, column=c, value=f"={col}{tot}/$F${tot}")
    cell.fill = solid(BLUE_LT)
    cell.font = Font(color=NAVY)
    cell.number_format = "0.0%"
    cell.alignment = Alignment(horizontal="center")
ws.merge_cells(start_row=tot + 2, start_column=6, end_row=tot + 3, end_column=8)
note = ws.cell(row=tot + 2, column=6, value="Q4 is the best quarter in every region except Denpasar.")
note.alignment = Alignment(wrap_text=True, vertical="center", horizontal="center")
note.fill = solid("FFF2CC")
note.font = Font(color="7F6000")
box = Side(style="medium", color="BF8F00")
for r in (tot + 2, tot + 3):
    for c in range(6, 9):
        ws.cell(row=r, column=c).border = Border(
            top=box if r == tot + 2 else None, bottom=box if r == tot + 3 else None,
            left=box if c == 6 else None, right=box if c == 8 else None)
wb.save(out / "sales.xlsx")

# ---------- A picture for the document ----------
from PIL import Image, ImageDraw, ImageFont

W, H = 1400, 520
img = Image.new("RGB", (W, H), "#F4F8FC")
dr = ImageDraw.Draw(img)
try:
    font = ImageFont.load_default(size=30)
    small = ImageFont.load_default(size=24)
except TypeError:
    font = small = ImageFont.load_default()
dr.text((40, 26), "Visitors per day, March", fill="#1F4E78", font=font)
vals = [42, 55, 51, 63, 70, 66, 81, 77, 92, 88, 97, 104, 99, 112]
bw, gap, base, top = 70, 22, H - 60, 100
mx = max(vals)
for i, v in enumerate(vals):
    x = 60 + i * (bw + gap)
    h = (base - top) * v / mx
    dr.rounded_rectangle([x, base - h, x + bw, base], radius=10,
                         fill="#2F80ED" if i < len(vals) - 1 else "#F2994A")
    dr.text((x + bw / 2, base + 12), str(i + 1), fill="#4F5B66", font=small, anchor="mt")
dr.line([40, base, W - 40, base], fill="#9AA7B4", width=3)
img.save(out / "visitors.png")

# ---------- Docs: a short report ----------
from docx import Document
from docx.enum.text import WD_ALIGN_PARAGRAPH
from docx.shared import Cm, Pt, RGBColor

d = Document()
sec = d.sections[0]
sec.page_width, sec.page_height = Cm(21), Cm(29.7)
sec.left_margin = sec.right_margin = Cm(2.5)
sec.top_margin = Cm(2)
normal = d.styles["Normal"]
normal.font.size = Pt(11)
normal.paragraph_format.space_after = Pt(4)
title = d.add_heading("Open day at the studio", level=0)
d.add_paragraph(
    "Thank you to everyone who came on Saturday. About 240 people visited, twice as many as last "
    "year, and most of them stayed for the afternoon workshop."
)
d.add_heading("What went well", level=1)
for t in [
    "The guided tours every hour, which were full by noon.",
    "Printing name cards at the door, so nobody waited in line.",
    "The short talk on how we plan a project, from sketch to delivery.",
]:
    d.add_paragraph(t, style="List Bullet")
pic = d.add_paragraph()
pic.alignment = WD_ALIGN_PARAGRAPH.CENTER
pic.add_run().add_picture(str(out / "visitors.png"), width=Cm(10))
d.add_heading("Next steps", level=1)
tbl = d.add_table(rows=4, cols=3)
tbl.style = "Light Grid Accent 1"
data = [("Task", "Owner", "Due"), ("Send thank-you notes", "Office", "14 March"),
        ("Share the photos", "Design", "18 March"), ("Book next year's date", "Office", "31 March")]
for r, row in enumerate(data):
    for c, v in enumerate(row):
        tbl.cell(r, c).text = v
d.add_paragraph()
for t in ["Collect feedback from the visitor form.", "Plan two extra tours for next year."]:
    d.add_paragraph(t, style="List Number")
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
WHITE = PRGB(0xFF, 0xFF, 0xFF)
C1, C2, C3, C4 = (PRGB(0x1F, 0x4E, 0x78), PRGB(0x2F, 0x80, 0xED),
                  PRGB(0x27, 0xAE, 0x60), PRGB(0xF2, 0x99, 0x4A))


def gradient(shape, a, b, angle=0):
    shape.fill.gradient()
    shape.fill.gradient_angle = angle
    st = shape.fill.gradient_stops
    st[0].color.rgb, st[1].color.rgb = a, b
    shape.line.fill.background()


def text(shape, s, size, color=INK, bold=False, align=None):
    tf = shape.text_frame
    tf.text = s
    p = tf.paragraphs[0]
    if align is not None:
        p.alignment = align
    r = p.runs[0]
    r.font.size, r.font.bold, r.font.color.rgb = PPt(size), bold, color


def heading(slide, s):
    band = slide.shapes.add_shape(MSO_SHAPE.RECTANGLE, 0, 0, prs.slide_width, Inches(1.25))
    gradient(band, C1, C2)
    t = slide.shapes.add_textbox(Inches(0.7), Inches(0.28), Inches(11), Inches(0.8))
    text(t, s, 34, WHITE, True)


# 1: title on a gradient
s = prs.slides.add_slide(prs.slide_layouts[6])
bg = s.shapes.add_shape(MSO_SHAPE.RECTANGLE, 0, 0, prs.slide_width, prs.slide_height)
gradient(bg, C1, C2, 45)
t = s.shapes.add_textbox(Inches(0.9), Inches(2.5), Inches(11), Inches(1.4))
text(t, "Sales review 2026", 60, WHITE, True)
p = t.text_frame.add_paragraph()
p.text = "Four quarters, six regions, one plan for next year"
p.runs[0].font.size, p.runs[0].font.color.rgb = PPt(26), PRGB(0xDD, 0xEB, 0xF7)
arrow = s.shapes.add_shape(MSO_SHAPE.RIGHT_ARROW, Inches(0.9), Inches(4.7), Inches(2.4), Inches(0.7))
arrow.fill.solid(); arrow.fill.fore_color.rgb = C4; arrow.line.fill.background()

# 2: process with chevrons and arrows
s = prs.slides.add_slide(prs.slide_layouts[6])
heading(s, "How we plan a quarter")
steps = [("Collect", C1), ("Review", C2), ("Decide", C3), ("Deliver", C4)]
for i, (lbl, col) in enumerate(steps):
    x = Inches(0.7 + i * 3.05)
    sh = s.shapes.add_shape(MSO_SHAPE.PENTAGON if i == 0 else MSO_SHAPE.CHEVRON,
                            x, Inches(2.4), Inches(3.1), Inches(1.4))
    sh.fill.solid(); sh.fill.fore_color.rgb = col; sh.line.fill.background()
    text(sh, lbl, 26, WHITE, True, PP_ALIGN.CENTER)
for i, lbl in enumerate(["Numbers from every region", "What changed and why",
                         "Targets and budget", "Weekly check-ins"]):
    tb = s.shapes.add_textbox(Inches(0.9 + i * 3.05), Inches(4.1), Inches(2.6), Inches(1))
    text(tb, lbl, 18)
    tb.text_frame.word_wrap = True
up = s.shapes.add_shape(MSO_SHAPE.CIRCULAR_ARROW, Inches(5.6), Inches(5.0), Inches(2.0), Inches(2.0))
up.fill.solid(); up.fill.fore_color.rgb = PRGB(0xBD, 0xD7, 0xEE); up.line.fill.background()

# 3: table and chart
s = prs.slides.add_slide(prs.slide_layouts[6])
heading(s, "Results by region")
regions = ["Jakarta", "Bandung", "Surabaya", "Medan", "Makassar"]
q = [[412, 186, 239, 122, 94], [438, 180, 251, 118, 102],
     [452, 201, 248, 127, 108], [497, 215, 268, 131, 118]]
gt = s.shapes.add_table(len(regions) + 1, 3, Inches(0.6), Inches(1.8), Inches(5.0), Inches(3.6))
tbl = gt.table
tbl.columns[0].width, tbl.columns[1].width, tbl.columns[2].width = Inches(1.8), Inches(1.6), Inches(1.6)
for c, h in enumerate(["Region", "Q1 (Rp jt)", "Q4 (Rp jt)"]):
    tbl.cell(0, c).text = h
for r, name in enumerate(regions, start=1):
    tbl.cell(r, 0).text = name
    tbl.cell(r, 1).text = f"{q[0][r - 1]}"
    tbl.cell(r, 2).text = f"{q[3][r - 1]}"
for r in range(len(regions) + 1):
    for c in range(3):
        para = tbl.cell(r, c).text_frame.paragraphs[0]
        para.runs[0].font.size = PPt(18)
        if c:
            para.alignment = PP_ALIGN.RIGHT
cd = CategoryChartData()
cd.categories = regions
for i, row in enumerate(q):
    cd.add_series(f"Q{i + 1}", row)
gf = s.shapes.add_chart(XL_CHART_TYPE.COLUMN_CLUSTERED, Inches(6.0), Inches(1.6),
                        Inches(6.8), Inches(5.4), cd)
ch = gf.chart
ch.has_legend = True
ch.legend.position = XL_LEGEND_POSITION.BOTTOM
ch.legend.include_in_layout = False
ch.has_title = True
ch.chart_title.text_frame.text = "Sales per quarter (Rp million)"
for ser, col in zip(ch.series, [C1, C2, C3, C4]):
    ser.format.fill.solid(); ser.format.fill.fore_color.rgb = col
note = s.shapes.add_shape(MSO_SHAPE.ROUNDED_RECTANGLE, Inches(0.6), Inches(5.8), Inches(5.0), Inches(1.1))
note.fill.solid(); note.fill.fore_color.rgb = PRGB(0xFF, 0xF2, 0xCC); note.line.color.rgb = PRGB(0xBF, 0x8F, 0x00)
text(note, "Every region grew; Makassar grew fastest (+25%).", 16, PRGB(0x7F, 0x60, 0x00), False, PP_ALIGN.CENTER)
s.notes_slide.notes_text_frame.text = "Start with Jakarta, then the two fastest-growing regions."

# 4: closing
s = prs.slides.add_slide(prs.slide_layouts[6])
bg = s.shapes.add_shape(MSO_SHAPE.RECTANGLE, 0, 0, prs.slide_width, prs.slide_height)
gradient(bg, C3, C2, 90)
t = s.shapes.add_textbox(Inches(0.9), Inches(3.0), Inches(11), Inches(1.4))
text(t, "Questions?", 54, WHITE, True)
prs.save(out / "review.pptx")
print("ok")
