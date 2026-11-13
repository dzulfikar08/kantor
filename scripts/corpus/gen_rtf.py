#!/usr/bin/env python3
"""Write the RTF import fixtures (CC0, synthetic): tests/corpus/rtf/*.rtf,
read by crates/docs_engine/tests/rtf.rs.

Hand-written, in the style Word writes:

* word_sample.rtf: font table with panose and charsets, colour table,
  \\*\\defchp/\\*\\defpap, a style sheet (Normal, heading 1-2, Title, a
  character style), list table with a bullet and a numbered list, header,
  footer with a PAGE field, formatting of every kind, a hyperlink field, a
  table with horizontal and vertical merges, borders and shading, a PNG
  (\\*\\shppict with a WMF \\nonshppict) and a JPEG picture, a landscape
  section, Indonesian, CJK and emoji text through \\u (surrogate pairs).
* charsets.rtf: \\ansicpg1251 text, a Shift_JIS (\\fcharset128) font, a
  Greek (\\fcharset161) font, \\uc2 fallbacks.
* pn_lists.rtf: Word 95 style \\pn bulleted and numbered paragraphs.
* truncated.rtf: word_sample.rtf cut in the middle of the table.

Converted by LibreOffice (with --soffice), from the synthetic .docx corpus
and from features.docx, which this script builds with python-docx:

* lo_report.rtf, lo_letter.rtf, lo_invoice.rtf, lo_features.rtf.

Regenerate with
`uv run --with python-docx==1.2.0 --with pillow==11.3.0 scripts/corpus/gen_rtf.py
tests/corpus/rtf --soffice ~/.cache/lo/lo.AppImage`.
"""
import io
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def u(text):
    """Text as RTF: ASCII as is, everything else as \\uN? (UTF-16 units)."""
    out = []
    for ch in text:
        if ch in "\\{}":
            out.append("\\" + ch)
        elif ord(ch) < 128:
            out.append(ch)
        else:
            data = ch.encode("utf-16-le")
            for i in range(0, len(data), 2):
                n = int.from_bytes(data[i : i + 2], "little")
                if n > 32767:
                    n -= 65536
                out.append(f"\\u{n}?")
    return "".join(out)


def hexdata(b, width=64):
    h = b.hex()
    return "\n".join(h[i : i + width] for i in range(0, len(h), width))


def png_bytes():
    from PIL import Image

    img = Image.new("RGB", (40, 20), (31, 78, 121))
    for x in range(20, 40):
        for y in range(20):
            img.putpixel((x, y), (237, 125, 49))
    buf = io.BytesIO()
    img.save(buf, "PNG", optimize=False)
    return buf.getvalue()


def jpeg_bytes():
    from PIL import Image

    img = Image.new("RGB", (30, 30), (112, 173, 71))
    buf = io.BytesIO()
    img.save(buf, "JPEG", quality=80)
    return buf.getvalue()


