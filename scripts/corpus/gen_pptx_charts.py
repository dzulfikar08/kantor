#!/usr/bin/env python3
"""Write the Slides chart types fixture (CC0, synthetic):
tests/corpus/pptx/charts.pptx. Read by crates/slides_scene/tests/chart_types.rs
and crates/slides_pdf tests.

One chart per slide, each with its embedded workbook:
- Slide 1 "Radar": a radar chart with markers, two series ("Skills").
- Slide 2 "Filled radar": a filled radar chart ("Coverage").
- Slide 3 "Bubble": a bubble chart, two series ("Markets").
- Slide 4 "Stock": a high-low-close stock chart with high-low lines
  ("Prices"; python-pptx has no stock charts, so a line chart's plot is
  rewritten as `c:stockChart` the way PowerPoint writes one).
- Slide 5 "Combo": clustered columns with a line on a secondary axis
  ("Revenue"; a column chart with a `c:lineChart` and second axes added).
- Slide 6 "3-D pie": a 3-D pie ("Share").

Regenerate with
`uv run --with python-pptx==1.0.2 scripts/corpus/gen_pptx_charts.py tests/corpus/pptx`.
"""
import io
import re
import sys
import zipfile
from pathlib import Path

from pptx import Presentation
from pptx.chart.data import BubbleChartData, CategoryChartData
from pptx.enum.chart import XL_CHART_TYPE, XL_LEGEND_POSITION
from pptx.util import Inches

out = Path(sys.argv[1] if len(sys.argv) > 1 else "tests/corpus/pptx")
prs = Presentation()
prs.slide_width, prs.slide_height = Inches(13.333), Inches(7.5)
TITLE_ONLY = prs.slide_layouts[5]
BOX = (Inches(1), Inches(1.5), Inches(11), Inches(5.6))


def slide(title):
    s = prs.slides.add_slide(TITLE_ONLY)
    s.shapes.title.text = title
    return s


def chart(s, kind, data, name):
    ch = s.shapes.add_chart(kind, *BOX, data)
    ch.name = name
    ch.chart.has_legend = True
    ch.chart.legend.position = XL_LEGEND_POSITION.BOTTOM
    ch.chart.legend.include_in_layout = False
    return ch


# Slide 1: radar with markers.
s = slide("Radar")
cd = CategoryChartData()
cd.categories = ["Speed", "Power", "Range", "Comfort", "Price"]
cd.add_series("Model A", (4, 3, 5, 2, 4))
cd.add_series("Model B", (2, 5, 3, 4, 3))
chart(s, XL_CHART_TYPE.RADAR_MARKERS, cd, "Skills")

# Slide 2: filled radar.
s = slide("Filled radar")
cd = CategoryChartData()
cd.categories = ["North", "East", "South", "West", "Centre", "Coast"]
cd.add_series("2025", (60, 40, 70, 50, 80, 30))
chart(s, XL_CHART_TYPE.RADAR_FILLED, cd, "Coverage")

# Slide 3: bubbles.
s = slide("Bubble")
bd = BubbleChartData()
a = bd.add_series("Retail")
for x, y, z in [(1, 4, 10), (2, 6, 40), (3, 3, 20)]:
    a.add_data_point(x, y, z)
b = bd.add_series("Online")
for x, y, z in [(1.5, 2, 30), (2.5, 5, 15), (3.5, 7, 60)]:
    b.add_data_point(x, y, z)
chart(s, XL_CHART_TYPE.BUBBLE, bd, "Markets")

# Slide 4: high-low-close (written as a line chart, rewritten below).
s = slide("Stock")
cd = CategoryChartData()
cd.categories = ["Mon", "Tue", "Wed", "Thu", "Fri"]
cd.add_series("High", (55, 57, 60, 58, 62))
cd.add_series("Low", (50, 51, 54, 52, 55))
cd.add_series("Close", (53, 56, 55, 57, 61))
chart(s, XL_CHART_TYPE.LINE_MARKERS, cd, "Prices")

# Slide 5: columns plus a line on a secondary axis (line added below).
s = slide("Combo")
cd = CategoryChartData()
cd.categories = ["Q1", "Q2", "Q3", "Q4"]
cd.add_series("Revenue", (120, 150, 170, 210))
cd.add_series("Margin", (0.12, 0.15, 0.14, 0.18))
chart(s, XL_CHART_TYPE.COLUMN_CLUSTERED, cd, "Revenue")

# Slide 6: 3-D pie.
s = slide("3-D pie")
cd = CategoryChartData()
cd.categories = ["Rent", "Food", "Travel"]
cd.add_series("Share", (5, 3, 2))
chart(s, XL_CHART_TYPE.PIE, cd, "Share")  # made 3-D below

FIXED = (2026, 11, 16, 0, 0, 0)
buf = io.BytesIO()
prs.save(buf)
src = zipfile.ZipFile(io.BytesIO(buf.getvalue()))
parts = {n: src.read(n) for n in src.namelist()}


def chart_of(slide_no):
    rels = parts[f"ppt/slides/_rels/slide{slide_no}.xml.rels"].decode()
    target = re.search(r'Target="\.\./charts/(chart\d+\.xml)"', rels).group(1)
    return f"ppt/charts/{target}"


