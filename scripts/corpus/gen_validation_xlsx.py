#!/usr/bin/env python3
"""Write the data validation fixtures (CC0, synthetic, deterministic):

* tests/corpus/xlsx/19_data_validation.xlsx, read by
  crates/sheets_engine/tests/validation.rs and the corpus test;
* with --sample OUT, a small form for screenshots of the drop-down, the
  input message and the error alert.

Raw SpreadsheetML with the Python standard library, so the markup is exactly
what Excel writes for validations (the x14 form in extLst for a list on
another sheet, showDropDown, error styles), without third-party attributes.

    python3 scripts/corpus/gen_validation_xlsx.py           # write the fixture
    python3 scripts/corpus/gen_validation_xlsx.py --check   # CI: it is up to date
    python3 scripts/corpus/gen_validation_xlsx.py --sample ~/.cache/x/form.xlsx
"""

import argparse
import sys
import zipfile
from pathlib import Path
from xml.sax.saxutils import escape, quoteattr

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "tests/corpus/xlsx/19_data_validation.xlsx"
ZIP_TIME = (2026, 1, 1, 0, 0, 0)

NS = (
    'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
    'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
)
X14 = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main"
XM = "http://schemas.microsoft.com/office/excel/2006/main"
DV_EXT = "{CCE6A557-97BC-4b89-ADB6-D9C93CAAB3DF}"


def col_name(c: int) -> str:
    s = ""
    while c:
        c, r = divmod(c - 1, 26)
        s = chr(65 + r) + s
    return s


def parse_ref(a1: str) -> tuple[int, int]:
    letters = "".join(ch for ch in a1 if ch.isalpha())
    c = 0
    for ch in letters:
        c = c * 26 + ord(ch) - 64
    return int(a1[len(letters):]), c


class Sheet:
    def __init__(self, name: str):
        self.name = name
        self.cells: dict[tuple[int, int], object] = {}
        self.widths: dict[int, float] = {}
        self.validations: list[str] = []
        self.x14: list[str] = []

    def set(self, a1: str, value) -> None:
        self.cells[parse_ref(a1)] = value

    def dv(self, sqref: str, kind: str, f1=None, f2=None, **attrs) -> None:
        a = {"type": kind} if kind != "none" else {}
        a.update(attrs)
        a["sqref"] = sqref
        head = "".join(f" {k}={quoteattr(str(v))}" for k, v in a.items())
        body = ""
        if f1 is not None:
            body += f"<formula1>{escape(f1)}</formula1>"
        if f2 is not None:
            body += f"<formula2>{escape(f2)}</formula2>"
        self.validations.append(f"<dataValidation{head}>{body}</dataValidation>")

    def dv_x14(self, sqref: str, f1: str, **attrs) -> None:
        a = {"type": "list"}
        a.update(attrs)
        head = "".join(f" {k}={quoteattr(str(v))}" for k, v in a.items())
        self.x14.append(
            f"<x14:dataValidation{head}><x14:formula1><xm:f>{escape(f1)}</xm:f>"
            f"</x14:formula1><xm:sqref>{sqref}</xm:sqref></x14:dataValidation>"
        )


def sheet_xml(sh: Sheet, sst: dict[str, int]) -> str:
    rows: dict[int, list[str]] = {}
    for (r, c), v in sorted(sh.cells.items()):
        ref = f"{col_name(c)}{r}"
        if isinstance(v, str) and v.startswith("="):
            cell = f'<c r="{ref}"><f>{escape(v[1:])}</f></c>'
        elif isinstance(v, str):
            idx = sst.setdefault(v, len(sst))
            cell = f'<c r="{ref}" t="s"><v>{idx}</v></c>'
        else:
            cell = f'<c r="{ref}"><v>{v}</v></c>'
        rows.setdefault(r, []).append(cell)
    data = "".join(f'<row r="{r}">{"".join(cs)}</row>' for r, cs in sorted(rows.items()))
    cols = ""
    if sh.widths:
        cols = "<cols>" + "".join(
            f'<col min="{c}" max="{c}" width="{w}" customWidth="1"/>'
            for c, w in sorted(sh.widths.items())
        ) + "</cols>"
    out = f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<worksheet {NS}>'
    out += f"{cols}<sheetData>{data}</sheetData>"
    if sh.validations:
        out += f'<dataValidations count="{len(sh.validations)}">{"".join(sh.validations)}</dataValidations>'
    out += '<pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/>'
    if sh.x14:
        out += (
            f'<extLst><ext uri="{DV_EXT}" xmlns:x14="{X14}">'
            f'<x14:dataValidations count="{len(sh.x14)}" xmlns:xm="{XM}">'
            f'{"".join(sh.x14)}</x14:dataValidations></ext></extLst>'
        )
    return out + "</worksheet>"


