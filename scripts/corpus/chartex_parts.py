"""Chart parts of the Office 2016 chart types (`cx:chartSpace`, "chartex"),
as Excel and PowerPoint write them, for the synthetic fixtures made by
gen_pptx_chartex.py (tests/corpus/pptx/chartex.pptx) and
gen_chartex_xlsx.py (tests/corpus/xlsx/25_chartex.xlsx). Standard library
only. CC0-1.0 like the rest of the corpus.

Each chart reads its data from a sheet named "Data" (`cx:f`) and caches
the values in the part (`cx:lvl`), as Office does.
"""

from __future__ import annotations

NS_CX = "http://schemas.microsoft.com/office/drawing/2014/chartex"
NS_A = "http://schemas.openxmlformats.org/drawingml/2006/main"
NS_R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
REL_CHARTEX = "http://schemas.microsoft.com/office/2014/relationships/chartEx"
CT_CHARTEX = "application/vnd.ms-office.chartex+xml"
URI_CHARTEX = "http://schemas.microsoft.com/office/drawing/2014/chartex"
# The namespace a viewer must know to show each type (mc:Choice Requires).
CX1 = "http://schemas.microsoft.com/office/drawing/2015/9/8/chartex"
CX2 = "http://schemas.microsoft.com/office/drawing/2015/10/21/chartex"


def esc(s: str) -> str:
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def lvl(values: list, fmt: str | None = None) -> str:
    f = f' formatCode="{fmt}"' if fmt else ""
    pts = "".join(f'<cx:pt idx="{i}">{esc(str(v))}</cx:pt>' for i, v in enumerate(values))
    return f'<cx:lvl ptCount="{len(values)}"{f}>{pts}</cx:lvl>'


def col(c: int) -> str:
    return chr(65 + c)


def data_block(cat_levels: list[list[str]], vals: list, first_row: int = 2, val_col: int | None = None) -> str:
    """`cx:data id="0"`: category levels innermost first (columns from the
    outermost on the left), then the values."""
    n = len(vals)
    last = first_row + n - 1
    out = '<cx:data id="0">'
    if cat_levels:
        ncol = len(cat_levels)
        rng = f"Data!$A${first_row}:${col(ncol - 1)}${last}"
        out += f'<cx:strDim type="cat"><cx:f>{rng}</cx:f>' + "".join(lvl(l) for l in cat_levels) + "</cx:strDim>"
    vc = col(len(cat_levels) if val_col is None else val_col)
    out += f'<cx:numDim type="val"><cx:f>Data!${vc}${first_row}:${vc}${last}</cx:f>{lvl(vals, "General")}</cx:numDim>'
    return out + "</cx:data>"


def series(layout: str, name: str, name_cell: str, inner: str, idx: int = 0, extra_attrs: str = "") -> str:
    return (
        f'<cx:series layoutId="{layout}" uniqueId="{{00000000-0000-4000-8000-00000000000{idx}}}"{extra_attrs}>'
        f'<cx:tx><cx:txData><cx:f>{name_cell}</cx:f><cx:v>{esc(name)}</cx:v></cx:txData></cx:tx>'
        f"{inner}</cx:series>"
    )


def space(title: str, data: str, region: str, axes: str, legend: bool) -> str:
    lg = '<cx:legend pos="t" align="ctr" overlay="0"/>' if legend else ""
    return (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        f'<cx:chartSpace xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:cx="{NS_CX}">'
        f"<cx:chartData>{data}</cx:chartData><cx:chart>"
        f'<cx:title pos="t" align="ctr" overlay="0"><cx:tx><cx:txData><cx:v>{esc(title)}</cx:v></cx:txData></cx:tx></cx:title>'
        f"<cx:plotArea><cx:plotAreaRegion>{region}</cx:plotAreaRegion>{axes}</cx:plotArea>{lg}"
        "</cx:chart></cx:chartSpace>"
    )


