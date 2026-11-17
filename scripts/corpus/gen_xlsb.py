#!/usr/bin/env python3
"""Write tests/corpus/xls/styled.xlsb (CC0, synthetic), an Excel binary
workbook built record by record with the standard library only
(deterministic: same bytes every run):

* Sheet "Data": A1 "Quarterly sales" in bold red 14 pt Arial, centred and
  merged over A1:C1, on a 30 pt row; A2:B2 headers with a thin bottom
  border; B3:B4 numbers in a custom "#,##0.00" format, B4 on a yellow fill;
  C3 a percentage (built-in format 10), right-aligned with wrapping; column
  A 20 characters wide, B 15, D hidden; a second merge B6:C7.
* Sheet "Plain": values only, default widths.

And tests/corpus/xls/features.xlsb, the same workbook with:

* defined names: "Amounts" (Data!$B$3:$B$4, workbook-wide, through the
  externals' self-reference) and "Rate" (Data!$C$3, local to "Data");
* data validation on "Data": D3:D4 a list "Yes,No" with an error message,
  E3:E4 a whole number between 1 and 10 with an input message;
* A5 on a darkDown pattern fill (red on white);
* a drawing on "Data" (BrtDrawing; drawings and charts are XML parts in
  .xlsb too): a PNG picture and a column chart of Data!$B$3:$B$4;
* conditional formatting on "Data" (BrtBeginConditionalFormatting /
  BrtBeginCFRule, [MS-XLSB] 2.4.34 and 2.4.23) with two differential
  formats in styles.bin (BrtDXF, XFProps): in priority order, B3:B4 cell
  value > 1000 (bold dark red on light red, stop if true), A3:A4 text
  contains "orth" (green, "0.0%"), C3 the formula C3<1, H1:H5 a two-colour
  scale, I1:I5 a data bar (bar only), J1:J5 3 traffic lights (icon only),
  K1:K5 top 2, L1:L5 bottom 10 %, M1:M5 above average, N1:N5 duplicates,
  O1:O5 blanks, P1:P5 dates yesterday, Q1:Q5 between 3 and 6, R1:R5 a
  5-rating icon set.

Regenerate with `python3 scripts/corpus/gen_xlsb.py`; `--check` compares
with the files in the repository.
"""
import io
import struct
import sys
import zipfile
from pathlib import Path

OUT = Path(__file__).resolve().parents[2] / "tests/corpus/xls/styled.xlsb"
OUT_FEATURES = OUT.with_name("features.xlsb")
STAMP = (2026, 11, 17, 0, 0, 0)


def var(v):
    out = bytearray()
    while True:
        b = v & 0x7F
        v >>= 7
        if v:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def rec(ty, data=b""):
    return var(ty) + var(len(data)) + data


