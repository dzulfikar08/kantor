"""The chat panel's scripted turns (KANTOR_CHAT_SCRIPT) for the site's Paparan AI shots, in English
and Indonesian. The tools are real: `build` builds book.json, `look` renders the page.
    python3 make_scripts.py BOOK_EN.json BOOK_ID.json OUT_DIR
The summary's figures (total, best and weakest month) are those of make_csv.py's data."""
import json, sys
book_en, book_id, out = open(sys.argv[1]).read(), open(sys.argv[2]).read(), sys.argv[3]
T = {
 "en": dict(book=book_en, read="I'll read the CSV first.",
   plan="96 rows: 12 months for 8 cities, sales in Rp million and cups. I'll keep the rows on a Data sheet as a table, and put a Summary sheet in front: SUMIFS per city and month, totals, and a line chart under them.",
   look="The check is clean. Let me look at the printed Summary page.",
   summary="Done: **kopi-senja-2026.xlsx**, checked with no errors or warnings.\n\n- **Summary**: sales per city for each month with SUMIFS on the data, a Total column and row, and a line chart of the monthly total.\n- **Data**: the 96 rows of the CSV as a table, so filters and new rows work.\n\nSales for the year come to **Rp 6,093.7 million**. The best month was October (Rp 538.2 million), the weakest February (Rp 465.8 million)."),
 "id": dict(book=book_id, read="Saya baca CSV-nya dulu.",
   plan="96 baris: 12 bulan untuk 8 kota, penjualan dalam Rp juta dan jumlah cangkir. Barisnya saya simpan di sheet Data sebagai tabel, dan sheet Ringkasan di depannya: SUMIFS per kota dan bulan, total, dan grafik garis di bawahnya.",
   look="Pemeriksaan bersih. Saya lihat halaman Ringkasan yang tercetak.",
   summary="Selesai: **kopi-senja-2026.xlsx**, sudah dicek tanpa galat atau peringatan.\n\n- **Ringkasan**: penjualan per kota tiap bulan dengan SUMIFS dari data, kolom dan baris Total, dan grafik garis total bulanan.\n- **Data**: 96 baris dari CSV sebagai tabel, jadi filter dan baris baru tetap jalan.\n\nPenjualan setahun **Rp 6.093,7 juta**. Bulan terbaik Oktober (Rp 538,2 juta), terendah Februari (Rp 465,8 juta)."),
}
for lang, t in T.items():
    s = [
        {"delay_ms": 1500, "text": t["read"], "calls": [{"name": "read_file", "args": {"path": "penjualan-2026.csv"}}]},
        {"delay_ms": 2500, "text": t["plan"], "calls": [{"name": "write_file", "args": {"path": "book.json", "content": t["book"]}}]},
        {"delay_ms": 1500, "text": "", "calls": [{"name": "build", "args": {"source": "book.json", "output": "kopi-senja-2026.xlsx"}}]},
        {"delay_ms": 1500, "text": t["look"], "calls": [{"name": "look", "args": {"file": "kopi-senja-2026.xlsx", "pages": "1"}}]},
        {"delay_ms": 2500, "text": "", "calls": [{"name": "finish", "args": {"summary": t["summary"], "outputs": ["kopi-senja-2026.xlsx"]}}]},
    ]
    json.dump(s, open(f"{out}/script-{lang}.json", "w"), indent=1)