def write_book(path: Path, sheets: list[Sheet], names: dict[str, str]) -> None:
    sst: dict[str, int] = {}
    parts = {}
    for i, sh in enumerate(sheets, start=1):
        parts[f"xl/worksheets/sheet{i}.xml"] = sheet_xml(sh, sst)
    strings = "".join(f"<si><t xml:space=\"preserve\">{escape(s)}</t></si>" for s in sst)
    parts["xl/sharedStrings.xml"] = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        f'<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
        f'count="{len(sst)}" uniqueCount="{len(sst)}">{strings}</sst>'
    )
    parts["xl/styles.xml"] = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        '<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
        '<numFmts count="1"><numFmt numFmtId="164" formatCode="yyyy-mm-dd"/></numFmts>'
        '<fonts count="2"><font><sz val="11"/><name val="Calibri"/></font>'
        '<font><b/><sz val="11"/><name val="Calibri"/></font></fonts>'
        '<fills count="2"><fill><patternFill patternType="none"/></fill>'
        '<fill><patternFill patternType="gray125"/></fill></fills>'
        '<borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>'
        '<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>'
        '<cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs>'
        '<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>'
        "</styleSheet>"
    )
    sheet_list = "".join(
        f'<sheet name={quoteattr(sh.name)} sheetId="{i}" r:id="rId{i}"/>'
        for i, sh in enumerate(sheets, start=1)
    )
    defined = ""
    if names:
        defined = "<definedNames>" + "".join(
            f"<definedName name={quoteattr(n)}>{escape(v)}</definedName>" for n, v in names.items()
        ) + "</definedNames>"
    parts["xl/workbook.xml"] = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        f"<workbook {NS}><sheets>{sheet_list}</sheets>{defined}</workbook>"
    )
    n = len(sheets)
    rels = "".join(
        f'<Relationship Id="rId{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{i}.xml"/>'
        for i in range(1, n + 1)
    )
    rels += (
        f'<Relationship Id="rId{n + 1}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>'
        f'<Relationship Id="rId{n + 2}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>'
    )
    pkg_rels = 'xmlns="http://schemas.openxmlformats.org/package/2006/relationships"'
    parts["xl/_rels/workbook.xml.rels"] = (
        f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<Relationships {pkg_rels}>{rels}</Relationships>'
    )
    parts["_rels/.rels"] = (
        f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<Relationships {pkg_rels}>'
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>'
        "</Relationships>"
    )
    overrides = "".join(
        f'<Override PartName="/xl/worksheets/sheet{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
        for i in range(1, n + 1)
    )
    parts["[Content_Types].xml"] = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
        f"{overrides}"
        '<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>'
        '<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>'
        "</Types>"
    )
    order = ["[Content_Types].xml", "_rels/.rels", "xl/workbook.xml", "xl/_rels/workbook.xml.rels"]
    order += sorted(p for p in parts if p not in order)
    path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name in order:
            info = zipfile.ZipInfo(name, ZIP_TIME)
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, parts[name].encode("utf-8"))


