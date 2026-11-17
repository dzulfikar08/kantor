#!/usr/bin/env python3
"""Write the 3-D charts and date axis fixture (CC0, synthetic):
tests/corpus/pptx/charts3d.pptx. Read by crates/slides_scene/tests/chart_types.rs.

One chart per slide, each with its embedded workbook (python-pptx writes
2-D charts; their plots are rewritten as the 3-D ones PowerPoint writes,
with `c:view3D`, floor and walls, and a series axis where the chart has
one):
- Slide 1 "3-D clustered column": two series side by side, axes at right
  angles (`rAngAx`), a grey floor.
- Slide 2 "3-D column": the series one behind the other (a series axis),
  in perspective, turned 30°.
- Slide 3 "3-D stacked bar": horizontal stacked bars.
- Slide 4 "3-D line": ribbons, one per series.
- Slide 5 "3-D area": slabs, one per series.
- Slide 6 "3-D surface": three series × five categories.
- Slide 7 "3-D pie": exploded slice, tilted 40°.
- Slide 8 "Date axis": columns on a date axis (days) with dates missing.
- Slide 9 "3-D shapes": cylinder, cone and pyramid columns (`c:shape` per
  series), with value data labels.
- Slide 10 "3-D pie labels": a 3-D pie with category and percent labels
  outside the slices.

Regenerate with
`uv run --with python-pptx==1.0.2 scripts/corpus/gen_pptx_charts3d.py tests/corpus/pptx`.
"""
import datetime
import io
import re
import sys
import zipfile
from pathlib import Path

from pptx import Presentation
from pptx.chart.data import CategoryChartData
from pptx.enum.chart import XL_CHART_TYPE, XL_LEGEND_POSITION
from pptx.util import Inches

out = Path(sys.argv[1] if len(sys.argv) > 1 else "tests/corpus/pptx")
prs = Presentation()
prs.slide_width, prs.slide_height = Inches(13.333), Inches(7.5)
TITLE_ONLY = prs.slide_layouts[5]

CATS = ["North", "South", "East", "West", "Centre"]
SERIES = [("2024", (4, 6, 3, 7, 5)), ("2025", (5, 7, 6, 8, 4)), ("2026", (6, 5, 8, 9, 7))]


def slide(title, kind, cats, series):
    s = prs.slides.add_slide(TITLE_ONLY)
    s.shapes.title.text = title
    cd = CategoryChartData()
    cd.categories = cats
    for name, vals in series:
        cd.add_series(name, vals)
    ch = s.shapes.add_chart(kind, Inches(1), Inches(1.6), Inches(11.3), Inches(5.6), cd)
    ch.chart.has_legend = True
    ch.chart.legend.position = XL_LEGEND_POSITION.RIGHT
    ch.chart.legend.include_in_layout = False


slide("3-D clustered column", XL_CHART_TYPE.COLUMN_CLUSTERED, CATS, SERIES[:2])
slide("3-D column", XL_CHART_TYPE.COLUMN_CLUSTERED, CATS, SERIES)
slide("3-D stacked bar", XL_CHART_TYPE.BAR_STACKED, CATS, SERIES)
slide("3-D line", XL_CHART_TYPE.LINE, CATS, SERIES)
slide("3-D area", XL_CHART_TYPE.AREA, CATS, SERIES)
slide("3-D surface", XL_CHART_TYPE.COLUMN_CLUSTERED, CATS, SERIES)
slide("3-D pie", XL_CHART_TYPE.PIE, CATS, SERIES[:1])
DATES = [datetime.date(2025, 1, d) for d in (1, 2, 3, 6, 7, 8, 9, 13)]
slide("Date axis", XL_CHART_TYPE.COLUMN_CLUSTERED, DATES, [("Visits", (5, 7, 6, 9, 8, 7, 10, 12))])
slide("3-D shapes", XL_CHART_TYPE.COLUMN_CLUSTERED, CATS, SERIES)
slide("3-D pie labels", XL_CHART_TYPE.PIE, CATS, SERIES[:1])

buf = io.BytesIO()
prs.save(buf)
src = zipfile.ZipFile(io.BytesIO(buf.getvalue()))
parts = {n: src.read(n) for n in src.namelist()}