HEADER = r"""{\rtf1\adeflang1025\ansi\ansicpg1252\uc1\adeff31507\deff0\stshfdbch31505\stshfloch31506\stshfhich31506\stshfbi31507\deflang1033\deflangfe1033\themelang1033\themelangfe0\themelangcs0
{\fonttbl{\f0\fbidi \froman\fcharset0\fprq2{\*\panose 02020603050405020304}Times New Roman;}{\f3\fbidi \froman\fcharset2\fprq2{\*\panose 05050102010706020507}Symbol;}
{\f34\fbidi \froman\fcharset0\fprq2{\*\panose 02040503050406030204}Cambria Math;}{\f37\fbidi \fswiss\fcharset0\fprq2{\*\panose 020f0502020204030204}Calibri;}
{\f38\fbidi \fswiss\fcharset0\fprq2{\*\panose 020f0302020204030204}Calibri Light;}{\f40\fbidi \fmodern\fcharset0\fprq1{\*\panose 02070309020205020404}Courier New;}
{\f41\fbidi \fnil\fcharset134\fprq2{\*\panose 02010600030101010101}SimSun{\*\falt \'cb\'ce\'cc\'e5};}}
{\colortbl;\red0\green0\blue0;\red0\green0\blue255;\red0\green255\blue255;\red0\green255\blue0;\red255\green0\blue255;\red255\green0\blue0;\red255\green255\blue0;\red255\green255\blue255;
\red0\green0\blue128;\red0\green128\blue128;\red0\green128\blue0;\red128\green0\blue128;\red128\green0\blue0;\red128\green128\blue0;\red128\green128\blue128;\red192\green192\blue192;\red5\green99\blue193;\red47\green84\blue150;\red222\green234\blue246;}
{\*\defchp \f37\fs22\lang1033\langfe1033\langnp1033\langfenp1033 }{\*\defpap \ql \li0\ri0\sa160\sl259\slmult1\widctlpar\wrapdefault\aspalpha\aspnum\faauto\adjustright\rin0\lin0\itap0 }
\noqfpromote {\stylesheet{\ql \li0\ri0\sa160\sl259\slmult1\widctlpar\wrapdefault\aspalpha\aspnum\faauto\adjustright\rin0\lin0\itap0 \rtlch\fcs1 \af31507\afs22\alang1025 \ltrch\fcs0 \f37\fs22\lang1033\langfe1033\cgrid\langnp1033\langfenp1033 \snext0 \sqformat \spriority0 Normal;}
{\s1\ql \li0\ri0\sb240\keepn\widctlpar\wrapdefault\aspalpha\aspnum\faauto\outlinelevel0\adjustright\rin0\lin0\itap0 \rtlch\fcs1 \af31503\afs32\alang1025 \ltrch\fcs0 \f38\fs32\cf18\lang1033\langfe1033\cgrid\langnp1033\langfenp1033 \sbasedon0 \snext0 \slink15 \sqformat \spriority9 heading 1;}
{\s2\ql \li0\ri0\sb40\keepn\widctlpar\wrapdefault\aspalpha\aspnum\faauto\outlinelevel1\adjustright\rin0\lin0\itap0 \rtlch\fcs1 \af31503\afs26\alang1025 \ltrch\fcs0 \f38\fs26\cf18\lang1033\langfe1033\cgrid\langnp1033\langfenp1033 \sbasedon0 \snext0 \slink16 \sunhideused\sqformat \spriority9 heading 2;}
{\*\cs10 \additive \ssemihidden \sunhideused \spriority1 Default Paragraph Font;}
{\*\ts11\tsrowd\trftsWidthB3\trpaddl108\trpaddr108\trpaddfl3\trpaddft3\trpaddfb3\trpaddfr3\tblind0\tblindtype3\tsvertalt\tsbrdrt\tsbrdrl\tsbrdrb\tsbrdrr\tsbrdrdgl\tsbrdrdgr\tsbrdrh\tsbrdrv \ql \li0\ri0\sa160\sl259\slmult1 \fs22 \snext11 \ssemihidden \sunhideused Normal Table;}
{\s17\qc \li0\ri0\sa0\widctlpar\wrapdefault\aspalpha\aspnum\faauto\adjustright\rin0\lin0\itap0 \rtlch\fcs1 \af31503\afs56\alang1025 \ltrch\fcs0 \f38\fs56\expnd-2\expndtw-10\kerning28\lang1033\langfe1033\cgrid\langnp1033\langfenp1033 \sbasedon0 \snext0 \sqformat \spriority10 Title;}
{\*\cs18 \additive \rtlch\fcs1 \af0 \ltrch\fcs0 \ul\cf17 \sbasedon10 \sunhideused \styrsid1 Hyperlink;}
{\*\cs19 \additive \rtlch\fcs1 \ab\af0 \ltrch\fcs0 \b\cf6 \sbasedon10 \sqformat Strong Red;}}
{\*\listtable{\list\listtemplateid-1\listhybrid{\listlevel\levelnfc23\levelnfcn23\leveljc0\leveljcn0\levelfollow0\levelstartat1\levelspace0\levelindent0{\leveltext\leveltemplateid67698689\'01\u-3913 ?;}{\levelnumbers;}\f3\fbias0 \fi-360\li720\lin720 }
{\listlevel\levelnfc23\levelnfcn23\leveljc0\leveljcn0\levelfollow0\levelstartat1\lvltentative\levelspace0\levelindent0{\leveltext\leveltemplateid67698691\'01o;}{\levelnumbers;}\f40\fbias0 \fi-360\li1440\lin1440 }{\listname ;}\listid1001}
{\list\listtemplateid-2\listhybrid{\listlevel\levelnfc0\levelnfcn0\leveljc0\leveljcn0\levelfollow0\levelstartat1\levelspace0\levelindent0{\leveltext\leveltemplateid67698703\'02\'00.;}{\levelnumbers\'01;}\fi-360\li720\lin720 }
{\listlevel\levelnfc4\levelnfcn4\leveljc0\leveljcn0\levelfollow0\levelstartat1\levelspace0\levelindent0{\leveltext\leveltemplateid67698713\'03\'00.\'01;}{\levelnumbers\'01\'03;}\fi-360\li1440\lin1440 }{\listname ;}\listid1002}}
{\*\listoverridetable{\listoverride\listid1001\listoverridecount0\ls1}{\listoverride\listid1002\listoverridecount0\ls2}}
{\*\rsidtbl \rsid1\rsid2}{\mmathPr\mmathFont34\mbrkBin0\mbrkBinSub0}
{\info{\title Kantor RTF sample}{\author Kantor}{\operator Kantor}{\creatim\yr2026\mo11\dy13\hr9\min0}{\version1}{\edmins1}{\nofpages2}{\nofwords120}{\nofchars700}}
{\*\xmlnstbl {\xmlns1 http://schemas.microsoft.com/office/word/2003/wordml}}
\paperw11906\paperh16838\margl1440\margr1440\margt1440\margb1440\gutter0\ltrsect
\deftab720\widowctrl\ftnbj\aenddoc\trackmoves0\trackformatting1\donotembedsysfont1\relyonvml0\donotembedlingdata0\grfdocevents0\validatexml1\showplaceholdtext0\ignoremixedcontent0\saveinvalidxml0\showxmlerrors1\noxlattoyen
\expshrtn\noultrlspc\dntblnsbdb\nospaceforul\formshade\horzdoc\dgmargin\dghspace180\dgvspace180\dghorigin1440\dgvorigin1440\dghshow1\dgvshow1
\jexpand\viewkind1\viewscale100\pgbrdrhead\pgbrdrfoot\splytwnine\ftnlytwnine\htmautsp\nolnhtadjtbl\useltbaln\alntblind\lytcalctblwd\lyttblrtgr\lnbrkrule\nobrkwrptbl\snaptogridincell\allowfieldendsel\wrppunct
\asianbrkrule\rsidroot1\newtblstyruls\nogrowautofit\usenormstyforlist\noindnmbrts\felnbrelev\nocxsptable\indrlsweleven\noafcnsttbl\afelev\utinl\hwelev\spltpgpar\notcvasp\notbrkcnstfrctbl\notvatxbx\krnprsnet\cachedcolbal \nouicompat \fet0
{\*\wgrffmtfilter 2450}\nofeatherthrottle1\ilfomacatclnup0{\*\ftnsep \ltrpar \pard\plain \ltrpar\ql \li0\ri0\widctlpar\wrapdefault\aspalpha\aspnum\faauto\adjustright\rin0\lin0\itap0 \rtlch\fcs1 \af31507\afs22\alang1025 \ltrch\fcs0 \f37\fs22\lang1033\langfe1033\cgrid\langnp1033\langfenp1033 {\rtlch\fcs1 \af31507 \ltrch\fcs0 \chftnsep \par }}
\ltrpar \sectd \ltrsect\linex0\headery708\footery708\colsx708\endnhere\sectlinegrid360\sectdefaultcl\titlepg\sftnbj
{\headerf \ltrpar \pard\plain \ltrpar\s0\qr \li0\ri0\sa160\sl259\slmult1\widctlpar\wrapdefault\aspalpha\aspnum\faauto\adjustright\rin0\lin0\itap0 \rtlch\fcs1 \af31507\afs22\alang1025 \ltrch\fcs0 \f37\fs22\lang1033\langfe1033\cgrid\langnp1033\langfenp1033 {\rtlch\fcs1 \af31507 \ltrch\fcs0 \i First page header\par }}
{\header \ltrpar \pard\plain \ltrpar\s0\qr \li0\ri0\sa160\sl259\slmult1\widctlpar\wrapdefault\aspalpha\aspnum\faauto\adjustright\rin0\lin0\itap0 \rtlch\fcs1 \af31507\afs22\alang1025 \ltrch\fcs0 \f37\fs22\lang1033\langfe1033\cgrid\langnp1033\langfenp1033 {\rtlch\fcs1 \af31507 \ltrch\fcs0 Kantor RTF sample\par }}
{\footer \ltrpar \pard\plain \ltrpar\s0\qc \li0\ri0\sa160\sl259\slmult1\widctlpar\wrapdefault\aspalpha\aspnum\faauto\adjustright\rin0\lin0\itap0 \rtlch\fcs1 \af31507\afs22\alang1025 \ltrch\fcs0 \f37\fs22\lang1033\langfe1033\cgrid\langnp1033\langfenp1033 {\rtlch\fcs1 \af31507 \ltrch\fcs0 Page }{\field{\*\fldinst {\rtlch\fcs1 \af31507 \ltrch\fcs0  PAGE   \\* MERGEFORMAT }}{\fldrslt {\rtlch\fcs1 \af31507 \ltrch\fcs0 \lang1024\langfe1024\noproof 1}}}\sectd \ltrsect\linex0\endnhere\sectlinegrid360\sectdefaultcl\sftnbj {\rtlch\fcs1 \af31507 \ltrch\fcs0 \par }}
"""

