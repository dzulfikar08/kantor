#!/usr/bin/env python3
"""Make the synthetic OpenDocument presentation fixtures in this directory.

Builds .pptx decks with python-pptx and converts them to .odp (and one
.otp template) with LibreOffice, so every byte here comes from this script.

    python3 -m venv ~/.cache/pptvenv
    ~/.cache/pptvenv/bin/pip install python-pptx Pillow
    ~/.cache/pptvenv/bin/python tests/corpus/odp/make_fixtures.py \
        --soffice ~/.cache/lo/lo.AppImage

The LibreOffice AppImage changes directory, so paths are made absolute.
"""

import argparse
import io
import os
import shutil
import subprocess
import tempfile

from PIL import Image, ImageDraw
from pptx import Presentation
from pptx.dml.color import RGBColor
from pptx.enum.dml import MSO_LINE_DASH_STYLE
from pptx.enum.shapes import MSO_CONNECTOR, MSO_SHAPE
from pptx.enum.text import PP_ALIGN
from pptx.util import Cm, Emu, Pt

HERE = os.path.dirname(os.path.abspath(__file__))


def png_bytes():
    img = Image.new("RGB", (64, 48), (30, 120, 200))
    d = ImageDraw.Draw(img)
    d.rectangle([8, 8, 40, 30], fill=(250, 200, 20))
    out = io.BytesIO()
    img.save(out, "PNG")
    return out.getvalue()


def jpeg_bytes():
    img = Image.new("RGB", (80, 60), (200, 40, 60))
    d = ImageDraw.Draw(img)
    d.ellipse([10, 10, 70, 50], fill=(255, 255, 255))
    out = io.BytesIO()
    img.save(out, "JPEG", quality=90)
    return out.getvalue()


def notes(slide, text):
    slide.notes_slide.notes_text_frame.text = text


def basic_deck():
    """The default 4:3 deck: text, lists, shapes, pictures, a table, a
    group, notes, a link and a hidden slide."""
    prs = Presentation()

    # 1. Title slide.
    s = prs.slides.add_slide(prs.slide_layouts[0])
    s.shapes.title.text = "Quarterly Review"
    s.placeholders[1].text = "Prepared with Kantor fixtures"
    notes(s, "Welcome everyone.")

    # 2. Bullets, numbering and character formatting.
    s = prs.slides.add_slide(prs.slide_layouts[1])
    s.shapes.title.text = "Formatting"
    tf = s.placeholders[1].text_frame
    tf.text = "First level bullet"
    p = tf.add_paragraph()
    p.text = "Second level"
    p.level = 1
    p = tf.add_paragraph()
    p.level = 0
    r = p.add_run()
    r.text = "Bold "
    r.font.bold = True
    r = p.add_run()
    r.text = "italic "
    r.font.italic = True
    r = p.add_run()
    r.text = "underlined "
    r.font.underline = True
    r = p.add_run()
    r.text = "red 28pt Courier"
    r.font.color.rgb = RGBColor(0xC0, 0x10, 0x10)
    r.font.size = Pt(28)
    r.font.name = "Courier New"
    p = tf.add_paragraph()
    r = p.add_run()
    r.text = "A link"
    r.hyperlink.address = "https://kantor.paparanai.id/"
    tb = s.shapes.add_textbox(Cm(14), Cm(14), Cm(10), Cm(3))
    for i, t in enumerate(["One", "Two", "Three"]):
        para = tb.text_frame.paragraphs[0] if i == 0 else tb.text_frame.add_paragraph()
        para.text = t
        ppr = para._p.get_or_add_pPr()
        ppr.set("marL", "342900")
        ppr.set("indent", "-342900")
        num = ppr.makeelement(
            "{http://schemas.openxmlformats.org/drawingml/2006/main}buAutoNum",
            {"type": "arabicPeriod"},
        )
        ppr.append(num)
    notes(s, "Talk about formatting.")

    # 3. Shapes.
    s = prs.slides.add_slide(prs.slide_layouts[6])
    rect = s.shapes.add_shape(MSO_SHAPE.RECTANGLE, Cm(1), Cm(1), Cm(6), Cm(3))
    rect.fill.solid()
    rect.fill.fore_color.rgb = RGBColor(0x20, 0x80, 0x40)
    rect.line.color.rgb = RGBColor(0x00, 0x00, 0x80)
    rect.line.width = Pt(3)
    rect.text_frame.text = "Green box"
    rr = s.shapes.add_shape(MSO_SHAPE.ROUNDED_RECTANGLE, Cm(9), Cm(1), Cm(6), Cm(3))
    rr.fill.gradient()
    rr.fill.gradient_angle = 0
    rr.fill.gradient_stops[0].color.rgb = RGBColor(0xFF, 0x00, 0x00)
    rr.fill.gradient_stops[1].color.rgb = RGBColor(0x00, 0x00, 0xFF)
    el = s.shapes.add_shape(MSO_SHAPE.OVAL, Cm(17), Cm(1), Cm(5), Cm(5))
    el.fill.solid()
    el.fill.fore_color.rgb = RGBColor(0x80, 0x20, 0xA0)
    arrow = s.shapes.add_shape(MSO_SHAPE.RIGHT_ARROW, Cm(7), Cm(6), Cm(6), Cm(3))
    arrow.rotation = 30
    arrow.fill.solid()
    arrow.fill.fore_color.rgb = RGBColor(0xE0, 0x30, 0x30)
    star = s.shapes.add_shape(MSO_SHAPE.STAR_5_POINT, Cm(15), Cm(8), Cm(4), Cm(4))
    star.fill.solid()
    star.fill.fore_color.rgb = RGBColor(0xFF, 0xD7, 0x00)
    line = s.shapes.add_connector(MSO_CONNECTOR.STRAIGHT, Cm(1), Cm(12), Cm(12), Cm(16))
    line.line.color.rgb = RGBColor(0x33, 0x33, 0x33)
    line.line.width = Pt(2)
    line.line.dash_style = MSO_LINE_DASH_STYLE.DASH
    tail = line.line._get_or_add_ln().makeelement(
        "{http://schemas.openxmlformats.org/drawingml/2006/main}tailEnd",
        {"type": "triangle"},
    )
    line.line._get_or_add_ln().append(tail)
    flip = s.shapes.add_shape(MSO_SHAPE.RIGHT_TRIANGLE, Cm(20), Cm(10), Cm(4), Cm(4))
    flip.fill.solid()
    flip.fill.fore_color.rgb = RGBColor(0x00, 0x99, 0x99)
    flip._element.spPr.xfrm.set("flipH", "1")
    grp = s.shapes.add_group_shape()
    a = grp.shapes.add_shape(MSO_SHAPE.RECTANGLE, Cm(1), Cm(17), Cm(3), Cm(1.5))
    a.fill.solid()
    a.fill.fore_color.rgb = RGBColor(0x11, 0x22, 0x33)
    b = grp.shapes.add_shape(MSO_SHAPE.OVAL, Cm(5), Cm(17), Cm(1.5), Cm(1.5))
    b.fill.solid()
    b.fill.fore_color.rgb = RGBColor(0x44, 0x55, 0x66)

    # 4. Pictures.
    s = prs.slides.add_slide(prs.slide_layouts[5])
    s.shapes.title.text = "Pictures"
    s.shapes.add_picture(io.BytesIO(png_bytes()), Cm(2), Cm(5), Cm(8), Cm(6))
    s.shapes.add_picture(io.BytesIO(jpeg_bytes()), Cm(14), Cm(5), Cm(8), Cm(6))

    # 5. Table with a merged cell.
    s = prs.slides.add_slide(prs.slide_layouts[5])
    s.shapes.title.text = "Table"
    t = s.shapes.add_table(3, 3, Cm(2), Cm(5), Cm(18), Cm(4.5)).table
    for r in range(3):
        for c in range(3):
            t.cell(r, c).text = f"R{r + 1}C{c + 1}"
    t.cell(2, 0).merge(t.cell(2, 1))
    t.cell(2, 0).text = "Merged"

    # 6. Unicode and alignment in a text box.
    s = prs.slides.add_slide(prs.slide_layouts[6])
    tb = s.shapes.add_textbox(Cm(2), Cm(2), Cm(20), Cm(4))
    tb.text_frame.text = "Selamat datang di rapat tahunan"
    p = tb.text_frame.add_paragraph()
    p.text = "日本語のテキスト 中文 한국어"
    p.alignment = PP_ALIGN.CENTER
    p = tb.text_frame.add_paragraph()
    p.text = "Right aligned — “quotes” € 100"
    p.alignment = PP_ALIGN.RIGHT
    notes(s, "Catatan pembicara: terima kasih")

    # 7. Hidden slide.
    s = prs.slides.add_slide(prs.slide_layouts[1])
    s.shapes.title.text = "Hidden slide"
    s.placeholders[1].text = "Not shown in the slide show"
    s._element.set("show", "0")
    return prs


