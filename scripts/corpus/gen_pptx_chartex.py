#!/usr/bin/env python3
"""Write the Slides fixture of the chart types Kantor drew as placeholders
(CC0, synthetic): tests/corpus/pptx/chartex.pptx. Read by
crates/slides_pdf/tests/chart_types.rs.

One chart per slide:
- Slide 1 "Surface": a surface chart (3 series × 5 categories), drawn as
  PowerPoint's contour view (python-pptx has no surface charts: a line
  chart's plot is rewritten as `c:surfaceChart` with a series axis).
- Slide 2 "Wireframe": the same as a wireframe contour.
- Slide 3 "Pie of pie": the last three points in a second pie.
- Slide 4 "Bar of pie": the same as a bar of pie (both rewritten from a pie).
- Slides 5-11: the Office 2016 types as chartex parts
  (`ppt/charts/chartExN.xml`, scripts/corpus/chartex_parts.py) in an
  `mc:AlternateContent` whose fallback is PowerPoint's "This chart isn't
  available" box: waterfall, funnel, treemap, sunburst, histogram, Pareto,
  box & whisker.

Regenerate with
`uv run --with python-pptx==1.0.2 scripts/corpus/gen_pptx_chartex.py tests/corpus/pptx`.
"""
import io
import re
import sys
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import chartex_parts as cxp  # noqa: E402

from pptx import Presentation  # noqa: E402
from pptx.chart.data import CategoryChartData  # noqa: E402
from pptx.enum.chart import XL_CHART_TYPE, XL_LEGEND_POSITION  # noqa: E402
from pptx.util import Inches  # noqa: E402

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
    ch.chart.legend.position = XL_LEGEND_POSITION.RIGHT
    ch.chart.legend.include_in_layout = False
    return ch


for title in ["Surface", "Wireframe"]:
    s = slide(title)
    cd = CategoryChartData()
    cd.categories = ["Jan", "Feb", "Mar", "Apr", "May"]
    cd.add_series("Low", (10, 20, 30, 25, 15))
    cd.add_series("Mid", (20, 35, 45, 40, 25))
    cd.add_series("High", (15, 30, 50, 45, 30))
    chart(s, XL_CHART_TYPE.LINE, cd, "Heat")

for title in ["Pie of pie", "Bar of pie"]:
    s = slide(title)
    cd = CategoryChartData()
    cd.categories = ["Rent", "Food", "Travel", "Books", "Music", "Games"]
    cd.add_series("Spend", (40, 25, 15, 8, 7, 5))
    chart(s, XL_CHART_TYPE.PIE, cd, "Spend")

ex_slides = []
for title, _rows, _make, _req in cxp.CHARTS:
    ex_slides.append(slide(title))

FIXED = (2026, 11, 16, 0, 0, 0)
buf = io.BytesIO()
prs.save(buf)
src = zipfile.ZipFile(io.BytesIO(buf.getvalue()))
parts = {n: src.read(n) for n in src.namelist()}


def chart_of(slide_no):
    rels = parts[f"ppt/slides/_rels/slide{slide_no}.xml.rels"].decode()
    target = re.search(r'Target="\.\./charts/(chart\d+\.xml)"', rels).group(1)
    return f"ppt/charts/{target}"


# Surface: the line chart becomes a surface chart with a series axis.
for k, wire in [(1, 0), (2, 1)]:
    name = chart_of(k)
    x = parts[name].decode()
    x = re.sub(r"<c:lineChart>.*?<c:varyColors val=\"\d\"/>", f'<c:surfaceChart><c:wireframe val="{wire}"/>', x, count=1, flags=re.S)
    x = x.replace("</c:lineChart>", "</c:surfaceChart>")
    x = re.sub(r"<c:marker>.*?</c:marker>", "", x, flags=re.S)
    x = re.sub(r"<c:marker val=\"\d\"/>", "", x)
    x = re.sub(r"<c:smooth val=\"\d\"/>", "", x)
    # Series lines (spPr) aren't wanted on a surface.
    x = re.sub(r"(<c:ser>.*?</c:tx>)<c:spPr>.*?</c:spPr>", r"\1", x, flags=re.S)
    ids = re.findall(r'<c:axId val="(-?\d+)"/>', x)[:2]
    x = x.replace(
        f'<c:axId val="{ids[1]}"/></c:surfaceChart>',
        f'<c:axId val="{ids[1]}"/><c:axId val="700"/></c:surfaceChart>',
    )
    ser_ax = (
        '<c:serAx><c:axId val="700"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/>'
        '<c:axPos val="b"/><c:majorTickMark val="out"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/>'
        f'<c:crossAx val="{ids[1]}"/><c:crosses val="autoZero"/></c:serAx>'
    )
    x = re.sub(r"(</c:valAx>)(?!.*</c:valAx>)", r"\1" + ser_ax, x, count=1, flags=re.S)
    # A major unit of 10: bands 10-20, 20-30...
    x = re.sub(r"(<c:valAx>.*?)(</c:valAx>)", lambda m: m.group(1) + '<c:majorUnit val="10"/>' + m.group(2)
               if "<c:majorUnit" not in m.group(1) else m.group(0), x, count=1, flags=re.S)
    parts[name] = x.encode()