PARA = r"\pard\plain \ltrpar\ql \li0\ri0\sa160\sl259\slmult1\widctlpar\wrapdefault\aspalpha\aspnum\faauto\adjustright\rin0\lin0\itap0 \rtlch\fcs1 \af31507\afs22\alang1025 \ltrch\fcs0 \f37\fs22\lang1033\langfe1033\cgrid\langnp1033\langfenp1033 "
H1 = r"\pard\plain \ltrpar\s1\ql \li0\ri0\sb240\keepn\widctlpar\wrapdefault\aspalpha\aspnum\faauto\outlinelevel0\adjustright\rin0\lin0\itap0 \rtlch\fcs1 \af31503\afs32\alang1025 \ltrch\fcs0 \f38\fs32\cf18\lang1033\langfe1033\cgrid\langnp1033\langfenp1033 "
H2 = r"\pard\plain \ltrpar\s2\ql \li0\ri0\sb40\keepn\widctlpar\wrapdefault\aspalpha\aspnum\faauto\outlinelevel1\adjustright\rin0\lin0\itap0 \rtlch\fcs1 \af31503\afs26\alang1025 \ltrch\fcs0 \f38\fs26\cf18\lang1033\langfe1033\cgrid\langnp1033\langfenp1033 "
TITLE = r"\pard\plain \ltrpar\s17\qc \li0\ri0\sa0\widctlpar\wrapdefault\aspalpha\aspnum\faauto\adjustright\rin0\lin0\itap0 \rtlch\fcs1 \af31503\afs56\alang1025 \ltrch\fcs0 \f38\fs56\expnd-2\expndtw-10\kerning28\lang1033\langfe1033\cgrid\langnp1033\langfenp1033 "


