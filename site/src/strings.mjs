// All page text, per language. Values may contain inline HTML (links, <code>).
// Keep both languages in step: every key in `en` must exist in `id`.

const GH = "https://github.com/dzulfikar08/kantor";

export const en = {
  lang: "en",
  path: "/",
  locale: "en_US",
  title: "Kantor: Sheets, Docs and Slides for Mac and Linux",
  description:
    "A small, fast office suite that opens and saves Word, Excel and PowerPoint files. Works offline, no account. Free and open source.",
  ogAlt: "Kantor icon next to a sales report open in Kantor Sheets",
  skip: "Skip to content",
  navLabel: "Main",
  navDownload: "Download",
  navLimits: "What's missing",
  navSource: "GitHub",
  switchLang: "Bahasa Indonesia",
  switchLangShort: "ID",
  switchLangLabel: "Baca dalam Bahasa Indonesia",

  h1: "Sheets, Docs and Slides that open your Office files",
  lede:
    "Kantor is a small, fast office app for Mac and Linux. Open a Word document, an Excel sheet or a PowerPoint deck, edit it, and save it back in the same format. It works offline and needs no account.",
  facts: ["Free and open source", "About 60 MB", "Mac and Linux"],
  preview:
    'Early preview: Kantor is new and some things are still missing. <a href="#limits">Read the list</a> before you rely on it for important work.',

  ctaChoose: "Choose your download",
  ctaMac: "Download for Mac",
  ctaMacMeta: "Kantor.dmg, for macOS 11 or later (Apple Silicon or Intel)",
  ctaLinux: "Download for Linux",
  ctaLinuxMetaX64: "Kantor-x86_64.AppImage, for most PCs (Intel or AMD)",
  ctaLinuxMetaArm: "Kantor-aarch64.AppImage, for ARM computers",
  ctaOtherFiles: "Other downloads",
  ctaWindows: "See the Mac and Linux downloads",
  ctaWindowsNote: "Kantor isn't available for Windows yet.",
  ctaMobile: "See the downloads",
  ctaMobileNote: "Kantor is a desktop app for Mac and Linux. Open this page on your computer to download it.",

  shotsLabel: "Screenshots of Kantor",
  shots: {
    sheets: {
      tab: "Sheets",
      alt: "Kantor Sheets with a quarterly sales workbook: a merged title row, coloured header cells with borders, amounts in rupiah and percentages against target. A block of cells is selected and the status bar shows their sum, average and count.",
      caption: "A formatted sales report in Sheets, with rupiah amounts and percentages. Select cells and their sum, average and count appear at the bottom.",
    },
    docs: {
      tab: "Docs",
      alt: "Kantor Docs showing a short report on an A4 page, with headings, a bulleted list, a bar chart picture and a table, below the formatting toolbar and ruler.",
      caption: "A report in Docs, on an A4 page with headings, a list, a picture and a table.",
    },
    slides: {
      tab: "Slides",
      alt: "Kantor Slides with a four-slide deck. The thumbnails are on the left, and the open slide has a gradient title band, a table of regions and a column chart, with speaker notes below.",
      caption: "A presentation in Slides with gradients, PowerPoint shapes, a table and a chart, as PowerPoint draws them.",
    },
  },
  shotsNote: "Real screenshots of Kantor on Linux. It follows your system's light or dark look.",

  whatH2: "Made for the files you already have",
  apps: {
    sheets: {
      name: "Sheets",
      formats: [".xlsx", ".csv", ".ods", ".xls"],
      text: "Spreadsheets with formulas, several sheets, number formats, Excel tables, charts, pivot tables, conditional formatting, data validation, sort and filter. Opens .csv, .ods and .xls files too, and saves as .xlsx.",
    },
    docs: {
      name: "Docs",
      formats: [".docx"],
      text: "Letters and reports with headings, lists, tables, pictures, headers and footers with page numbers, and page sizes such as A4, Letter and F4, landscape pages too. Comments and tracked changes, find and replace, and a word count.",
    },
    slides: {
      name: "Slides",
      formats: [".pptx"],
      text: "Presentations with text, shapes and pictures you can move and resize, tables and chart data you can edit, speaker notes, and a full-screen slide show.",
    },
  },
  keepH3: "Saving doesn't throw away what Kantor can't show yet",
  keepText:
    "If a file holds something Kantor doesn't understand yet, such as a Word equation or an Excel comment, Kantor leaves it as it was and writes it back when you save. When it can't do that safely, it saves to a new file or tells you what will be left out.",
  more: [
    ["Print and export to PDF", "Docs and Slides save a PDF that looks like what you see on screen, and print it through your PDF viewer; Slides also prints notes pages and handouts. Sheets prints pages the way Excel does: paper size including F4, fit to width, print titles, headers and footers."],
    ["Offline, no account", "Your files stay on your computer. There is nothing to sign up for."],
    ["Spell check in English and Indonesian", "Docs and Slides underline misspelled words as you type and suggest corrections, with no internet needed. Add your own words to a personal dictionary."],
    [
      "Your work is safe",
      'While a document has unsaved changes, Kantor keeps a copy of it on your computer. After a crash or a power cut it offers to restore your changes, and nothing is written over your file until you save. Autosave can be <a href="' + GH + '#your-work-is-safe">turned on</a> too.',
    ],
    [
      "Updates",
      'Kantor checks for updates at most once a day, sending only its version and operating system, and installs a new version when you restart it. You can <a href="' + GH + '#updates">turn this off</a>.',
    ],
    [
      "AI only if you want it",
      "Off until you turn it on. Use your own key from a provider such as Anthropic, OpenAI or Google, or run a model on your own computer with Ollama. Kantor has no AI service of its own.",
    ],
    [
      "AI assistants can edit open files",
      'Assistants such as Claude can read and change the documents you have open, while you watch. Each change is one step you can undo. This uses <a href="https://modelcontextprotocol.io">MCP</a>; the setup is in the <a href="' + GH + '#ai-agents-mcp">README</a>.',
    ],
  ],

  dlH2: "Download Kantor",
  dlIntro: "Free, with no account and no trial period.",
  verLoading: "Checking the latest version…",
  verOk: "Latest version: {tag}, released {date}.",
  verNotes: "What's new",
  verError: "Couldn't check the latest version just now. The download buttons still get the newest release.",
  verNone: "No release is published yet, so the download buttons won't work. Check back soon.",
  assetMissing:
    'The newest release ({tag}) doesn\'t have this download yet. <a href="{url}">See the files it does have</a>.',
  yourComputer: "Looks like your computer",
  mac: {
    name: "Mac",
    req: "macOS 11 or later, Apple Silicon or Intel",
    button: "Download Kantor.dmg",
    steps: [
      "Open <strong>Kantor.dmg</strong> from your Downloads folder.",
      "Drag <strong>Kantor</strong> into the Applications folder.",
      "Open Kantor from Applications or Launchpad. It's signed and notarised by Apple, so it opens like any other app.",
    ],
  },
  linux: {
    name: "Linux",
    req: "64-bit Linux. One file, nothing else to install.",
    buttonX64: "Download for most PCs",
    buttonX64Meta: "x86_64, Intel or AMD",
    buttonArm: "Download for ARM",
    buttonArmMeta: "aarch64",
    steps: [
      "Download the file for your computer. If you're not sure, it's almost always the first one.",
      "Allow it to run: right-click the file, choose <strong>Properties</strong> and turn on <strong>Allow executing file as program</strong>. The wording differs a little between file managers.",
      "Double-click the file to open Kantor.",
    ],
    termH: "Prefer the terminal?",
    termText:
      "This command installs Kantor in <code>~/.local/bin</code> and adds it to your app menu. It picks the right file for your computer and installs the nightly build, made from the newest code and tested less than a release.",
    copy: "Copy",
    copied: "Copied",
    copyFail: "Couldn't copy. Select the command and copy it yourself.",
  },
  windows: {
    name: "Windows",
    text: "Not available yet. Kantor runs on Mac and Linux for now.",
  },
  // Shown once build.mjs has WINDOWS_RELEASED (the first release with Windows downloads).
  win: {
    title: "Kantor: Sheets, Docs and Slides for Windows, Mac and Linux",
    lede: "Kantor is a small, fast office app for Windows, Mac and Linux. Open a Word document, an Excel sheet or a PowerPoint deck, edit it, and save it back in the same format. It works offline and needs no account.",
    fact: "Windows, Mac and Linux",
    cta: "Download for Windows",
    ctaMeta: "Kantor-Setup-x64.exe, for Windows 10 and 11",
    ctaMetaArm: "Kantor-Setup-arm64.exe, for Windows on Arm",
    ctaMobileNote: "Kantor is a desktop app for Windows, Mac and Linux. Open this page on your computer to download it.",
    req: "Windows 10 or 11. No administrator rights needed.",
    buttonX64: "Download for most PCs",
    buttonX64Meta: "x64, Intel or AMD",
    buttonArm: "Download for Arm",
    buttonArmMeta: "arm64, Snapdragon",
    steps: [
      "Open <strong>Kantor-Setup-x64.exe</strong> from your Downloads folder.",
      "If Windows says <strong>Windows protected your PC</strong>, choose <strong>More info</strong> and then <strong>Run anyway</strong>. This appears because Kantor isn't code-signed yet.",
      "Choose <strong>Install</strong>. Kantor is in the Start menu, and Word, Excel and PowerPoint files now list Kantor under <strong>Open with</strong>. Your default apps stay as they are.",
    ],
    portable:
      'No installer? Get the portable zip for <a href="{dl}/Kantor-x64.zip">x64</a> or <a href="{dl}/Kantor-arm64.zip">Arm</a>, unzip it anywhere and open <strong>kantor.exe</strong>.',
  },

  limitsH2: "What's missing in this preview",
  limitsIntro:
    "Kantor is early software and hasn't been tried on a large set of real-world files yet. Keep a copy of anything important, and expect these gaps:",
  limits: [
    "Printing goes through your PDF viewer: Kantor makes the pages and the viewer sends them to the printer.",
    "Word: floating pictures, shapes and equations aren't shown properly yet, and headers and footers with pictures or tables can't be edited. Kantor keeps them and saves them unchanged.",
    "Word: dot leaders at tab stops aren't drawn, and a footnote or endnote too long for its page doesn't continue onto the next one.",
    "Word: you can track your own edits, add comments and accept or reject changes, but formatting changes you make aren't tracked yet.",
    "Excel: conditional formatting rules, tables (Format as Table), data validation and page setup can be made and changed in Kantor; files changed this way haven't been opened in Excel itself yet.",
    "Excel: macros aren't carried over when you save, and Kantor tells you when that happens. Pivot tables can be created, rearranged, refreshed and deleted, but not grouped by date or number, and calculated fields aren't supported yet.",
    "Excel: charts can be inserted (column, bar, line, pie, area, scatter) and changed in a Chart panel, including charts that came with the file (combo and 3-D charts keep their type and data range); chart sheets, shapes and text boxes aren't shown yet.",
    "PowerPoint: table cells and chart data (with series and categories added or removed) are edited in a panel, not on the slide, and SmartArt can't be edited (some shows as a plain box).",
  ],
  reportH3: "Found a file that Kantor gets wrong?",
  reportText:
    "Tell us what happened. Attach the file only if it holds nothing private.",
  reportButton: "Report a problem on GitHub",

  footSource: "Source code",
  footChangelog: "Changelog",
  footLicense: "AGPL-3.0 licence",
  footBy: "by",
};

