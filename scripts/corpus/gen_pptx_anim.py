#!/usr/bin/env python3
"""Write the Slides transitions and builds fixture (CC0, synthetic):
tests/corpus/pptx/anim.pptx. Read by crates/slides_engine/tests/anim.rs.

- Slide 1 "Fade": a fade transition, no builds.
- Slide 2 "Push": Push From Bottom, slow, advancing after 3 s. Builds:
  "Box A" fades in by itself after the transition (After Previous); click
  1 flies "Box B" in from the bottom, with "Box C" wiping in from the left
  (With Previous, 250 ms later), then "Box A" fades out (After Previous).
- Slide 3 "Pan": PowerPoint 2010's Pan (up) in `mc:AlternateContent`, a
  fade as the fallback; the body's three paragraphs come in one per click
  (Fade, by paragraph).
- Slide 4 "Split": Split vertical in; "Odd one" enters with an effect
  Kantor doesn't draw (Rise Up, preset 37) and leaves with another
  (Collapse, preset 21 exit).

Regenerate with
`uv run --with python-pptx==1.0.2 scripts/corpus/gen_pptx_anim.py tests/corpus/pptx`.
"""
import datetime
import io
import sys
import zipfile
from pathlib import Path

from pptx import Presentation
from pptx.dml.color import RGBColor
from pptx.enum.shapes import MSO_SHAPE
from pptx.util import Inches

out = Path(sys.argv[1] if len(sys.argv) > 1 else "tests/corpus/pptx")
prs = Presentation()
prs.slide_width, prs.slide_height = Inches(13.333), Inches(7.5)
TITLE_ONLY = prs.slide_layouts[5]


def box(slide, name, x, rgb):
    s = slide.shapes.add_shape(MSO_SHAPE.RECTANGLE, Inches(x), Inches(3), Inches(3), Inches(2))
    s.name = name
    s.text_frame.text = name
    s.fill.solid()
    s.fill.fore_color.rgb = RGBColor.from_string(rgb)
    return s.shape_id


s1 = prs.slides.add_slide(TITLE_ONLY)
s1.shapes.title.text = "Fade"
s2 = prs.slides.add_slide(TITLE_ONLY)
s2.shapes.title.text = "Push"
a = box(s2, "Box A", 1, "4472C4")
b = box(s2, "Box B", 5, "ED7D31")
c = box(s2, "Box C", 9, "70AD47")
s3 = prs.slides.add_slide(prs.slide_layouts[1])
s3.shapes.title.text = "Pan"
tf = s3.placeholders[1].text_frame
tf.text = "First"
tf.add_paragraph().text = "Second"
tf.add_paragraph().text = "Third"
body = s3.placeholders[1].shape_id
s4 = prs.slides.add_slide(TITLE_ONLY)
s4.shapes.title.text = "Split"
odd = box(s4, "Odd one", 5, "A5A5A5")

FIXED_DT = datetime.datetime(2026, 11, 24, 0, 0, 0)
prs.core_properties.created = FIXED_DT
prs.core_properties.modified = FIXED_DT
prs.core_properties.last_modified_by = "Kantor corpus"
prs.core_properties.author = "Kantor corpus"
buf = io.BytesIO()
prs.save(buf)


class Ids:
    """cTn ids, unique in a slide (1 and 2 are the root and main sequence)."""

    def __init__(self):
        self.n = 2

    def __call__(self):
        self.n += 1
        return self.n


def tgt(spid, para=None):
    if para is None:
        return f'<p:tgtEl><p:spTgt spid="{spid}"/></p:tgtEl>'
    return (
        f'<p:tgtEl><p:spTgt spid="{spid}"><p:txEl><p:pRg st="{para}" end="{para}"/></p:txEl>'
        "</p:spTgt></p:tgtEl>"
    )


def visibility(ids, spid, para, value, delay):
    return (
        f'<p:set><p:cBhvr><p:cTn id="{ids()}" dur="1" fill="hold"><p:stCondLst><p:cond delay="{delay}"/>'
        f"</p:stCondLst></p:cTn>{tgt(spid, para)}<p:attrNameLst><p:attrName>style.visibility</p:attrName>"
        f'</p:attrNameLst></p:cBhvr><p:to><p:strVal val="{value}"/></p:to></p:set>'
    )


def filt(ids, spid, para, way, f, dur):
    return (
        f'<p:animEffect transition="{way}" filter="{f}"><p:cBhvr><p:cTn id="{ids()}" dur="{dur}"/>'
        f"{tgt(spid, para)}</p:cBhvr></p:animEffect>"
    )


def fly_y(ids, spid, dur):
    return (
        f'<p:anim calcmode="lin" valueType="num"><p:cBhvr additive="base"><p:cTn id="{ids()}" dur="{dur}" fill="hold"/>'
        f"{tgt(spid)}<p:attrNameLst><p:attrName>ppt_y</p:attrName></p:attrNameLst></p:cBhvr><p:tavLst>"
        '<p:tav tm="0"><p:val><p:strVal val="1+#ppt_h/2"/></p:val></p:tav>'
        '<p:tav tm="100000"><p:val><p:strVal val="#ppt_y"/></p:val></p:tav></p:tavLst></p:anim>'
    )


def effect(ids, spid, cls, preset, sub, node, delay, body, para=None, grp=0):
    return (
        f'<p:par><p:cTn id="{ids()}" presetID="{preset}" presetClass="{cls}" presetSubtype="{sub}" fill="hold" '
        f'grpId="{grp}" nodeType="{node}"><p:stCondLst><p:cond delay="{delay}"/></p:stCondLst>'
        f"<p:childTnLst>{body}</p:childTnLst></p:cTn></p:par>"
    )