def list_para(ls, ilvl, li, marker):
    return (
        rf"{{\listtext\pard\plain\ltrpar \rtlch\fcs1 \af0\afs22 \ltrch\fcs0 {marker}\tab}}"
        + rf"\pard\plain \ltrpar\ql \fi-360\li{li}\ri0\sa160\sl259\slmult1\widctlpar\wrapdefault\aspalpha\aspnum\faauto\ls{ls}\ilvl{ilvl}\adjustright\rin0\lin{li}\itap0\pararsid1\contextualspace "
        + r"\rtlch\fcs1 \af31507\afs22\alang1025 \ltrch\fcs0 \f37\fs22\lang1033\langfe1033\cgrid\langnp1033\langfenp1033 "
    )


def row_def(merge_row):
    b = r"\brdrs\brdrw10 \brdrcf1 "
    cells = []
    widths = [3005, 6010, 9016]
    for i, x in enumerate(widths):
        props = rf"\clvertalt\clbrdrt{b}\clbrdrl{b}\clbrdrb{b}\clbrdrr{b}"
        if merge_row == "head":
            props = r"\clcbpat19" + props
        if merge_row == "hmerge":
            props = (r"\clmgf" if i == 0 else r"\clmrg" if i == 1 else "") + props
        if merge_row == "vstart" and i == 2:
            props = r"\clvmgf" + props
        if merge_row == "vcont" and i == 2:
            props = r"\clvmrg" + props
        cells.append(rf"{props}\cltxlrtb\clftsWidth3\clwWidth3005 \cellx{x}")
    hdr = r"\trhdr" if merge_row == "head" else ""
    return (
        rf"\trowd \irow0\irowband0\ltrrow\ts11\trgaph108\trleft-108{hdr}\trbrdrt{b}\trbrdrl{b}\trbrdrb{b}\trbrdrr{b}\trbrdrh{b}\trbrdrv{b}"
        + r"\trftsWidth1\trftsWidthB3\trftsWidthA3\trautofit1\trpaddl108\trpaddr108\trpaddfl3\trpaddft3\trpaddfb3\trpaddfr3\tblrsid1\tbllkhdrrows\tbllkhdrcols\tbllknocolband\tblind0\tblindtype3 "
        + "".join(cells)
    )


