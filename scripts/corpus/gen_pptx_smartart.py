#!/usr/bin/env python3
"""Write the Slides SmartArt layouts fixture (CC0, synthetic):
tests/corpus/pptx/smartart.pptx. Read by crates/slides_engine/tests/smartart_layout.rs.

Hand-written SmartArt (data model with its connection and presentation
items, layout definition, style, colours and the cached drawing), one per
slide, in the layout families Kantor lays out itself:

- Slide 1 "Block list": Basic Block List (`layout/default`), items A-E.
- Slide 2 "Bullet list": Vertical Bullet List (`layout/vList2`), "North"
  (sub-items "Oslo", "Bergen") and "South" (sub-item "Rome"), each a header
  shape and a text shape with its sub-items.
- Slide 3 "Process": Basic Process (`layout/process1`), "Plan", "Build",
  "Ship" with arrows between them.
- Slide 4 "Cycle": Basic Cycle (`layout/cycle2`), "Spring", "Summer",
  "Autumn", "Winter" round a circle, arrows between them.
- Slide 5 "Other layout": a layout Kantor doesn't lay out
  (`layout/pyramid1`, two items), drawn from its saved drawing.
- Slides 6-8: the hierarchies, each the tree CEO → (CFO, CTO → (Dev, Ops)):
  "Org chart" (`layout/orgChart1`, a box per item), "Hierarchy"
  (`layout/hierarchy1`, a background and a text box per item, as
  PowerPoint draws it) and "Horizontal hierarchy" (`layout/hierarchy2`),
  with elbow connectors drawn for the parent transitions.
- Slide 9 "Labeled hierarchy": Labeled Hierarchy (`layout/hierarchy6`), the
  same tree, then the level labels "Board", "Managers", "Staff" (the
  document's later top-level items), each a band across the diagram with
  its text at the left, drawn behind the tree.

Regenerate with
`uv run --with python-pptx==1.0.2 scripts/corpus/gen_pptx_smartart.py tests/corpus/pptx`.
"""
import io
import math
import sys
import zipfile
from pathlib import Path

from pptx import Presentation
from pptx.util import Inches

out = Path(sys.argv[1] if len(sys.argv) > 1 else "tests/corpus/pptx")
prs = Presentation()
prs.slide_width, prs.slide_height = Inches(13.333), Inches(7.5)
TITLE_ONLY = prs.slide_layouts[5]
EMU = 914400
FRAME = (EMU, int(1.5 * EMU), 10 * EMU, 5 * EMU)
DGM = "http://schemas.openxmlformats.org/drawingml/2006/diagram"
DSP = "http://schemas.microsoft.com/office/drawing/2008/diagram"
A = "http://schemas.openxmlformats.org/drawingml/2006/main"
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/"
CT = "application/vnd.openxmlformats-officedocument.drawingml."


def gid(kind, n):
    return "{%s%07X-0000-4000-8000-000000000000}" % (kind, n)


def paras(texts, rpr='lang="en-US"'):
    out = ""
    for t in texts:
        if t:
            out += f"<a:p><a:r><a:rPr {rpr}/><a:t>{t}</a:t></a:r></a:p>"
        else:
            out += f"<a:p><a:endParaRPr {rpr}/></a:p>"
    return out


def text_body(texts):
    return f"<dgm:t><a:bodyPr/><a:lstStyle/>{paras(texts)}</dgm:t>"


def sp(model, prst, box, texts, rot=0, fill="accent1", ins=38100, font="lt1"):
    x, y, cx, cy = (int(v) for v in box)
    r = f' rot="{rot}"' if rot else ""
    return (
        f'<dsp:sp modelId="{model}"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr>'
        f'<dsp:spPr><a:xfrm{r}><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
        f'<a:prstGeom prst="{prst}"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="{fill}"/></a:solidFill>'
        '<a:ln w="12700"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></dsp:spPr>'
        '<dsp:style><a:lnRef idx="2"><a:scrgbClr r="0" g="0" b="0"/></a:lnRef><a:fillRef idx="1"><a:scrgbClr r="0" g="0" b="0"/></a:fillRef>'
        f'<a:effectRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="{font}"/></a:fontRef></dsp:style>'
        f'<dsp:txBody><a:bodyPr spcFirstLastPara="0" vert="horz" wrap="square" lIns="{ins}" tIns="{ins}" rIns="{ins}" bIns="{ins}" '
        'numCol="1" spcCol="1270" anchor="ctr" anchorCtr="0"><a:noAutofit/></a:bodyPr><a:lstStyle/>'
        + paras(texts, 'lang="en-US" sz="2000" kern="1200"')
        + f'</dsp:txBody><dsp:txXfrm{r}><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></dsp:txXfrm></dsp:sp>'
    )


