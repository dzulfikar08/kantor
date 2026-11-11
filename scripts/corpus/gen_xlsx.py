#!/usr/bin/env python3
"""Generate the .xlsx test corpus in tests/corpus/xlsx/ (plan.md §0 item 9, §5, §7).

Standard library only: the files are written as raw SpreadsheetML (ECMA-376) so
every byte is ours (CC0, see tests/corpus/README.md) and the output is
deterministic (fixed zip timestamps, fixed part order). Re-running the script
must not change any committed file.

Formula cells carry cached values computed here, the way Excel would compute
them, so the fidelity test can compare IronCalc's results with "what Excel
stored" (plan.md §7 "Formulas").

Besides the workbooks, the script writes `expectations.tsv`: what Excel shows
for selected cells (number formats) and the layout we wrote (column widths, row
heights, frozen panes). The Rust test checks IronCalc's import against it.

Usage:
  python3 scripts/corpus/gen_xlsx.py [out_dir]   write the corpus
  python3 scripts/corpus/gen_xlsx.py --check     verify the committed corpus is
      what this script generates (compares unzipped parts, so a different zlib
      build compressing differently doesn't count as a change)
"""

from __future__ import annotations

import datetime as dt
import os
import sys
import tempfile
import zipfile
from dataclasses import dataclass, field
from xml.sax.saxutils import escape

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DEFAULT_OUT = os.path.join(ROOT, "tests", "corpus", "xlsx")
ZIP_TIME = (1980, 1, 1, 0, 0, 0)
EPOCH = dt.date(1899, 12, 30)


def serial(d: dt.date) -> int:
    return (d - EPOCH).days


def col_name(c: int) -> str:
    """0 -> A."""
    s = ""
    c += 1
    while c:
        c, r = divmod(c - 1, 26)
        s = chr(65 + r) + s
    return s


def ref(r: int, c: int) -> str:
    """0-based (row, col) -> A1."""
    return f"{col_name(c)}{r + 1}"


def parse_ref(a1: str) -> tuple[int, int]:
    letters = "".join(ch for ch in a1 if ch.isalpha())
    digits = "".join(ch for ch in a1 if ch.isdigit())
    c = 0
    for ch in letters:
        c = c * 26 + (ord(ch) - 64)
    return int(digits) - 1, c - 1


def attr(s: str) -> str:
    return escape(s, {'"': "&quot;"})


def text_xml(s: str) -> str:
    """<t> element; keeps leading/trailing/multiple spaces and newlines."""
    space = ' xml:space="preserve"' if (s != s.strip() or "\n" in s or "  " in s) else ""
    return f"<t{space}>{escape(s)}</t>"


# ---------------------------------------------------------------- styles


@dataclass(frozen=True)
class Style:
    numfmt: str | int = 0  # builtin id or custom format code
    bold: bool = False
    italic: bool = False
    underline: bool = False
    strike: bool = False
    size: float = 11
    font: str = "Calibri"
    color: str | None = None  # RRGGBB
    fill: str | None = None  # RRGGBB solid fill
    border: tuple[tuple[str, str], ...] = ()  # ((side, style), ...)
    halign: str | None = None
    valign: str | None = None
    wrap: bool = False


class Styles:
    def __init__(self) -> None:
        self.xfs: list[Style] = [Style()]
        self.numfmts: dict[str, int] = {}

    def id(self, st: Style | None) -> int:
        if st is None or st == Style():
            return 0
        if st not in self.xfs:
            self.xfs.append(st)
            if isinstance(st.numfmt, str) and st.numfmt not in self.numfmts:
                self.numfmts[st.numfmt] = 164 + len(self.numfmts)
        return self.xfs.index(st)

    def xml(self) -> str:
        fonts: list[tuple] = []
        fills: list[str | None] = [None, "gray125"]
        borders: list[tuple] = [()]
        xf_rows = []

        def index(lst: list, item) -> int:
            if item not in lst:
                lst.append(item)
            return lst.index(item)

        for st in self.xfs:
            f = index(fonts, (st.bold, st.italic, st.underline, st.strike, st.size, st.font, st.color))
            fi = index(fills, st.fill) if st.fill else 0
            b = index(borders, st.border)
            nf = st.numfmt if isinstance(st.numfmt, int) else self.numfmts[st.numfmt]
            align = ""
            if st.halign or st.valign or st.wrap:
                a = ""
                if st.halign:
                    a += f' horizontal="{st.halign}"'
                if st.valign:
                    a += f' vertical="{st.valign}"'
                if st.wrap:
                    a += ' wrapText="1"'
                align = f"<alignment{a}/>"
            applies = ""
            if nf:
                applies += ' applyNumberFormat="1"'
            if f:
                applies += ' applyFont="1"'
            if fi:
                applies += ' applyFill="1"'
            if b:
                applies += ' applyBorder="1"'
            if align:
                applies += ' applyAlignment="1"'
            body = f">{align}</xf>" if align else "/>"
            xf_rows.append(
                f'<xf numFmtId="{nf}" fontId="{f}" fillId="{fi}" borderId="{b}" xfId="0"{applies}{body}'
            )

        out = ['<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
               '<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">']
        if self.numfmts:
            out.append(f'<numFmts count="{len(self.numfmts)}">')
            for code, i in self.numfmts.items():
                out.append(f'<numFmt numFmtId="{i}" formatCode="{attr(code)}"/>')
            out.append("</numFmts>")
        out.append(f'<fonts count="{len(fonts)}">')
        for bold, italic, underline, strike, size, name, color in fonts:
            out.append(
                "<font>"
                + ("<b/>" if bold else "")
                + ("<i/>" if italic else "")
                + ("<strike/>" if strike else "")
                + ("<u/>" if underline else "")
                + f'<sz val="{size:g}"/>'
                + (f'<color rgb="FF{color}"/>' if color else '<color theme="1"/>')
                + f'<name val="{attr(name)}"/><family val="2"/>'
                + ('<scheme val="minor"/>' if name == "Calibri" else "")
                + "</font>"
            )
        out.append("</fonts>")
        out.append(f'<fills count="{len(fills)}">')
        for fill in fills:
            if fill is None:
                out.append('<fill><patternFill patternType="none"/></fill>')
            elif fill == "gray125":
                out.append('<fill><patternFill patternType="gray125"/></fill>')
            else:
                out.append(
                    f'<fill><patternFill patternType="solid"><fgColor rgb="FF{fill}"/>'
                    '<bgColor indexed="64"/></patternFill></fill>'
                )
        out.append("</fills>")
        out.append(f'<borders count="{len(borders)}">')
        for border in borders:
            sides = dict(border)
            parts = []
            for side in ("left", "right", "top", "bottom"):
                if side in sides:
                    parts.append(f'<{side} style="{sides[side]}"><color indexed="64"/></{side}>')
                else:
                    parts.append(f"<{side}/>")
            out.append("<border>" + "".join(parts) + "<diagonal/></border>")
        out.append("</borders>")
        out.append('<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>')
        out.append(f'<cellXfs count="{len(xf_rows)}">' + "".join(xf_rows) + "</cellXfs>")
        out.append('<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>')
        out.append('<dxfs count="0"/><tableStyles count="0"/></styleSheet>')
        return "".join(out)