def wide_deck():
    """16:9 deck with a coloured slide background."""
    prs = Presentation()
    prs.slide_width = Emu(12192000)
    prs.slide_height = Emu(6858000)
    s = prs.slides.add_slide(prs.slide_layouts[6])
    bg = s.background.fill
    bg.solid()
    bg.fore_color.rgb = RGBColor(0xFF, 0xF4, 0xD6)
    tb = s.shapes.add_textbox(Cm(2), Cm(2), Cm(28), Cm(3))
    tb.text_frame.text = "Widescreen"
    tb.text_frame.paragraphs[0].runs[0].font.size = Pt(40)
    box = s.shapes.add_shape(MSO_SHAPE.RECTANGLE, Cm(25), Cm(12), Cm(6), Cm(5))
    box.fill.solid()
    box.fill.fore_color.rgb = RGBColor(0x1F, 0x4E, 0x79)
    return prs


def convert(soffice, src, outdir, filt, ext):
    os.makedirs(os.path.expanduser("~/.cache/ktmp"), exist_ok=True)
    profile = tempfile.mkdtemp(prefix="lo-profile-", dir=os.path.expanduser("~/.cache/ktmp"))
    cmd = [
        soffice,
        f"-env:UserInstallation=file://{profile}",
        "--headless",
        "--convert-to",
        f"{ext}:{filt}",
        "--outdir",
        outdir,
        src,
    ]
    # 81: a fresh profile was created and LibreOffice asks to be restarted.
    if subprocess.run(cmd).returncode == 81:
        subprocess.run(cmd, check=True)
    shutil.rmtree(profile, ignore_errors=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--soffice", required=True)
    args = ap.parse_args()
    soffice = os.path.abspath(os.path.expanduser(args.soffice))
    work = tempfile.mkdtemp(prefix="odpfix-", dir=os.path.expanduser("~/.cache/ktmp"))
    decks = {"basic": basic_deck(), "wide": wide_deck()}
    for name, prs in decks.items():
        src = os.path.join(work, f"{name}.pptx")
        prs.save(src)
        convert(soffice, src, HERE, "impress8", "odp")
    # The widescreen deck as a template too.
    convert(soffice, os.path.join(work, "wide.pptx"), HERE, "impress8_template", "otp")
    shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