def smartart(lo, cat, items, shapes_of):
    """items: [(text, [sub-item texts])]; shapes_of(ids) -> (pres points, drawing shapes)."""
    doc = gid("D", 0)
    pts = (
        f'<dgm:pt modelId="{doc}" type="doc"><dgm:prSet loTypeId="urn:microsoft.com/office/officeart/2005/8/layout/{lo}" '
        f'loCatId="{cat}" qsTypeId="urn:microsoft.com/office/officeart/2005/8/quickstyle/simple1" qsCatId="simple" '
        'csTypeId="urn:microsoft.com/office/officeart/2005/8/colors/accent1_2" csCatId="accent1" phldr="1"/><dgm:spPr/>'
        f"{text_body([''])}</dgm:pt>"
    )
    cxns = ""
    ids = []
    n = 0

    def add(text, parent, ordinal):
        nonlocal pts, cxns, n
        n += 1
        node, par, sib, cxn = gid("A", n), gid("B", n), gid("C", n), gid("E", n)
        pts += f'<dgm:pt modelId="{node}"><dgm:prSet phldrT="[Text]"/><dgm:spPr/>{text_body([text])}</dgm:pt>'
        for tid, ty in [(par, "parTrans"), (sib, "sibTrans")]:
            pts += f'<dgm:pt modelId="{tid}" type="{ty}" cxnId="{cxn}"><dgm:prSet/><dgm:spPr/>{text_body([""])}</dgm:pt>'
        cxns += (
            f'<dgm:cxn modelId="{cxn}" srcId="{parent}" destId="{node}" srcOrd="{ordinal}" destOrd="0" '
            f'parTransId="{par}" sibTransId="{sib}"/>'
        )
        return node, sib

    for k, (text, subs) in enumerate(items):
        node, sib = add(text, doc, k)
        kids = [add(t, node, j)[0] for j, t in enumerate(subs)]
        ids.append((node, sib, kids, text, subs))
    pres, shapes = shapes_of(ids)
    for m, (pid, assoc, name, idx, cnt) in enumerate(pres):
        pts += (
            f'<dgm:pt modelId="{pid}" type="pres"><dgm:prSet presAssocID="{assoc}" presName="{name}" '
            f'presStyleLbl="node1" presStyleIdx="{idx}" presStyleCnt="{cnt}"/><dgm:spPr/></dgm:pt>'
        )
        cxns += f'<dgm:cxn modelId="{gid("F", m)}" type="presOf" srcId="{assoc}" destId="{pid}" srcOrd="0" destOrd="0"/>'
    data = (
        f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dgm:dataModel xmlns:dgm="{DGM}" xmlns:a="{A}">'
        f"<dgm:ptLst>{pts}</dgm:ptLst><dgm:cxnLst>{cxns}</dgm:cxnLst><dgm:bg/><dgm:whole/>"
        f'<dgm:extLst><a:ext uri="http://schemas.microsoft.com/office/drawing/2008/diagram"><dsp:dataModelExt xmlns:dsp="{DSP}" '
        'relId="rIdDr" minVer="http://schemas.openxmlformats.org/drawingml/2006/diagram"/></a:ext></dgm:extLst></dgm:dataModel>'
    )
    drawing = (
        f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dsp:drawing xmlns:dgm="{DGM}" xmlns:dsp="{DSP}" xmlns:a="{A}">'
        '<dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/>'
        f"{''.join(shapes)}</dsp:spTree></dsp:drawing>"
    )
    alg = {"default": "snake", "vList2": "lin", "process1": "lin", "cycle2": "cycle"}.get(lo, "hierRoot")
    layout = (
        f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dgm:layoutDef xmlns:dgm="{DGM}" xmlns:a="{A}" '
        f'uniqueId="urn:microsoft.com/office/officeart/2005/8/layout/{lo}"><dgm:title val=""/><dgm:desc val=""/>'
        f'<dgm:catLst><dgm:cat type="{cat}" pri="400"/></dgm:catLst><dgm:layoutNode name="diagram"><dgm:alg type="{alg}"/>'
        "<dgm:shape/><dgm:presOf/></dgm:layoutNode></dgm:layoutDef>"
    )
    return data, drawing, layout


