# Indonesian interface: catalogs and glossary

Kantor shows its interface in English or Indonesian. The language comes from
the system locale (`id_*` gives Indonesian). All Indonesian text lives in
plain-text catalogs, keyed by the English string.

## How the catalogs work

The catalogs are in `crates/kantor_ui/i18n/`:

| File | Holds |
|---|---|
| `common.tsv` | Shared words used everywhere (Save, Open, Undo, …) |
| `kantor.tsv` | The app shell, command palette, AI, updates, recovery |
| `sheets.tsv` | Sheets toolbar, menus, dialogs, status line |
| `docs.tsv` | Docs |
| `slides.tsv` | Slides |
| `widgets.tsv`, `content.tsv` | Shared widgets and built-in content |

Each line is `English<TAB>Indonesian`, with one literal tab. Lines starting
with `#` are comments. A literal tab or newline inside a string is written
`\t` or `\n`. The code looks up the English text in `t("…")`, `tf("…", &[…])`
(with `{}` placeholders) or `tk("…")` (a key kept in data, shown later with
`t_dyn`), so the English side must match the code exactly.

### Adding a string

1. In the code, wrap the English text: `t("Insert Chart")`, or
   `tf("Saved {}", &[&name])` when it has placeholders.
2. Add `Insert Chart<TAB>Sisipkan Bagan` to the catalog of that area.
3. If the same English already exists in another file, use the same
   Indonesian; the test rejects two different translations of one key.
4. Run the tests:

```sh
cargo test -p kantor_ui --lib i18n
```

The tests check that every `t`/`tf`/`tk` key has a line
(`catalog_has_every_key`), that lines are well formed with the same number of
`{}` on both sides and one translation per key
(`catalog_lines_are_well_formed`), and that no line is left unused
(`catalog_has_no_unused_lines`). To fix a translation, change only the
Indonesian side; changing the English key breaks the lookup.

## Style rules

- Formal register. Address the user as "Anda"; no slang or chat words
  ("bisa" → "dapat", "dulu" → "terlebih dahulu", "punya" → "memiliki").
- Keep the capitalisation style of the English: title case menu items and
  buttons stay title case ("Sisipkan Baris di Atas"), sentence case stays
  sentence case. Short function words (di, ke, dari, dan, atau, untuk) stay
  lowercase inside titles.
- Commands are imperative verbs ("Simpan", "Sisipkan"). Status messages that
  report a finished action use the passive: object first, then the
  di- verb ("{} baris disisipkan", "Rumus disisipkan di {}").
- Keep `{}` placeholders (same count; the order may change only if the
  meaning still works with the arguments in the English order), `…`,
  keyboard names (Ctrl+S, Shift+Tab, Enter, Escape, F9), file paths, cell
  references and formula text unchanged.
- Decimal numbers shown as prose use a comma (21 × 29,7 cm). Number format
  samples such as `1,234.50` are left as in English because they show what
  the format produces.
- Keep established loanwords and names: PDF, CSV, PNG, JPEG, AI, MCP, API,
  URL, WebDAV, cloud, Excel, Word, PowerPoint, Kantor, Sheets, Docs, Slides,
  PivotTable, slide, font, filter, header, footer, margin, spreadsheet,
  keychain, handout, file extensions.
- "Can't" in errors: "Tidak dapat …". "Couldn't …" also "Tidak dapat …".
- One term per concept, as listed below. Where Microsoft Office in
  Indonesian (MS) or LibreOffice in Indonesian (LO) has an established name,
  Kantor follows it unless noted.

## Glossary

### Files and general commands