# Pie of pie / bar of pie: the pie becomes an ofPieChart.
for k, ty in [(3, "pie"), (4, "bar")]:
    name = chart_of(k)
    x = parts[name].decode()
    x = x.replace("<c:pieChart>", f'<c:ofPieChart><c:ofPieType val="{ty}"/>')
    x = re.sub(r"<c:firstSliceAng val=\"\d+\"/>", "", x)
    x = x.replace(
        "</c:pieChart>",
        '<c:gapWidth val="100"/><c:splitType val="pos"/><c:splitPos val="3"/>'
        '<c:secondPieSize val="75"/><c:serLines/></c:ofPieChart>',
    )
    parts[name] = x.encode()

# python-pptx writes negative axis ids, which the Open XML SDK rejects.
for n in parts:
    if n.startswith("ppt/charts/chart"):
        parts[n] = re.sub(rb'(<c:(?:axId|crossAx) val=")-', rb"\1", parts[n])

# Chartex slides.
ct = parts["[Content_Types].xml"].decode()
first_ex = 5
for i, (title, _rows, make, req) in enumerate(cxp.CHARTS):
    k = first_ex + i
    part = f"ppt/charts/chartEx{i + 1}.xml"
    parts[part] = make().encode()
    ct = ct.replace("</Types>", f'<Override PartName="/{part}" ContentType="{cxp.CT_CHARTEX}"/></Types>')
    rels_name = f"ppt/slides/_rels/slide{k}.xml.rels"
    rels = parts[rels_name].decode()
    rels = rels.replace(
        "</Relationships>",
        f'<Relationship Id="rId9" Type="{cxp.REL_CHARTEX}" Target="../charts/chartEx{i + 1}.xml"/></Relationships>',
    )
    parts[rels_name] = rels.encode()
    xfrm = '<a:off x="914400" y="1371600"/><a:ext cx="10058400" cy="5120640"/>'
    frame = (
        '<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">'
        f'<mc:Choice xmlns:cx1="{req}" Requires="cx1">'
        '<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="10" name="Chart 9"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>'
        f"<p:xfrm>{xfrm}</p:xfrm>"
        f'<a:graphic><a:graphicData uri="{cxp.URI_CHARTEX}"><cx:chart xmlns:cx="{cxp.NS_CX}" r:id="rId9"/></a:graphicData></a:graphic>'
        "</p:graphicFrame></mc:Choice><mc:Fallback>"
        '<p:sp><p:nvSpPr><p:cNvPr id="10" name="Chart 9"/><p:cNvSpPr><a:spLocks noTextEdit="1"/></p:cNvSpPr><p:nvPr/></p:nvSpPr>'
        f'<p:spPr><a:xfrm>{xfrm}</a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>'
        '<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="1100"/>'
        "<a:t>This chart isn't available in your version of PowerPoint.</a:t></a:r></a:p></p:txBody></p:sp>"
        "</mc:Fallback></mc:AlternateContent>"
    )
    name = f"ppt/slides/slide{k}.xml"
    x = parts[name].decode()
    x = x.replace("</p:spTree>", frame + "</p:spTree>", 1)
    if req == cxp.CX2:
        x = x.replace("cx1", "cx2")
    parts[name] = x.encode()
parts["[Content_Types].xml"] = ct.encode()

with zipfile.ZipFile(out / "chartex.pptx", "w", zipfile.ZIP_DEFLATED) as z:
    for name in ["[Content_Types].xml"] + sorted(n for n in parts if n != "[Content_Types].xml"):
        info = zipfile.ZipInfo(name, date_time=FIXED)
        info.compress_type = zipfile.ZIP_DEFLATED
        z.writestr(info, parts[name])
print("chartex ok")
