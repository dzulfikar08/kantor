#!/usr/bin/env python3
"""Write the Slides fixture (CC0, synthetic): tests/corpus/pptx/deck.pptx, a
10-slide 16:9 business deck: title, bullets with speaker notes, a 5 x 4
table, a clustered column chart and a pie chart (with their embedded
workbooks), closing slide. Read by crates/slides_engine/tests/ and
crates/slides_pdf/tests/.

Regenerate with
`uv run --with python-pptx==1.0.2 scripts/corpus/gen_pptx.py tests/corpus/pptx`.
"""
import datetime
import sys
from pathlib import Path

from pptx import Presentation
from pptx.chart.data import CategoryChartData
from pptx.enum.chart import XL_CHART_TYPE
from pptx.util import Inches, Pt

out = Path(sys.argv[1] if len(sys.argv) > 1 else "tests/corpus/pptx")
prs = Presentation()
prs.slide_width, prs.slide_height = Inches(13.333), Inches(7.5)


def title_content(title, bullets, notes=None):
    s = prs.slides.add_slide(prs.slide_layouts[1])
    s.shapes.title.text = title
    tf = s.placeholders[1].text_frame
    tf.text = bullets[0]
    for b in bullets[1:]:
        tf.add_paragraph().text = b
    if notes:
        s.notes_slide.notes_text_frame.text = notes
    return s


s = prs.slides.add_slide(prs.slide_layouts[0])
s.shapes.title.text = "FY2027 Plan"
s.placeholders[1].text = "Operations · November 2026"
title_content("Agenda", ["Where we are", "Targets", "Budget", "Risks", "Next steps"], "Keep this short.")
title_content("Where we are", ["Uptime 97.4 %", "Backlog down 31 %", "Two new sites live"])
s = prs.slides.add_slide(prs.slide_layouts[5])
s.shapes.title.text = "Regional results"
rows, cols = 5, 4
tbl = s.shapes.add_table(rows, cols, Inches(1), Inches(1.6), Inches(11), Inches(3.5)).table
for c, h in enumerate(["Region", "Q1", "Q2", "Q3"]):
    tbl.cell(0, c).text = h
for r, name in enumerate(["North", "South", "East", "West"], 1):
    tbl.cell(r, 0).text = name
    for c in range(1, 4):
        tbl.cell(r, c).text = str(100 + r * 10 + c)
s = prs.slides.add_slide(prs.slide_layouts[5])
s.shapes.title.text = "Revenue by quarter"
cd = CategoryChartData()
cd.categories = ["Q1", "Q2", "Q3", "Q4"]
cd.add_series("2025", (12.1, 13.4, 14.0, 15.2))
cd.add_series("2026", (13.0, 14.9, 15.8, 17.1))
s.shapes.add_chart(XL_CHART_TYPE.COLUMN_CLUSTERED, Inches(1), Inches(1.6), Inches(11), Inches(5), cd)
title_content("Targets", ["Uptime 98 %", "MTTR under 2 h", "Backlog under 2 weeks"], "Targets agreed with plant managers.")
s = prs.slides.add_slide(prs.slide_layouts[5])
s.shapes.title.text = "Budget split"
cd = CategoryChartData()
cd.categories = ["People", "Parts", "Contractors", "Tools"]
cd.add_series("Budget", (45, 30, 15, 10))
s.shapes.add_chart(XL_CHART_TYPE.PIE, Inches(3), Inches(1.5), Inches(7), Inches(5.5), cd)
title_content("Risks", ["Parts lead times", "Contractor availability", "FX on imported spares"])
title_content("Next steps", ["Approve budget", "Hire two planners", "Roll out to site C"])
s = prs.slides.add_slide(prs.slide_layouts[0])
s.shapes.title.text = "Thank you"
s.placeholders[1].text = "Questions?"
FIXED = datetime.datetime(2026, 11, 12, 0, 0, 0)
prs.core_properties.created = FIXED
prs.core_properties.modified = FIXED
prs.core_properties.last_modified_by = "Kantor corpus"
prs.core_properties.author = "Kantor corpus"
prs.save(out / "deck.pptx")
print("pptx ok", len(prs.slides))