| English | Indonesian | Note |
|---|---|---|
| File | Berkas | MS uses "File"; LO and KBBI use "Berkas" |
| Open | Buka | MS, LO |
| Save | Simpan | MS, LO |
| Save As | Simpan Sebagai | MS, LO |
| Save All | Simpan Semua | |
| Don't Save | Jangan Simpan | MS |
| Autosave | Simpan Otomatis | MS "Simpan Otomatis" |
| Close | Tutup | |
| Print | Cetak | |
| Export as PDF | Ekspor sebagai PDF | |
| New | Baru | "Dokumen Baru", "Slide Baru" (adjective after noun) |
| Recent | Terbaru | |
| Restore / Recover | Pulihkan | recovery copy = salinan pemulihan |
| Discard | Buang | |
| Undo | Urungkan | MS, LO |
| Redo | Ulangi | MS, LO |
| Cut | Potong | |
| Copy | Salin | |
| Paste | Tempel | |
| Delete | Hapus | Also used for Remove and Clear (filters, rules, print area) |
| Clear (empty a field) | Kosongkan | only where a box is emptied |
| Find | Cari | |
| Replace | Ganti | |
| Replace All | Ganti Semua | |
| match (search result) | hasil | "1 hasil", "Hasil berikutnya", "{} hasil diganti" |
| Match case | Cocokkan huruf besar/kecil | |
| Rename | Ganti Nama | MS |
| Edit (verb) | Edit | MS keeps "Edit"; "pengeditan" for the noun |
| Settings | Pengaturan | |
| Apply | Terapkan | |
| Cancel | Batal | MS, LO |
| OK | OK | |
| Done | Selesai | |
| Yes / No | Ya / Tidak | |
| Retry | Coba Lagi | |
| Not Now | Nanti Saja | |
| Insert | Sisipkan | MS; LO uses "Sisipkan" too |
| Add | Tambahkan | |
| Create | Buat | |
| Duplicate | Duplikat | MS "Duplikat Slide" |
| Move Up / Down | Pindahkan ke Atas / ke Bawah | |
| Show / Hide | Tampilkan / Sembunyikan | Toggle = "Tampilkan/Sembunyikan …" |
| Turn on / off | Aktifkan / Nonaktifkan | KBBI spelling; state words Aktif / Nonaktif |
| Read-only | Hanya Baca | |
| Allow Editing | Izinkan Pengeditan | |
| Check for Updates | Periksa Pembaruan | |
| Restart | Mulai Ulang | |
| Download / Install | Unduh / Pasang | |
| Quit | Keluar | |
| Zoom In / Out | Perbesar / Perkecil | |
| Actual Size | Ukuran Sebenarnya | |
| Custom | Kustom | Used everywhere ("Urutan Kustom", "Margin kustom", "Nama kustom"); MS mixes "Kustom" and "Khusus" |
| Automatic | Otomatis | |
| None | Tidak ada | "(tidak ada)" in lists; "Tanpa …" for "No X" (Tanpa Isian, Tanpa Batas) |
| Error | Kesalahan | MS "Kesalahan"; "galat" (KBBI) is too technical for most users |
| Warning | Peringatan | |
| Information | Informasi | |

### App shell

| English | Indonesian | Note |
|---|---|---|
| Home | Beranda | |
| Command / Commands | Perintah | |
| Sidebar | Bilah samping | |
| Inspector | Inspektur | |
| Explorer | Penjelajah | |
| Pane | Panel | |
| Tab | Tab | |
| Split Right / Down | Bagi ke Kanan / ke Bawah | |
| Theme | Tema | |
| Toolbar | Bilah alat | |
| Unsaved changes | Perubahan belum disimpan | |
| Ready | Siap | |
| Provider (AI) | Penyedia | |
| API key | Kunci API | |
| Model id | ID model | |
| Agent | Agen | |
| Cloud account | Akun Cloud | |
| Sync Now | Sinkronkan Sekarang | |
| offline | luring | KBBI |
| Spelling suggestions | Saran Ejaan | |
| Add to Dictionary | Tambahkan ke Kamus | MS |
| Ignore All | Abaikan Semua | MS |

### Formatting (all apps)

