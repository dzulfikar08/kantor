#!/usr/bin/env python3
"""Write the Slides frames and rotated text fixture (CC0, synthetic):
tests/corpus/pptx/frames.pptx. Read by crates/slides_engine/tests/frames.rs.

- Slide 1 "Merged table": a 4 x 4 table "Plan" whose top left cell "Area"
  spans two rows, "Budget" spans two columns, and a 2 x 2 block "Total"
  at the bottom right.
- Slide 2 "Chart": a clustered column chart "Sales" (with its workbook)
  and a rectangle "Note".
- Slide 3 "Rotated text": text boxes turned 30, 90, 180 and 315 degrees,
  a shape with vertical text (vert), one reading upwards (vert270) and an
  East Asian vertical one (eaVert); then a grid of 40 small turned text
  boxes (for timing).

Regenerate with
`uv run --with python-pptx==1.0.2 scripts/corpus/gen_pptx_frames.py tests/corpus/pptx`.
"""
import io
import re
import sys
import zipfile
from pathlib import Path

from pptx import Presentation
from pptx.chart.data import CategoryChartData
from pptx.dml.color import RGBColor
from pptx.enum.chart import XL_CHART_TYPE
from pptx.enum.shapes import MSO_SHAPE
from pptx.oxml.ns import qn
from pptx.util import Inches, Pt

out = Path(sys.argv[1] if len(sys.argv) > 1 else "tests/corpus/pptx")
prs = Presentation()
prs.slide_width, prs.slide_height = Inches(13.333), Inches(7.5)
TITLE_ONLY = prs.slide_layouts[5]

# Slide 1: a table with merged cells.
s = prs.slides.add_slide(TITLE_ONLY)
s.shapes.title.text = "Merged table"
gf = s.shapes.add_table(4, 4, Inches(1), Inches(1.8), Inches(10), Inches(3))
gf.name = "Plan"
t = gf.table
texts = [
    ["Area", "Budget", "", "Owner"],
    ["", "2025", "2026", "Lead"],
    ["North", "10", "12", "Ana"],
    ["South", "8", "Total", ""],
]
for r, row in enumerate(texts):
    for c, v in enumerate(row):
        t.cell(r, c).text = v
t.cell(0, 0).merge(t.cell(1, 0))
t.cell(0, 1).merge(t.cell(0, 2))
t.cell(2, 2).merge(t.cell(3, 3))
t.cell(2, 2).text = "Total"

# Slide 2: a chart beside a shape.
s = prs.slides.add_slide(TITLE_ONLY)
s.shapes.title.text = "Chart"
cd = CategoryChartData()
cd.categories = ["Q1", "Q2", "Q3"]
cd.add_series("North", (10, 12, 14))
cd.add_series("South", (8, 9, 11))
ch = s.shapes.add_chart(
    XL_CHART_TYPE.COLUMN_CLUSTERED, Inches(1), Inches(1.6), Inches(7), Inches(5), cd
)
ch.name = "Sales"
note = s.shapes.add_shape(MSO_SHAPE.RECTANGLE, Inches(9), Inches(2), Inches(3), Inches(1.5))
note.name = "Note"
note.text_frame.text = "Beside the chart"

# Slide 3: rotated and vertical text.
s = prs.slides.add_slide(TITLE_ONLY)
s.shapes.title.text = "Rotated text"
COLORS = ["4472C4", "ED7D31", "70AD47", "FFC000", "5B9BD5", "A5A5A5", "264478"]


def text_shape(name, x, y, w, h, text, rot=0, vert=None, color=None):
    sh = s.shapes.add_shape(MSO_SHAPE.RECTANGLE, Inches(x), Inches(y), Inches(w), Inches(h))
    sh.name = name
    sh.rotation = rot
    sh.text_frame.text = text
    for p in sh.text_frame.paragraphs:
        for r in p.runs:
            r.font.size = Pt(16)
    if color:
        sh.fill.solid()
        sh.fill.fore_color.rgb = RGBColor.from_string(color)
    if vert:
        sh.text_frame._txBody.find(qn("a:bodyPr")).set("vert", vert)
    return sh


text_shape("Turn 30", 0.5, 1.6, 2.6, 0.9, "Turned 30°", 30, color=COLORS[0])
text_shape("Turn 90", 3.4, 1.6, 2.6, 0.9, "Turned 90°", 90, color=COLORS[1])
text_shape("Turn 180", 6.3, 1.6, 2.6, 0.9, "Upside down", 180, color=COLORS[2])
text_shape("Turn 315", 9.2, 1.6, 2.6, 0.9, "Turned 315°", 315, color=COLORS[3])
text_shape("Vert", 0.6, 3.0, 0.9, 2.6, "Reads down", vert="vert", color=COLORS[4])
text_shape("Vert270", 1.8, 3.0, 0.9, 2.6, "Reads up", vert="vert270", color=COLORS[5])
text_shape("EaVert", 3.0, 3.0, 0.9, 2.6, "縦書き", vert="eaVert", color=COLORS[6])
for k in range(40):
    x = 4.3 + (k % 10) * 0.85
    y = 3.1 + (k // 10) * 1.0
    text_shape(f"Small {k + 1}", x, y, 0.8, 0.35, f"t{k + 1}", (k * 37) % 360,
               color=COLORS[k % len(COLORS)])

FIXED = (2026, 11, 15, 0, 0, 0)
buf = io.BytesIO()
prs.save(buf)
src = zipfile.ZipFile(io.BytesIO(buf.getvalue()))
parts = {n: src.read(n) for n in src.namelist()}
# python-pptx writes negative axis ids, which the Open XML SDK rejects
# (they must be unsigned): make them positive.
for n in parts:
    if n.startswith("ppt/charts/chart"):
        parts[n] = re.sub(rb'(<c:(?:axId|crossAx) val=")-', rb"\1", parts[n])
with zipfile.ZipFile(out / "frames.pptx", "w", zipfile.ZIP_DEFLATED) as z:
    for name in ["[Content_Types].xml"] + sorted(n for n in parts if n != "[Content_Types].xml"):
        info = zipfile.ZipInfo(name, date_time=FIXED)
        info.compress_type = zipfile.ZIP_DEFLATED
        z.writestr(info, parts[name])
print("frames ok")