W, H = FRAME[2], FRAME[3]


def blocks(ids):
    pres, shapes = [], []
    w = W / 3.2
    h = w * 0.6
    for k, (node, _sib, _kids, text, _subs) in enumerate(ids):
        pid = gid("9", k)
        pres.append((pid, node, "node", k, len(ids)))
        row, col = divmod(k, 3)
        shapes.append(sp(pid, "rect", (col * w * 1.1, row * h * 1.2, w, h), [text]))
    return pres, shapes


def bullets(ids):
    pres, shapes = [], []
    y = 0
    for k, (node, _sib, kids, text, subs) in enumerate(ids):
        p1, p2 = gid("9", 2 * k), gid("9", 2 * k + 1)
        pres.append((p1, node, "parentText", k, len(ids)))
        shapes.append(sp(p1, "roundRect", (0, y, W, H * 0.18), [text]))
        y += H * 0.18
        if kids:
            pres.append((p2, node, "childText", k, len(ids)))
            shapes.append(sp(p2, "rect", (0, y, W, H * 0.25), subs, fill="lt1", font="tx1"))
            y += H * 0.25
        y += H * 0.03
    return pres, shapes


def process(ids):
    pres, shapes = [], []
    n = len(ids)
    w = W / (n + 0.45 * (n - 1))
    h = w * 0.6
    y = (H - h) / 2
    for k, (node, sib, _kids, text, _subs) in enumerate(ids):
        pid = gid("9", 2 * k)
        pres.append((pid, node, "node", k, n))
        x = k * w * 1.45
        shapes.append(sp(pid, "roundRect", (x, y, w, h), [text]))
        if k + 1 < n:
            aid = gid("9", 2 * k + 1)
            pres.append((aid, sib, "sibTrans", k, n - 1))
            shapes.append(sp(aid, "rightArrow", (x + w * 1.1, y + h / 2 - w * 0.15, w * 0.25, w * 0.3), [""], fill="accent1"))
    return pres, shapes


def cycle(ids):
    pres, shapes = [], []
    n = len(ids)
    m = min(W, H)
    k_ = 1.5 * math.sin(math.pi / n)
    r = (m / 2) / (1 + k_ / 2)
    d = k_ * r
    cx, cy = W / 2, H / 2
    for k, (node, sib, _kids, text, _subs) in enumerate(ids):
        a = math.radians(-90 + 360 * k / n)
        pid = gid("9", 2 * k)
        pres.append((pid, node, "node", k, n))
        shapes.append(sp(pid, "ellipse", (cx + r * math.cos(a) - d / 2, cy + r * math.sin(a) - d / 2, d, d), [text]))
        am = math.radians(-90 + 360 * (k + 0.5) / n)
        aid = gid("9", 2 * k + 1)
        pres.append((aid, sib, "sibTrans", k, n))
        s = d * 0.3
        shapes.append(sp(aid, "rightArrow", (cx + r * math.cos(am) - s / 2, cy + r * math.sin(am) - s / 2, s, s), [""],
                         rot=int(((360 * (k + 0.5) / n) % 360) * 60000)))
    return pres, shapes


def pyramid(ids):
    pres, shapes = [], []
    for k, (node, _sib, _kids, text, _subs) in enumerate(ids):
        pid = gid("9", k)
        pres.append((pid, node, "hierRoot", k, len(ids)))
        shapes.append(sp(pid, "rect", (W * 0.3, k * H * 0.5, W * 0.4, H * 0.4), [text]))
    return pres, shapes