# ---------------------------------------------------------------- model


@dataclass
class Cell:
    value: object = None  # number | str | bool | None (cached value for formulas)
    formula: str | None = None
    style: Style | None = None
    error: str | None = None  # cached error, e.g. "#DIV/0!"


@dataclass
class Sheet:
    name: str
    cells: dict[tuple[int, int], Cell] = field(default_factory=dict)
    cols: list[tuple[int, int, float, bool]] = field(default_factory=list)  # (min, max, width, hidden) 1-based
    rows: dict[int, tuple[float | None, bool]] = field(default_factory=dict)  # row(1-based) -> (height, hidden)
    merges: list[str] = field(default_factory=list)
    freeze: tuple[int, int] = (0, 0)  # (rows, cols)
    state: str = "visible"
    tab_color: str | None = None
    comments: list[tuple[str, str, str]] = field(default_factory=list)  # (ref, author, text)
    links: list[tuple[str, str]] = field(default_factory=list)  # (ref, url)

    def set(self, a1: str, value=None, formula: str | None = None, style: Style | None = None,
            error: str | None = None) -> None:
        self.cells[parse_ref(a1)] = Cell(value, formula, style, error)


@dataclass
class Book:
    sheets: list[Sheet]
    names: list[tuple[str, str, int | None]] = field(default_factory=list)  # (name, formula, localSheetId)
    custom_props: list[tuple[str, str]] = field(default_factory=list)


EXPECT: list[tuple[str, str, str, str, str]] = []  # (file, kind, sheet, where, expected)


def expect(file: str, kind: str, sheet: str, where: str, expected: str) -> None:
    EXPECT.append((file, kind, sheet, where, expected))


# ---------------------------------------------------------------- writer

NS_MAIN = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
NS_R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
NS_PKG_REL = "http://schemas.openxmlformats.org/package/2006/relationships"
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
XML_DECL = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'


def sheet_xml(sh: Sheet, styles: Styles, sst: dict[str, int], rels: list[tuple[str, str, str, str]]) -> str:
    out = [XML_DECL, f'<worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}">']
    if sh.tab_color:
        out.append(f'<sheetPr><tabColor rgb="FF{sh.tab_color}"/></sheetPr>')
    if sh.cells:
        rs = [r for r, _ in sh.cells]
        cs = [c for _, c in sh.cells]
        dim = f"{ref(min(rs), min(cs))}:{ref(max(rs), max(cs))}"
        if dim.split(":")[0] == dim.split(":")[1]:
            dim = dim.split(":")[0]
    else:
        dim = "A1"
    out.append(f'<dimension ref="{dim}"/>')
    fr, fc = sh.freeze
    if fr or fc:
        top_left = ref(fr, fc)
        pane = "bottomRight" if fr and fc else ("bottomLeft" if fr else "topRight")
        split = (f' xSplit="{fc}"' if fc else "") + (f' ySplit="{fr}"' if fr else "")
        out.append(
            f'<sheetViews><sheetView workbookViewId="0"><pane{split} topLeftCell="{top_left}" '
            f'activePane="{pane}" state="frozen"/></sheetView></sheetViews>'
        )
    out.append('<sheetFormatPr defaultRowHeight="15"/>')
    if sh.cols:
        out.append("<cols>")
        for mn, mx, width, hidden in sh.cols:
            out.append(
                f'<col min="{mn}" max="{mx}" width="{width:g}" customWidth="1"'
                + (' hidden="1"' if hidden else "") + "/>"
            )
        out.append("</cols>")
    out.append("<sheetData>")
    by_row: dict[int, list[int]] = {}
    for r, c in sh.cells:
        by_row.setdefault(r, []).append(c)
    for r in sh.rows:
        by_row.setdefault(r - 1, [])
    for r in sorted(by_row):
        height, hidden = sh.rows.get(r + 1, (None, False))
        ra = ""
        if height is not None:
            ra += f' ht="{height:g}" customHeight="1"'
        if hidden:
            ra += ' hidden="1"'
        cols = sorted(by_row[r])
        if not cols and not ra:
            continue
        out.append(f'<row r="{r + 1}"{ra}>')
        for c in cols:
            cell = sh.cells[(r, c)]
            s = styles.id(cell.style)
            sa = f' s="{s}"' if s else ""
            a1 = ref(r, c)
            if cell.formula is not None:
                f = f"<f>{escape(cell.formula)}</f>"
                v = cell.value
                if cell.error:
                    out.append(f'<c r="{a1}"{sa} t="e">{f}<v>{cell.error}</v></c>')
                elif isinstance(v, bool):
                    out.append(f'<c r="{a1}"{sa} t="b">{f}<v>{int(v)}</v></c>')
                elif isinstance(v, str):
                    out.append(f'<c r="{a1}"{sa} t="str">{f}<v>{escape(v)}</v></c>')
                else:
                    out.append(f'<c r="{a1}"{sa}>{f}<v>{num(v)}</v></c>')
            else:
                v = cell.value
                if cell.error:
                    out.append(f'<c r="{a1}"{sa} t="e"><v>{cell.error}</v></c>')
                elif v is None:
                    out.append(f'<c r="{a1}"{sa}/>')
                elif isinstance(v, bool):
                    out.append(f'<c r="{a1}"{sa} t="b"><v>{int(v)}</v></c>')
                elif isinstance(v, str):
                    if v not in sst:
                        sst[v] = len(sst)
                    out.append(f'<c r="{a1}"{sa} t="s"><v>{sst[v]}</v></c>')
                else:
                    out.append(f'<c r="{a1}"{sa}><v>{num(v)}</v></c>')
        out.append("</row>")
    out.append("</sheetData>")
    if sh.merges:
        out.append(f'<mergeCells count="{len(sh.merges)}">')
        out.extend(f'<mergeCell ref="{m}"/>' for m in sh.merges)
        out.append("</mergeCells>")
    if sh.links:
        out.append("<hyperlinks>")
        for a1, url in sh.links:
            rid = f"rId{len(rels) + 1}"
            rels.append((rid, f"{REL}/hyperlink", url, "External"))
            out.append(f'<hyperlink ref="{a1}" r:id="{rid}"/>')
        out.append("</hyperlinks>")
    out.append('<pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/>')
    if sh.comments:
        rid = f"rId{len(rels) + 1}"
        rels.append((rid, f"{REL}/vmlDrawing", "../drawings/vmlDrawing{n}.vml", ""))
        out.append(f'<legacyDrawing r:id="{rid}"/>')
    out.append("</worksheet>")
    return "".join(out)


