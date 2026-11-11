#!/usr/bin/env python3
"""Generate a large synthetic .xlsx for performance testing (CC0, no real data).

Shaped like a sales-order export a finance team works on: one big sheet of
orders, two master sheets the orders look up, and a summary of whole-column
SUMIFS over the orders. Every formula carries its correct cached value, as
Excel would save it, so a viewer that trusts cached values shows the right
numbers without recalculating.

    uv run --with xlsxwriter scripts/corpus/gen_big_xlsx.py [OUT.xlsx] [--rows N] [--cf] [--table]

Defaults: 90 000 orders x 9 formulas = 810 000 formula cells, about 30 MB.
Deterministic (fixed seed), streamed to disk (xlsxwriter constant_memory), so
it needs little RAM and takes well under a minute.

`--cf` adds conditional formatting over the whole order list, for measuring
it: a 3-colour scale on Amount, a data bar on Qty, an icon set on Margin %,
top 10 on Margin, duplicate customer ids, and an expression rule that tints
every row with a low margin (`=$N2="Low"` on A:S).

`--table` makes the order list an Excel table `Orders` (TableStyleMedium2,
banded rows, filter buttons), for measuring sorting and filtering; it is
written without constant_memory (xlsxwriter can't stream tables) and needs
a few GB of RAM.
"""

import argparse
import datetime as dt
import random

import xlsxwriter

REGIONS = ["Jawa Timur", "Jawa Barat", "Jawa Tengah", "DKI Jakarta", "Bali", "Sumatera Utara",
           "Sulawesi Selatan", "Kalimantan Timur"]