| English | Indonesian | Note |
|---|---|---|
| Format (noun and verb) | Format | "memformat", "pemformatan" |
| Font | Font | MS |
| Font Size | Ukuran Font | |
| Bold / Italic / Underline | Tebal / Miring / Garis Bawah | MS |
| Strikethrough | Coret | MS "Coretan"; shorter form kept |
| Text Colour | Warna Teks | |
| Fill | Isian | MS "Isian" |
| Fill Colour | Warna Isian | |
| Border(s) | Batas | MS "Batas"; All/Outside/No Borders = Semua/Batas Luar/Tanpa Batas |
| Outline (shape) | Garis Tepi | MS "Kerangka Bentuk"; "Garis Tepi" chosen as clearer |
| Align Left / Center / Right | Rata Kiri / Rata Tengah / Rata Kanan | MS |
| Justify | Rata Kiri-Kanan | MS |
| Align Top / Middle / Bottom | Rata Atas / Rata Tengah Vertikal / Rata Bawah | |
| Alignment | Perataan | |
| Style | Gaya | MS |
| Paragraph | Paragraf | |
| Heading 1 | Judul 1 | MS |
| Normal Text | Teks Normal | |
| Bulleted / Numbered List | Daftar Berpoin / Daftar Bernomor | MS |
| List level | Tingkat daftar | MS "Tingkat Daftar" |
| Wrap Text | Bungkus Teks | MS |
| Theme colours | Warna tema | |
| Accent | Aksen | |
| Background | Latar Belakang | |
| Picture | Gambar | MS |
| Table | Tabel | |
| Shape | Bentuk | MS |
| Text Box | Kotak Teks | MS |
| Placeholder | Tempat Penampung | MS |
| Comment | Komentar | |

### Page and printing

| English | Indonesian | Note |
|---|---|---|
| Page Setup | Pengaturan Halaman | MS (Excel) |
| Page Layout | Tata Letak Halaman | MS |
| Layout | Tata Letak | |
| Margins | Margin | MS |
| Orientation | Orientasi | |
| Portrait / Landscape | Potret / Lanskap | MS |
| Paper size | Ukuran kertas | |
| Header / Footer | Header / Footer | MS keeps the English words |
| Page number | Nomor halaman | |
| Section break | Pemisah bagian | MS "Pemisah Bagian" |
| Print Area | Area Cetak | MS |
| Scaling | Penskalaan | |
| Fit to | Paskan ke | |
| Gridlines | Garis kisi | MS |
| Handouts | Handout | MS keeps "Handout" |
| Notes Pages | Halaman Catatan | MS |
| PDF viewer | Penampil PDF | |

### Sheets

| English | Indonesian | Note |
|---|---|---|
| Spreadsheet | Spreadsheet | file type in the start screen |
| Workbook | Buku Kerja | MS |
| Sheet | Lembar | MS "Lembar"; "Sheet1" in examples stays |
| Cell | Sel | |
| Row / Column | Baris / Kolom | |
| Range | Rentang | MS |
| Used range | Rentang terpakai | |
| Formula | Rumus | MS |
| Value | Nilai | |
| Active cell | Sel aktif | |
| Selection | Pilihan | MS |
| Freeze Panes | Bekukan Panel | MS |
| Unfreeze Panes | Lepaskan Pembekuan Panel | MS |
| Hide / Unhide | Sembunyikan / Tampilkan | |
| Autofit | Paskan Otomatis | MS "Paskan Otomatis" |
| Fill Down / Right | Isi ke Bawah / ke Kanan | |
| Merge & Center | Gabung & Tengahkan | MS |
| Sort | Urutkan | |
| Sort A to Z | Urutkan dari A ke Z | MS |
| Ascending / Descending | Naik / Turun | MS |
| Custom Sort | Urutan Kustom | MS |
| Level (sort) | Tingkat | MS |
| Filter | Filter | MS |
| Clear Filter | Hapus Filter | |
| Calculation (Automatic/Manual) | Penghitungan | MS "Penghitungan" (KBBI) |
| Calculate Now / Recalculate | Hitung Sekarang / Hitung ulang | |
| Conditional Formatting | Pemformatan Bersyarat | MS |
| Highlight Cells Rules | Aturan Sorot Sel | MS |
| Data Bars / Color Scales / Icon Sets | Bilah Data / Skala Warna / Set Ikon | MS |
| Data Validation | Validasi Data | MS |
| Input Message / Error Alert | Pesan Input / Peringatan Kesalahan | MS |
| Drop-down list | Daftar menurun | MS "daftar menurun" |
| Chart | Bagan | MS |
| Column / Bar / Line / Pie Chart | Bagan Kolom / Batang / Garis / Pai | MS |
| Area / Scatter / Doughnut | Area / Sebar / Donat | MS |
| Stacked | Bertumpuk | MS |
| Series | Seri | |
| Category | Kategori | |
| Legend | Legenda | |
| Axis | Sumbu | |
| Format as Table | Format sebagai Tabel | MS |
| Table Design | Desain Tabel | MS |
| Header Row / Total Row | Baris Header / Baris Total | MS |
| Banded Rows / Columns | Baris Berpita / Kolom Berpita | MS |
| Convert to Range | Konversi ke Rentang | MS |
| PivotTable | PivotTable | MS keeps the name |
| PivotTable Fields | Bidang PivotTable | MS |
| Value Field Settings | Pengaturan Bidang Nilai | MS |
| Refresh | Segarkan | MS |
| Grand total | Total keseluruhan | MS |
| Sum / Count / Average | Jumlah / Hitungan / Rata-rata | MS status bar |
| Points (pt) | Poin | |