def connector(model, pts):
    """An elbow line through `pts` (frame EMUs) as PowerPoint draws a
    hierarchy's connectors: a custGeom path without fill."""
    xs, ys = [int(p[0]) for p in pts], [int(p[1]) for p in pts]
    x0, y0 = min(xs), min(ys)
    w, h = max(max(xs) - x0, 1), max(max(ys) - y0, 1)
    path = "".join(
        f'<a:{"moveTo" if i == 0 else "lnTo"}><a:pt x="{x - x0}" y="{y - y0}"/></a:{"moveTo" if i == 0 else "lnTo"}>'
        for i, (x, y) in enumerate(zip(xs, ys))
    )
    return (
        f'<dsp:sp modelId="{model}"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr>'
        f'<dsp:spPr><a:xfrm><a:off x="{x0}" y="{y0}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm>'
        '<a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l="0" t="0" r="0" b="0"/>'
        f'<a:pathLst><a:path w="{w}" h="{h}" fill="none">{path}</a:path></a:pathLst></a:custGeom>'
        '<a:noFill/><a:ln w="12700"><a:solidFill><a:schemeClr val="accent1"><a:shade val="60000"/></a:schemeClr></a:solidFill></a:ln></dsp:spPr>'
        '<dsp:style><a:lnRef idx="2"><a:scrgbClr r="0" g="0" b="0"/></a:lnRef><a:fillRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:fillRef>'
        '<a:effectRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:effectRef><a:fontRef idx="minor"/></dsp:style></dsp:sp>'
    )


TREE = ("CEO", [("CFO", []), ("CTO", [("Dev", []), ("Ops", [])])])


LABELS = ["Board", "Managers", "Staff"]


