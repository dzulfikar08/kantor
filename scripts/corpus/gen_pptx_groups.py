#!/usr/bin/env python3
"""Write the Slides groups and SmartArt fixture (CC0, synthetic):
tests/corpus/pptx/groups.pptx. Read by crates/slides_engine/tests/groups.rs.

- Slide 1 "Groups": group "Pair" (rectangles "Left", "Right" with text),
  group "Outer" holding rectangle "Solo" and the nested group "Inner"
  (ovals "Dot A", "Dot B"), group "Scaled" (drawn at twice its child
  space's width), and two loose rectangles "Loose A", "Loose B".
- Slide 2 "SmartArt": a hand-written SmartArt (data model, layout, style,
  colours and PowerPoint's cached drawing): items "Plan", an empty
  placeholder item, "Ship", and "Steps" with the sub-items "One" and "Two"
  drawn as one shape with two paragraphs.
- Slide 3 "SmartArt in a group": group "Diagram group" holding a copy of
  that SmartArt ("Diagram 21", its own parts data2.xml...drawing2.xml) and
  the rectangle "Caption".

Regenerate with
`uv run --with python-pptx==1.0.2 scripts/corpus/gen_pptx_groups.py tests/corpus/pptx`.
"""
import datetime
import io
import sys
import zipfile
from pathlib import Path

from pptx import Presentation
from pptx.dml.color import RGBColor
from pptx.enum.shapes import MSO_SHAPE
from pptx.util import Emu, Inches

out = Path(sys.argv[1] if len(sys.argv) > 1 else "tests/corpus/pptx")
prs = Presentation()
prs.slide_width, prs.slide_height = Inches(13.333), Inches(7.5)
TITLE_ONLY = prs.slide_layouts[5]


def box(shapes, kind, name, x, y, w, h, text=None, rgb=None):
    s = shapes.add_shape(kind, Inches(x), Inches(y), Inches(w), Inches(h))
    s.name = name
    if text is not None:
        s.text_frame.text = text
    if rgb:
        s.fill.solid()
        s.fill.fore_color.rgb = RGBColor.from_string(rgb)
    return s


s = prs.slides.add_slide(TITLE_ONLY)
s.shapes.title.text = "Groups"
pair = s.shapes.add_group_shape()
pair.name = "Pair"
box(pair.shapes, MSO_SHAPE.RECTANGLE, "Left", 1, 2, 2, 1, "Left", "4472C4")
box(pair.shapes, MSO_SHAPE.RECTANGLE, "Right", 3.5, 2, 2, 1, "Right", "ED7D31")
outer = s.shapes.add_group_shape()
outer.name = "Outer"
box(outer.shapes, MSO_SHAPE.RECTANGLE, "Solo", 7, 2, 2, 1, "Solo", "70AD47")
inner = outer.shapes.add_group_shape()
inner.name = "Inner"
box(inner.shapes, MSO_SHAPE.OVAL, "Dot A", 9.5, 2, 1, 1, None, "FFC000")
box(inner.shapes, MSO_SHAPE.OVAL, "Dot B", 10.75, 2, 1, 1, None, "5B9BD5")
scaled = s.shapes.add_group_shape()
scaled.name = "Scaled"
box(scaled.shapes, MSO_SHAPE.RECTANGLE, "Wide 1", 1, 4.5, 1, 1, "W1", "A5A5A5")
box(scaled.shapes, MSO_SHAPE.RECTANGLE, "Wide 2", 2.5, 4.5, 1, 1, "W2", "264478")
# Drawn at twice the width of its child space.
scaled.width = Emu(scaled.width * 2)
box(s.shapes, MSO_SHAPE.RECTANGLE, "Loose A", 8, 4.5, 1.5, 1, "A", "9E480E")
box(s.shapes, MSO_SHAPE.ROUNDED_RECTANGLE, "Loose B", 10, 5, 1.5, 1, "B", "636363")

s2 = prs.slides.add_slide(TITLE_ONLY)
s2.shapes.title.text = "SmartArt"
s3 = prs.slides.add_slide(TITLE_ONLY)
s3.shapes.title.text = "SmartArt in a group"

FIXED_DT = datetime.datetime(2026, 11, 15, 0, 0, 0)
prs.core_properties.created = FIXED_DT
prs.core_properties.modified = FIXED_DT
prs.core_properties.last_modified_by = "Kantor corpus"
prs.core_properties.author = "Kantor corpus"
buf = io.BytesIO()
prs.save(buf)