def wide(s):
    units = s.encode("utf-16-le")
    return struct.pack("<I", len(units) // 2) + units


def color_rgb(r, g, b):
    return bytes([(2 << 1) | 1, 0]) + struct.pack("<h", 0) + bytes([r, g, b, 255])


AUTO = bytes([1, 0, 0, 0, 0, 0, 0, 0])


def font(height_pt, bold=False, color=AUTO, name="Arial"):
    return rec(
        43,
        struct.pack("<HHHHBBBB", int(height_pt * 20), 0, 700 if bold else 400, 0, 0, 2, 0, 0)
        + color
        + bytes([0])
        + wide(name),
    )


def fill(pattern, fore=AUTO, back=AUTO):
    return rec(45, struct.pack("<I", pattern) + fore + back + struct.pack("<I", 0) + bytes(40) + struct.pack("<I", 0))


def border(bottom=0):
    side = lambda dg: bytes([dg, 0]) + AUTO  # noqa: E731
    return rec(46, bytes([0]) + side(0) + side(bottom) + side(0) + side(0) + side(0))


def xf(fmt=0, font_=0, fill_=0, border_=0, halign=0, valign=2, wrap=False):
    flags = halign | valign << 3 | (0x40 if wrap else 0) | 0x1000  # locked
    return rec(47, struct.pack("<HHHHHBBHBB", 0xFFFF, fmt, font_, fill_, border_, 0, 0, flags, 0, 0))


def xfprop(ty, data):
    return struct.pack("<HH", ty, len(data) + 4) + data


def dxf(props):
    # BrtDXF: fNewBorder and unused bits, then XFProps (reserved, cprops).
    return rec(507, struct.pack("<HHH", 0, 0, len(props)) + b"".join(props))


def styles(features=False):
    out = rec(278)  # BrtBeginStyleSheet
    out += rec(615, struct.pack("<I", 1)) + rec(44, struct.pack("<H", 164) + wide("#,##0.00")) + rec(616)
    out += rec(611, struct.pack("<I", 2))
    out += font(10) + font(14, bold=True, color=color_rgb(0xC0, 0, 0))
    out += rec(612)
    out += rec(603, struct.pack("<I", 4 if features else 3))
    out += fill(0) + fill(17) + fill(1, fore=color_rgb(0xFF, 0xFF, 0x00))
    if features:
        # darkDown (7): red stripes on white.
        out += fill(7, fore=color_rgb(0xC0, 0, 0), back=color_rgb(0xFF, 0xFF, 0xFF))
    out += rec(604)
    out += rec(613, struct.pack("<I", 2)) + border() + border(bottom=1) + rec(614)
    out += rec(626, struct.pack("<I", 1)) + xf() + rec(627)
    out += rec(617, struct.pack("<I", 7 if features else 6))
    out += xf()  # 0 default
    out += xf(font_=1, halign=2, valign=1)  # 1 title
    out += xf(border_=1)  # 2 header
    out += xf(fmt=164)  # 3 amount
    out += xf(fmt=164, fill_=2)  # 4 amount on yellow
    out += xf(fmt=10, halign=3, wrap=True)  # 5 percent
    if features:
        out += xf(fill_=3)  # 6 striped
    out += rec(618)
    if features:
        out += rec(505, struct.pack("<I", 2))  # BrtBeginDXFs
        out += dxf([xfprop(0x05, color_rgb(0x9C, 0, 0x06)), xfprop(0x19, struct.pack("<H", 700)),
                    xfprop(0x00, bytes([1])), xfprop(0x02, color_rgb(0xFF, 0xC7, 0xCE))])
        code = "0.0%".encode("utf-16-le")
        out += dxf([xfprop(0x00, bytes([1])), xfprop(0x02, color_rgb(0xC6, 0xEF, 0xCE)),
                    xfprop(0x26, struct.pack("<H", len(code) // 2) + code),
                    xfprop(0x29, struct.pack("<H", 170))])
        out += rec(506)  # BrtEndDXFs
    out += rec(279)  # BrtEndStyleSheet
    return out


def cell(col, style):
    return struct.pack("<II", col, style & 0xFFFFFF)


def row_hdr(r, height_pt=None):
    flags11 = 0x20 if height_pt else 0  # fUnsynced: custom height
    miy = int((height_pt or 15) * 20)
    return rec(0, struct.pack("<IIHBBBI", r, 0, miy, 0, flags11, 0, 0))


def st(col, style, s):
    return rec(6, cell(col, style) + wide(s))


def real(col, style, v):
    return rec(5, cell(col, style) + struct.pack("<d", v))


def col_info(first, last, width, hidden=False):
    flags = (1 if hidden else 0) | 2
    return rec(60, struct.pack("<IIIIH", first, last, int(width * 256), 0, flags))


def merge(r1, r2, c1, c2):
    return rec(176, struct.pack("<IIII", r1, r2, c1, c2))


def sheet(dim, cols, rows, merges, tail=b""):
    out = rec(129)  # BrtBeginSheet
    out += rec(148, struct.pack("<IIII", *dim))  # BrtWsDim
    out += rec(485, struct.pack("<IHHHH", 0xFFFFFFFF, 8, 300, 0, 0))  # BrtWsFmtInfo
    if cols:
        out += rec(390) + b"".join(cols) + rec(391)
    out += rec(145)  # BrtBeginSheetData
    out += b"".join(rows)
    out += rec(146)  # BrtEndSheetData
    if merges:
        out += rec(177, struct.pack("<I", len(merges))) + b"".join(merges) + rec(178)
    out += tail
    out += rec(130)  # BrtEndSheet
    return out


def nullable(s):
    return struct.pack("<I", 0xFFFFFFFF) if s is None else wide(s)


def formula(rgce):
    return struct.pack("<I", len(rgce)) + rgce + struct.pack("<I", 0)


def dval(flags, rng, error_title=None, error=None, prompt_title=None, prompt=None,
         f1=b"", f2=b""):
    r1, r2, c1, c2 = rng
    return rec(64, struct.pack("<II", flags, 1) + struct.pack("<IIII", r1, r2, c1, c2)
               + nullable(error_title) + nullable(error) + nullable(prompt_title)
               + nullable(prompt) + formula(f1) + formula(f2))


def ptg_str(s):
    units = s.encode("utf-16-le")
    return bytes([0x17]) + struct.pack("<H", len(units) // 2) + units


def ptg_int(v):
    return bytes([0x1E]) + struct.pack("<H", v)


def ptg_ref(r, c):
    # PtgRef, row and column relative.
    return bytes([0x24]) + struct.pack("<IH", r, c | 0xC000)


def ptg_func(iftab):
    return bytes([0x41]) + struct.pack("<H", iftab)


def cf_block(ranges, rules):
    out = rec(461, struct.pack("<III", len(rules), 0, len(ranges))
              + b"".join(struct.pack("<IIII", r1, r2, c1, c2) for r1, r2, c1, c2 in ranges))
    return out + b"".join(rules) + rec(462)


def cf_rule(ty, template, pri, dxf_id=-1, param=0, flags=0, text=None, f1=b"", f2=b"",
            body=b""):
    fmlas = [f for f in (f1, f2) if f]
    out = rec(463, struct.pack("<IIiiIIIH", ty, template, dxf_id, pri, param, 0, 0, flags)
              + struct.pack("<III", len(f1), len(f2), 0) + nullable(text)
              + b"".join(formula(f) for f in fmlas))
    return out + body + rec(464)


def cfvo(kind, num=0.0, gte=True):
    return rec(471, struct.pack("<IdIII", kind, num, 1, 1 if gte else 0, 0))


def conditional_formats():
    gt, lt = bytes([0x0D]), bytes([0x09])
    a3_text = (ptg_str("orth") + ptg_ref(2, 0) + bytes([0x42, 2]) + struct.pack("<H", 82)
               + ptg_func(3) + ptg_func(38))  # NOT(ISERROR(SEARCH("orth",A3)))
    yesterday = (ptg_ref(0, 15) + ptg_int(1) + ptg_func(285) + ptg_func(221) + ptg_int(1)
                 + bytes([0x04, 0x0B]))  # FLOOR(P1,1)=TODAY()-1
    blank = ptg_ref(0, 14) + ptg_func(118) + ptg_func(32) + ptg_int(0) + bytes([0x0B])  # LEN(TRIM(O1))=0
    col = lambda c: [(0, 4, c, c)]  # noqa: E731
    out = cf_block([(2, 3, 1, 1)], [cf_rule(1, 0, 1, 0, param=5, flags=0x2, f1=ptg_int(1000))])
    out += cf_block([(2, 3, 0, 0)], [cf_rule(2, 8, 2, 1, param=0, text="orth", f1=a3_text)])
    out += cf_block([(2, 2, 2, 2)], [cf_rule(2, 1, 3, 0, f1=ptg_ref(2, 2) + ptg_int(1) + lt)])
    scale = (rec(469) + cfvo(2) + cfvo(3) + rec(564, color_rgb(0xF8, 0x69, 0x6B))
             + rec(564, color_rgb(0x63, 0xBE, 0x7B)) + rec(470))
    out += cf_block(col(7), [cf_rule(3, 2, 4, body=scale)])
    bar = rec(467, bytes([10, 90, 0])) + cfvo(2) + cfvo(3) + rec(564, color_rgb(0x63, 0x8E, 0xC6)) + rec(468)
    out += cf_block(col(8), [cf_rule(4, 3, 5, body=bar)])
    lights = (rec(465, struct.pack("<IH", 3, 0x2)) + cfvo(4, 0) + cfvo(4, 33) + cfvo(4, 67)
              + rec(466))
    out += cf_block(col(9), [cf_rule(6, 4, 6, body=lights)])
    out += cf_block(col(10), [cf_rule(5, 5, 7, 1, param=2)])
    out += cf_block(col(11), [cf_rule(5, 5, 8, 0, param=10, flags=0x8 | 0x10)])
    out += cf_block(col(12), [cf_rule(2, 0x19, 9, 1, flags=0x4)])
    out += cf_block(col(13), [cf_rule(2, 0x1B, 10, 0)])
    out += cf_block(col(14), [cf_rule(2, 9, 11, 1, f1=blank)])
    out += cf_block(col(15), [cf_rule(2, 0x11, 12, 0, param=2, f1=yesterday)])
    out += cf_block(col(16), [cf_rule(1, 0, 13, 0, param=1, f1=ptg_int(3), f2=ptg_int(6))])
    rating = (rec(465, struct.pack("<IH", 15, 0)) + cfvo(4, 0) + cfvo(4, 20) + cfvo(4, 40)
              + cfvo(4, 60) + cfvo(4, 80) + rec(466))
    out += cf_block(col(17), [cf_rule(6, 4, 14, body=rating)])
    return out


def features_tail():
    out = conditional_formats()
    out += rec(573, bytes(14) + struct.pack("<I", 2))  # BrtBeginDVals
    # A list (valType 3) of literals (fStrLookup), blanks allowed, error shown.
    out += dval(3 | 0x80 | 0x100 | 0x80000, (2, 3, 3, 3), "Choose", "Pick Yes or No",
                f1=ptg_str("Yes\0No"))
    # A whole number (1) between (operator 0) 1 and 10, input message shown.
    out += dval(1 | 0x40000 | 0x80000, (2, 3, 4, 4), None, None, "Score", "1 to 10",
                f1=ptg_int(1), f2=ptg_int(10))
    out += rec(574)  # BrtEndDVals
    out += rec(550, wide("rId1"))  # BrtDrawing
    return out


def data_sheet(tail=b""):
    striped = [row_hdr(4) + st(0, 6, "Striped")] if tail else []
    rows = [
        row_hdr(0, 30) + st(0, 1, "Quarterly sales"),
        row_hdr(1) + st(0, 2, "Item") + st(1, 2, "Amount"),
        row_hdr(2) + st(0, 0, "North") + real(1, 3, 1234.5) + real(2, 5, 0.25),
        row_hdr(3) + st(0, 0, "South") + real(1, 4, 987.25),
    ] + striped + [
        row_hdr(5) + st(1, 0, "Merged block"),
    ]
    cols = [col_info(0, 0, 20), col_info(1, 1, 15), col_info(3, 3, 9, hidden=True)]
    return sheet((0, 6, 0, 3), cols, rows, [merge(0, 0, 0, 2), merge(5, 6, 1, 2)], tail)


def plain_sheet():
    rows = [row_hdr(0) + st(0, 0, "Plain") + real(1, 0, 42)]
    return sheet((0, 0, 0, 1), [], rows, [])


def workbook(names=False):
    def bundle(tab, rel, name):
        return rec(156, struct.pack("<II", 0, tab) + wide(rel) + wide(name))

    def name(text, itab, rgce):
        return rec(39, struct.pack("<IBI", 0, 0, itab) + wide(text) + formula(rgce)
                   + nullable(None))

    out = rec(131) + rec(143) + bundle(1, "rId1", "Data") + bundle(2, "rId2", "Plain") + rec(144)
    if names:
        # Externals: this workbook (BrtSupSelf), and one XTI for "Data".
        out += rec(353) + rec(357) + rec(362, struct.pack("<IIii", 1, 0, 0, 0)) + rec(354)
        area3d = bytes([0x3B]) + struct.pack("<HIIHH", 0, 2, 3, 1, 1)
        ref3d = bytes([0x3A]) + struct.pack("<HIH", 0, 2, 2)
        out += name("Amounts", 0xFFFFFFFF, area3d) + name("Rate", 0, ref3d)
    return out + rec(132)


CT = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
    '<Default Extension="bin" ContentType="application/vnd.ms-excel.sheet.binary.macroEnabled.main"/>'
    '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
    '<Default Extension="xml" ContentType="application/xml"/>'
    '<Override PartName="/xl/worksheets/sheet1.bin" ContentType="application/vnd.ms-excel.worksheet"/>'
    '<Override PartName="/xl/worksheets/sheet2.bin" ContentType="application/vnd.ms-excel.worksheet"/>'
    '<Override PartName="/xl/styles.bin" ContentType="application/vnd.ms-excel.styles"/>'
    "</Types>"
)
ROOT_RELS = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
    '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.bin"/>'
    "</Relationships>"
)
WB_RELS = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
    '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.bin"/>'
    '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.bin"/>'
    '<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.bin"/>'
    "</Relationships>"
)


REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"


def rels(items):
    return ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            + "".join(f'<Relationship Id="{i}" Type="{REL}/{t}" Target="{g}"/>' for i, t, g in items)
            + "</Relationships>")