def chart_of(slide_no):
    rels = parts[f"ppt/slides/_rels/slide{slide_no}.xml.rels"].decode()
    target = re.search(r'Target="\.\./charts/(chart\d+\.xml)"', rels).group(1)
    return f"ppt/charts/{target}"


GREY = '<a:solidFill><a:srgbClr val="D9D9D9"/></a:solidFill>'


def view(rot_x, rot_y, r_ang, persp=30, floor=GREY):
    return (
        f'<c:view3D><c:rotX val="{rot_x}"/><c:rotY val="{rot_y}"/><c:depthPercent val="100"/>'
        f'<c:rAngAx val="{r_ang}"/><c:perspective val="{persp}"/></c:view3D>'
        f'<c:floor><c:thickness val="0"/><c:spPr>{floor}</c:spPr></c:floor>'
        '<c:sideWall><c:thickness val="0"/><c:spPr><a:noFill/></c:spPr></c:sideWall>'
        '<c:backWall><c:thickness val="0"/><c:spPr><a:noFill/></c:spPr></c:backWall>'
    )


def ser_ax(cross):
    return (
        '<c:serAx><c:axId val="3"/><c:scaling><c:orientation val="minMax"/></c:scaling>'
        '<c:delete val="0"/><c:axPos val="b"/><c:majorTickMark val="out"/><c:minorTickMark val="none"/>'
        f'<c:tickLblPos val="nextTo"/><c:crossAx val="{cross}"/><c:crosses val="autoZero"/></c:serAx>'
    )


def rewrite(k, old, new, grouping, view3d, extra_end, series_axis, strip=()):
    name = chart_of(k)
    x = parts[name].decode()
    x = re.sub(rb'(<c:(?:axId|crossAx) val=")-'.decode(), r"\1", x)
    x = x.replace(f"<c:{old}>", f"<c:{new}>").replace(f"</c:{old}>", f"</c:{new}>")
    if grouping is not None:
        x = re.sub(r'<c:grouping val="\w+"/>', f'<c:grouping val="{grouping}"/>', x, count=1)
    for pat in strip:
        x = re.sub(pat, "", x)
    ids = re.findall(rf'<c:{new}>.*?((?:<c:axId val="\d+"/>)+)</c:{new}>', x, re.S)[0]
    val_id = re.search(r'<c:valAx><c:axId val="(\d+)"/>', x).group(1)
    axes = ids + ('<c:axId val="3"/>' if series_axis else "")
    x = x.replace(ids + f"</c:{new}>", extra_end + axes + f"</c:{new}>", 1)
    if series_axis:
        x = re.sub(r"(</c:valAx>)", r"\1" + ser_ax(val_id), x, count=1)
    x = x.replace("<c:plotArea>", view3d + "<c:plotArea>", 1)
    parts[name] = x.encode()


BAR_STRIP = (r'<c:overlap val="-?\d+"/>',)
rewrite(1, "barChart", "bar3DChart", "clustered", view(15, 20, 1),
        '<c:gapDepth val="150"/><c:shape val="box"/>', False, BAR_STRIP)
rewrite(2, "barChart", "bar3DChart", "standard", view(20, 30, 0),
        '<c:gapDepth val="150"/><c:shape val="box"/>', True, BAR_STRIP)
rewrite(3, "barChart", "bar3DChart", "stacked", view(15, 20, 1),
        '<c:shape val="box"/>', False, BAR_STRIP)
rewrite(4, "lineChart", "line3DChart", "standard", view(15, 20, 0),
        '<c:gapDepth val="150"/>', True,
        (r'<c:marker val="\d"/>', r"<c:marker><c:symbol val=\"\w+\"/></c:marker>",
         r'(?<=</c:ser>)<c:smooth val="\d"/>'))
rewrite(5, "areaChart", "area3DChart", "standard", view(15, 20, 0),
        '<c:gapDepth val="150"/>', True, ())
rewrite(6, "barChart", "surface3DChart", None, view(15, 20, 0),
        "", True,
        (r'<c:overlap val="-?\d+"/>', r'<c:barDir val="\w+"/>', r'<c:grouping val="\w+"/>',
         r'<c:varyColors val="\d"/>', r'<c:invertIfNegative val="\d"/>', r'<c:gapWidth val="\d+"/>'))
# The surface chart's first element: c:wireframe.
name = chart_of(6)
parts[name] = parts[name].replace(b"<c:surface3DChart>", b'<c:surface3DChart><c:wireframe val="0"/>', 1)