CAT_VAL_AXES = (
    '<cx:axis id="0"><cx:catScaling gapWidth="0.5"/><cx:tickLabels/></cx:axis>'
    '<cx:axis id="1"><cx:valScaling/><cx:majorGridlines/><cx:tickLabels/></cx:axis>'
)
VALUE_LABELS = '<cx:dataLabels pos="outEnd"><cx:visibility seriesName="0" categoryName="0" value="1"/></cx:dataLabels>'


# Each chart: (title, sheet rows with a header, part XML, Requires namespace).
WATERFALL_ROWS = [["Step", "Amount"], ["Start", 100], ["Sales", 50], ["Costs", -30], ["Tax", -10], ["Other", 15], ["End", 125]]
FUNNEL_ROWS = [["Stage", "Count"], ["Leads", 500], ["Calls", 300], ["Demos", 120], ["Offers", 60], ["Deals", 25]]
TREE_ROWS = [
    ["Region", "Store", "Sales"],
    ["North", "Alpha", 30], ["North", "Beta", 20],
    ["South", "Gamma", 25], ["South", "Delta", 10], ["South", "Epsilon", 5],
    ["West", "Zeta", 10],
]
SUN_ROWS = [
    ["Area", "Region", "Store", "Sales"],
    ["Europe", "North", "Alpha", 30], ["Europe", "North", "Beta", 20], ["Europe", "South", "Gamma", 25],
    ["Asia", "East", "Delta", 15], ["Asia", "East", "Epsilon", 5], ["Asia", "Islands", "", 12],
]
HIST_ROWS = [["Score"]] + [[v] for v in [12, 18, 23, 25, 31, 34, 36, 38, 41, 44, 45, 47, 52, 55, 58, 63, 67, 72, 81, 95]]
PARETO_ROWS = [["Complaint", "Count"], ["Late", 1], ["Rude", 1], ["Late", 1], ["Broken", 1], ["Late", 1], ["Rude", 1], ["Price", 1], ["Late", 1]]
BOX_ROWS = [
    ["Day", "Morning", "Evening"],
    ["Mon", 12, 20], ["Mon", 15, 22], ["Mon", 14, 25], ["Mon", 18, 21], ["Mon", 40, 23],
    ["Tue", 10, 30], ["Tue", 11, 28], ["Tue", 13, 35], ["Tue", 16, 33], ["Tue", 12, 31],
]


def waterfall() -> str:
    rows = WATERFALL_ROWS[1:]
    inner = (
        VALUE_LABELS + '<cx:dataId val="0"/><cx:layoutPr><cx:visibility connectorLines="1"/>'
        '<cx:subtotals><cx:idx val="0"/><cx:idx val="5"/></cx:subtotals></cx:layoutPr>'
    )
    return space("Cash flow", data_block([[r[0] for r in rows]], [r[1] for r in rows]),
                 series("waterfall", "Amount", "Data!$B$1", inner), CAT_VAL_AXES, True)


def funnel() -> str:
    rows = FUNNEL_ROWS[1:]
    inner = VALUE_LABELS.replace('pos="outEnd"', 'pos="ctr"') + '<cx:dataId val="0"/>'
    axes = '<cx:axis id="0"><cx:catScaling gapWidth="0.06"/><cx:tickLabels/></cx:axis>'
    return space("Pipeline", data_block([[r[0] for r in rows]], [r[1] for r in rows]),
                 series("funnel", "Count", "Data!$B$1", inner), axes, False)


def treemap() -> str:
    rows = TREE_ROWS[1:]
    levels = [[r[1] for r in rows], [r[0] for r in rows]]
    inner = (
        '<cx:dataLabels pos="inEnd"><cx:visibility seriesName="0" categoryName="1" value="0"/></cx:dataLabels>'
        '<cx:dataId val="0"/><cx:layoutPr><cx:parentLabelLayout val="banner"/></cx:layoutPr>'
    )
    return space("Sales by store", data_block(levels, [r[2] for r in rows]),
                 series("treemap", "Sales", "Data!$C$1", inner), "", True)