### Docs

| English | Indonesian | Note |
|---|---|---|
| Document | Dokumen | |
| Word / Character | Kata / Karakter | |
| Ruler | Penggaris | MS |
| Footnote / Endnote | Catatan Kaki / Catatan Akhir | MS |
| Track Changes | Lacak Perubahan | MS |
| Tracked change | Perubahan terlacak | |
| Accept / Reject | Terima / Tolak | MS |
| Review | Tinjau | MS |
| Caret | Kursor | |
| Check Spelling as You Type | Periksa Ejaan Saat Mengetik | |
| Rewrite | Tulis ulang | |

### Slides

| English | Indonesian | Note |
|---|---|---|
| Presentation | Presentasi | |
| Slide | Slide | MS |
| New Slide | Slide Baru | MS |
| Present | Presentasikan | |
| Speaker Notes | Catatan Pembicara | MS |
| Rectangle / Ellipse | Persegi Panjang / Elips | MS |
| Deck (.pptx) | slide .pptx | "dek" is not used in Indonesian |
| Click to add title / text | Klik untuk menambahkan judul / teks | MS |
| Custom slide show | Peragaan slide kustom | MS |

### Widgets (`widgets.tsv`)

| English | Indonesian | Note |
|---|---|---|
| Select All | Pilih Semua | |
| Show / Hide password | Tampilkan / Sembunyikan kata sandi | |
| Increase / Decrease | Tambah / Kurangi | number stepper |
| Previous / Next | Sebelumnya / Berikutnya | |
| Reset All | Atur Ulang Semua | |
| Collapse / Expand | Ciutkan / Perluas | MS |
| Hue / Saturation / Lightness / Opacity | Rona / Saturasi / Kecerahan / Opasitas | |
| Unnamed | Tanpa nama | |
| Select date | Pilih tanggal | |
| Weekdays Su–Sa | Min, Sen, Sel, Rab, Kam, Jum, Sab | |
| Months | Januari … Desember; short Jan, Feb, Mar, Apr, Mei, Jun, Jul, Agu, Sep, Okt, Nov, Des | |

### Text written into files (`content.tsv`)

These strings end up inside the saved document, so they follow what Excel,
Word and PowerPoint in Indonesian write.

| English | Indonesian | Note |
|---|---|---|
| Row Labels / Column Labels | Label Baris / Label Kolom | Excel PivotTable |
| Grand Total | Total Keseluruhan | Excel |
| "{} Total" (subtotal) | Total {} | Excel puts "Total" first |
| (blank) | (kosong) | Excel |
| (Multiple Items) | (Beberapa Item) | Excel |
| Sum of X | Jumlah dari X | Excel value-field caption |
| Column1 (new table header) | Kolom1 | Excel |
| Series1 | Seri1 | Excel |
| Chart Title | Judul Bagan | Excel |
| Formulas kept as values | Rumus diubah menjadi nilai | import note |
| Embedded object | Objek tersemat | |
| Nested tables | Tabel bersarang | |
| Content controls | Kontrol konten | Word |
| Fields (Word) | Bidang | Word |
| Equations | Persamaan | Word |