# ── SmartArt, by hand ───────────────────────────────────────────────────
DGM = "http://schemas.openxmlformats.org/drawingml/2006/diagram"
A = "http://schemas.openxmlformats.org/drawingml/2006/main"
DSP = "http://schemas.microsoft.com/office/drawing/2008/diagram"
EMU = 914400
FRAME = (int(1 * EMU), int(2 * EMU), int(11 * EMU), int(4 * EMU))

doc_id = "{00000000-0000-0000-0000-000000000001}"
items = [  # (id, text, parent, placeholder)
    ("{A0000000-0000-0000-0000-000000000001}", "Plan", doc_id, False),
    ("{A0000000-0000-0000-0000-000000000002}", "", doc_id, True),
    ("{A0000000-0000-0000-0000-000000000003}", "Ship", doc_id, False),
    ("{A0000000-0000-0000-0000-000000000004}", "Steps", doc_id, False),
    ("{A0000000-0000-0000-0000-000000000005}", "One", "{A0000000-0000-0000-0000-000000000004}", False),
    ("{A0000000-0000-0000-0000-000000000006}", "Two", "{A0000000-0000-0000-0000-000000000004}", False),
]
# Drawn shapes: (presentation item id, data item it shows, text paragraphs, box)
w = FRAME[2] // 5
drawn = [
    ("{B0000000-0000-0000-0000-000000000001}", items[0][0], ["Plan"], (0, 0, w, FRAME[3] // 2)),
    ("{B0000000-0000-0000-0000-000000000002}", items[1][0], [""], (int(w * 1.25), 0, w, FRAME[3] // 2)),
    ("{B0000000-0000-0000-0000-000000000003}", items[2][0], ["Ship"], (int(w * 2.5), 0, w, FRAME[3] // 2)),
    ("{B0000000-0000-0000-0000-000000000004}", items[3][0], ["Steps"], (int(w * 3.75), 0, w, FRAME[3] // 4)),
    ("{B0000000-0000-0000-0000-000000000005}", items[3][0], ["One", "Two"], (int(w * 3.75), FRAME[3] // 4, w, FRAME[3] // 2)),
]


def paras(texts):
    out = ""
    for t in texts:
        if t:
            out += f'<a:p><a:r><a:rPr lang="en-US"/><a:t>{t}</a:t></a:r></a:p>'
        else:
            out += '<a:p><a:endParaRPr lang="en-US"/></a:p>'
    return out


pts = (
    f'<dgm:pt modelId="{doc_id}" type="doc"><dgm:prSet loTypeId="urn:microsoft.com/office/officeart/2005/8/layout/default" '
    'loCatId="list" qsTypeId="urn:microsoft.com/office/officeart/2005/8/quickstyle/simple1" qsCatId="simple" '
    'csTypeId="urn:microsoft.com/office/officeart/2005/8/colors/accent1_2" csCatId="accent1"/><dgm:spPr/>'
    f'<dgm:t><a:bodyPr/><a:lstStyle/>{paras([""])}</dgm:t></dgm:pt>'
)
for mid, text, _, ph in items:
    pr = '<dgm:prSet phldrT="[Text]" phldr="1"/>' if ph else '<dgm:prSet phldrT="[Text]"/>'
    pts += f'<dgm:pt modelId="{mid}">{pr}<dgm:spPr/><dgm:t><a:bodyPr/><a:lstStyle/>{paras([text])}</dgm:t></dgm:pt>'
for i, (pid, assoc, _, _) in enumerate(drawn):
    pts += (
        f'<dgm:pt modelId="{pid}" type="pres"><dgm:prSet presAssocID="{assoc}" presName="node" '
        f'presStyleLbl="node1" presStyleIdx="{i}" presStyleCnt="{len(drawn)}"/><dgm:spPr/></dgm:pt>'
    )
cxns = ""
order = {}
for n, (mid, _, parent, _) in enumerate(items):
    k = order.get(parent, 0)
    order[parent] = k + 1
    cxns += f'<dgm:cxn modelId="{{C0000000-0000-0000-0000-00000000000{n}}}" srcId="{parent}" destId="{mid}" srcOrd="{k}" destOrd="0"/>'
for n, (pid, assoc, _, _) in enumerate(drawn):
    cxns += (
        f'<dgm:cxn modelId="{{D0000000-0000-0000-0000-00000000000{n}}}" type="presOf" srcId="{assoc}" '
        f'destId="{pid}" srcOrd="0" destOrd="0"/>'
    )
data = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dgm:dataModel xmlns:dgm="{DGM}" xmlns:a="{A}">'
    f"<dgm:ptLst>{pts}</dgm:ptLst><dgm:cxnLst>{cxns}</dgm:cxnLst><dgm:bg/><dgm:whole/>"
    f'<dgm:extLst><a:ext uri="http://schemas.microsoft.com/office/drawing/2008/diagram"><dsp:dataModelExt xmlns:dsp="{DSP}" '
    f'relId="rIdDr" minVer="http://schemas.openxmlformats.org/drawingml/2006/diagram"/></a:ext></dgm:extLst></dgm:dataModel>'
)
sps = ""
for pid, _, texts, (x, y, cx, cy) in drawn:
    ins = 60000
    sps += (
        f'<dsp:sp modelId="{pid}"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr>'
        f'<dsp:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
        '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="accent1"/></a:solidFill>'
        '<a:ln w="12700"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></dsp:spPr>'
        '<dsp:style><a:lnRef idx="2"><a:scrgbClr r="0" g="0" b="0"/></a:lnRef><a:fillRef idx="1"><a:scrgbClr r="0" g="0" b="0"/></a:fillRef>'
        '<a:effectRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></dsp:style>'
        '<dsp:txBody><a:bodyPr spcFirstLastPara="0" vert="horz" wrap="square" lIns="76200" tIns="76200" rIns="76200" bIns="76200" '
        f'numCol="1" spcCol="1270" anchor="ctr" anchorCtr="0"><a:noAutofit/></a:bodyPr><a:lstStyle/>'
        + paras(texts).replace('lang="en-US"/>', 'lang="en-US" sz="2400" kern="1200"/>')
        + f'</dsp:txBody><dsp:txXfrm><a:off x="{x + ins}" y="{y + ins}"/><a:ext cx="{cx - 2 * ins}" cy="{cy - 2 * ins}"/></dsp:txXfrm></dsp:sp>'
    )
drawing = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dsp:drawing xmlns:dgm="{DGM}" xmlns:dsp="{DSP}" xmlns:a="{A}">'
    '<dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/>'
    f"{sps}</dsp:spTree></dsp:drawing>"
)
layout = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dgm:layoutDef xmlns:dgm="{DGM}" xmlns:a="{A}" '
    'uniqueId="urn:microsoft.com/office/officeart/2005/8/layout/default"><dgm:title val=""/><dgm:desc val=""/>'
    '<dgm:catLst><dgm:cat type="list" pri="400"/></dgm:catLst><dgm:layoutNode name="diagram"><dgm:alg type="snake"/>'
    '<dgm:shape/><dgm:presOf/></dgm:layoutNode></dgm:layoutDef>'
)
style = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dgm:styleDef xmlns:dgm="{DGM}" xmlns:a="{A}" '
    'uniqueId="urn:microsoft.com/office/officeart/2005/8/quickstyle/simple1"><dgm:title val=""/><dgm:desc val=""/>'
    '<dgm:styleLbl name="node1"/></dgm:styleDef>'
)
colors = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dgm:colorsDef xmlns:dgm="{DGM}" xmlns:a="{A}" '
    'uniqueId="urn:microsoft.com/office/officeart/2005/8/colors/accent1_2"><dgm:title val=""/><dgm:desc val=""/></dgm:colorsDef>'
)
frame = (
    '<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="10" name="Diagram 9"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>'
    f'<p:xfrm><a:off x="{FRAME[0]}" y="{FRAME[1]}"/><a:ext cx="{FRAME[2]}" cy="{FRAME[3]}"/></p:xfrm>'
    '<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram">'
    f'<dgm:relIds xmlns:dgm="{DGM}" r:dm="rIdDm" r:lo="rIdLo" r:qs="rIdQs" r:cs="rIdCs"/></a:graphicData></a:graphic></p:graphicFrame>'
)
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/"
rels = (
    f'<Relationship Id="rIdDm" Type="{REL}diagramData" Target="../diagrams/data1.xml"/>'
    f'<Relationship Id="rIdLo" Type="{REL}diagramLayout" Target="../diagrams/layout1.xml"/>'
    f'<Relationship Id="rIdQs" Type="{REL}diagramQuickStyle" Target="../diagrams/quickStyle1.xml"/>'
    f'<Relationship Id="rIdCs" Type="{REL}diagramColors" Target="../diagrams/colors1.xml"/>'
    '<Relationship Id="rIdDr" Type="http://schemas.microsoft.com/office/2007/relationships/diagramDrawing" Target="../diagrams/drawing1.xml"/>'
)
CT = "application/vnd.openxmlformats-officedocument.drawingml."
overrides = (
    f'<Override PartName="/ppt/diagrams/data1.xml" ContentType="{CT}diagramData+xml"/>'
    f'<Override PartName="/ppt/diagrams/layout1.xml" ContentType="{CT}diagramLayout+xml"/>'
    f'<Override PartName="/ppt/diagrams/quickStyle1.xml" ContentType="{CT}diagramStyle+xml"/>'
    f'<Override PartName="/ppt/diagrams/colors1.xml" ContentType="{CT}diagramColors+xml"/>'
    '<Override PartName="/ppt/diagrams/drawing1.xml" ContentType="application/vnd.ms-office.drawingml.diagramDrawing+xml"/>'
)

FIXED = (2026, 11, 15, 0, 0, 0)
src = zipfile.ZipFile(io.BytesIO(buf.getvalue()))
parts = {n: src.read(n) for n in src.namelist()}
parts["ppt/slides/slide2.xml"] = (
    parts["ppt/slides/slide2.xml"].decode().replace("</p:spTree>", frame + "</p:spTree>").encode()
)
parts["ppt/slides/_rels/slide2.xml.rels"] = (
    parts["ppt/slides/_rels/slide2.xml.rels"].decode().replace("</Relationships>", rels + "</Relationships>").encode()
)
parts["[Content_Types].xml"] = parts["[Content_Types].xml"].decode().replace("</Types>", overrides + "</Types>").encode()
parts["ppt/diagrams/data1.xml"] = data.encode()
parts["ppt/diagrams/layout1.xml"] = layout.encode()
parts["ppt/diagrams/quickStyle1.xml"] = style.encode()
parts["ppt/diagrams/colors1.xml"] = colors.encode()
parts["ppt/diagrams/drawing1.xml"] = drawing.encode()

# Slide 3: the same SmartArt (own parts) in a group with a caption.
fy, fh = FRAME[1], FRAME[3]
group = (
    '<p:grpSp><p:nvGrpSpPr><p:cNvPr id="20" name="Diagram group"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>'
    f'<p:grpSpPr><a:xfrm><a:off x="{FRAME[0]}" y="{fy}"/><a:ext cx="{FRAME[2]}" cy="{fh + EMU}"/>'
    f'<a:chOff x="{FRAME[0]}" y="{fy}"/><a:chExt cx="{FRAME[2]}" cy="{fh + EMU}"/></a:xfrm></p:grpSpPr>'
    + frame.replace('id="10" name="Diagram 9"', 'id="21" name="Diagram 21"')
    + '<p:sp><p:nvSpPr><p:cNvPr id="22" name="Caption"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>'
    f'<a:xfrm><a:off x="{FRAME[0]}" y="{fy + fh + EMU // 4}"/><a:ext cx="{FRAME[2]}" cy="{EMU // 2}"/></a:xfrm>'
    '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/>'
    '<a:p><a:r><a:rPr lang="en-US"/><a:t>Caption</a:t></a:r></a:p></p:txBody></p:sp></p:grpSp>'
)
parts["ppt/slides/slide3.xml"] = (
    parts["ppt/slides/slide3.xml"].decode().replace("</p:spTree>", group + "</p:spTree>").encode()
)
parts["ppt/slides/_rels/slide3.xml.rels"] = (
    parts["ppt/slides/_rels/slide3.xml.rels"].decode()
    .replace("</Relationships>", rels.replace("1.xml", "2.xml") + "</Relationships>").encode()
)
parts["[Content_Types].xml"] = (
    parts["[Content_Types].xml"].decode()
    .replace("</Types>", overrides.replace("1.xml", "2.xml") + "</Types>").encode()
)
for k in ["data", "layout", "quickStyle", "colors", "drawing"]:
    parts[f"ppt/diagrams/{k}2.xml"] = parts[f"ppt/diagrams/{k}1.xml"]

with zipfile.ZipFile(out / "groups.pptx", "w", zipfile.ZIP_DEFLATED) as z:
    for name in ["[Content_Types].xml"] + sorted(n for n in parts if n != "[Content_Types].xml"):
        info = zipfile.ZipInfo(name, date_time=FIXED)
        info.compress_type = zipfile.ZIP_DEFLATED
        z.writestr(info, parts[name])
print("groups ok")