def png(w, h, rgb):
    import zlib

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))

    raw = b"".join(b"\0" + bytes(rgb) * w for _ in range(h))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


XDR = ('xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" '
       'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
       f'xmlns:r="{REL}"')


def anchor(c1, r1, c2, r2, body):
    m = lambda c, r: f"<xdr:col>{c}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{r}</xdr:row><xdr:rowOff>0</xdr:rowOff>"
    return (f'<xdr:twoCellAnchor><xdr:from>{m(c1, r1)}</xdr:from><xdr:to>{m(c2, r2)}</xdr:to>'
            f"{body}<xdr:clientData/></xdr:twoCellAnchor>")


DRAWING = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    f"<xdr:wsDr {XDR}>"
    + anchor(5, 1, 7, 5, '<xdr:pic><xdr:nvPicPr><xdr:cNvPr id="2" name="Logo"/><xdr:cNvPicPr/>'
             '</xdr:nvPicPr><xdr:blipFill><a:blip r:embed="rId1"/><a:stretch><a:fillRect/>'
             '</a:stretch></xdr:blipFill><xdr:spPr><a:prstGeom prst="rect"><a:avLst/>'
             "</a:prstGeom></xdr:spPr></xdr:pic>")
    + anchor(5, 6, 12, 20, '<xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="3" '
             'name="Amounts chart"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm>'
             '<a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData '
             'uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart '
             'xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="rId2"/>'
             "</a:graphicData></a:graphic></xdr:graphicFrame>")
    + "</xdr:wsDr>"
)

