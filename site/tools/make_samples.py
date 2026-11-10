"""Generic sample files for the Kantor download site screenshots."""
import sys
from pathlib import Path

out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)

# ---------- Sheets: a small team budget ----------
from openpyxl import Workbook
from openpyxl.styles import Font, PatternFill, Alignment, Border, Side

wb = Workbook()
ws = wb.active
ws.title = "Budget 2026"
ws["A1"] = "Studio budget, first half of 2026"
ws["A1"].font = Font(size=14, bold=True)
head = ["Item", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Total"]
ws.append([])
ws.append(head)
rows = [
    ("Rent", 1200, 1200, 1200, 1200, 1200, 1200),
    ("Electricity", 140, 132, 118, 121, 135, 150),
    ("Internet", 45, 45, 45, 45, 45, 45),
    ("Software", 210, 210, 230, 230, 230, 245),
    ("Printing", 60, 85, 40, 95, 70, 55),
    ("Travel", 300, 0, 420, 150, 0, 380),
    ("Coffee and snacks", 75, 68, 82, 77, 90, 71),
    ("Equipment", 0, 650, 0, 0, 220, 0),
]
for r in rows:
    ws.append(list(r))
first, last = 4, 3 + len(rows)
for i in range(first, last + 1):
    ws[f"H{i}"] = f"=SUM(B{i}:G{i})"
tot = last + 1
ws[f"A{tot}"] = "Total"
for col in "BCDEFGH":
    ws[f"{col}{tot}"] = f"=SUM({col}{first}:{col}{last})"
ws[f"A{tot+2}"] = "Average per month"
ws[f"B{tot+2}"] = f"=H{tot}/6"
ws[f"A{tot+3}"] = "Largest item"
ws[f"B{tot+3}"] = f"=INDEX(A{first}:A{last},MATCH(MAX(H{first}:H{last}),H{first}:H{last},0))"

hdr_fill = PatternFill("solid", fgColor="2FBF71")
for c in ws[3]:
    c.font = Font(bold=True)
    c.alignment = Alignment(horizontal="center")
thin = Side(style="thin", color="2FBF71")
for c in ws[tot]:
    c.font = Font(bold=True)
    c.border = Border(top=thin)
for row in ws.iter_rows(min_row=first, max_row=tot, min_col=2, max_col=8):
    for c in row:
        c.number_format = "#,##0"
ws[f"B{tot+2}"].number_format = "#,##0.00"
ws.column_dimensions["A"].width = 22
for col in "BCDEFGH":
    ws.column_dimensions[col].width = 11
wb.save(out / "budget.xlsx")

# ---------- Docs: a short letter ----------
from docx import Document
from docx.shared import Pt, RGBColor, Cm
from docx.enum.text import WD_ALIGN_PARAGRAPH

d = Document()
sec = d.sections[0]
sec.page_width, sec.page_height = Cm(21), Cm(29.7)
sec.left_margin = sec.right_margin = Cm(2.5)
p = d.add_paragraph("North Street Studio")
p.runs[0].bold = True
p.runs[0].font.size = Pt(16)
p.runs[0].font.color.rgb = RGBColor(0x4F, 0x8F, 0xF7)
d.add_paragraph("12 North Street, Springfield\n10 November 2026")
d.add_heading("Plan for the spring workshop", level=1)
d.add_paragraph("Dear team,")
d.add_paragraph(
    "Thank you for the ideas you sent last week. We will hold the spring workshop over two days in "
    "the main room. Below is the plan we agreed on, with the budget kept in the shared spreadsheet."
)
d.add_heading("What we will cover", level=2)
for t in [
    "Day one: reviewing last year's projects and what we learned from them.",
    "Day two: planning the next six months, one project at a time.",
    "Both days end at four, so everyone can get home before the evening traffic.",
]:
    d.add_paragraph(t, style="List Bullet")
d.add_heading("Who does what", level=2)
tbl = d.add_table(rows=4, cols=3)
tbl.style = "Light Grid Accent 1"
data = [("Task", "Owner", "Due"), ("Book the room", "Office", "1 March"),
        ("Print the handouts", "Design", "8 March"), ("Order lunch", "Office", "10 March")]
for r, row in enumerate(data):
    for c, v in enumerate(row):
        tbl.cell(r, c).text = v
d.add_paragraph()
d.add_paragraph(
    "If something in this plan does not work for you, reply to this letter before the end of the month "
    "and we will adjust it together."
)
d.add_paragraph("Kind regards,")
d.add_paragraph("The office team")
d.save(out / "letter.docx")

# ---------- Slides: a three-slide deck ----------
from pptx import Presentation
from pptx.util import Inches, Pt as PPt
from pptx.dml.color import RGBColor as PRGB
from pptx.enum.shapes import MSO_SHAPE

prs = Presentation()
prs.slide_width, prs.slide_height = Inches(13.333), Inches(7.5)
ORANGE = PRGB(0xF4, 0x7A, 0x3A)
INK = PRGB(0x1F, 0x23, 0x2E)

s = prs.slides.add_slide(prs.slide_layouts[6])
bg = s.shapes.add_shape(MSO_SHAPE.RECTANGLE, 0, 0, prs.slide_width, prs.slide_height)
bg.fill.solid(); bg.fill.fore_color.rgb = PRGB(0xFF, 0xF4, 0xEC); bg.line.fill.background()
bar = s.shapes.add_shape(MSO_SHAPE.RECTANGLE, Inches(0.8), Inches(2.6), Inches(0.25), Inches(2.2))
bar.fill.solid(); bar.fill.fore_color.rgb = ORANGE; bar.line.fill.background()
tb = s.shapes.add_textbox(Inches(1.3), Inches(2.4), Inches(10), Inches(1.5))
tb.text_frame.text = "Spring workshop"
r = tb.text_frame.paragraphs[0].runs[0]; r.font.size = PPt(60); r.font.bold = True; r.font.color.rgb = INK
p = tb.text_frame.add_paragraph(); p.text = "Plan, budget and schedule for March"
p.runs[0].font.size = PPt(28); p.runs[0].font.color.rgb = PRGB(0x6B, 0x55, 0x48)

s = prs.slides.add_slide(prs.slide_layouts[1])
s.shapes.title.text = "Two days, two goals"
body = s.placeholders[1].text_frame
body.text = "Day one: review last year's projects"
for t in ["What went well, and what we would change", "Day two: plan the next six months",
          "One project at a time, with an owner and a date"]:
    para = body.add_paragraph(); para.text = t
    para.level = 1 if t.startswith(("What", "One")) else 0
s.placeholders[1].left, s.placeholders[1].top = Inches(0.8), Inches(1.9)
s.placeholders[1].width, s.placeholders[1].height = Inches(7.2), Inches(4.8)
s.shapes.title.left, s.shapes.title.width = Inches(0.8), Inches(11.5)
s.shapes.title.top, s.shapes.title.height = Inches(0.5), Inches(1.2)
for i, (lbl, col) in enumerate([("Review", ORANGE), ("Plan", PRGB(0x4F, 0x8F, 0xF7))]):
    sh = s.shapes.add_shape(MSO_SHAPE.ROUNDED_RECTANGLE, Inches(8.6), Inches(2.0 + i * 2.3), Inches(3.9), Inches(1.9))
    sh.fill.solid(); sh.fill.fore_color.rgb = col; sh.line.fill.background()
    sh.text_frame.text = lbl
    sh.text_frame.paragraphs[0].alignment = 2
    sh.text_frame.paragraphs[0].runs[0].font.size = PPt(30)
    sh.text_frame.paragraphs[0].runs[0].font.bold = True
    sh.text_frame.paragraphs[0].runs[0].font.color.rgb = PRGB(0xFF, 0xFF, 0xFF)

s = prs.slides.add_slide(prs.slide_layouts[5])
s.shapes.title.text = "Schedule"
s.shapes.title.left, s.shapes.title.width = Inches(0.8), Inches(11.5)
s.shapes.title.top, s.shapes.title.height = Inches(0.5), Inches(1.2)
steps = [("1 March", "Book the room"), ("8 March", "Print the handouts"), ("10 March", "Order lunch"), ("15 March", "Workshop")]
for i, (date, what) in enumerate(steps):
    x = Inches(0.8 + i * 3.05)
    box = s.shapes.add_shape(MSO_SHAPE.CHEVRON if i < 3 else MSO_SHAPE.PENTAGON, x, Inches(3.0), Inches(2.9), Inches(1.3))
    box.fill.solid(); box.fill.fore_color.rgb = ORANGE if i == 3 else PRGB(0xFB, 0xC6, 0xA4)
    box.line.fill.background()
    box.text_frame.text = date
    run = box.text_frame.paragraphs[0].runs[0]
    run.font.size = PPt(22); run.font.bold = True; run.font.color.rgb = INK
    t = s.shapes.add_textbox(x, Inches(4.5), Inches(2.9), Inches(1))
    t.text_frame.text = what
    t.text_frame.paragraphs[0].runs[0].font.size = PPt(20)
    t.text_frame.paragraphs[0].runs[0].font.color.rgb = INK
prs.save(out / "workshop.pptx")
print("ok")