def fixture() -> tuple[list[Sheet], dict[str, str]]:
    """One column per kind of rule; row 1 has headers, row 3 an invalid value."""
    form = Sheet("Form")
    lists = Sheet("Lists")
    for i, fruit in enumerate(["Apple", "Pear", "Plum", "Fig"], start=1):
        lists.set(f"A{i}", fruit)
    heads = {
        "A": "Inline", "B": "Range", "C": "Other sheet", "D": "Name", "E": "Whole",
        "F": "Decimal", "G": "Date", "I": "Time", "K": "Text length", "L": "Custom",
        "M": "No button", "N": "Indirect", "O": "Not between", "P": "Offset",
    }
    for col, title in heads.items():
        form.set(f"{col}1", title)
    for i, color in enumerate(["Red", "Green", "Blue"], start=1):
        form.set(f"H{i}", color)
    form.set("J1", 2.5)
    form.set("J2", 2)
    form.set("A3", "Bogus")
    form.set("E3", 42)
    form.set("L3", 3)
    stop = {"showInputMessage": 1, "showErrorMessage": 1}
    form.dv(
        "A2:A6", "list", '"Open,Closed,Pending"', allowBlank=1, **stop,
        errorTitle="Not a status", error="Pick Open, Closed or Pending.",
        promptTitle="Status", prompt="Pick a status from the list.",
    )
    form.dv("B2:B6", "list", "$H$1:$H$3", allowBlank=1, **stop)
    form.dv_x14("C2:C6", "Lists!$A$1:$A$4", allowBlank=1, **stop)
    form.dv("D2:D6", "list", "Fruit", allowBlank=1, **stop)
    form.dv("E2:E6", "whole", "1", "10", allowBlank=1, errorStyle="warning", **stop)
    form.dv(
        "F2:F6", "decimal", "$J$1", operator="greaterThan", allowBlank=1,
        errorStyle="information", **stop,
    )
    form.dv("G2:G6", "date", "45292", "45657", allowBlank=1, **stop)
    form.dv("I2:I6", "time", "0.5", operator="lessThan", allowBlank=1, **stop)
    form.dv("K2:K6", "textLength", "5", operator="lessThanOrEqual", allowBlank=1, **stop)
    form.dv("L2:L6", "custom", "MOD(L2,2)=0", allowBlank=1, **stop)
    form.dv("M2:M6", "list", '"x,y"', showDropDown=1, allowBlank=1, **stop)
    form.dv("N2:N6", "list", 'INDIRECT("Lists!A1:A"&$J$2)', allowBlank=1, **stop)
    form.dv("O2:O6", "whole", "1", "5", operator="notBetween", allowBlank=1, **stop)
    form.dv("P2:P6", "list", "OFFSET(Lists!$A$1,1,0,2,1)", allowBlank=1, **stop)
    form.dv("Q2:Q6", "none", promptTitle="Note", prompt="Anything goes.", showInputMessage=1)
    return [form, lists], {"Fruit": "Lists!$A$1:$A$4"}


def sample() -> tuple[list[Sheet], dict[str, str]]:
    """A small order form for screenshots."""
    form = Sheet("Orders")
    lists = Sheet("Lists")
    form.widths = {1: 14, 2: 16, 3: 10, 4: 14, 5: 14}
    for col, title in zip("ABCDE", ["Order", "Status", "Qty", "Region", "Ship by"]):
        form.set(f"{col}1", title)
    rows = [
        ("SO-1001", "Open", 4, "North"),
        ("SO-1002", "Shipped", 12, "East"),
        ("SO-1003", "Pending", 1, "South"),
        ("SO-1004", "Open", 7, "West"),
    ]
    for i, (order, status, qty, region) in enumerate(rows, start=2):
        form.set(f"A{i}", order)
        form.set(f"B{i}", status)
        form.set(f"C{i}", qty)
        form.set(f"D{i}", region)
    for i, region in enumerate(["North", "East", "South", "West"], start=1):
        lists.set(f"A{i}", region)
    stop = {"showInputMessage": 1, "showErrorMessage": 1}
    form.dv(
        "B2:B20", "list", '"Open,Pending,Shipped,Cancelled"', allowBlank=1, **stop,
        promptTitle="Status", prompt="Choose where the order stands.",
        errorTitle="Unknown status", error="Use one of the statuses in the list.",
    )
    form.dv(
        "C2:C20", "whole", "1", "100", allowBlank=1, errorStyle="warning", **stop,
        promptTitle="Quantity", prompt="Whole number from 1 to 100.",
    )
    form.dv_x14("D2:D20", "Lists!$A$1:$A$4", allowBlank=1, **stop)
    return [form, lists], {}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--sample", type=Path)
    args = ap.parse_args()
    if args.sample:
        write_book(args.sample, *sample())
        return
    if args.check:
        tmp = FIXTURE.with_suffix(".check.xlsx")
        write_book(tmp, *fixture())
        same = FIXTURE.exists() and tmp.read_bytes() == FIXTURE.read_bytes()
        tmp.unlink()
        if not same:
            print(f"{FIXTURE.relative_to(ROOT)} is out of date; run {Path(__file__).name}")
            sys.exit(1)
        return
    write_book(FIXTURE, *fixture())


if __name__ == "__main__":
    main()