CHART = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    '<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" '
    'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">'
    "<c:chart><c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>Amounts</a:t></a:r></a:p>"
    "</c:rich></c:tx><c:overlay val=\"0\"/></c:title><c:autoTitleDeleted val=\"0\"/><c:plotArea>"
    '<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/>'
    '<c:ser><c:idx val="0"/><c:order val="0"/><c:cat><c:strRef><c:f>Data!$A$3:$A$4</c:f>'
    '<c:strCache><c:ptCount val="2"/><c:pt idx="0"><c:v>North</c:v></c:pt><c:pt idx="1">'
    "<c:v>South</c:v></c:pt></c:strCache></c:strRef></c:cat><c:val><c:numRef>"
    '<c:f>Data!$B$3:$B$4</c:f><c:numCache><c:ptCount val="2"/><c:pt idx="0"><c:v>1234.5</c:v>'
    '</c:pt><c:pt idx="1"><c:v>987.25</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser>'
    '<c:axId val="1"/><c:axId val="2"/></c:barChart>'
    '<c:catAx><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling>'
    '<c:delete val="0"/><c:axPos val="b"/><c:crossAx val="2"/></c:catAx>'
    '<c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling>'
    '<c:delete val="0"/><c:axPos val="l"/><c:crossAx val="1"/></c:valAx>'
    "</c:plotArea></c:chart></c:chartSpace>"
)