CELL = r"\pard \ltrpar\ql \li0\ri0\widctlpar\intbl\wrapdefault\aspalpha\aspnum\faauto\adjustright\rin0\lin0 "


def table():
    rows = [
        ("head", ["Item", "Owner", "Status"]),
        ("hmerge", ["Merged across two columns", "", "Done"]),
        ("vstart", ["Kick-off", "Rina", "Spans two rows"]),
        ("vcont", ["Review", "Budi", ""]),
    ]
    out = []
    for kind, texts in rows:
        rd = row_def(kind)
        out.append(rd + "\n")
        for t in texts:
            bold = r"\b " if kind == "head" else ""
            out.append(rf"{CELL}{{\rtlch\fcs1 \af31507 \ltrch\fcs0 {bold}{u(t)}}}{{\cell }}")
        out.append(
            rf"\pard\plain \ltrpar\ql \li0\ri0\sa160\sl259\slmult1\widctlpar\intbl\wrapdefault\aspalpha\aspnum\faauto\adjustright\rin0\lin0 {{\rtlch\fcs1 \af31507 \ltrch\fcs0 {rd}\row }}"
            + "\n"
        )
    return "".join(out)


def word_sample():
    png = png_bytes()
    jpg = jpeg_bytes()
    body = []
    body.append(TITLE + r"{\rtlch\fcs1 \af31507 \ltrch\fcs0 Quarterly Report\par }")
    body.append(H1 + r"{\rtlch\fcs1 \af31507 \ltrch\fcs0 Introduction\par }")
    body.append(
        PARA
        + r"{\rtlch\fcs1 \af31507 \ltrch\fcs0 Plain, }{\b bold}{, }{\i italic}{, }{\ul underline}{, }{\uldb double}{, }"
        + r"{\strike strike}{, }{\caps caps}{, }{\scaps small caps}{, }{\cf6 red}{, }{\highlight7 highlighted}{, }"
        + r"{\cb19\chcbpat19 shaded}{, x}{\super 2}{, H}{\sub 2}{O, }{\fs32 16 pt}{, }{\f40 Courier New}{, }"
        + r'{\cs19\b\cf6 strong red}{, }{\v hidden text}{ and an }{\field{\*\fldinst {HYPERLINK "https://kantor.paparanai.id/" }}{\fldrslt {\cs18\ul\cf17 external link}}}{.\par }'
    )
    body.append(
        PARA
        + r"\qj\sb120\sa240\sl360\slmult1\li720\ri360\fi360 {Justified, indented, 1.5 lines, 6 pt before and 12 pt after: "
        + u("Selamat datang di Kantor. Dokumen ini berbahasa Indonesia: «ä ö ü» — dan seterusnya.")
        + r"\par }"
    )
    body.append(PARA + r"\qc {Centred line\line second line in the same paragraph\par }")
    body.append(PARA + r"\qr\tx2880\tqr\tldot\tx8640 {Right\tab dotted\tab tab\par }")
    body.append(
        PARA
        + r"{"
        + u("Chinese 中文, Japanese 日本語, Korean 한국어, emoji 😀👍🏽 and a flag 🇮🇩.")
        + r"\par }"
    )
    body.append(PARA + r"{\f41\fs22 \'d6\'d0\'ce\'c4}{ (SimSun, GBK bytes)\par }")
    body.append(H2 + r"{Lists\par }")
    body.append(list_para(1, 0, 720, r"\f3 \'b7") + r"{First bullet\par }")
    body.append(list_para(1, 0, 720, r"\f3 \'b7") + r"{Second bullet\par }")
    body.append(list_para(1, 1, 1440, r"\f40 o") + r"{Nested bullet\par }")
    body.append(list_para(2, 0, 720, "1.") + r"{First step\par }")
    body.append(list_para(2, 0, 720, "2.") + r"{Second step\par }")
    body.append(list_para(2, 1, 1440, "a.") + r"{Sub-step\par }")
    body.append(H2 + r"{Table\par }")
    body.append(table())
    body.append(H2 + r"{Pictures\par }")
    body.append(
        PARA
        + r"{\*\shppict{\pict{\*\picprop\shplid1025{\sp{\sn shapeType}{\sv 75}}}\picscalex100\picscaley100\piccropl0\piccropr0\piccropt0\piccropb0"
        + r"\picw1058\pich529\picwgoal600\pichgoal300\pngblip\bliptag1{\*\blipuid 0123456789abcdef0123456789abcdef}"
        + "\n"
        + hexdata(png)
        + "}}"
        + r"{\nonshppict{\pict\picscalex100\picscaley100\picw1058\pich529\picwgoal600\pichgoal300\wmetafile8 0100090000}}"
        + r"{ and }{\pict\jpegblip\picw30\pich30\picwgoal450\pichgoal450 "
        + "\n"
        + hexdata(jpg)
        + r"}{\par }"
    )
    body.append(PARA + r"{Before the page break}{\page }{After the page break\par }")
    # A landscape section.
    body.append(
        PARA
        + r"{End of the portrait section\par }\sect \sectd \ltrsect\lndscpsxn\pgwsxn16838\pghsxn11906\marglsxn1134\margrsxn1134\margtsxn1134\margbsxn1134\sbkpage "
    )
    body.append(H1 + r"{Landscape section\par }")
    body.append(PARA + r"{Wide pages for wide tables.}")
    return HEADER + "".join(body) + "\n}\n"