def tree_smartart(lo, style):
    """A hierarchy SmartArt of TREE: (data, drawing, layout). `style`:
    "org" (one box per item), "hier" (background + text box), "horiz"
    (left to right), "label" (LABELS as bands behind the tree)."""
    doc = gid("D", 0)
    pts = (
        f'<dgm:pt modelId="{doc}" type="doc"><dgm:prSet loTypeId="urn:microsoft.com/office/officeart/2005/8/layout/{lo}" '
        'loCatId="hierarchy" qsTypeId="urn:microsoft.com/office/officeart/2005/8/quickstyle/simple1" qsCatId="simple" '
        'csTypeId="urn:microsoft.com/office/officeart/2005/8/colors/accent1_2" csCatId="accent1" phldr="1"/><dgm:spPr/>'
        f"{text_body([''])}</dgm:pt>"
    )
    cxns = ""
    items = []  # (node, par, text, depth, parent index)
    n = 0

    def add(item, parent, ordinal, depth, pidx):
        nonlocal pts, cxns, n
        text, kids = item
        n += 1
        node, par, sib, cxn = gid("A", n), gid("B", n), gid("C", n), gid("E", n)
        pts += f'<dgm:pt modelId="{node}"><dgm:prSet phldrT="[Text]"/><dgm:spPr/>{text_body([text])}</dgm:pt>'
        for tid, ty in [(par, "parTrans"), (sib, "sibTrans")]:
            pts += f'<dgm:pt modelId="{tid}" type="{ty}" cxnId="{cxn}"><dgm:prSet/><dgm:spPr/>{text_body([""])}</dgm:pt>'
        cxns += (
            f'<dgm:cxn modelId="{cxn}" srcId="{parent}" destId="{node}" srcOrd="{ordinal}" destOrd="0" '
            f'parTransId="{par}" sibTransId="{sib}"/>'
        )
        me = len(items)
        items.append([node, par, text, depth, pidx, 0.0])
        for j, k in enumerate(kids):
            add(k, node, j, depth + 1, me)

    add(TREE, doc, 0, 0, None)
    labels = []  # (node, text)
    if style == "label":
        for j, text in enumerate(LABELS):
            n += 1
            node, par, sib, cxn = gid("A", n), gid("B", n), gid("C", n), gid("E", n)
            pts += f'<dgm:pt modelId="{node}"><dgm:prSet phldrT="[Text]"/><dgm:spPr/>{text_body([text])}</dgm:pt>'
            for tid, ty in [(par, "parTrans"), (sib, "sibTrans")]:
                pts += f'<dgm:pt modelId="{tid}" type="{ty}" cxnId="{cxn}"><dgm:prSet/><dgm:spPr/>{text_body([""])}</dgm:pt>'
            cxns += (
                f'<dgm:cxn modelId="{cxn}" srcId="{doc}" destId="{node}" srcOrd="{j + 1}" destOrd="0" '
                f'parTransId="{par}" sibTransId="{sib}"/>'
            )
            labels.append((node, text))
    # Tidy tree: leaves side by side, parents over their children.
    leaves = [i for i in range(len(items)) if not any(it[4] == i for it in items)]
    for slot, i in enumerate(leaves):
        items[i][5] = slot
    for i in reversed(range(len(items))):
        kids = [k for k in range(len(items)) if items[k][4] == i]
        if kids:
            items[i][5] = sum(items[k][5] for k in kids) / len(kids)
    nslots, nlev = len(leaves), max(it[3] for it in items) + 1
    horiz = style == "horiz"
    # Labeled: the tree right of a label column, a row per level or label.
    lw = W * 0.2 if style == "label" else 0
    tw = W - lw
    if style == "label":
        nlev = max(nlev, len(labels))
        bw = min(tw / (nslots + 0.25 * (nslots - 1)), H * 0.92 / (nlev + 0.5 * (nlev - 1)) / 0.6)
        bh = bw * 0.6
    elif horiz:
        bh = min(H / (nslots + 0.25 * (nslots - 1)), W / (nlev + 0.5 * (nlev - 1)) * 0.6)
        bw = bh / 0.6
    else:
        bw = min(W / (nslots + 0.25 * (nslots - 1)), H / (nlev + 0.5 * (nlev - 1)) / 0.6)
        bh = bw * 0.6

    def box(i):
        slot, depth = items[i][5], items[i][3]
        if horiz:
            return (depth * bw * 1.5, slot * bh * 1.25, bw, bh)
        top = bh * 0.2 if style == "label" else 0
        return (lw + slot * bw * 1.25, top + depth * bh * 1.5, bw, bh)

    pres, shapes = [], []
    for j, (node, text) in enumerate(labels):
        y = j * bh * 1.5
        band, h = gid("7", j), bh * 1.4
        pres.append((band, node, "rootComposite", j, len(labels)))
        sh = sp(band, "roundRect", (0, y, W, h), [text], fill="accent1", font="tx1")
        # The label's text sits at the band's left.
        sh = sh.replace(f'<dsp:txXfrm><a:off x="0" y="{int(y)}"/><a:ext cx="{int(W)}" cy="{int(h)}"/></dsp:txXfrm>',
                        f'<dsp:txXfrm><a:off x="0" y="{int(y)}"/><a:ext cx="{int(lw)}" cy="{int(h)}"/></dsp:txXfrm>')
        sh = sh.replace('<a:solidFill><a:schemeClr val="accent1"/></a:solidFill>',
                        '<a:solidFill><a:schemeClr val="accent1"><a:tint val="40000"/><a:alpha val="90000"/></a:schemeClr></a:solidFill>', 1)
        shapes.append(sh)
    for i, (node, par, text, _depth, pidx, _slot) in enumerate(items):
        x, y, w, h = box(i)
        if pidx is not None:
            px, py, pw, ph = box(pidx)
            if horiz:
                mx = px + pw + (x - px - pw) / 2
                line = [(px + pw, py + ph / 2), (mx, py + ph / 2), (mx, y + h / 2), (x, y + h / 2)]
            else:
                my = py + ph + (y - py - ph) / 2
                line = [(px + pw / 2, py + ph), (px + pw / 2, my), (x + w / 2, my), (x + w / 2, y)]
            cid = gid("8", i)
            pres.append((cid, par, "Name" + str(i), i, len(items)))
            shapes.append(connector(cid, line))
        if style == "hier":
            bg, tx = gid("9", 2 * i), gid("9", 2 * i + 1)
            pres.append((bg, node, "background", i, len(items)))
            pres.append((tx, node, "text", i, len(items)))
            shapes.append(sp(bg, "roundRect", (x, y, w, h), [""]))
            shapes.append(sp(tx, "roundRect", (x + w * 0.1, y + h * 0.1, w * 0.9, h * 0.9), [text], fill="lt1", font="tx1"))
        else:
            pid = gid("9", i)
            pres.append((pid, node, "rootText" if pidx is None else "hierChild", i, len(items)))
            shapes.append(sp(pid, "roundRect" if style == "label" else "rect", (x, y, w, h), [text]))
    for m, (pid, assoc, name, idx, cnt) in enumerate(pres):
        pts += (
            f'<dgm:pt modelId="{pid}" type="pres"><dgm:prSet presAssocID="{assoc}" presName="{name}" '
            f'presStyleLbl="node1" presStyleIdx="{idx}" presStyleCnt="{cnt}"/><dgm:spPr/></dgm:pt>'
        )
        cxns += f'<dgm:cxn modelId="{gid("F", m)}" type="presOf" srcId="{assoc}" destId="{pid}" srcOrd="0" destOrd="0"/>'
    data = (
        f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dgm:dataModel xmlns:dgm="{DGM}" xmlns:a="{A}">'
        f"<dgm:ptLst>{pts}</dgm:ptLst><dgm:cxnLst>{cxns}</dgm:cxnLst><dgm:bg/><dgm:whole/>"
        f'<dgm:extLst><a:ext uri="http://schemas.microsoft.com/office/drawing/2008/diagram"><dsp:dataModelExt xmlns:dsp="{DSP}" '
        'relId="rIdDr" minVer="http://schemas.openxmlformats.org/drawingml/2006/diagram"/></a:ext></dgm:extLst></dgm:dataModel>'
    )
    drawing = (
        f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dsp:drawing xmlns:dgm="{DGM}" xmlns:dsp="{DSP}" xmlns:a="{A}">'
        '<dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/>'
        f"{''.join(shapes)}</dsp:spTree></dsp:drawing>"
    )
    alg = "hierRoot" if lo != "hierarchy2" else "hierChild"
    layout = (
        f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dgm:layoutDef xmlns:dgm="{DGM}" xmlns:a="{A}" '
        f'uniqueId="urn:microsoft.com/office/officeart/2005/8/layout/{lo}"><dgm:title val=""/><dgm:desc val=""/>'
        f'<dgm:catLst><dgm:cat type="hierarchy" pri="400"/></dgm:catLst><dgm:layoutNode name="diagram"><dgm:alg type="{alg}"/>'
        "<dgm:shape/><dgm:presOf/></dgm:layoutNode></dgm:layoutDef>"
    )
    return data, drawing, layout