def build(features=False):
    ct = CT
    if features:
        ct = ct.replace("</Types>",
                        '<Default Extension="png" ContentType="image/png"/>'
                        '<Override PartName="/xl/drawings/drawing1.xml" '
                        'ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/>'
                        '<Override PartName="/xl/charts/chart1.xml" '
                        'ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/>'
                        "</Types>")
    parts = [
        ("[Content_Types].xml", ct.encode()),
        ("_rels/.rels", ROOT_RELS.encode()),
        ("xl/_rels/workbook.bin.rels", WB_RELS.encode()),
        ("xl/workbook.bin", workbook(names=features)),
        ("xl/styles.bin", styles(features)),
        ("xl/worksheets/sheet1.bin", data_sheet(features_tail() if features else b"")),
        ("xl/worksheets/sheet2.bin", plain_sheet()),
    ]
    if features:
        parts += [
            ("xl/worksheets/_rels/sheet1.bin.rels",
             rels([("rId1", "drawing", "../drawings/drawing1.xml")]).encode()),
            ("xl/drawings/drawing1.xml", DRAWING.encode()),
            ("xl/drawings/_rels/drawing1.xml.rels",
             rels([("rId1", "image", "../media/image1.png"),
                   ("rId2", "chart", "../charts/chart1.xml")]).encode()),
            ("xl/charts/chart1.xml", CHART.encode()),
            ("xl/media/image1.png", png(40, 30, (0x2E, 0x75, 0xB6))),
        ]
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts:
            info = zipfile.ZipInfo(name, STAMP)
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, data)
    return buf.getvalue()


def main():
    for out, data in [(OUT, build()), (OUT_FEATURES, build(features=True))]:
        if "--check" in sys.argv:
            if not out.exists() or out.read_bytes() != data:
                sys.exit(f"{out} is out of date: run {sys.argv[0]}")
            print(f"{out.name} up to date")
            continue
        out.write_bytes(data)
        print(out)


if __name__ == "__main__":
    main()