def charsets():
    return (
        r"{\rtf1\ansi\ansicpg1251\deff0{\fonttbl{\f0\fswiss\fcharset204 Arial;}{\f1\fnil\fcharset128 MS Mincho;}{\f2\froman\fcharset161 Times New Roman Greek;}}"
        + "\n"
        + r"\pard\plain\f0\fs24 \'cf\'f0\'e8\'e2\'e5\'f2, \'ec\'e8\'f0!\par"
        + "\n"
        + r"\f1 \'82\'b1\'82\'f1\'82\'c9\'82\'bf\'82\'cd\par"
        + "\n"
        + r"\f2 \'c1\'e8\'ee\'df\par"
        + "\n"
        + r"\f0\uc2 Euro \u8364\'88\'88 sign\uc1  and \u8211? dash\par"
        + "\n}"
    )


def pn_lists():
    return (
        r"{\rtf1\ansi\deff0{\fonttbl{\f0\froman Times New Roman;}{\f1\fdecor\fcharset2 Symbol;}}"
        + "\n"
        + r"{\pntext\f1\'b7\tab}\pard\plain{\*\pn\pnlvlblt\pnf1\pnindent360{\pntxtb\'b7}}\fi-360\li720 Apples\par"
        + "\n"
        + r"{\pntext\f1\'b7\tab}\pard\plain{\*\pn\pnlvlblt\pnf1\pnindent360{\pntxtb\'b7}}\fi-360\li720 Pears\par"
        + "\n"
        + r"{\pntext 1.\tab}\pard\plain{\*\pn\pnlvlbody\pndec\pnstart1\pnindent360{\pntxta .}}\fi-360\li720 One\par"
        + "\n"
        + r"{\pntext 2.\tab}\pard\plain{\*\pn\pnlvlbody\pndec\pnstart1\pnindent360{\pntxta .}}\fi-360\li720 Two\par"
        + "\n"
        + r"\pard\plain Not a list\par"
        + "\n}"
    )