SLIDES = [
    ("Block list", "default", "list", [(t, []) for t in "ABCDE"], blocks),
    ("Bullet list", "vList2", "list", [("North", ["Oslo", "Bergen"]), ("South", ["Rome"])], bullets),
    ("Process", "process1", "process", [("Plan", []), ("Build", []), ("Ship", [])], process),
    ("Cycle", "cycle2", "cycle", [("Spring", []), ("Summer", []), ("Autumn", []), ("Winter", [])], cycle),
    ("Other layout", "pyramid1", "pyramid", [("Boss", []), ("Team", [])], pyramid),
    ("Org chart", "orgChart1", "org", None, None),
    ("Hierarchy", "hierarchy1", "hier", None, None),
    ("Horizontal hierarchy", "hierarchy2", "horiz", None, None),
    ("Labeled hierarchy", "hierarchy6", "label", None, None),
]

for title, *_ in SLIDES:
    s = prs.slides.add_slide(TITLE_ONLY)
    s.shapes.title.text = title

buf = io.BytesIO()
prs.save(buf)
src = zipfile.ZipFile(io.BytesIO(buf.getvalue()))
parts = {n: src.read(n) for n in src.namelist()}
style = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dgm:styleDef xmlns:dgm="{DGM}" xmlns:a="{A}" '
    'uniqueId="urn:microsoft.com/office/officeart/2005/8/quickstyle/simple1"><dgm:title val=""/><dgm:desc val=""/>'
    '<dgm:styleLbl name="node1"/></dgm:styleDef>'
)
colors = (
    f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dgm:colorsDef xmlns:dgm="{DGM}" xmlns:a="{A}" '
    'uniqueId="urn:microsoft.com/office/officeart/2005/8/colors/accent1_2"><dgm:title val=""/><dgm:desc val=""/></dgm:colorsDef>'
)
ct = parts["[Content_Types].xml"].decode()
for k, (title, lo, cat, items, shapes_of) in enumerate(SLIDES):
    i = k + 1
    if shapes_of is None:
        data, drawing, layout = tree_smartart(lo, cat)
    else:
        data, drawing, layout = smartart(lo, cat, items, shapes_of)
    frame = (
        f'<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="10" name="Diagram {i}"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>'
        f'<p:xfrm><a:off x="{FRAME[0]}" y="{FRAME[1]}"/><a:ext cx="{FRAME[2]}" cy="{FRAME[3]}"/></p:xfrm>'
        '<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram">'
        f'<dgm:relIds xmlns:dgm="{DGM}" r:dm="rIdDm" r:lo="rIdLo" r:qs="rIdQs" r:cs="rIdCs"/></a:graphicData></a:graphic></p:graphicFrame>'
    )
    rels = (
        f'<Relationship Id="rIdDm" Type="{REL}diagramData" Target="../diagrams/data{i}.xml"/>'
        f'<Relationship Id="rIdLo" Type="{REL}diagramLayout" Target="../diagrams/layout{i}.xml"/>'
        f'<Relationship Id="rIdQs" Type="{REL}diagramQuickStyle" Target="../diagrams/quickStyle{i}.xml"/>'
        f'<Relationship Id="rIdCs" Type="{REL}diagramColors" Target="../diagrams/colors{i}.xml"/>'
        '<Relationship Id="rIdDr" Type="http://schemas.microsoft.com/office/2007/relationships/diagramDrawing" '
        f'Target="../diagrams/drawing{i}.xml"/>'
    )
    ct = ct.replace(
        "</Types>",
        f'<Override PartName="/ppt/diagrams/data{i}.xml" ContentType="{CT}diagramData+xml"/>'
        f'<Override PartName="/ppt/diagrams/layout{i}.xml" ContentType="{CT}diagramLayout+xml"/>'
        f'<Override PartName="/ppt/diagrams/quickStyle{i}.xml" ContentType="{CT}diagramStyle+xml"/>'
        f'<Override PartName="/ppt/diagrams/colors{i}.xml" ContentType="{CT}diagramColors+xml"/>'
        f'<Override PartName="/ppt/diagrams/drawing{i}.xml" ContentType="application/vnd.ms-office.drawingml.diagramDrawing+xml"/>'
        "</Types>",
    )
    name = f"ppt/slides/slide{i}.xml"
    parts[name] = parts[name].decode().replace("</p:spTree>", frame + "</p:spTree>").encode()
    rname = f"ppt/slides/_rels/slide{i}.xml.rels"
    parts[rname] = parts[rname].decode().replace("</Relationships>", rels + "</Relationships>").encode()
    parts[f"ppt/diagrams/data{i}.xml"] = data.encode()
    parts[f"ppt/diagrams/layout{i}.xml"] = layout.encode()
    parts[f"ppt/diagrams/quickStyle{i}.xml"] = style.encode()
    parts[f"ppt/diagrams/colors{i}.xml"] = colors.encode()
    parts[f"ppt/diagrams/drawing{i}.xml"] = drawing.encode()
parts["[Content_Types].xml"] = ct.encode()

FIXED = (2026, 11, 16, 0, 0, 0)
with zipfile.ZipFile(out / "smartart.pptx", "w", zipfile.ZIP_DEFLATED) as z:
    for name in ["[Content_Types].xml"] + sorted(n for n in parts if n != "[Content_Types].xml"):
        info = zipfile.ZipInfo(name, date_time=FIXED)
        info.compress_type = zipfile.ZIP_DEFLATED
        z.writestr(info, parts[name])
print("smartart ok")
