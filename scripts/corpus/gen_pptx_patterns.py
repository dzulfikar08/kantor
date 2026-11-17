#!/usr/bin/env python3
"""Write the Slides pattern fill and grouped chart fixture (CC0, synthetic):
tests/corpus/pptx/patterns.pptx. Read by crates/slides_scene/tests/patterns.rs
and crates/slides_pdf/tests/export.rs.

Slide 1 "Patterns":
- "Pattern box": a rectangle filled with `dkUpDiag` (accent 1 on white).
- "Pattern oval": an ellipse filled with `smCheck` (dark red on yellow).
- "Chart group": a group of the rectangle "Caption" and the clustered
  column chart "Grouped chart" (Sales by region, its series filled with
  `ltUpDiag`).

Regenerate with
`uv run --with python-pptx==1.0.2 scripts/corpus/gen_pptx_patterns.py tests/corpus/pptx`.
"""
import datetime
import io
import sys
import zipfile
from pathlib import Path

from pptx import Presentation
from pptx.chart.data import CategoryChartData
from pptx.dml.color import RGBColor
from pptx.enum.chart import XL_CHART_TYPE
from pptx.enum.dml import MSO_PATTERN
from pptx.enum.shapes import MSO_SHAPE
from pptx.util import Inches

out = Path(sys.argv[1] if len(sys.argv) > 1 else "tests/corpus/pptx")
prs = Presentation()
prs.slide_width, prs.slide_height = Inches(13.333), Inches(7.5)
slide = prs.slides.add_slide(prs.slide_layouts[5])
slide.shapes.title.text = "Patterns"


def patterned(kind, name, x, y, w, h, pattern, fg, bg):
    s = slide.shapes.add_shape(kind, Inches(x), Inches(y), Inches(w), Inches(h))
    s.name = name
    s.fill.patterned()
    s.fill.pattern = pattern
    s.fill.fore_color.rgb = RGBColor.from_string(fg)
    s.fill.back_color.rgb = RGBColor.from_string(bg)
    s.line.color.rgb = RGBColor(0, 0, 0)
    return s


patterned(MSO_SHAPE.RECTANGLE, "Pattern box", 0.5, 1.5, 3, 2, MSO_PATTERN.DARK_UPWARD_DIAGONAL, "4472C4", "FFFFFF")
patterned(MSO_SHAPE.OVAL, "Pattern oval", 4, 1.5, 3, 2, MSO_PATTERN.SMALL_CHECKER_BOARD, "C00000", "FFFF00")

group = slide.shapes.add_group_shape()
group.name = "Chart group"
cap = group.shapes.add_shape(MSO_SHAPE.RECTANGLE, Inches(7.5), Inches(1.5), Inches(5), Inches(0.5))
cap.name = "Caption"
cap.text = "Sales by region"
cap.fill.solid()
cap.fill.fore_color.rgb = RGBColor.from_string("DEEBF7")
cap.text_frame.paragraphs[0].runs[0].font.color.rgb = RGBColor(0, 0, 0)
data = CategoryChartData()
data.categories = ["North", "South", "East", "West"]
data.add_series("Sales", (120, 95, 143, 88))
frame = group.shapes.add_chart(
    XL_CHART_TYPE.COLUMN_CLUSTERED, Inches(7.5), Inches(2), Inches(5), Inches(3.5), data
)
frame.name = "Grouped chart"
plot = frame.chart.plots[0]
ser = plot.series[0]
ser.format.fill.patterned()
ser.format.fill.pattern = MSO_PATTERN.LIGHT_UPWARD_DIAGONAL
ser.format.fill.fore_color.rgb = RGBColor.from_string("548235")
ser.format.fill.back_color.rgb = RGBColor.from_string("FFFFFF")

buf = io.BytesIO()
prs.save(buf)

# Fixed timestamps and core properties: the same bytes on every run.
FIXED = (2026, 11, 17, 0, 0, 0)
src = zipfile.ZipFile(io.BytesIO(buf.getvalue()))
final = io.BytesIO()
with zipfile.ZipFile(final, "w", zipfile.ZIP_DEFLATED) as z:
    for info in src.infolist():
        data = src.read(info.filename)
        if info.filename == "docProps/core.xml":
            stamp = datetime.datetime(*FIXED).strftime("%Y-%m-%dT%H:%M:%SZ")
            text = data.decode()
            import re

            text = re.sub(r"(<dcterms:(created|modified)[^>]*>)[^<]*", rf"\g<1>{stamp}", text)
            data = text.encode()
        zi = zipfile.ZipInfo(info.filename, FIXED)
        zi.compress_type = zipfile.ZIP_DEFLATED
        z.writestr(zi, data)
(out / "patterns.pptx").write_bytes(final.getvalue())
print(out / "patterns.pptx")