export const id = {
  lang: "id",
  path: "/id/",
  locale: "id_ID",
  title: "Kantor: Sheets, Docs, dan Slides untuk Mac dan Linux",
  description:
    "Aplikasi perkantoran yang ringan dan cepat untuk membuka dan menyimpan file Word, Excel, dan PowerPoint. Bisa tanpa internet, tanpa akun. Gratis dan open source.",
  ogAlt: "Ikon Kantor di samping laporan penjualan yang dibuka di Kantor Sheets",
  skip: "Langsung ke isi",
  navLabel: "Utama",
  navDownload: "Unduh",
  navLimits: "Yang belum ada",
  navSource: "GitHub",
  switchLang: "English",
  switchLangShort: "EN",
  switchLangLabel: "Read in English",

  h1: "Sheets, Docs, dan Slides untuk file Office Anda",
  lede:
    "Kantor adalah aplikasi perkantoran yang ringan dan cepat untuk Mac dan Linux. Buka dokumen Word, lembar Excel, atau presentasi PowerPoint, edit, lalu simpan lagi dalam format yang sama. Bisa dipakai tanpa internet dan tanpa akun.",
  facts: ["Gratis dan open source", "Sekitar 60 MB", "Mac dan Linux"],
  preview:
    'Versi awal: Kantor masih baru dan beberapa hal belum ada. <a href="#limits">Baca daftarnya</a> sebelum memakainya untuk pekerjaan penting.',

  ctaChoose: "Pilih file unduhan",
  ctaMac: "Unduh untuk Mac",
  ctaMacMeta: "Kantor.dmg, untuk macOS 11 ke atas (Apple Silicon atau Intel)",
  ctaLinux: "Unduh untuk Linux",
  ctaLinuxMetaX64: "Kantor-x86_64.AppImage, untuk kebanyakan PC (Intel atau AMD)",
  ctaLinuxMetaArm: "Kantor-aarch64.AppImage, untuk komputer ARM",
  ctaOtherFiles: "Unduhan lain",
  ctaWindows: "Lihat unduhan untuk Mac dan Linux",
  ctaWindowsNote: "Kantor belum tersedia untuk Windows.",
  ctaMobile: "Lihat unduhan",
  ctaMobileNote: "Kantor adalah aplikasi desktop untuk Mac dan Linux. Buka halaman ini di komputer untuk mengunduhnya.",

  shotsLabel: "Tangkapan layar Kantor",
  shots: {
    sheets: {
      tab: "Sheets",
      alt: "Kantor Sheets membuka laporan penjualan per kuartal: baris judul yang digabung, sel judul kolom berwarna dengan garis tepi, nominal rupiah, dan persentase terhadap target. Sekelompok sel dipilih dan bilah status menampilkan jumlah, rata-rata, dan banyaknya.",
      caption: "Laporan penjualan berformat di Sheets, dengan nominal rupiah dan persentase. Pilih beberapa sel, lalu jumlah, rata-rata, dan banyaknya muncul di bagian bawah.",
    },
    docs: {
      tab: "Docs",
      alt: "Kantor Docs menampilkan laporan singkat di kertas A4, dengan judul, daftar berpoin, gambar grafik batang, dan tabel, di bawah toolbar dan penggaris.",
      caption: "Laporan di Docs, di kertas A4 dengan judul, daftar, gambar, dan tabel.",
    },
    slides: {
      tab: "Slides",
      alt: "Kantor Slides dengan presentasi empat slide. Gambar kecil slide ada di kiri, dan slide yang dibuka berisi pita judul bergradasi, tabel per wilayah, dan grafik kolom, dengan catatan pembicara di bawahnya.",
      caption: "Presentasi di Slides dengan gradasi, bentuk PowerPoint, tabel, dan grafik, tampil seperti di PowerPoint.",
    },
  },
  shotsNote: "Tangkapan layar asli Kantor di Linux. Tampilannya mengikuti mode terang atau gelap sistem Anda.",

  whatH2: "Dibuat untuk file yang sudah Anda punya",
  apps: {
    sheets: {
      name: "Sheets",
      formats: [".xlsx", ".csv", ".ods", ".xls"],
      text: "Lembar kerja dengan rumus, beberapa sheet, format angka (termasuk Rupiah), tabel Excel, grafik, pivot table, format bersyarat, validasi data, urutkan, dan filter. Bisa membuka file .csv, .ods, dan .xls, lalu menyimpannya sebagai .xlsx.",
    },
    docs: {
      name: "Docs",
      formats: [".docx"],
      text: "Surat dan laporan dengan judul, daftar, tabel, gambar, header dan footer dengan nomor halaman, serta ukuran kertas seperti A4, Letter, dan F4, termasuk halaman lanskap. Ada komentar dan lacak perubahan, cari dan ganti, juga penghitung kata.",
    },
    slides: {
      name: "Slides",
      formats: [".pptx"],
      text: "Presentasi dengan teks, bentuk, dan gambar yang bisa dipindah dan diubah ukurannya, tabel dan data grafik yang bisa diedit, catatan pembicara, dan tayangan layar penuh.",
    },
  },
  keepH3: "Menyimpan tidak membuang yang belum bisa ditampilkan Kantor",
  keepText:
    "Kalau file berisi sesuatu yang belum dipahami Kantor, misalnya persamaan di Word atau komentar di Excel, Kantor membiarkannya apa adanya dan menuliskannya kembali saat Anda menyimpan. Kalau itu tidak bisa dilakukan dengan aman, Kantor menyimpan ke file baru atau memberi tahu bagian mana yang tidak ikut tersimpan.",
  more: [
    ["Cetak dan ekspor ke PDF", "Docs dan Slides menyimpan PDF yang sama dengan tampilan di layar dan mencetaknya lewat penampil PDF; Slides juga bisa mencetak halaman catatan dan handout. Sheets mencetak halaman seperti Excel: ukuran kertas termasuk F4, muat selebar halaman, judul cetak, header, dan footer."],
    ["Tanpa internet, tanpa akun", "File Anda tetap di komputer Anda. Tidak perlu mendaftar apa pun."],
    ["Cek ejaan Bahasa Indonesia dan Inggris", "Docs dan Slides menggarisbawahi kata yang salah eja saat Anda mengetik dan memberi saran perbaikan, tanpa perlu internet. Kata Anda sendiri bisa ditambahkan ke kamus pribadi."],
    [
      "Pekerjaan Anda aman",
      'Selama dokumen punya perubahan yang belum disimpan, Kantor menyimpan salinannya di komputer Anda. Setelah aplikasi tertutup mendadak atau listrik padam, Kantor menawarkan untuk memulihkan perubahan Anda, dan file Anda tidak ditimpa sampai Anda menyimpannya. Simpan otomatis juga <a href="' + GH + '#your-work-is-safe">bisa dinyalakan</a> (bahasa Inggris).',
    ],
    [
      "Pembaruan",
      'Kantor memeriksa pembaruan paling sering sekali sehari, hanya dengan mengirim versi dan sistem operasinya, lalu memasang versi baru saat Anda membukanya ulang. Fitur ini <a href="' + GH + '#updates">bisa dimatikan</a> (bahasa Inggris).',
    ],
    [
      "AI hanya kalau Anda mau",
      "Mati sampai Anda menyalakannya. Pakai kunci API Anda sendiri dari penyedia seperti Anthropic, OpenAI, atau Google, atau jalankan model di komputer sendiri dengan Ollama. Kantor tidak punya layanan AI sendiri.",
    ],
    [
      "Asisten AI bisa mengedit file yang terbuka",
      'Asisten seperti Claude bisa membaca dan mengubah dokumen yang sedang Anda buka, sementara Anda melihatnya. Setiap perubahan bisa dibatalkan satu per satu. Fitur ini memakai <a href="https://modelcontextprotocol.io">MCP</a>; cara memasangnya ada di <a href="' + GH + '#ai-agents-mcp">README</a> (bahasa Inggris).',
    ],
  ],

  dlH2: "Unduh Kantor",
  dlIntro: "Gratis, tanpa akun dan tanpa masa percobaan.",
  verLoading: "Memeriksa versi terbaru…",
  verOk: "Versi terbaru: {tag}, dirilis {date}.",
  verNotes: "Apa yang baru",
  verError: "Versi terbaru tidak bisa diperiksa saat ini. Tombol unduh tetap mengambil rilis terbaru.",
  verNone: "Belum ada rilis yang diterbitkan, jadi tombol unduh belum berfungsi. Coba lagi nanti.",
  assetMissing:
    'Rilis terbaru ({tag}) belum menyertakan unduhan ini. <a href="{url}">Lihat file yang tersedia di rilis itu</a>.',
  yourComputer: "Sepertinya komputer Anda",
  mac: {
    name: "Mac",
    req: "macOS 11 ke atas, Apple Silicon atau Intel",
    button: "Unduh Kantor.dmg",
    steps: [
      "Buka <strong>Kantor.dmg</strong> dari folder Downloads.",
      "Seret <strong>Kantor</strong> ke folder Applications.",
      "Buka Kantor dari Applications atau Launchpad. Aplikasinya sudah ditandatangani dan dinotarisasi Apple, jadi bisa dibuka seperti aplikasi lain.",
    ],
  },
  linux: {
    name: "Linux",
    req: "Linux 64-bit. Satu file, tidak perlu memasang apa pun lagi.",
    buttonX64: "Unduh untuk kebanyakan PC",
    buttonX64Meta: "x86_64, Intel atau AMD",
    buttonArm: "Unduh untuk ARM",
    buttonArmMeta: "aarch64",
    steps: [
      "Unduh file untuk komputer Anda. Kalau ragu, hampir selalu yang pertama.",
      "Izinkan file dijalankan: klik kanan file, pilih <strong>Properties</strong>, lalu nyalakan <strong>Allow executing file as program</strong>. Tulisannya sedikit berbeda di tiap pengelola file.",
      "Klik dua kali file itu untuk membuka Kantor.",
    ],
    termH: "Lebih suka terminal?",
    termText:
      "Perintah ini memasang Kantor di <code>~/.local/bin</code> dan menambahkannya ke menu aplikasi. File yang cocok dipilih otomatis. Yang terpasang adalah versi nightly: dibuat dari kode terbaru dan belum diuji sebanyak rilis.",
    copy: "Salin",
    copied: "Tersalin",
    copyFail: "Tidak bisa menyalin. Pilih perintahnya dan salin sendiri.",
  },
  windows: {
    name: "Windows",
    text: "Belum tersedia. Untuk saat ini Kantor hanya untuk Mac dan Linux.",
  },
  win: {
    title: "Kantor: Sheets, Docs, dan Slides untuk Windows, Mac, dan Linux",
    lede: "Kantor adalah aplikasi perkantoran yang ringan dan cepat untuk Windows, Mac, dan Linux. Buka dokumen Word, lembar Excel, atau presentasi PowerPoint, edit, lalu simpan lagi dalam format yang sama. Bisa dipakai tanpa internet dan tanpa akun.",
    fact: "Windows, Mac, dan Linux",
    cta: "Unduh untuk Windows",
    ctaMeta: "Kantor-Setup-x64.exe, untuk Windows 10 dan 11",
    ctaMetaArm: "Kantor-Setup-arm64.exe, untuk Windows di prosesor Arm",
    ctaMobileNote: "Kantor adalah aplikasi desktop untuk Windows, Mac, dan Linux. Buka halaman ini di komputer untuk mengunduhnya.",
    req: "Windows 10 atau 11. Tidak perlu hak administrator.",
    buttonX64: "Unduh untuk kebanyakan PC",
    buttonX64Meta: "x64, Intel atau AMD",
    buttonArm: "Unduh untuk Arm",
    buttonArmMeta: "arm64, Snapdragon",
    steps: [
      "Buka <strong>Kantor-Setup-x64.exe</strong> dari folder Downloads.",
      "Kalau Windows menampilkan <strong>Windows protected your PC</strong>, pilih <strong>More info</strong> lalu <strong>Run anyway</strong>. Pesan ini muncul karena Kantor belum ditandatangani secara digital.",
      "Pilih <strong>Install</strong>. Kantor ada di menu Start, dan file Word, Excel, serta PowerPoint kini bisa dibuka lewat <strong>Open with</strong> &gt; Kantor. Aplikasi bawaan Anda tidak berubah.",
    ],
    portable:
      'Tidak mau pakai installer? Unduh zip portabel untuk <a href="{dl}/Kantor-x64.zip">x64</a> atau <a href="{dl}/Kantor-arm64.zip">Arm</a>, ekstrak di mana saja, lalu buka <strong>kantor.exe</strong>.',
  },

  limitsH2: "Yang belum ada di versi awal ini",
  limitsIntro:
    "Kantor masih baru dan belum dicoba dengan banyak file dari dunia nyata. Simpan salinan file penting, dan perhatikan kekurangan berikut:",
  limits: [
    "Pencetakan lewat penampil PDF: Kantor menyiapkan halamannya, lalu penampil PDF mengirimnya ke printer.",
    "Word: gambar mengambang, bentuk, dan persamaan belum tampil dengan benar, dan header atau footer yang berisi gambar atau tabel belum bisa diedit. Kantor menyimpannya tanpa diubah.",
    "Word: garis titik-titik (leader) di tab belum digambar, dan catatan kaki atau catatan akhir yang terlalu panjang untuk halamannya belum berlanjut ke halaman berikutnya.",
    "Word: Anda bisa melacak suntingan Anda sendiri (track changes), menambah komentar, dan menerima atau menolak perubahan, tetapi perubahan format yang Anda buat belum dilacak.",
    "Excel: aturan format bersyarat, tabel (Format as Table), validasi data, dan pengaturan halaman bisa dibuat dan diubah di Kantor; file yang diubah dengan cara ini belum dicoba dibuka di Excel sendiri.",
    "Excel: makro tidak ikut tersimpan, dan Kantor memberi tahu Anda saat itu terjadi. Pivot table bisa dibuat, disusun ulang, di-refresh, dan dihapus, tetapi belum bisa dikelompokkan per tanggal atau angka, dan calculated field belum didukung.",
    "Excel: grafik bisa disisipkan (kolom, batang, garis, pai, area, sebar) dan diubah lewat panel Grafik, termasuk grafik bawaan file (grafik kombinasi dan 3-D tetap dengan jenis dan rentang datanya); chart sheet, bentuk (shape), serta kotak teks belum ditampilkan.",
    "PowerPoint: isi tabel dan data grafik (termasuk menambah atau menghapus seri dan kategori) diedit lewat panel, bukan langsung di slide, dan SmartArt belum bisa diedit (sebagian tampil sebagai kotak polos).",
  ],
  reportH3: "Menemukan file yang salah tampil di Kantor?",
  reportText:
    "Ceritakan apa yang terjadi. Lampirkan file-nya hanya jika tidak berisi data pribadi.",
  reportButton: "Laporkan masalah di GitHub",

  footSource: "Kode sumber",
  footChangelog: "Catatan perubahan",
  footLicense: "Lisensi AGPL-3.0",
  footBy: "oleh",
};