SEGMENTS = ["Retail", "Wholesale", "Project", "Government"]
CATEGORIES = ["Pump", "Valve", "Bearing", "Motor", "Sensor", "Cable", "Filter", "Seal"]
STATUSES = ["Open", "Delivered", "Invoiced", "Paid"]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("out", nargs="?", default="big-sample.xlsx")
    ap.add_argument("--rows", type=int, default=90_000)
    ap.add_argument("--customers", type=int, default=5_000)
    ap.add_argument("--materials", type=int, default=3_000)
    ap.add_argument("--cf", action="store_true", help="add conditional formatting rules")
    ap.add_argument("--table", action="store_true", help="make the orders an Excel table")
    args = ap.parse_args()
    rnd = random.Random(20261110)

    # Tables need the whole sheet in memory (xlsxwriter has no streamed tables).
    wb = xlsxwriter.Workbook(args.out, {"constant_memory": not args.table, "strings_to_numbers": False})
    bold = wb.add_format({"bold": True, "bg_color": "#DDEBF7", "border": 1})
    money = wb.add_format({"num_format": "#,##0"})
    pct = wb.add_format({"num_format": "0.0%"})
    date = wb.add_format({"num_format": "yyyy-mm-dd"})

    # Masters first (generated up front so orders can compute cached values).
    customers = []
    for i in range(args.customers):
        cid = f"C{100000 + i}"
        customers.append((cid, f"PT Pelanggan {i + 1:05d}", rnd.choice(REGIONS), rnd.choice(SEGMENTS)))
    cust = {c[0]: c for c in customers}
    materials = []
    for i in range(args.materials):
        mid = f"M{200000 + i}"
        cat = rnd.choice(CATEGORIES)
        cost = round(rnd.uniform(50_000, 25_000_000), -3)
        materials.append((mid, f"{cat} type {i + 1:04d}", cost, cat))
    mat = {m[0]: m for m in materials}

    # Sheet order matters in constant_memory mode only per sheet, so write
    # Orders (the big one) first to match a typical export.
    ws = wb.add_worksheet("Orders")
    head = ["Order", "Date", "Customer", "Material", "Qty", "Unit price", "Amount",
            "Customer name", "Region", "Description", "Std cost", "Margin", "Margin %",
            "Margin class", "Month", "Status", "Sales rep", "Plant", "Note"]
    ws.write_row(0, 0, head, bold)
    if args.table:
        ws.add_table(0, 0, args.rows, len(head) - 1, {
            "name": "Orders",
            "style": "Table Style Medium 2",
            "autofilter": True,
            "columns": [{"header": h, "header_format": bold} for h in head],
        })
    ws.set_column(0, 0, 11)
    ws.set_column(1, 1, 11, date)
    ws.set_column(5, 6, 14, money)
    ws.set_column(7, 9, 22)
    ws.set_column(10, 11, 14, money)
    ws.set_column(12, 12, 9, pct)
    ws.freeze_panes(1, 0)
    start = dt.date(2025, 1, 1)
    sums = {}  # (region) -> [amount, margin, count]
    for r in range(1, args.rows + 1):
        n = r + 1  # Excel row number
        c = rnd.choice(customers)
        m = rnd.choice(materials)
        qty = rnd.randint(1, 40)
        price = round(m[2] * rnd.uniform(1.05, 1.6), -3)
        day = start + dt.timedelta(days=rnd.randint(0, 680))
        amount = qty * price
        std = m[2] * qty
        margin = amount - std
        mpct = 0 if amount == 0 else margin / amount
        mclass = "Low" if mpct < 0.15 else ("Mid" if mpct < 0.35 else "High")
        s = sums.setdefault(c[2], [0.0, 0.0, 0])
        s[0] += amount
        s[1] += margin
        s[2] += 1

        ws.write_string(r, 0, f"SO{5000000 + r}")
        ws.write_datetime(r, 1, dt.datetime(day.year, day.month, day.day), date)
        ws.write_string(r, 2, c[0])
        ws.write_string(r, 3, m[0])
        ws.write_number(r, 4, qty)
        ws.write_number(r, 5, price, money)
        ws.write_formula(r, 6, f"=E{n}*F{n}", money, amount)
        ws.write_formula(r, 7, f"=VLOOKUP(C{n},Customers!$A:$D,2,FALSE)", None, c[1])
        ws.write_formula(r, 8, f"=VLOOKUP(C{n},Customers!$A:$D,3,FALSE)", None, c[2])
        ws.write_formula(r, 9, f"=VLOOKUP(D{n},Materials!$A:$D,2,FALSE)", None, m[1])
        ws.write_formula(r, 10, f"=VLOOKUP(D{n},Materials!$A:$D,3,FALSE)*E{n}", money, std)
        ws.write_formula(r, 11, f"=G{n}-K{n}", money, margin)
        ws.write_formula(r, 12, f"=IF(G{n}=0,0,L{n}/G{n})", pct, mpct)
        ws.write_formula(r, 13, f'=IF(M{n}<0.15,"Low",IF(M{n}<0.35,"Mid","High"))', None, mclass)
        ws.write_formula(r, 14, f'=TEXT(B{n},"yyyy-mm")', None, day.strftime("%Y-%m"))
        ws.write_string(r, 15, rnd.choice(STATUSES))
        ws.write_string(r, 16, f"Rep {rnd.randint(1, 60):02d}")
        ws.write_string(r, 17, f"P{rnd.randint(1, 12):03d}")
        if rnd.random() < 0.08:
            ws.write_string(r, 18, "Urgent, call before delivery")

    if args.cf:
        last = args.rows + 1
        low = wb.add_format({"bg_color": "#FCE4D6"})
        top = wb.add_format({"bold": True, "font_color": "#006100", "bg_color": "#C6EFCE"})
        dup = wb.add_format({"font_color": "#9C0006"})
        ws.conditional_format(f"A2:S{last}", {"type": "formula", "criteria": '=$N2="Low"', "format": low})
        ws.conditional_format(f"G2:G{last}", {"type": "3_color_scale"})
        ws.conditional_format(f"E2:E{last}", {"type": "data_bar", "bar_color": "#638EC6", "data_bar_2010": True})
        ws.conditional_format(f"M2:M{last}", {"type": "icon_set", "icon_style": "3_arrows"})
        ws.conditional_format(f"L2:L{last}", {"type": "top", "value": 10, "format": top})
        ws.conditional_format(f"C2:C{last}", {"type": "duplicate", "format": dup})

    wc = wb.add_worksheet("Customers")
    wc.write_row(0, 0, ["Customer", "Name", "Region", "Segment"], bold)
    wc.set_column(1, 1, 24)
    wc.set_column(2, 3, 18)
    for i, c in enumerate(customers, start=1):
        wc.write_row(i, 0, c)

    wm = wb.add_worksheet("Materials")
    wm.write_row(0, 0, ["Material", "Description", "Std cost", "Category"], bold)
    wm.set_column(1, 1, 22)
    wm.set_column(2, 2, 14, money)
    for i, m in enumerate(materials, start=1):
        wm.write_row(i, 0, m)

    wsum = wb.add_worksheet("Summary")
    wsum.write_row(0, 0, ["Region", "Orders", "Amount", "Margin", "Margin %"], bold)
    wsum.set_column(0, 0, 20)
    wsum.set_column(2, 3, 18, money)
    wsum.set_column(4, 4, 10, pct)
    for i, region in enumerate(REGIONS, start=1):
        n = i + 1
        a, mg, cnt = sums.get(region, [0.0, 0.0, 0])
        wsum.write_string(i, 0, region)
        wsum.write_formula(i, 1, f"=COUNTIF(Orders!$I:$I,A{n})", None, cnt)
        wsum.write_formula(i, 2, f"=SUMIF(Orders!$I:$I,A{n},Orders!$G:$G)", money, a)
        wsum.write_formula(i, 3, f"=SUMIF(Orders!$I:$I,A{n},Orders!$L:$L)", money, mg)
        wsum.write_formula(i, 4, f"=IF(C{n}=0,0,D{n}/C{n})", pct, 0 if a == 0 else mg / a)
    t = len(REGIONS) + 1
    tot_a = sum(v[0] for v in sums.values())
    tot_m = sum(v[1] for v in sums.values())
    wsum.write_string(t, 0, "Total", bold)
    wsum.write_formula(t, 1, f"=SUM(B2:B{t})", bold, sum(v[2] for v in sums.values()))
    wsum.write_formula(t, 2, f"=SUM(C2:C{t})", money, tot_a)
    wsum.write_formula(t, 3, f"=SUM(D2:D{t})", money, tot_m)
    wsum.write_formula(t, 4, f"=IF(C{t + 1}=0,0,D{t + 1}/C{t + 1})", pct, 0 if tot_a == 0 else tot_m / tot_a)

    wb.close()
    formulas = args.rows * 9 + len(REGIONS) * 4 + 4
    print(f"wrote {args.out}: {args.rows} orders, {formulas} formula cells")


if __name__ == "__main__":
    main()
