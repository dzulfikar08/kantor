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
  (`layout/funnel1`, two items), drawn from its saved drawing.
- Slides 6-8: the hierarchies, each the tree CEO → (CFO, CTO → (Dev, Ops)):
  "Org chart" (`layout/orgChart1`, a box per item), "Hierarchy"
  (`layout/hierarchy1`, a background and a text box per item, as
  PowerPoint draws it) and "Horizontal hierarchy" (`layout/hierarchy2`),
  with elbow connectors drawn for the parent transitions.
- Slide 9 "Labeled hierarchy": Labeled Hierarchy (`layout/hierarchy6`), the
  same tree, then the level labels "Board", "Managers", "Staff" (the
  document's later top-level items), each a band across the diagram with
  its text at the left, drawn behind the tree.
- Slide 10 "Picture list": Horizontal Picture List (`layout/pList2`),
  "Lake", "Forest", "Desert" on a background, each a picture placeholder
  over its text; Lake's holds a picture (a small generated PNG).
- Slide 11 "Org chart layouts": Organization Chart (`layout/orgChart1`)
  CEO with an assistant ("Aide", `type="asst"`) and reports "Sales"
  (Left Hanging: `hierBranch val="l"` on its `hierRoot2` presentation
  item, over "North" and "South"), "Ops" (Both: `hang`, over "Plant",
  "Fleet", "Depot") and "Labs" (PowerPoint's initial layout, over
  "Research"), drawn as PowerPoint places them.
- Slides 12-17: picture layouts, "Lake" (sub-items "Calm", "Deep"),
  "Forest" (sub-item "Green") and "Desert", each with a picture
  placeholder (Lake's holding the PNG): Picture Accent List
  (`layout/hList2`), Bending Picture Accent List (`layout/bList2`, its text
  in a `parentText` shape over the `parentRect` bar), Vertical Picture List
  (`layout/vList4`), Picture Caption List (`layout/pList1`), Titled Picture
  Blocks and Picture Grid (`2008/layout/TitledPictureBlocks`,
  `2008/layout/PictureGrid`).

Regenerate with
`uv run --with python-pptx==1.0.2 scripts/corpus/gen_pptx_smartart.py tests/corpus/pptx`.
"""
import io
import math
import struct
import sys
import zipfile
import zlib
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
    lo_id = lo if "/" in lo else "2005/8/layout/" + lo
    pts = (
        f'<dgm:pt modelId="{doc}" type="doc"><dgm:prSet loTypeId="urn:microsoft.com/office/officeart/{lo_id}" '
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
        f'uniqueId="urn:microsoft.com/office/officeart/{lo_id}"><dgm:title val=""/><dgm:desc val=""/>'
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


def png(w, h):
    """A small blue-to-green gradient PNG (stdlib only, deterministic)."""
    rows = b"".join(
        b"\x00" + b"".join(bytes((40, 90 + 120 * x // w, 200 - 100 * y // h)) for x in range(w)) for y in range(h)
    )

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows, 9))
        + chunk(b"IEND", b"")
    )


def pictures(ids):
    """pList2: a background for the document item, then per item its
    picture placeholder (the first holding a picture) and its text."""
    pres, shapes = [], []
    n = len(ids)
    bg = gid("9", 99)
    pres.append((bg, gid("D", 0), "bkgdShp", 0, 1))
    shapes.append(sp(bg, "roundRect", (0, 0, W, H), [""], fill="accent1"))
    m = W * 0.03
    cw = (W - 2 * m) / (n + 0.1 * (n - 1))
    for k, (node, _sib, _kids, text, _subs) in enumerate(ids):
        x = m + k * cw * 1.1
        img, txt = gid("9", 2 * k), gid("9", 2 * k + 1)
        pres.append((img, node, "imagNode", k, n))
        pic = sp(img, "roundRect", (x + cw * 0.05, H * 0.08, cw * 0.9, H * 0.48), [""], fill="accent1")
        if k == 0:
            pic = pic.replace(
                '<a:solidFill><a:schemeClr val="accent1"/></a:solidFill><a:ln',
                f'<a:blipFill><a:blip xmlns:r="{REL[:-1]}" r:embed="rIdImg1"/>'
                '<a:stretch><a:fillRect/></a:stretch></a:blipFill><a:ln',
                1,
            )
        shapes.append(pic)
        pres.append((txt, node, "node", k, n))
        shapes.append(sp(txt, "rect", (x, H * 0.58, cw, H * 0.36), [text]))
    return pres, shapes


def grid(n, aspect, gx, gy):
    """n cells `aspect` times as tall as wide in rows (PowerPoint's snake),
    gx/gy cell widths apart, the biggest that fit, the last row centred."""
    best = (0.0, 1)
    for c in range(1, n + 1):
        rows = -(-n // c)
        cw = min(W / (c + gx * (c - 1)), H / (aspect * rows + gy * (rows - 1)))
        if cw > best[0] + 1e-6:
            best = (cw, c)
    cw, c = best
    ch = cw * aspect
    rows = -(-n // c)
    y0 = (H - (rows * ch + (rows - 1) * gy * cw)) / 2
    out = []
    for i in range(n):
        row, col = divmod(i, c)
        in_row = n - row * c if row + 1 == rows else c
        x = (W - (in_row * cw + (in_row - 1) * gx * cw)) / 2 + col * cw * (1 + gx)
        out.append((x, y0 + row * (ch + gy * cw), cw, ch))
    return out


def pic_boxes(lo, n):
    """Each item's boxes in picture layout `lo`: pic, node, text, sub."""
    out = []
    if lo == "hList2":
        cw = W / (n + 0.1 * (n - 1))
        p = min(cw * 0.3, H * 0.3)
        for i in range(n):
            x = i * cw * 1.1
            out.append(dict(pic=(x, 0, p, p), node=(x + p * 1.1, 0, cw - p * 1.1, p), sub=(x, p * 1.1, cw, H - p * 1.1)))
    elif lo == "bList2":
        for x, y, cw, ch in grid(n, 1.06, 0.08, 0.16):
            sh, d = ch * 0.65, cw * 0.923 * 0.35
            th = sh * 0.43
            out.append(dict(sub=(x, y, cw * 0.923, sh), node=(x, y + sh, cw * 0.923, th),
                            text=(x, y + sh, cw * 0.65, th), pic=(x + cw - d, y + ch - d, d, d)))
    elif lo == "vList4":
        rh = H / (n + 0.1 * (n - 1))
        for i in range(n):
            y = i * rh * 1.1
            out.append(dict(node=(0, y, W, rh), pic=(rh * 0.1, y + rh * 0.1, W * 0.2, rh * 0.8),
                            tx=(rh * 0.1 + W * 0.2, y, W - rh * 0.1 - W * 0.2, rh)))
    elif lo == "pList1":
        for x, y, cw, ch in grid(n, 1.06, 0.08, 0.16):
            out.append(dict(pic=(x, y, cw, ch * 0.8), node=(x, y + ch * 0.8, cw, ch * 0.2)))
    elif lo.endswith("TitledPictureBlocks"):
        for x, y, cw, ch in grid(n, 1 / 1.3787, 0.1, 0.15):
            th = ch * 0.2
            out.append(dict(node=(x, y, cw, th), pic=(x, y + th, cw * 0.7, ch - th),
                            sub=(x + cw * 0.6, y + th + (ch - th) * 0.25, cw * 0.4, (ch - th) * 0.6)))
    else:  # PictureGrid
        for x, y, cw, ch in grid(n, 1 / 0.7568, 0.06, 0.06):
            th = ch - cw
            out.append(dict(node=(x, y, cw, th), pic=(x, y + th, cw, cw)))
    return out


# Picture layouts: picture, item and sub-item box presentation names.
PIC_NAMES = {
    "hList2": ("image", "parentNode", "childNode"),
    "bList2": ("adorn", "parentRect", "childRect"),
    "vList4": ("img", "box", None),
    "pList1": ("pictRect", "textRect", None),
    "2008/layout/TitledPictureBlocks": ("Image", "ParentText", "ChildText"),
    "2008/layout/PictureGrid": ("rect1", "rect2", None),
}
LIGHT = '<a:solidFill><a:schemeClr val="accent1"><a:tint val="40000"/><a:alpha val="90000"/></a:schemeClr></a:solidFill>'


def pic_layout(lo):
    """shapes_of for picture layout `lo`, drawn as Kantor places it."""
    pic_name, node_name, sub_name = PIC_NAMES[lo]
    caption = lo in ("hList2", "pList1", "2008/layout/PictureGrid")

    def tx(shape, box):
        x, y, cx, cy = (int(v) for v in box)
        i = shape.index("<dsp:txXfrm")
        return shape[:i] + f'<dsp:txXfrm><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></dsp:txXfrm></dsp:sp>'

    def shapes_of(ids):
        n = len(ids)
        pres, shapes = [], []
        boxes = pic_boxes(lo, n)
        for k, (node, _sib, _kids, text, subs) in enumerate(ids):
            b = boxes[k]
            pid, nid, sid, tid = (gid("9", 4 * k + j) for j in range(4))
            pic = sp(pid, "ellipse" if lo == "bList2" else "rect", b["pic"], [""])
            pic = pic.replace('<a:solidFill><a:schemeClr val="accent1"/></a:solidFill>',
                              '<a:solidFill><a:schemeClr val="accent1"><a:tint val="50000"/></a:schemeClr></a:solidFill>', 1)
            if k == 0:
                pic = pic.replace(
                    '<a:solidFill><a:schemeClr val="accent1"><a:tint val="50000"/></a:schemeClr></a:solidFill><a:ln',
                    f'<a:blipFill><a:blip xmlns:r="{REL[:-1]}" r:embed="rIdImg1"/>'
                    '<a:stretch><a:fillRect/></a:stretch></a:blipFill><a:ln', 1)
            texts = [text] if sub_name else [text] + subs
            if lo == "bList2":
                nd = sp(nid, "rect", b["node"], [""])
                txt = sp(tid, "rect", b["text"], [text], fill="lt1", font="lt1").replace(
                    '<a:solidFill><a:schemeClr val="lt1"/></a:solidFill><a:ln', "<a:noFill/><a:ln", 1)
            elif caption:
                nd = sp(nid, "rect", b["node"], texts, fill="lt1", font="tx1").replace(
                    '<a:solidFill><a:schemeClr val="lt1"/></a:solidFill><a:ln', "<a:noFill/><a:ln", 1)
            else:
                nd = sp(nid, "roundRect" if lo == "vList4" else "rect", b["node"], texts)
                if "tx" in b:
                    nd = tx(nd, b["tx"])
            sub = None
            if sub_name:
                sub = sp(sid, "round2SameRect" if lo == "bList2" else "rect", b["sub"], subs or [""],
                         font="lt1" if lo == "hList2" else "tx1")
                if lo != "hList2":
                    sub = sub.replace('<a:solidFill><a:schemeClr val="accent1"/></a:solidFill>', LIGHT, 1)
                pres.append((sid, node, sub_name, k, n))
            pres.append((pid, node, pic_name, k, n))
            pres.append((nid, node, node_name, k, n))
            order = [sub, nd] if lo != "2008/layout/TitledPictureBlocks" else [nd]
            if lo == "bList2":
                pres.append((tid, node, "parentText", k, n))
                order.append(txt)
            order.append(pic)
            if lo == "2008/layout/TitledPictureBlocks":
                order.append(sub)
            shapes.extend(x for x in order if x)
        return pres, shapes

    return shapes_of


# The org chart layouts slide: (text, assistant, hierBranch, children).
ORG = ("CEO", False, "init", [
    ("Aide", True, "init", []),
    ("Sales", False, "l", [("North", False, "init", []), ("South", False, "init", [])]),
    ("Ops", False, "hang", [("Plant", False, "init", []), ("Fleet", False, "init", []), ("Depot", False, "init", [])]),
    ("Labs", False, "init", [("Research", False, "init", [])]),
])


def org_place(root):
    """Kantor's (and PowerPoint's) Organization Chart placement in box
    widths: boxes 2:1, gaps 0.21; returns {path: (x, y)} and connectors."""
    g, bh = 0.21, 0.5

    def mode(item, depth):
        text, _asst, branch, kids = item
        if branch != "init":
            return branch
        if depth > 0 and kids and all(not k[3] for k in kids):
            return "r"
        return "std"

    def branch(item, path, depth):
        at = {path: (0.0, 0.0)}
        links = []
        ext = [0.0, 1.0, bh]  # x0, x1, y1

        def take(sub, dx, dy):
            a, l, e = sub
            for p, (x, y) in a.items():
                at[p] = (x + dx, y + dy)
            for p, pts in l:
                links.append((p, [(x + dx, y + dy) for x, y in pts]))
            ext[0], ext[1], ext[2] = min(ext[0], e[0] + dx), max(ext[1], e[1] + dx), max(ext[2], e[2] + dy)

        kids = [(k, path + (j,)) for j, k in enumerate(item[3])]
        assts = [c for c in kids if c[0][1]]
        rest = [c for c in kids if not c[0][1]]
        y = bh + g

        def pairs(lst, y):
            for r in range(0, len(lst), 2):
                h = 0
                for j, (c, cp) in enumerate(lst[r:r + 2]):
                    sub = branch(c, cp, depth + 1)
                    ox = 0.5 - g / 2 - sub[2][1] if j == 0 else 0.5 + g / 2 - sub[2][0]
                    mid = y + bh / 2
                    end = ox + 1 if j == 0 else ox
                    links.append((cp, [(0.5, bh), (0.5, mid), (end, mid)]))
                    h = max(h, sub[2][2])
                    take(sub, ox, y)
                y += h + g
            return y

        y = pairs(assts, y)
        if rest:
            m = mode(item, depth)
            if m == "std":
                cursor, row = 0.0, []
                for c, cp in rest:
                    sub = branch(c, cp, depth + 1)
                    ox = cursor - sub[2][0]
                    cursor = ox + sub[2][1] + g
                    row.append((cp, ox, sub))
                dx = 0.5 - (row[0][1] + row[-1][1] + 1) / 2
                bend = y - g / 2
                for cp, ox, sub in row:
                    cx = ox + dx + 0.5
                    links.append((cp, [(0.5, bh), (0.5, bend), (cx, bend), (cx, y)]))
                    take(sub, ox + dx, y)
            elif m == "hang":
                y = pairs(rest, y)
            else:
                right = m == "r"
                stem, align = (0.5, 0.65) if assts else ((0.1, 0.25) if right else (0.9, 0.25))
                for c, cp in rest:
                    sub = branch(c, cp, depth + 1)
                    ox = max(align, stem + g / 2 - sub[2][0]) if right else min(-align, stem - g / 2 - sub[2][1])
                    mid = y + bh / 2
                    links.append((cp, [(stem, bh), (stem, mid), (ox if right else ox + 1, mid)]))
                    take(sub, ox, y)
                    y = at[cp][1] + sub[2][2] + g
        return at, links, ext

    at, links, (x0, x1, y1) = branch(root, (), 0)
    k = min(W / (x1 - x0), H / y1)
    ox, oy = (W - (x1 - x0) * k) / 2 - x0 * k, (H - y1 * k) / 2
    boxes = {p: (ox + x * k, oy + y * k, k, k * bh) for p, (x, y) in at.items()}
    lines = {p: [(ox + x * k, oy + y * k) for x, y in pts] for p, pts in links}
    return boxes, lines


def org_smartart():
    """Slide 11: ORG as an Organization Chart (data, drawing, layout)."""
    doc = gid("D", 0)
    pts = (
        f'<dgm:pt modelId="{doc}" type="doc"><dgm:prSet loTypeId="urn:microsoft.com/office/officeart/2005/8/layout/orgChart1" '
        'loCatId="hierarchy" qsTypeId="urn:microsoft.com/office/officeart/2005/8/quickstyle/simple1" qsCatId="simple" '
        'csTypeId="urn:microsoft.com/office/officeart/2005/8/colors/accent1_2" csCatId="accent1" phldr="1"/><dgm:spPr/>'
        f"{text_body([''])}</dgm:pt>"
    )
    cxns, shapes = "", []
    boxes, lines = org_place(ORG)
    n = 0

    def add(item, path, parent, ordinal, depth):
        nonlocal pts, cxns, n
        text, asst, branch, kids = item
        n += 1
        node, par, sib, cxn = gid("A", n), gid("B", n), gid("C", n), gid("E", n)
        kind = ' type="asst"' if asst else ""
        pts += f'<dgm:pt modelId="{node}"{kind}><dgm:prSet phldrT="[Text]"/><dgm:spPr/>{text_body([text])}</dgm:pt>'
        for tid, ty in [(par, "parTrans"), (sib, "sibTrans")]:
            pts += f'<dgm:pt modelId="{tid}" type="{ty}" cxnId="{cxn}"><dgm:prSet/><dgm:spPr/>{text_body([""])}</dgm:pt>'
        cxns += (
            f'<dgm:cxn modelId="{cxn}" srcId="{parent}" destId="{node}" srcOrd="{ordinal}" destOrd="0" '
            f'parTransId="{par}" sibTransId="{sib}"/>'
        )
        root = "hierRoot1" if depth == 0 else ("hierRoot3" if asst else "hierRoot2")
        hb = '<dgm:hierBranch val="init"/>' if branch == "init" else f'<dgm:hierBranch val="{branch}"/>'
        pts += (
            f'<dgm:pt modelId="{gid("6", n)}" type="pres"><dgm:prSet presAssocID="{node}" presName="{root}" '
            f'presStyleCnt="0"><dgm:presLayoutVars>{hb}</dgm:presLayoutVars></dgm:prSet><dgm:spPr/></dgm:pt>'
        )
        pid = gid("9", n)
        pts += (
            f'<dgm:pt modelId="{pid}" type="pres"><dgm:prSet presAssocID="{node}" presName="rootText" '
            f'presStyleLbl="node1" presStyleIdx="{n - 1}" presStyleCnt="11"/><dgm:spPr/></dgm:pt>'
        )
        if path:
            cid = gid("8", n)
            pts += (
                f'<dgm:pt modelId="{cid}" type="pres"><dgm:prSet presAssocID="{par}" presName="Name{n}" '
                f'presStyleLbl="parChTrans1D2" presStyleIdx="{n - 1}" presStyleCnt="11"/><dgm:spPr/></dgm:pt>'
            )
            shapes.insert(0, connector(cid, lines[path]))
        shapes.append(sp(pid, "rect", boxes[path], [text]))
        for j, k in enumerate(kids):
            add(k, path + (j,), node, j, depth + 1)

    add(ORG, (), doc, 0, 0)
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
    layout = (
        f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<dgm:layoutDef xmlns:dgm="{DGM}" xmlns:a="{A}" '
        'uniqueId="urn:microsoft.com/office/officeart/2005/8/layout/orgChart1"><dgm:title val=""/><dgm:desc val=""/>'
        '<dgm:catLst><dgm:cat type="hierarchy" pri="1000"/></dgm:catLst><dgm:layoutNode name="hierChild1"><dgm:varLst><dgm:orgChart val="1"/></dgm:varLst>'
        '<dgm:alg type="hierChild"/><dgm:shape/><dgm:presOf/></dgm:layoutNode></dgm:layoutDef>'
    )
    return data, drawing, layout


PICS = [("Lake", ["Calm", "Deep"]), ("Forest", ["Green"]), ("Desert", [])]


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
    ("Other layout", "funnel1", "relationship", [("Boss", []), ("Team", [])], pyramid),
    ("Org chart", "orgChart1", "org", None, None),
    ("Hierarchy", "hierarchy1", "hier", None, None),
    ("Horizontal hierarchy", "hierarchy2", "horiz", None, None),
    ("Labeled hierarchy", "hierarchy6", "label", None, None),
    ("Picture list", "pList2", "picture", [("Lake", []), ("Forest", []), ("Desert", [])], pictures),
    ("Org chart layouts", "orgChart1", "org-layouts", None, None),
    ("Picture accent list", "hList2", "picture", PICS, pic_layout("hList2")),
    ("Bending picture accent list", "bList2", "picture", PICS, pic_layout("bList2")),
    ("Vertical picture list", "vList4", "picture", PICS, pic_layout("vList4")),
    ("Picture caption list", "pList1", "picture", PICS, pic_layout("pList1")),
    ("Titled picture blocks", "2008/layout/TitledPictureBlocks", "picture", PICS,
     pic_layout("2008/layout/TitledPictureBlocks")),
    ("Picture grid", "2008/layout/PictureGrid", "picture", PICS, pic_layout("2008/layout/PictureGrid")),
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
    if cat == "org-layouts":
        data, drawing, layout = org_smartart()
    elif shapes_of is None:
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
    if "rIdImg1" in drawing:
        parts[f"ppt/diagrams/_rels/drawing{i}.xml.rels"] = (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            f'<Relationship Id="rIdImg1" Type="{REL}image" Target="../media/smartart_pic1.png"/></Relationships>'
        ).encode()
        parts["ppt/media/smartart_pic1.png"] = png(64, 48)
        if 'Extension="png"' not in ct:
            ct = ct.replace("</Types>", '<Default Extension="png" ContentType="image/png"/></Types>')
parts["[Content_Types].xml"] = ct.encode()

FIXED = (2026, 11, 16, 0, 0, 0)
with zipfile.ZipFile(out / "smartart.pptx", "w", zipfile.ZIP_DEFLATED) as z:
    for name in ["[Content_Types].xml"] + sorted(n for n in parts if n != "[Content_Types].xml"):
        info = zipfile.ZipInfo(name, date_time=FIXED)
        info.compress_type = zipfile.ZIP_DEFLATED
        z.writestr(info, parts[name])
print("smartart ok")