def num(v) -> str:
    if isinstance(v, int):
        return str(v)
    return repr(float(v)) if v != int(v) else str(int(v))


def rels_xml(rels: list[tuple[str, str, str, str]]) -> str:
    out = [XML_DECL, f'<Relationships xmlns="{NS_PKG_REL}">']
    for rid, typ, target, mode in rels:
        m = f' TargetMode="{mode}"' if mode else ""
        out.append(f'<Relationship Id="{rid}" Type="{typ}" Target="{attr(target)}"{m}/>')
    out.append("</Relationships>")
    return "".join(out)


def comments_xml(sh: Sheet) -> str:
    authors = sorted({a for _, a, _ in sh.comments})
    out = [XML_DECL, f'<comments xmlns="{NS_MAIN}"><authors>']
    out.extend(f"<author>{escape(a)}</author>" for a in authors)
    out.append("</authors><commentList>")
    for a1, author, txt in sh.comments:
        out.append(
            f'<comment ref="{a1}" authorId="{authors.index(author)}"><text><r><t>{escape(txt)}</t></r></text></comment>'
        )
    out.append("</commentList></comments>")
    return "".join(out)


def vml_xml(sh: Sheet) -> str:
    out = ['<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" '
           'xmlns:x="urn:schemas-microsoft-com:office:excel">'
           '<o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="1"/></o:shapelayout>'
           '<v:shapetype id="_x0000_t202" coordsize="21600,21600" o:spt="202" path="m,l,21600r21600,l21600,xe">'
           '<v:stroke joinstyle="miter"/><v:path gradientshapeok="t" o:connecttype="rect"/></v:shapetype>']
    for i, (a1, _, _) in enumerate(sh.comments):
        r, c = parse_ref(a1)
        out.append(
            f'<v:shape id="_x0000_s{1025 + i}" type="#_x0000_t202" '
            'style="position:absolute;margin-left:59.25pt;margin-top:1.5pt;width:108pt;height:59.25pt;'
            'z-index:1;visibility:hidden" fillcolor="#ffffe1" o:insetmode="auto">'
            '<v:fill color2="#ffffe1"/><v:shadow on="t" color="black" obscured="t"/>'
            '<v:path o:connecttype="none"/><v:textbox style="mso-direction-alt:auto"/>'
            '<x:ClientData ObjectType="Note"><x:MoveWithCells/><x:SizeWithCells/>'
            f'<x:AutoFill>False</x:AutoFill><x:Row>{r}</x:Row><x:Column>{c}</x:Column></x:ClientData></v:shape>'
        )
    out.append("</xml>")
    return "".join(out)