def entrance(ids, spid, preset, sub, node, delay, extra, para=None):
    return effect(ids, spid, "entr", preset, sub, node, delay, visibility(ids, spid, para, "visible", 0) + extra, para)


def exit_(ids, spid, preset, sub, node, delay, extra, dur):
    return effect(ids, spid, "exit", preset, sub, node, delay, extra + visibility(ids, spid, None, "hidden", dur), grp=1)


def click(ids, groups, auto=False):
    """One click step: `groups` are (delay, effects xml)."""
    conds = '<p:cond delay="indefinite"/>'
    if auto:
        conds += '<p:cond evt="onBegin" delay="0"><p:tn val="2"/></p:cond>'
    inner = "".join(
        f'<p:par><p:cTn id="{ids()}" fill="hold"><p:stCondLst><p:cond delay="{d}"/></p:stCondLst>'
        f"<p:childTnLst>{e}</p:childTnLst></p:cTn></p:par>"
        for d, e in groups
    )
    return (
        f'<p:par><p:cTn id="{ids()}" fill="hold"><p:stCondLst>{conds}</p:stCondLst>'
        f"<p:childTnLst>{inner}</p:childTnLst></p:cTn></p:par>"
    )


def timing(clicks, bld):
    return (
        '<p:timing><p:tnLst><p:par><p:cTn id="1" dur="indefinite" restart="never" nodeType="tmRoot"><p:childTnLst>'
        '<p:seq concurrent="1" nextAc="seek"><p:cTn id="2" dur="indefinite" nodeType="mainSeq"><p:childTnLst>'
        + "".join(clicks)
        + "</p:childTnLst></p:cTn>"
        '<p:prevCondLst><p:cond evt="onPrev" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:prevCondLst>'
        '<p:nextCondLst><p:cond evt="onNext" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:nextCondLst>'
        "</p:seq></p:childTnLst></p:cTn></p:par></p:tnLst>"
        f"<p:bldLst>{bld}</p:bldLst></p:timing>"
    )


# Slide 2.
ids = Ids()
step0 = click(ids, [(0, entrance(ids, a, 10, 0, "afterEffect", 0, filt(ids, a, None, "in", "fade", 500)))], auto=True)
step1 = click(
    ids,
    [
        (
            0,
            entrance(ids, b, 2, 4, "clickEffect", 0, fly_y(ids, b, 500))
            + entrance(ids, c, 22, 8, "withEffect", 250, filt(ids, c, None, "in", "wipe(right)", 500)),
        ),
        (750, exit_(ids, a, 10, 0, "afterEffect", 0, filt(ids, a, None, "out", "fade", 500), 499)),
    ],
)
bld = "".join(f'<p:bldP spid="{s}" grpId="{g}" animBg="1"/>' for s, g in [(a, 0), (b, 0), (c, 0), (a, 1)])
slide2 = (
    '<p:transition spd="slow" advTm="3000"><p:push dir="u"/></p:transition>' + timing([step0, step1], bld)
)

# Slide 3.
ids = Ids()
paras = [
    click(ids, [(0, entrance(ids, body, 10, 0, "clickEffect", 0, filt(ids, body, p, "in", "fade", 500), para=p))])
    for p in range(3)
]
slide3 = (
    '<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">'
    '<mc:Choice xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main" Requires="p14">'
    '<p:transition spd="slow" p14:dur="1250"><p14:pan dir="u"/></p:transition></mc:Choice>'
    '<mc:Fallback><p:transition spd="slow"><p:fade/></p:transition></mc:Fallback></mc:AlternateContent>'
    + timing(paras, f'<p:bldP spid="{body}" grpId="0" build="p"/>')
)

# Slide 4.
ids = Ids()
odd_in = click(ids, [(0, entrance(ids, odd, 37, 0, "clickEffect", 0, filt(ids, odd, None, "in", "fade", 1000)))])
odd_out = click(ids, [(0, exit_(ids, odd, 21, 0, "clickEffect", 0, "", 500))])
slide4 = (
    '<p:transition><p:split orient="vert" dir="in"/></p:transition>'
    + timing([odd_in, odd_out], f'<p:bldP spid="{odd}" grpId="0" animBg="1"/><p:bldP spid="{odd}" grpId="1" animBg="1"/>')
)

FIXED = (2026, 11, 24, 0, 0, 0)
src = zipfile.ZipFile(io.BytesIO(buf.getvalue()))
parts = {n: src.read(n) for n in src.namelist()}
for n, tail in [(1, '<p:transition><p:fade/></p:transition>'), (2, slide2), (3, slide3), (4, slide4)]:
    name = f"ppt/slides/slide{n}.xml"
    xml = parts[name].decode()
    assert xml.count("</p:clrMapOvr>") == 1, name
    parts[name] = xml.replace("</p:clrMapOvr>", "</p:clrMapOvr>" + tail).encode()

with zipfile.ZipFile(out / "anim.pptx", "w", zipfile.ZIP_DEFLATED) as z:
    for name in ["[Content_Types].xml"] + sorted(n for n in parts if n != "[Content_Types].xml"):
        info = zipfile.ZipInfo(name, date_time=FIXED)
        info.compress_type = zipfile.ZIP_DEFLATED
        z.writestr(info, parts[name])
print("anim ok")