# python-pptx writes c:smooth in radar series, which the schema doesn't
# allow there.
for k in (1, 2):
    name = chart_of(k)
    parts[name] = re.sub(rb'<c:smooth val="\d"/>', b"", parts[name])

# Stock: the line chart's plot becomes a stock chart (no series lines,
# dash markers on the close, high-low lines), as PowerPoint writes one.
name = chart_of(4)
x = parts[name].decode()
x = x.replace("<c:lineChart>", "<c:stockChart>").replace("</c:lineChart>", "</c:stockChart>")
x = re.sub(r"<c:grouping val=\"\w+\"/>", "", x, count=1)
x = re.sub(r"<c:stockChart><c:varyColors val=\"\d\"/>", "<c:stockChart>", x)
x = re.sub(
    r"<c:marker><c:symbol val=\"\w+\"/></c:marker>",
    "",
    x,
)
nofill = '<c:spPr><a:ln w="19050"><a:noFill/></a:ln></c:spPr>'
none = '<c:marker><c:symbol val="none"/></c:marker>'
dash = '<c:marker><c:symbol val="dash"/><c:size val="9"/></c:marker>'
for k, m in enumerate([none, none, dash]):
    # After each series' c:tx (spPr and marker come next in schema order).
    x = re.sub(
        r'(<c:ser><c:idx val="%d"/><c:order val="%d"/><c:tx>.*?</c:tx>)(<c:marker>.*?</c:marker>)?' % (k, k),
        lambda mo: mo.group(1) + nofill + m,
        x,
        count=1,
        flags=re.S,
    )
x = re.sub(r"<c:marker val=\"1\"/>", "", x)
x = x.replace('</c:ser><c:smooth val="0"/><c:axId', "</c:ser><c:axId")
x = x.replace(
    '<c:axId val="',
    '<c:hiLowLines><c:spPr><a:ln w="9525"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:ln></c:spPr></c:hiLowLines><c:axId val="',
    1,
)
parts[name] = x.encode()

# Combo: the second series moves into a line chart on its own axes.
name = chart_of(5)
x = parts[name].decode()
ser = re.search(r'<c:ser><c:idx val="1"/>.*?</c:ser>', x, re.S).group(0)
x = x.replace(ser, "", 1)
ser = re.sub(r"<c:invertIfNegative val=\"\d\"/>", "", ser)
ser = ser.replace("</c:val>", '</c:val><c:smooth val="0"/>')
line = (
    '<c:lineChart><c:grouping val="standard"/><c:varyColors val="0"/>'
    + ser
    + '<c:marker val="1"/><c:axId val="500"/><c:axId val="501"/></c:lineChart>'
)
x = x.replace("</c:barChart>", "</c:barChart>" + line, 1)
axes = (
    '<c:valAx><c:axId val="501"/><c:scaling><c:orientation val="minMax"/></c:scaling>'
    '<c:delete val="0"/><c:axPos val="r"/><c:numFmt formatCode="0%" sourceLinked="0"/>'
    '<c:majorTickMark val="out"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/>'
    '<c:crossAx val="500"/><c:crosses val="max"/><c:crossBetween val="between"/></c:valAx>'
    '<c:catAx><c:axId val="500"/><c:scaling><c:orientation val="minMax"/></c:scaling>'
    '<c:delete val="1"/><c:axPos val="b"/><c:majorTickMark val="out"/>'
    '<c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="501"/>'
    '<c:crosses val="autoZero"/><c:auto val="1"/><c:lblAlgn val="ctr"/>'
    '<c:lblOffset val="100"/><c:noMultiLvlLbl val="0"/></c:catAx>'
)
x = re.sub(r"(</c:valAx>)(?!.*</c:valAx>)", r"\1" + axes, x, count=1, flags=re.S)
parts[name] = x.encode()

# 3-D pie: python-pptx can't write one, so the pie is turned into one.
name = chart_of(6)
x = parts[name].decode()
x = x.replace("<c:pieChart>", "<c:pie3DChart>").replace("</c:pieChart>", "</c:pie3DChart>")
x = re.sub(r"<c:firstSliceAng val=\"\d+\"/>", "", x)
x = x.replace(
    "<c:plotArea>",
    '<c:view3D><c:rotX val="30"/><c:rotY val="0"/><c:rAngAx val="0"/></c:view3D><c:plotArea>',
    1,
)
parts[name] = x.encode()

# python-pptx writes negative axis ids, which the Open XML SDK rejects
# (they must be unsigned): make them positive.
for n in parts:
    if n.startswith("ppt/charts/chart"):
        parts[n] = re.sub(rb'(<c:(?:axId|crossAx) val=")-', rb"\1", parts[n])
with zipfile.ZipFile(out / "charts.pptx", "w", zipfile.ZIP_DEFLATED) as z:
    for name in ["[Content_Types].xml"] + sorted(n for n in parts if n != "[Content_Types].xml"):
        info = zipfile.ZipInfo(name, date_time=FIXED)
        info.compress_type = zipfile.ZIP_DEFLATED
        z.writestr(info, parts[name])
print("charts ok")