def write_book(path: str, book: Book) -> None:
    styles = Styles()
    sst: dict[str, int] = {}
    parts: list[tuple[str, str]] = []
    overrides = [
        ("/xl/workbook.xml", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"),
        ("/xl/styles.xml", "application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"),
        ("/docProps/core.xml", "application/vnd.openxmlformats-package.core-properties+xml"),
        ("/docProps/app.xml", "application/vnd.openxmlformats-officedocument.extended-properties+xml"),
    ]
    wb_rels: list[tuple[str, str, str, str]] = []
    sheet_entries = []
    vml_n = 0
    for i, sh in enumerate(book.sheets, start=1):
        rels: list[tuple[str, str, str, str]] = []
        xml = sheet_xml(sh, styles, sst, rels)
        if sh.comments:
            vml_n += 1
            rels = [(rid, typ, tgt.replace("{n}", str(vml_n)), m) for rid, typ, tgt, m in rels]
            rels.append((f"rId{len(rels) + 1}", f"{REL}/comments", f"../comments{vml_n}.xml", ""))
            parts.append((f"xl/comments{vml_n}.xml", comments_xml(sh)))
            parts.append((f"xl/drawings/vmlDrawing{vml_n}.vml", vml_xml(sh)))
            overrides.append((f"/xl/comments{vml_n}.xml",
                              "application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml"))
        parts.append((f"xl/worksheets/sheet{i}.xml", xml))
        if rels:
            parts.append((f"xl/worksheets/_rels/sheet{i}.xml.rels", rels_xml(rels)))
        overrides.append((f"/xl/worksheets/sheet{i}.xml",
                          "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"))
        rid = f"rId{i}"
        wb_rels.append((rid, f"{REL}/worksheet", f"worksheets/sheet{i}.xml", ""))
        state = f' state="{sh.state}"' if sh.state != "visible" else ""
        sheet_entries.append(f'<sheet name="{attr(sh.name)}" sheetId="{i}"{state} r:id="{rid}"/>')
    n = len(book.sheets)
    wb_rels.append((f"rId{n + 1}", f"{REL}/styles", "styles.xml", ""))
    if sst:
        wb_rels.append((f"rId{n + 2}", f"{REL}/sharedStrings", "sharedStrings.xml", ""))
        overrides.append(("/xl/sharedStrings.xml",
                          "application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"))
        strings = sorted(sst, key=sst.get)
        parts.append(("xl/sharedStrings.xml",
                      XML_DECL + f'<sst xmlns="{NS_MAIN}" count="{len(strings)}" uniqueCount="{len(strings)}">'
                      + "".join(f"<si>{text_xml(s)}</si>" for s in strings) + "</sst>"))
    names = ""
    if book.names:
        names = "<definedNames>" + "".join(
            f'<definedName name="{attr(nm)}"' + (f' localSheetId="{loc}"' if loc is not None else "")
            + f">{escape(f)}</definedName>"
            for nm, f, loc in book.names
        ) + "</definedNames>"
    workbook = (XML_DECL + f'<workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}">'
                '<workbookPr defaultThemeVersion="164011"/>'
                '<bookViews><workbookView xWindow="0" yWindow="0" windowWidth="16384" windowHeight="8192"/></bookViews>'
                f"<sheets>{''.join(sheet_entries)}</sheets>{names}"
                '<calcPr calcId="191029"/></workbook>')
    root_rels = [
        ("rId1", f"{REL}/officeDocument", "xl/workbook.xml", ""),
        ("rId2", "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties",
         "docProps/core.xml", ""),
        ("rId3", f"{REL}/extended-properties", "docProps/app.xml", ""),
    ]
    if book.custom_props:
        root_rels.append(("rId4", f"{REL}/custom-properties", "docProps/custom.xml", ""))
        overrides.append(("/docProps/custom.xml",
                          "application/vnd.openxmlformats-officedocument.custom-properties+xml"))
        props = "".join(
            f'<property fmtid="{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}" pid="{i + 2}" name="{attr(k)}">'
            f"<vt:lpwstr>{escape(v)}</vt:lpwstr></property>"
            for i, (k, v) in enumerate(book.custom_props)
        )
        parts.append(("docProps/custom.xml",
                      XML_DECL + '<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/'
                      'custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/'
                      f'docPropsVTypes">{props}</Properties>'))
    core = (XML_DECL + '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/'
            'core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" '
            'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>Kantor test corpus</dc:title>'
            "<dc:creator>Kantor scripts/corpus/gen_xlsx.py</dc:creator>"
            '<dcterms:created xsi:type="dcterms:W3CDTF">2026-11-07T00:00:00Z</dcterms:created>'
            "</cp:coreProperties>")
    app = (XML_DECL + '<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">'
           "<Application>Microsoft Excel</Application></Properties>")
    defaults = ('<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
                '<Default Extension="xml" ContentType="application/xml"/>')
    if vml_n:
        defaults += '<Default Extension="vml" ContentType="application/vnd.openxmlformats-officedocument.vmlDrawing"/>'
    content_types = (XML_DECL + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
                     + defaults
                     + "".join(f'<Override PartName="{p}" ContentType="{t}"/>' for p, t in overrides)
                     + "</Types>")
    all_parts = [
        ("[Content_Types].xml", content_types),
        ("_rels/.rels", rels_xml(root_rels)),
        ("docProps/core.xml", core),
        ("docProps/app.xml", app),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", rels_xml(wb_rels)),
        ("xl/styles.xml", styles.xml()),
        *parts,
    ]
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in all_parts:
            info = zipfile.ZipInfo(name, ZIP_TIME)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            z.writestr(info, data.encode("utf-8"))


# ---------------------------------------------------------------- corpus

BOLD = Style(bold=True)


def f01_formulas() -> Book:
    data = Sheet("Data")
    items = [("P01", "Pensil", 2.5), ("P02", "Buku tulis", 12), ("P03", "Penghapus", 4.75),
             ("P04", "Penggaris", 30), ("P05", "Tas sekolah", 110.25)]
    for c, h in enumerate(["Code", "Item", "Price"]):
        data.set(ref(0, c), h, style=BOLD)
    for i, (code, name, price) in enumerate(items, start=1):
        data.set(ref(i, 0), code)
        data.set(ref(i, 1), name)
        data.set(ref(i, 2), price)
    prices = [p for _, _, p in items]
    total = sum(prices)
    avg = total / len(prices)
    calc = Sheet("Calc")
    rows = [
        ("Total", "SUM(Data!C2:C6)", total),
        ("Average", "AVERAGE(Data!C2:C6)", avg),
        ("High or low", 'IF(B2>100,"high","low")', "high"),
        ("Price of P03", 'VLOOKUP("P03",Data!A2:C6,3,FALSE)', 4.75),
        ("Name of P03", 'VLOOKUP("P03",Data!A2:C6,2,FALSE)', "Penghapus"),
        ("Rounded average", "ROUND(B3,0)", 32),
        ("Count > 20", 'COUNTIF(Data!C2:C6,">20")', 2),
        ("Label", 'CONCATENATE("Total: ",B2)', "Total: 159.5"),
        ("Both", "AND(B2>0,B8>1)", True),
        ("Safe divide", 'IFERROR(1/0,"n/a")', "n/a"),
        ("Divide by zero", "1/0", None),
        ("Index/match", "INDEX(Data!B2:B6,MATCH(30,Data!C2:C6,0))", "Penggaris"),
        ("Cross-sheet ref", "Data!C2*2", 5),
        ("Mixed args", "SUM(Data!C2:C3,10)", 24.5),
        ("Max", "MAX(Data!C2:C6)", 110.25),
        ("Min", "MIN(Data!C2:C6)", 2.5),
        ("Absolute refs", "$B$2-Data!$C$6", total - 110.25),
        ("Nested IF", 'IF(B3>50,"big",IF(B3>20,"medium","small"))', "medium"),
        ("SUMIF", 'SUMIF(Data!A2:A6,"P0*",Data!C2:C6)', total),
        ("Power", "2^10", 1024),
    ]
    calc.set("A1", "What", style=BOLD)
    calc.set("B1", "Result", style=BOLD)
    for i, (label, formula, value) in enumerate(rows, start=2):
        calc.set(f"A{i}", label)
        if formula == "1/0":
            calc.set(f"B{i}", formula=formula, error="#DIV/0!")
        else:
            calc.set(f"B{i}", value, formula=formula)
    return Book([data, calc])


def f02_multi_sheet() -> Book:
    months = {"Jan": [120, 80, 45.5], "Feb": [130, 75, 50], "Mar": [125.5, 90, 60.25]}
    sheets = []
    for m, vals in months.items():
        sh = Sheet(m)
        sh.set("A1", "Region", style=BOLD)
        sh.set("B1", "Sales", style=BOLD)
        for i, (region, v) in enumerate(zip(["Jawa", "Sumatra", "Bali"], vals), start=2):
            sh.set(f"A{i}", region)
            sh.set(f"B{i}", v)
        sh.set("A5", "Total")
        sh.set("B5", sum(vals), formula="SUM(B2:B4)")
        sheets.append(sh)
    summary = Sheet("Q1 Summary", tab_color="4472C4")
    summary.set("A1", "Region", style=BOLD)
    summary.set("B1", "Q1", style=BOLD)
    for i, region in enumerate(["Jawa", "Sumatra", "Bali"], start=2):
        summary.set(f"A{i}", region)
        summary.set(f"B{i}", sum(v[i - 2] for v in months.values()), formula=f"Jan!B{i}+Feb!B{i}+Mar!B{i}")
    grand = sum(sum(v) for v in months.values())
    summary.set("A5", "Grand total")
    summary.set("B5", grand, formula="SUM(B2:B4)")
    summary.set("A6", "Via 3-D ref")
    summary.set("B6", grand, formula="SUM(Jan:Mar!B2:B4)")
    summary.set("A7", "Share of Jan")
    summary.set("B7", sum(months["Jan"]) / grand, formula="Jan!B5/B5", style=Style(numfmt=10))
    hidden = Sheet("Hidden", state="hidden")
    hidden.set("A1", "secret")
    hidden.set("A2", 42)
    back = Sheet("Back refs")
    back.set("A1", "From summary")
    back.set("B1", grand, formula="'Q1 Summary'!B5")
    back.set("A2", "From hidden")
    back.set("B2", 42, formula="Hidden!A2")
    return Book([*sheets, summary, hidden, back])


def f03_formats() -> Book:
    name = "03_number_formats.xlsx"
    sh = Sheet("Formats")
    sh.cols = [(1, 1, 22, False), (2, 2, 18, False), (3, 3, 20, False)]
    sh.set("A1", "Format", style=BOLD)
    sh.set("B1", "Value", style=BOLD)
    sh.set("C1", "Excel shows", style=BOLD)
    d = dt.date(2025, 8, 17)
    t = 14.5 / 24  # 14:30
    cases = [
        ("General", 1234.5, 0, "1234.5"),
        ("0", 1234.5, 1, "1235"),
        ("0.00", 1234.5, 2, "1234.50"),
        ("#,##0", 1234567, 3, "1,234,567"),
        ("#,##0.00", 1234.5, 4, "1,234.50"),
        ("0%", 0.256, 9, "26%"),
        ("0.00%", 0.256, 10, "25.60%"),
        ("0.00E+00", 12345.678, 11, "1.23E+04"),
        ("$ currency", 1234.5, '"$"#,##0.00', "$1,234.50"),
        ("Rupiah literal", 1500000, '"Rp "#,##0', "Rp 1,500,000"),
        ("Rupiah locale tag", 1500000, "[$Rp-421]#,##0", "Rp1,500,000"),
        ("Euro after", 99.9, '#,##0.00 "€"', "99.90 €"),
        ("Negative red", -500, "#,##0;[Red]-#,##0", "-500"),
        ("Negative parens", -500, "#,##0.00;(#,##0.00)", "(500.00)"),
        ("Date m/d/yyyy (14)", serial(d), 14, "8/17/2025"),
        ("Date yyyy-mm-dd", serial(d), "yyyy-mm-dd", "2025-08-17"),
        ("Date d mmm yyyy", serial(d), "d mmm yyyy", "17 Aug 2025"),
        ("Date dddd", serial(d), "dddd", "Sunday"),
        ("Time h:mm", t, "h:mm", "14:30"),
        ("Time h:mm AM/PM", t, "h:mm AM/PM", "2:30 PM"),
        ("Date time", serial(d) + t, "yyyy-mm-dd hh:mm", "2025-08-17 14:30"),
        ("Fraction", 1.5, "# ?/?", "1 1/2"),
        ("Text @", "00123", 49, "00123"),
        ("Leading zeros", 42, "00000", "00042"),
        ("Thousands (K)", 1500000, '#,##0,"K"', "1,500K"),
    ]
    for i, (label, value, fmt, shows) in enumerate(cases, start=2):
        sh.set(f"A{i}", label)
        sh.set(f"B{i}", value, style=Style(numfmt=fmt))
        sh.set(f"C{i}", shows)
        expect(name, "display", "Formats", f"B{i}", shows)
    types = Sheet("Types")
    vals = [
        ("Integer", 42), ("Negative", -17), ("Float", 3.14159265358979), ("Tiny", 1e-10),
        ("Huge", 1.5e300), ("0.1+0.2", None), ("Big integer", 9007199254740993),
        ("Boolean TRUE", True), ("Boolean FALSE", False), ("Error literal", None),
        ("Empty string formula", None), ("Number as text", "123"),
    ]
    for i, (label, v) in enumerate(vals, start=1):
        types.set(f"A{i}", label)
        if label == "0.1+0.2":
            types.set(f"B{i}", 0.1 + 0.2, formula="0.1+0.2")
        elif label == "Error literal":
            types.set(f"B{i}", error="#N/A")
        elif label == "Empty string formula":
            types.set(f"B{i}", "", formula='""')
        elif label == "Big integer":
            types.set(f"B{i}", float(v))
        else:
            types.set(f"B{i}", v)
    return Book([sh, types])


def f04_dates() -> Book:
    name = "04_dates.xlsx"
    sh = Sheet("Dates")
    sh.cols = [(1, 1, 18, False), (2, 2, 16, False)]
    ymd = Style(numfmt="yyyy-mm-dd")
    start, end = dt.date(2025, 8, 17), dt.date(2025, 12, 31)
    sh.set("A1", "Start")
    sh.set("B1", serial(start), style=ymd)
    sh.set("A2", "End")
    sh.set("B2", serial(end), style=ymd)

    def edate(d: dt.date, months: int) -> dt.date:
        m = d.month - 1 + months
        y, m = d.year + m // 12, m % 12 + 1
        import calendar
        return dt.date(y, m, min(d.day, calendar.monthrange(y, m)[1]))

    def eomonth(d: dt.date, months: int) -> dt.date:
        import calendar
        e = edate(d.replace(day=1), months)
        return e.replace(day=calendar.monthrange(e.year, e.month)[1])

    def networkdays(a: dt.date, b: dt.date) -> int:
        return sum(1 for k in range((b - a).days + 1) if (a + dt.timedelta(k)).weekday() < 5)

    rows = [
        ("DATE()", "DATE(2025,8,17)", serial(start), ymd, "2025-08-17"),
        ("YEAR", "YEAR(B1)", 2025, None, "2025"),
        ("MONTH", "MONTH(B1)", 8, None, "8"),
        ("DAY", "DAY(B1)", 17, None, "17"),
        ("+30 days", "B1+30", serial(start) + 30, ymd, (start + dt.timedelta(30)).isoformat()),
        ("Days between", "B2-B1", (end - start).days, None, str((end - start).days)),
        ("EDATE +1", "EDATE(B1,1)", serial(edate(start, 1)), ymd, edate(start, 1).isoformat()),
        ("EOMONTH", "EOMONTH(B1,0)", serial(eomonth(start, 0)), ymd, eomonth(start, 0).isoformat()),
        ("WEEKDAY", "WEEKDAY(B1)", (start.isoweekday() % 7) + 1, None, str((start.isoweekday() % 7) + 1)),
        ("NETWORKDAYS", "NETWORKDAYS(B1,B2)", networkdays(start, end), None, str(networkdays(start, end))),
        ("TEXT()", 'TEXT(B1,"dd/mm/yyyy")', start.strftime("%d/%m/%Y"), None, start.strftime("%d/%m/%Y")),
        ("DATEDIF months", 'DATEDIF(B1,B2,"m")', 4, None, "4"),
        ("TIME()", "TIME(14,30,0)", 14.5 / 24, Style(numfmt="h:mm"), "14:30"),
        ("DATEVALUE", 'DATEVALUE("2025-01-31")', serial(dt.date(2025, 1, 31)), ymd, "2025-01-31"),
        ("Leap day", "DATE(2024,2,29)", serial(dt.date(2024, 2, 29)), ymd, "2024-02-29"),
        ("Pre-1900 safe", "DATE(1900,3,1)", serial(dt.date(1900, 3, 1)), ymd, "1900-03-01"),
    ]
    for i, (label, formula, value, style, shows) in enumerate(rows, start=4):
        sh.set(f"A{i}", label)
        sh.set(f"B{i}", value, formula=formula, style=style)
        expect(name, "display", "Dates", f"B{i}", shows)
    return Book([sh])


def f05_styles() -> Book:
    sh = Sheet("Styles")
    sh.cols = [(1, 1, 24, False), (2, 2, 14, False)]
    thin = tuple((s, "thin") for s in ("left", "right", "top", "bottom"))
    cases = [
        ("Bold", Style(bold=True)),
        ("Italic", Style(italic=True)),
        ("Underline", Style(underline=True)),
        ("Strikethrough", Style(strike=True)),
        ("Bold italic red", Style(bold=True, italic=True, color="FF0000")),
        ("Size 16 Arial", Style(size=16, font="Arial")),
        ("Yellow fill", Style(fill="FFFF00")),
        ("Blue fill white text", Style(fill="1F4E78", color="FFFFFF", bold=True)),
        ("Thin box border", Style(border=thin)),
        ("Thick bottom", Style(border=(("bottom", "thick"),))),
        ("Double bottom", Style(border=(("bottom", "double"),))),
        ("Dashed box", Style(border=tuple((s, "dashed") for s in ("left", "right", "top", "bottom")))),
        ("Hair top", Style(border=(("top", "hair"),))),
        ("Centered", Style(halign="center")),
        ("Right aligned", Style(halign="right")),
        ("Top aligned", Style(valign="top")),
        ("Wrapped text that is long enough to wrap", Style(wrap=True)),
        ("Everything", Style(bold=True, italic=True, fill="E2EFDA", color="375623",
                             border=thin, halign="center", numfmt=4)),
    ]
    sh.set("A1", "Style", style=BOLD)
    sh.set("B1", "Sample", style=BOLD)
    for i, (label, st) in enumerate(cases, start=2):
        sh.set(f"A{i}", label)
        sh.set(f"B{i}", 1234.5 if label == "Everything" else "Kantor", style=st)
    sh.rows[18] = (45, False)
    # Styled cells with no value: must keep their formatting.
    sh.set("D2", None, style=Style(fill="FFC7CE"))
    sh.set("D3", None, style=Style(border=thin))
    return Book([sh])


def f06_merged() -> Book:
    sh = Sheet("Merged")
    sh.set("A1", "Laporan Penjualan 2025", style=Style(bold=True, size=14, halign="center"))
    sh.merges.append("A1:E1")
    sh.set("A3", "Wilayah")
    sh.set("B3", "Jawa", style=Style(valign="center"))
    sh.merges.append("B3:B5")
    sh.set("D3", "Block 2x2")
    sh.merges.append("D3:E4")
    sh.set("A7", "Row merge", style=Style(halign="center"))
    sh.merges.append("A7:C7")
    sh.set("A9", "Not merged")
    sh.set("B9", 1)
    return Book([sh])


def f07_dimensions() -> Book:
    name = "07_dimensions.xlsx"
    sh = Sheet("Layout")
    sh.cols = [(1, 1, 5, False), (2, 2, 30, False), (3, 3, 12.5, False), (4, 4, 9, True), (5, 7, 20, False)]
    sh.rows = {1: (40, False), 3: (8, False), 5: (None, True), 6: (60, False)}
    sh.freeze = (1, 1)
    for r in range(1, 9):
        for c in range(7):
            sh.set(ref(r - 1, c), f"{col_name(c)}{r}" if r == 1 else (r - 1) * 10 + c)
    for mn, mx, width, hidden in sh.cols:
        for c in range(mn, mx + 1):
            expect(name, "colwidth", "Layout", col_name(c - 1), f"{width:g}")
            if hidden:
                expect(name, "colhidden", "Layout", col_name(c - 1), "1")
    for r, (h, hidden) in sh.rows.items():
        if h is not None:
            expect(name, "rowheight", "Layout", str(r), f"{h:g}")
        if hidden:
            expect(name, "rowhidden", "Layout", str(r), "1")
    expect(name, "frozen", "Layout", "-", "1,1")
    return Book([sh])


def f08_names() -> Book:
    inputs = Sheet("Inputs")
    inputs.set("A1", "Tax rate")
    inputs.set("B1", 0.11, style=Style(numfmt=9))
    inputs.set("A2", "Prices", style=BOLD)
    prices = [10000, 25000, 7500, 12000]
    for i, p in enumerate(prices, start=3):
        inputs.set(f"A{i}", f"Item {i - 2}")
        inputs.set(f"B{i}", p)
    report = Sheet("Report")
    report.set("A1", "Subtotal")
    report.set("B1", sum(prices), formula="SUM(Prices)")
    report.set("A2", "With tax")
    report.set("B2", sum(prices) * 1.11, formula="SUM(Prices)*(1+TaxRate)")
    report.set("A3", "Constant name")
    report.set("B3", 20, formula="Ten*2")
    report.set("A4", "Sheet-local name")
    report.set("B4", 3, formula="LocalCount")
    report.set("C1", 3)
    report.set("A5", "COUNT via name")
    report.set("B5", 4, formula="COUNT(Prices)")
    return Book([inputs, report], names=[
        ("TaxRate", "Inputs!$B$1", None),
        ("Prices", "Inputs!$B$3:$B$6", None),
        ("Ten", "10", None),
        ("LocalCount", "Report!$C$1", 1),
    ])


def f09_unicode() -> Book:
    def utf16_len(s: str) -> int:
        return len(s.encode("utf-16-le")) // 2

    texts = [
        ("Indonesian", "Selamat pagi, apa kabar? Terima kasih banyak!"),
        ("Address", "Jl. Malioboro No. 52, Yogyakarta 55271"),
        ("Javanese script", "ꦲꦏ꧀ꦱꦫꦗꦮ"),
        ("Chinese", "你好，世界"),
        ("Japanese", "こんにちは、東京"),
        ("Korean", "안녕하세요"),
        ("Arabic (RTL)", "مرحبا بالعالم"),
        ("Emoji", "Mantap 👍🏽🎉🇮🇩"),
        ("ZWJ family", "👨‍👩‍👧"),
        ("Combining accent", "Café"),
        ("XML specials", '<tag> & "quotes" \'apos\''),
        ("Spaces kept", "  two leading, two trailing  "),
        ("Newline", "baris satu\nbaris dua"),
        ("Tab", "a\tb"),
    ]
    sh = Sheet("Teks")
    sh.cols = [(1, 1, 20, False), (2, 2, 45, False)]
    sh.set("A1", "Kind", style=BOLD)
    sh.set("B1", "Text", style=BOLD)
    sh.set("C1", "LEN", style=BOLD)
    for i, (label, s) in enumerate(texts, start=2):
        sh.set(f"A{i}", label)
        sh.set(f"B{i}", s)
        # Excel's LEN counts UTF-16 code units.
        sh.set(f"C{i}", utf16_len(s), formula=f"LEN(B{i})")
    n = len(texts) + 2
    sh.set(f"A{n}", "UPPER")
    sh.set(f"B{n}", "SELAMAT DATANG", formula='UPPER("selamat datang")')
    sh.set(f"A{n + 1}", "Concat CJK")
    sh.set(f"B{n + 1}", "你好，世界 / 안녕하세요", formula=f'B5&" / "&B7')
    cjk = Sheet("数据")
    cjk.set("A1", "名前")
    cjk.set("B1", "值")
    cjk.set("A2", "東京")
    cjk.set("B2", 100)
    emoji = Sheet("Emoji 🎉")
    emoji.set("A1", "From CJK sheet")
    emoji.set("B1", 100, formula="'数据'!B2")
    emoji.set("A2", "😀")
    return Book([sh, cjk, emoji])


def f10_large() -> Book:
    sh = Sheet("Data")
    rows, cols = 1000, 8
    seed = 12345
    for c in range(cols):
        sh.set(ref(0, c), f"Col {col_name(c)}", style=BOLD)
    sh.set(ref(0, cols), "Row sum", style=BOLD)
    sh.set(ref(0, cols + 1), "Label", style=BOLD)
    total = 0
    for r in range(1, rows + 1):
        vals = []
        for c in range(cols):
            seed = (seed * 1103515245 + 12345) % 2**31
            v = seed % 10000 / 100  # two decimals
            vals.append(v)
            sh.set(ref(r, c), v)
        s = sum(vals)
        total += s
        sh.set(ref(r, cols), round(s, 10), formula=f"SUM(A{r + 1}:H{r + 1})")
        sh.set(ref(r, cols + 1), f"row {r}")
    sh.set(ref(rows + 1, cols - 1), "Total", style=BOLD)
    sh.set(ref(rows + 1, cols), round(total, 8), formula=f"SUM(I2:I{rows + 1})", style=BOLD)
    sh.freeze = (1, 0)
    return Book([sh])


def f11_empty() -> Book:
    first = Sheet("Kosong")
    mid = Sheet("Data")
    mid.set("A1", "Only sheet with data")
    mid.set("B2", 1)
    mid.set("C3", formula="B2*2", value=2)
    last = Sheet("Empty too")
    return Book([first, mid, last])


def f12_parts() -> Book:
    sh = Sheet("Notes")
    sh.set("A1", "Cell with a comment")
    sh.set("B1", 100)
    sh.set("A2", "Link to Kantor")
    sh.links.append(("A2", "https://github.com/dzulfikar08/kantor"))
    sh.comments = [("B1", "Kantor", "Budget figure from the planning meeting."),
                   ("A3", "Kantor", "Komentar dalam bahasa Indonesia.")]
    sh.set("A3", "Second comment")
    return Book([sh], custom_props=[("Department", "Finance"), ("Project", "Kantor corpus")])


FILES = [
    ("01_formulas.xlsx", f01_formulas),
    ("02_multi_sheet.xlsx", f02_multi_sheet),
    ("03_number_formats.xlsx", f03_formats),
    ("04_dates.xlsx", f04_dates),
    ("05_styles.xlsx", f05_styles),
    ("06_merged_cells.xlsx", f06_merged),
    ("07_dimensions.xlsx", f07_dimensions),
    ("08_defined_names.xlsx", f08_names),
    ("09_unicode.xlsx", f09_unicode),
    ("10_large_10k.xlsx", f10_large),
    ("11_empty_sheets.xlsx", f11_empty),
    ("12_comments_links_props.xlsx", f12_parts),
]


# Written by the other generators, which check them themselves (fidelity.yml runs each --check):
# gen_features_xlsx.py (13–18), gen_validation_xlsx.py (19), gen_charts_xlsx.py (20),
# gen_pivot_xlsx.py (21).
FEATURE_FILES = [
    "13_tables.xlsx",
    "14_charts_images.xlsx",
    "15_sparklines_cf_dv.xlsx",
    "16_print_protect_outline.xlsx",
    "17_cell_styles.xlsx",
    "18_conditional_formatting.xlsx",
    "19_data_validation.xlsx",
    "20_charts.xlsx",
    "21_pivot_tables.xlsx",
]


def generate(out: str) -> list[str]:
    EXPECT.clear()
    os.makedirs(out, exist_ok=True)
    names = []
    for name, make in FILES:
        write_book(os.path.join(out, name), make())
        names.append(name)
    with open(os.path.join(out, "expectations.tsv"), "w", encoding="utf-8", newline="\n") as f:
        f.write("# Generated by scripts/corpus/gen_xlsx.py. file\tkind\tsheet\twhere\texpected\n")
        for row in EXPECT:
            f.write("\t".join(row) + "\n")
    return names + ["expectations.tsv"]


def unpacked(path: str) -> dict[str, bytes] | bytes | None:
    if not os.path.exists(path):
        return None
    if path.endswith(".xlsx"):
        with zipfile.ZipFile(path) as z:
            return {n: z.read(n) for n in z.namelist()}
    with open(path, "rb") as f:
        return f.read()


def check() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        stale = [n for n in generate(tmp)
                 if unpacked(os.path.join(tmp, n)) != unpacked(os.path.join(DEFAULT_OUT, n))]
    committed = {n for n in os.listdir(DEFAULT_OUT) if n.endswith(".xlsx")}
    extra = sorted(committed - {n for n, _ in FILES} - set(FEATURE_FILES))
    for n in stale:
        print(f"out of date: tests/corpus/xlsx/{n}")
    for n in extra:
        print(f"not generated by this script (list it in FILES or remove it): {n}")
    if stale or extra:
        print("run: python3 scripts/corpus/gen_xlsx.py")
        return 1
    print("corpus is up to date")
    return 0


def main() -> None:
    if sys.argv[1:] == ["--check"]:
        sys.exit(check())
    out = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_OUT
    for name in generate(out):
        print(f"wrote {name}")


if __name__ == "__main__":
    main()