name = chart_of(7)
x = parts[name].decode()
x = x.replace("<c:pieChart>", "<c:pie3DChart>").replace("</c:pieChart>", "</c:pie3DChart>")
x = re.sub(r"<c:firstSliceAng val=\"\d+\"/>", "", x)
x = x.replace("<c:plotArea>", '<c:view3D><c:rotX val="40"/><c:rotY val="30"/><c:rAngAx val="0"/></c:view3D><c:plotArea>', 1)
x = re.sub(r"(<c:ser><c:idx val=\"0\"/><c:order val=\"0\"/><c:tx>.*?</c:tx>)", r'\1<c:explosion val="12"/>', x, count=1, flags=re.S)
parts[name] = x.encode()

# Date axis: python-pptx writes c:dateAx for date categories; days as
# the base unit, two-day major unit.
name = chart_of(8)
x = parts[name].decode()
x = re.sub(rb'(<c:(?:axId|crossAx) val=")-'.decode(), r"\1", x)
x = re.sub(r'<c:baseTimeUnit val="\w+"/>', "", x)
x = re.sub(r"(<c:dateAx>.*?)(<c:lblOffset val=\"\d+\"/>)", r'\1\2<c:baseTimeUnit val="days"/>', x, count=1, flags=re.S)
assert "<c:dateAx>" in x and "baseTimeUnit" in x, "date axis"
parts[name] = x.encode()

# Slide 9: one shape per series, value labels on all of them.
VAL_LABELS = (
    '<c:dLbls><c:showLegendKey val="0"/><c:showVal val="1"/><c:showCatName val="0"/>'
    '<c:showSerName val="0"/><c:showPercent val="0"/><c:showBubbleSize val="0"/></c:dLbls>'
)
rewrite(9, "barChart", "bar3DChart", "clustered", view(15, 20, 1),
        '<c:gapDepth val="150"/><c:shape val="box"/>', False, BAR_STRIP + (r"<c:dLbls>.*?</c:dLbls>",))
name = chart_of(9)
x = parts[name].decode()
for shape in ("cylinder", "cone", "pyramid"):
    x = re.sub(r"(</c:val>)(</c:ser>)", rf'\1<c:shape val="{shape}"/>\2', x, count=1)
x = x.replace("<c:gapDepth", VAL_LABELS + "<c:gapDepth", 1)
assert VAL_LABELS in x, "labels"
assert x.count("<c:shape ") == 4, "shapes"
parts[name] = x.encode()

# Slide 10: a 3-D pie, category name and percent outside the slices.
name = chart_of(10)
x = parts[name].decode()
x = x.replace("<c:pieChart>", "<c:pie3DChart>").replace("</c:pieChart>", "</c:pie3DChart>")
x = re.sub(r"<c:firstSliceAng val=\"\d+\"/>", "", x)
x = re.sub(r"<c:dLbls>.*?</c:dLbls>", "", x, flags=re.S)
x = x.replace("<c:plotArea>", '<c:view3D><c:rotX val="30"/><c:rotY val="0"/><c:rAngAx val="0"/></c:view3D><c:plotArea>', 1)
x = x.replace(
    "</c:ser>",
    '</c:ser><c:dLbls><c:dLblPos val="outEnd"/><c:showLegendKey val="0"/><c:showVal val="0"/>'
    '<c:showCatName val="1"/><c:showSerName val="0"/><c:showPercent val="1"/><c:showBubbleSize val="0"/>'
    '<c:showLeaderLines val="1"/></c:dLbls>',
    1,
)
parts[name] = x.encode()

for n in parts:
    if n.startswith("ppt/charts/chart"):
        parts[n] = re.sub(rb'(<c:(?:axId|crossAx) val=")-', rb"\1", parts[n])

FIXED = (2026, 11, 17, 0, 0, 0)
with zipfile.ZipFile(out / "charts3d.pptx", "w", zipfile.ZIP_DEFLATED) as z:
    for name in ["[Content_Types].xml"] + sorted(n for n in parts if n != "[Content_Types].xml"):
        info = zipfile.ZipInfo(name, date_time=FIXED)
        info.compress_type = zipfile.ZIP_DEFLATED
        z.writestr(info, parts[name])
print("charts3d ok")