def features_docx(path):
    """A Word-style .docx for LibreOffice to turn into RTF."""
    from docx import Document
    from docx.enum.section import WD_ORIENT
    from docx.enum.text import WD_ALIGN_PARAGRAPH
    from docx.shared import Pt, RGBColor

    doc = Document()
    doc.add_heading("Features for LibreOffice", 0)
    doc.add_heading("Formatting", 1)
    p = doc.add_paragraph("Plain, ")
    p.add_run("bold").bold = True
    p.add_run(", ")
    p.add_run("italic").italic = True
    p.add_run(", ")
    r = p.add_run("red 14 pt")
    r.font.color.rgb = RGBColor(0xC0, 0x00, 0x00)
    r.font.size = Pt(14)
    p.add_run(". Indonesian: Rapat dimulai pukul sembilan. 中文 日本語 😀")
    p = doc.add_paragraph("Centred")
    p.alignment = WD_ALIGN_PARAGRAPH.CENTER
    doc.add_heading("Lists", 2)
    for t in ("Alpha", "Beta"):
        doc.add_paragraph(t, style="List Bullet")
    for t in ("One", "Two"):
        doc.add_paragraph(t, style="List Number")
    doc.add_heading("Table", 2)
    t = doc.add_table(rows=3, cols=3)
    t.style = "Table Grid"
    for i, row in enumerate(t.rows):
        for j, cell in enumerate(row.cells):
            cell.text = f"R{i + 1}C{j + 1}"
    t.cell(0, 0).merge(t.cell(0, 1)).text = "Merged"
    t.cell(1, 2).merge(t.cell(2, 2)).text = "Tall"
    pic = Path(tempfile.mkdtemp()) / "p.png"
    pic.write_bytes(png_bytes())
    doc.add_picture(str(pic))
    sec = doc.add_section()
    sec.orientation = WD_ORIENT.LANDSCAPE
    sec.page_width, sec.page_height = sec.page_height, sec.page_width
    doc.add_paragraph("Landscape section")
    doc.sections[0].header.paragraphs[0].text = "Features header"
    doc.save(path)


def convert(soffice, src, outdir):
    work = Path(tempfile.mkdtemp(dir=Path.home() / ".cache"))
    profile = (Path.home() / ".cache" / "lo-profile-gen-rtf").as_uri()
    cmd = [
        str(soffice),
        f"-env:UserInstallation={profile}",
        "--headless",
        "--convert-to",
        "rtf",
        "--outdir",
        str(work),
        str(src.resolve()),
    ]
    # Exit status 81: a new profile was set up and LibreOffice restarted.
    if subprocess.run(cmd, capture_output=True, timeout=180).returncode != 0:
        subprocess.run(cmd, check=True, capture_output=True, timeout=180)
    made = work / (src.stem + ".rtf")
    shutil.move(made, outdir / ("lo_" + src.stem + ".rtf"))
    shutil.rmtree(work, ignore_errors=True)


def main():
    args = sys.argv[1:]
    soffice = None
    if "--soffice" in args:
        i = args.index("--soffice")
        soffice = Path(args[i + 1]).expanduser().resolve()
        del args[i : i + 2]
    out = Path(args[0] if args else "tests/corpus/rtf")
    out.mkdir(parents=True, exist_ok=True)
    sample = word_sample()
    (out / "word_sample.rtf").write_bytes(sample.encode("ascii"))
    (out / "charsets.rtf").write_bytes(charsets().encode("ascii"))
    (out / "pn_lists.rtf").write_bytes(pn_lists().encode("ascii"))
    cut = sample.index(r"Spans two rows")
    (out / "truncated.rtf").write_bytes(sample[:cut].encode("ascii"))
    if soffice:
        tmp = Path(tempfile.mkdtemp(dir=Path.home() / ".cache"))
        features = tmp / "features.docx"
        features_docx(features)
        for src in [
            ROOT / "tests/corpus/docx/report.docx",
            ROOT / "tests/corpus/docx/letter.docx",
            ROOT / "tests/corpus/docx/invoice.docx",
            features,
        ]:
            convert(soffice, src, out)
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