def sunburst() -> str:
    rows = SUN_ROWS[1:]
    levels = [[r[2] for r in rows], [r[1] for r in rows], [r[0] for r in rows]]
    inner = (
        '<cx:dataLabels pos="ctr"><cx:visibility seriesName="0" categoryName="1" value="0"/></cx:dataLabels>'
        '<cx:dataId val="0"/>'
    )
    return space("Sales by area", data_block(levels, [r[3] for r in rows]),
                 series("sunburst", "Sales", "Data!$D$1", inner), "", True)


def histogram() -> str:
    vals = [r[0] for r in HIST_ROWS[1:]]
    inner = (
        '<cx:dataId val="0"/><cx:layoutPr><cx:binning intervalClosed="r"><cx:binSize>20</cx:binSize></cx:binning></cx:layoutPr>'
    )
    axes = (
        '<cx:axis id="0"><cx:catScaling gapWidth="0"/><cx:tickLabels/></cx:axis>'
        '<cx:axis id="1"><cx:valScaling/><cx:majorGridlines/><cx:tickLabels/></cx:axis>'
    )
    return space("Scores", data_block([], vals), series("clusteredColumn", "Score", "Data!$A$1", inner), axes, False)


def pareto() -> str:
    rows = PARETO_ROWS[1:]
    bars = series("clusteredColumn", "Count", "Data!$B$1",
                  '<cx:dataId val="0"/><cx:layoutPr><cx:aggregation/></cx:layoutPr><cx:axisId>1</cx:axisId>')
    line = (
        '<cx:series layoutId="paretoLine" ownerIdx="0" uniqueId="{00000000-0000-4000-8000-000000000001}">'
        '<cx:axisId>2</cx:axisId></cx:series>'
    )
    axes = (
        '<cx:axis id="0"><cx:catScaling gapWidth="0"/><cx:tickLabels/></cx:axis>'
        '<cx:axis id="1"><cx:valScaling/><cx:majorGridlines/><cx:tickLabels/></cx:axis>'
        '<cx:axis id="2"><cx:valScaling max="1" min="0"/><cx:units unit="percentage"/><cx:tickLabels/></cx:axis>'
    )
    return space("Complaints", data_block([[r[0] for r in rows]], [r[1] for r in rows]), bars + line, axes, True)


def box_whisker() -> str:
    rows = BOX_ROWS[1:]
    cats = [r[0] for r in rows]
    n = len(rows)
    data = data_block([cats], [r[1] for r in rows])
    data += (
        f'<cx:data id="1"><cx:strDim type="cat"><cx:f>Data!$A$2:$A${n + 1}</cx:f>{lvl(cats)}</cx:strDim>'
        f'<cx:numDim type="val"><cx:f>Data!$C$2:$C${n + 1}</cx:f>{lvl([r[2] for r in rows], "General")}</cx:numDim></cx:data>'
    )
    lp = (
        '<cx:layoutPr><cx:visibility meanLine="0" meanMarker="1" nonoutliers="0" outliers="1"/>'
        '<cx:statistics quartileMethod="exclusive"/></cx:layoutPr>'
    )
    region = series("boxWhisker", "Morning", "Data!$B$1", f'<cx:dataId val="0"/>{lp}', 0) + series(
        "boxWhisker", "Evening", "Data!$C$1", f'<cx:dataId val="1"/>{lp}', 1
    )
    return space("Waiting times", data, region, CAT_VAL_AXES, True)


CHARTS = [
    ("Waterfall", WATERFALL_ROWS, waterfall, CX1),
    ("Funnel", FUNNEL_ROWS, funnel, CX2),
    ("Treemap", TREE_ROWS, treemap, CX1),
    ("Sunburst", SUN_ROWS, sunburst, CX1),
    ("Histogram", HIST_ROWS, histogram, CX1),
    ("Pareto", PARETO_ROWS, pareto, CX1),
    ("Box and whisker", BOX_ROWS, box_whisker, CX1),
]
