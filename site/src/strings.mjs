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
  facts: ["Free and open source", "About 65 MB", "Mac and Linux"],
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
      alt: "Kantor Sheets with a coffee chain's sales by city for 2026, in rupiah millions. The quarters are shaded by a green colour scale, cities below target are highlighted in red, best sellers have data bars, and a bar chart beside the table compares the first and fourth quarters. The year's totals are selected and the status bar shows their sum, average and count.",
      caption: "Sales by city in Sheets, with conditional formatting, data bars and a chart that follows the cells. Select cells and their sum, average and count appear at the bottom.",
    },
    docs: {
      tab: "Docs",
      alt: "Kantor Docs showing a quarterly review on an A4 page: a heading, a table of sales by city, and a paragraph with tracked changes, the inserted words underlined and the deleted ones struck through in the reviewer's colour. A comment card sits beside the page, a misspelt word has a red wavy underline, and two footnotes are at the bottom of the page.",
      caption: "A report in Docs with a table, tracked changes, a comment, spell check and footnotes, as Word lays them out.",
    },
    slides: {
      tab: "Slides",
      alt: "Kantor Slides with a four-slide deck. The thumbnails are on the left, and the open slide has a title, three key figures in rounded boxes and a clustered column chart of sales per city for four quarters, with speaker notes below.",
      caption: "A presentation in Slides with key figures and a chart, as PowerPoint draws them.",
    },
  },
  shotsNote: "Real screenshots of Kantor on Linux. It follows your system's light or dark look.",

  whatH2: "Made for the files you already have",
  apps: {
    sheets: {
      name: "Sheets",
      formats: [".xlsx", ".csv", ".ods", ".xls"],
      text: "Spreadsheets with formulas, several sheets, number formats, Excel tables, charts, pivot tables, conditional formatting, data validation, notes, sort and filter. Opens .csv, .ods and .xls files too (with their formatting, comments, data validation, conditional formatting, charts and pictures), and saves as .xlsx.",
    },
    docs: {
      name: "Docs",
      formats: [".docx", ".doc", ".rtf", ".odt"],
      text: "Letters and reports with headings, lists, tables, pictures, headers and footers with page numbers, and page sizes such as A4, Letter and F4, landscape pages too. Comments and tracked changes, find and replace, and a word count. Opens .doc, .rtf and .odt files too, and saves them as .docx.",
    },
    slides: {
      name: "Slides",
      formats: [".pptx", ".ppt", ".odp"],
      text: "Presentations with text, shapes and pictures you can move and resize, tables and chart data you can edit, speaker notes, and a full-screen slide show. Opens .ppt and .odp files too, and saves as .pptx.",
    },
  },
  keepH3: "Saving doesn't throw away what Kantor can't show yet",
  keepText:
    "If a file holds something Kantor doesn't understand yet, such as a Word equation or an Excel form control, Kantor leaves it as it was and writes it back when you save. When it can't do that safely, it saves to a new file or tells you what will be left out.",
  more: [
    ["Print and export to PDF", "Docs and Slides save a PDF that looks like what you see on screen, and print it through your PDF viewer; Slides also prints notes pages and handouts. Sheets prints pages the way Excel does: paper size including F4, fit to width, print titles, headers and footers."],
    ["Read and organize PDFs", "Open .pdf files in Kantor: scroll, zoom, find and copy text, follow links and print. Drag thumbnails to reorder pages, rotate or delete them, insert pages from other PDFs or pictures, combine and split files, add page numbers or a watermark, and save. Highlight and comment, fill in forms, add a company stamp or your signature, or sign digitally with your own certificate (.p12/.pfx file, USB token or smart card). With your provider account, add an e-meterai (VIDA, Mekari Sign) or a certified electronic signature (Xignature). Do it to a whole folder at once with Batch: stamp, fill forms from a spreadsheet, number pages, then sign every file digitally with one password. Password-protected PDFs stay protected, and digitally signed ones stay valid."],
    ["Offline, no account", "Your files stay on your computer. There is nothing to sign up for."],
    ["In English or Indonesian", "The whole app, menus to messages, is available in Indonesian. It follows your system language, and the command palette finds commands in both languages."],
    ["Your team's cloud drive", "Open and save files on Nextcloud, OpenCloud or any WebDAV server. If a colleague saved first, Kantor asks before anything is overwritten; a file someone else has open opens read-only with their name. Without internet you keep working, and it syncs when the server is back."],
    ["Spell check in English and Indonesian", "Docs and Slides underline misspelled words as you type and suggest corrections; Sheets checks your text cells with F7, as Excel does. No internet needed. Add your own words to a personal dictionary."],
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
      'Off until you turn it on. Use <a href="#paparan">Paparan AI</a> with an account and credits, your own key from a provider such as Anthropic, OpenAI or Google, or a model on your own computer with Ollama.',
    ],
    [
      "AI assistants can edit open files",
      'Assistants such as Claude can read and change the documents you have open, while you watch. Each change is one step you can undo. They can also build whole presentations, with shapes, pictures, tables and backgrounds, from a description (also on the command line: <code>kantor slides build</code>). This uses <a href="https://modelcontextprotocol.io">MCP</a>; the setup is in the <a href="' + GH + '#ai-agents-mcp">README</a>.',
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
    req: "64-bit Linux: Ubuntu 22.04, Debian 12, RHEL 9, Fedora 35 or newer (glibc 2.34). One file, nothing else to install.",
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

  navThesis: "Thesis",
  thesis: {
    h2: "For your thesis: PLS-SEM in Sheets, citations in Docs",
    lede:
      "Run the structural model for your results chapter next to the survey data, and keep your Zotero or Mendeley citations working while you write. Both are part of the free app and work offline on Windows, Mac and Linux.",
    shot: "pls",
    shotAlt:
      "Kantor Sheets with a PLS-SEM model tab. The path diagram shows construct X pointing to Y directly and through a mediator M, with ten indicators on M laid out in two staggered rows. The loadings sit on the indicator links, the paths show coefficients with bootstrap p values in brackets, and the significant paths are drawn in green. Below the diagram, the results pane lists reliability and convergent validity for each construct.",
    shotCaption:
      "A mediation model after a bootstrap: loadings, path coefficients with p values and R² on the diagram, the tables below it.",
    plsH3: "PLS-SEM, drawn the way SmartPLS draws it",
    pls: [
      ["Draw the model", "Select the questionnaire data, drag the item columns onto the canvas to make constructs, then draw the paths. Reflective or formative constructs, PLS or consistent PLS (PLSc)."],
      ["Bootstrap", "5,000 subsamples by default, on every processor core, with a progress bar and a Stop button. t and p values, one- or two-tailed, with percentile and BCa intervals."],
      ["Every table your results chapter needs", "Loadings, Cronbach's alpha, rho_A, composite reliability, AVE, Fornell-Larcker, HTMT, cross-loadings, VIF, R², f², indirect effects, SRMR and NFI. Values outside the usual thresholds (Hair et al.) are marked."],
      ["A report to start from", "Export an APA-style report in Indonesian or English (.docx) or every table as .xlsx, or insert the tables as a new sheet. The model is saved inside the workbook."],
      ["Ask Paparan AI to explain", 'Explain results hands the model, the data and the results to <a href="#paparan">Paparan AI</a>, which writes the discussion of the results (paid, and optional).'],
    ],
    plsCheck:
      "Checked against SmartPLS 4's own output for 14 textbook models: the numbers match to the three decimals SmartPLS prints. There's no limit on the number of respondents.",
    citeH3: "Citations that still work in Word",
    cite: [
      "Docs opens a thesis written with Zotero, Mendeley, Mendeley Cite, Word or EndNote with its citations intact, and saves it so your plug-in still works when you go back to Word.",
      "With Zotero open, <strong>Insert Citation</strong> and <strong>Insert Bibliography</strong> add new ones. <strong>Citation Style</strong> switches the whole document between APA, IEEE, Harvard, Chicago, Vancouver, MLA and about 80 other styles, in Indonesian or English.",
      "No Zotero? Import a .bib, RIS or CSL-JSON file into the document and cite from that.",
    ],
  },

  navPaparan: "Paparan AI",
  pap: {
    h2: "Let Paparan AI make the deck, document or workbook",
    lede:
      "Paparan AI is the paid AI inside Kantor. Say what you need and attach your files, and it builds a real PowerPoint deck, Word document or Excel workbook. Before it hands the file back, it runs Kantor's checks on it and looks at the pages.",
    shot: "paparan",
    shotAlt:
      "Kantor with the Paparan AI chat panel on the right. The request was to turn penjualan-2026.csv into a workbook with monthly totals per city and a line chart. The panel shows a picture of the printed page the agent checked, a summary of what it made, and the file kopi-senja-2026.xlsx with an Open button. On the left the workbook is open: each city's sales for every month of 2026, a Total column and row, and a line chart of the monthly total below.",
    shotCaption:
      "A CSV file in, a checked workbook out: formulas that stay live, totals and a chart, opened in Sheets next to the chat.",
    asksH3: "What you can ask for",
    asks: [
      {
        app: "slides",
        ask: "A 6-slide deck on this quarter's sales for the board, from sales.xlsx",
        get: "An editable .pptx with the key figures, a chart drawn from your numbers, and speaker notes.",
      },
      {
        app: "docs",
        ask: "Tighten proposal.docx and show the edits as tracked changes",
        get: "Your document back with every edit tracked, to accept or reject in Kantor or Word.",
      },
      {
        app: "sheets",
        ask: "Is rekap.xlsx right? Fix the totals.",
        get: "What was wrong (formula errors, typed totals, sums that skip rows, numbers stored as text) and a fixed copy. Your own file is never changed.",
      },
    ],
    startH3: "How to start",
    start: [
      'Install Kantor 0.25.0 or later from the <a href="#download">downloads</a> above.',
      "Open the chat with the sparkle button at the top right of the window, or find <strong>Paparan AI Chat</strong> in the command palette <span class=\"nowrap\">(<span class=\"kbd\">Ctrl K</span>)</span>.",
      "Choose <strong>Sign in</strong> and connect a Paparan account (Google sign-in works). The first connection adds 300 free credits.",
      "Describe the job, attach CSV, Excel, Word or PowerPoint files or pictures, and send. Ask for changes in the same chat and it works on the same files.",
    ],
    priceH3: "What it costs",
    priceText:
      "You pay per job with credits, and 1 credit is Rp10. In our tests a 6-slide deck took about 120 credits (about Rp1,200), and a checked workbook with a chart from a CSV file about 100. Bigger jobs take more; the panel shows the credits you have left after each job.",
    packsLabel: "Credit packs",
    packs: [
      ["5,000 credits", "Rp50,000", "US$3"],
      ["22,000 credits", "Rp200,000", "US$12"],
      ["60,000 credits", "Rp500,000", "US$30"],
    ],
    packsNote: "Pay in rupiah by QRIS, virtual account or e-wallet, or by card in US dollars. No subscription needed.",
    privacy:
      "Paparan AI gets only what each step of a job needs, such as text from your files and pictures of the pages it checks, and doesn't keep your documents. The job's files stay in a folder on your computer.",
    free:
      "Kantor stays free without it. Its built-in AI tools also run with your own key for an OpenAI-compatible service, or with Ollama on your own computer.",
    more: "Plans, your account and more about Paparan AI",
  },

  limitsH2: "What's missing in this preview",
  limitsIntro:
    "Kantor is early software. It has been tried on a few hundred real-world files, but not yet for how pages look next to Office. Keep a copy of anything important, and expect these gaps:",
  limits: [
    "Printing goes through your PDF viewer: Kantor makes the pages and the viewer sends them to the printer.",
    "PDF: Kantor adds comments, stamps, form entries and signatures (pictures, or digital signatures with your own certificate), organizes pages and checks digital signatures (password-protected PDFs too), but doesn't edit the text on a page. e-Meterai and certified signatures need your own account at a provider and haven't been tried against the providers' live services yet, and a password-protected PDF gets an e-meterai only as a copy without the password; providers without public API documentation (Peruri, Privy, Tilaka…) aren't available.",
    "Word: equations aren't shown properly yet (Kantor keeps them and saves them unchanged); SmartArt's text can be edited (its pictures are drawn too), its layout changed (lists, processes, cycles, hierarchies, Venn, matrix, pyramid and picture layouts) and items added and removed in those layouts, with Organization Chart assistants and hanging layouts (other layouts keep their items).",
    "Word: fields show their results and F9 updates them (dates, cross-references, captions, tables of contents, formulas, IF, STYLEREF, LISTNUM), Alt+F9 shows and edits field codes, and Insert Field, Bookmark and Cross-reference work as in Word; an index (INDEX) lists its entries, pages, cross-references and letter headings, in its columns. Mail merge fills letters, certificates and labels from a workbook or CSV file as Word's Mailings tab does, Address Block and Greeting Line included (one document, or one .docx or PDF per recipient); unticked recipients aren't saved in the file, and envelopes, label sheets and e-mail aren't made.",
    "Word: Track Changes records your edits (text, formatting, table rows and columns, merged and split cells, field codes and updates, headers, footers and notes) and you can accept or reject them. Comments can be replied to and resolved.",
    "Word 97-2003 (.doc): floating pictures (WMF and EMF too), shapes, groups and text boxes (in headers and footers too), comments, nested tables and tracked changes come in, but files from Word 6 or Word 95 and password-protected files don't open.",
    "Excel: conditional formatting rules, tables (Format as Table) and their custom table styles, data validation and page setup can be made and changed in Kantor; files changed this way haven't been opened in Excel itself yet.",
    "Excel: macros in .xlsm workbooks are kept when you save (Kantor doesn't run them, and says so); saving as .xlsx warns that they'll be left out, as Excel does. Pivot tables can be created, rearranged, grouped, refreshed and deleted, with calculated fields and items (items named by position too), Show Values As, filters (several per field when the pivot allows it) and styles.",
    "OpenDocument (.ods): charts are brought in as column, bar, line, area, pie, doughnut, scatter, radar, bubble and stock charts; freehand shapes, groups (with the charts inside them), hatched fills (as patterns) and form controls come in too (controls are drawn, not run).",
    "Excel: charts can be inserted (column, bar, line, pie, area, scatter) and changed in a Chart panel, including charts that came with the file (combo charts with a secondary axis, 3-D charts and the Office 2016 types too); chart sheets are shown, printed and edited (their chart in the Chart panel; Rename, Delete, Move or Copy, Tab Color, Hide and Unhide from the tab), and Move Chart puts a chart on its own chart sheet and back, keeping its data link and user shapes; shapes, text boxes and groups (with the charts inside them) are shown, printed and added (a text box and a few common shapes), and their text, fill (also a picture or a pattern), outline and shadow can be changed (Format Shape); 3-D effects are drawn approximately, and text with fields or equations is shown but not edited. Sparklines (line, column and win/loss) are shown and printed but can't be made or changed yet.",
    "PLS-SEM: PLSpredict, IPMA, multigroup analysis, moderation with interaction terms and higher-order constructs aren't there yet.",
    "Citations: Word and EndNote citations are kept and listed but not restyled, footnote styles put the citation in the text, and punctuation can differ slightly from Zotero's own output, so Refresh Citations runs only when you ask.",
    "PowerPoint: chart data (series and categories inserted, deleted or reordered anywhere) is edited in a panel, not on the slide; SmartArt text can be edited and its shapes moved and resized; its layout can be changed to a list, process, cycle, hierarchy, Venn, matrix, pyramid or picture layout, and items added and removed in those layouts, with Organization Chart assistants and hanging layouts (some SmartArt shows as a plain box); map charts list their values when their map data is in a form Kantor can't read, and the series of the newer chart types (waterfall, treemap…) can't be added or removed (their data can be edited).",
  ],
  reportH3: "Found a file that Kantor gets wrong?",
  reportText:
    "Tell us what happened. Attach the file only if it holds nothing private.",
  reportButton: "Report a problem on GitHub",

  footSource: "Source code",
  footChangelog: "Changelog",
  footLicense: "AGPL-3.0 licence",
  footPrivacy: "Privacy",
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
  facts: ["Gratis dan open source", "Sekitar 65 MB", "Mac dan Linux"],
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
      alt: "Kantor Sheets membuka penjualan sebuah kedai kopi per kota tahun 2026, dalam juta rupiah. Kolom kuartal diwarnai skala warna hijau, kota yang di bawah target disorot merah, menu terlaris punya bilah data, dan grafik batang di samping tabel membandingkan kuartal pertama dan keempat. Total setahun dipilih dan bilah status menampilkan jumlah, rata-rata, dan banyaknya.",
      caption: "Penjualan per kota di Sheets, dengan pemformatan bersyarat, bilah data, dan grafik yang mengikuti isi sel. Pilih beberapa sel, lalu jumlah, rata-rata, dan banyaknya muncul di bagian bawah.",
    },
    docs: {
      tab: "Docs",
      alt: "Kantor Docs menampilkan laporan kuartalan di kertas A4: judul bagian, tabel penjualan per kota, dan paragraf dengan perubahan terlacak, kata yang ditambahkan bergaris bawah dan yang dihapus dicoret dengan warna peninjau. Kartu komentar ada di samping halaman, kata yang salah eja bergaris bawah merah bergelombang, dan dua catatan kaki ada di bagian bawah halaman.",
      caption: "Laporan di Docs dengan tabel, perubahan terlacak, komentar, cek ejaan, dan catatan kaki, tersusun seperti di Word.",
    },
    slides: {
      tab: "Slides",
      alt: "Kantor Slides dengan presentasi empat slide. Gambar kecil slide ada di kiri, dan slide yang dibuka berisi judul, tiga angka utama dalam kotak bersudut bulat, dan grafik kolom penjualan per kota selama empat kuartal, dengan catatan pembicara di bawahnya.",
      caption: "Presentasi di Slides dengan angka utama dan grafik, tampil seperti di PowerPoint.",
    },
  },
  shotsNote: "Tangkapan layar asli Kantor di Linux. Tampilannya mengikuti mode terang atau gelap sistem Anda.",

  whatH2: "Dibuat untuk file yang sudah Anda punya",
  apps: {
    sheets: {
      name: "Sheets",
      formats: [".xlsx", ".csv", ".ods", ".xls"],
      text: "Lembar kerja dengan rumus, beberapa sheet, format angka (termasuk Rupiah), tabel Excel, grafik, pivot table, format bersyarat, validasi data, catatan sel (notes), urutkan, dan filter. Bisa membuka file .csv, .ods, dan .xls (tetap dengan format, komentar, validasi data, format bersyarat, grafik, dan gambarnya), lalu menyimpannya sebagai .xlsx.",
    },
    docs: {
      name: "Docs",
      formats: [".docx", ".doc", ".rtf", ".odt"],
      text: "Surat dan laporan dengan judul, daftar, tabel, gambar, header dan footer dengan nomor halaman, serta ukuran kertas seperti A4, Letter, dan F4, termasuk halaman lanskap. Ada komentar dan lacak perubahan, cari dan ganti, juga penghitung kata. Bisa membuka file .doc, .rtf, dan .odt, lalu menyimpannya sebagai .docx.",
    },
    slides: {
      name: "Slides",
      formats: [".pptx", ".ppt", ".odp"],
      text: "Presentasi dengan teks, bentuk, dan gambar yang bisa dipindah dan diubah ukurannya, tabel dan data grafik yang bisa diedit, catatan pembicara, dan tayangan layar penuh. Bisa membuka file .ppt dan .odp, lalu menyimpannya sebagai .pptx.",
    },
  },
  keepH3: "Menyimpan tidak membuang yang belum bisa ditampilkan Kantor",
  keepText:
    "Kalau file berisi sesuatu yang belum dipahami Kantor, misalnya persamaan di Word atau kontrol formulir di Excel, Kantor membiarkannya apa adanya dan menuliskannya kembali saat Anda menyimpan. Kalau itu tidak bisa dilakukan dengan aman, Kantor menyimpan ke file baru atau memberi tahu bagian mana yang tidak ikut tersimpan.",
  more: [
    ["Cetak dan ekspor ke PDF", "Docs dan Slides menyimpan PDF yang sama dengan tampilan di layar dan mencetaknya lewat penampil PDF; Slides juga bisa mencetak halaman catatan dan handout. Sheets mencetak halaman seperti Excel: ukuran kertas termasuk F4, muat selebar halaman, judul cetak, header, dan footer."],
    ["Baca dan atur PDF", "Buka berkas .pdf di Kantor: gulir, perbesar, cari dan salin teks, ikuti tautan, dan cetak. Seret gambar mini untuk menyusun ulang halaman, putar atau hapus, sisipkan halaman dari PDF lain atau gambar, gabungkan dan pisahkan berkas, tambahkan nomor halaman atau tanda air, lalu simpan. Sorot dan beri komentar, isi formulir, bubuhkan stempel perusahaan atau tanda tangan Anda, atau tanda tangani secara digital dengan sertifikat Anda sendiri (berkas .p12/.pfx, token USB, atau kartu pintar). Dengan akun penyedia Anda, bubuhkan e-meterai (VIDA, Mekari Sign) atau tanda tangan elektronik tersertifikasi (Xignature). Lakukan sekaligus untuk satu folder dengan Batch: stempel, isi formulir dari spreadsheet, beri nomor halaman, lalu tanda tangani setiap berkas secara digital dengan satu kata sandi. PDF yang dilindungi kata sandi tetap terlindungi, dan PDF bertanda tangan digital tetap sah."],
    ["Tanpa internet, tanpa akun", "File Anda tetap di komputer Anda. Tidak perlu mendaftar apa pun."],
    ["Dalam Bahasa Indonesia atau Inggris", "Seluruh aplikasi, dari menu sampai pesan, tersedia dalam Bahasa Indonesia. Bahasanya mengikuti sistem Anda, dan palet perintah menemukan perintah dalam kedua bahasa."],
    ["Penyimpanan cloud tim Anda", "Buka dan simpan berkas di Nextcloud, OpenCloud, atau server WebDAV lain. Kalau rekan Anda sudah menyimpan lebih dulu, Kantor bertanya sebelum ada yang ditimpa; berkas yang sedang dibuka orang lain terbuka hanya-baca dengan namanya. Tanpa internet Anda tetap bisa bekerja, lalu berkas disinkronkan saat server kembali tersedia."],
    ["Cek ejaan Bahasa Indonesia dan Inggris", "Docs dan Slides menggarisbawahi kata yang salah eja saat Anda mengetik dan memberi saran perbaikan; Sheets memeriksa sel teks Anda dengan F7, seperti Excel. Tanpa perlu internet. Kata Anda sendiri bisa ditambahkan ke kamus pribadi."],
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
      'Mati sampai Anda menyalakannya. Pakai <a href="#paparan">Paparan AI</a> dengan akun dan kredit, kunci API Anda sendiri dari penyedia seperti Anthropic, OpenAI, atau Google, atau model di komputer sendiri dengan Ollama.',
    ],
    [
      "Asisten AI bisa mengedit file yang terbuka",
      'Asisten seperti Claude bisa membaca dan mengubah dokumen yang sedang Anda buka, sementara Anda melihatnya. Setiap perubahan bisa dibatalkan satu per satu. Asisten juga bisa membuat presentasi utuh, lengkap dengan bentuk, gambar, tabel, dan latar, dari sebuah deskripsi (juga lewat baris perintah: <code>kantor slides build</code>). Fitur ini memakai <a href="https://modelcontextprotocol.io">MCP</a>; cara memasangnya ada di <a href="' + GH + '#ai-agents-mcp">README</a> (bahasa Inggris).',
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
    req: "Linux 64-bit: Ubuntu 22.04, Debian 12, RHEL 9, Fedora 35 atau yang lebih baru (glibc 2.34). Satu file, tidak perlu memasang apa pun lagi.",
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

  navThesis: "Skripsi",
  thesis: {
    h2: "Untuk skripsi: PLS-SEM di Sheets, sitasi di Docs",
    lede:
      "Olah model struktural untuk Bab 4 langsung di samping data kuesioner, dan sitasi Zotero atau Mendeley Anda tetap berfungsi selama menulis. Keduanya bagian dari aplikasi gratis ini dan jalan tanpa internet di Windows, Mac, dan Linux.",
    shot: "pls-id",
    shotAlt:
      "Kantor Sheets dengan tab model PLS-SEM. Diagram jalurnya menunjukkan konstruk X menuju Y secara langsung dan lewat mediator M, dengan sepuluh indikator M tersusun dalam dua baris berselang-seling. Nilai loading ada di garis indikator, jalur menampilkan koefisien dengan nilai p bootstrap dalam kurung, dan jalur yang signifikan berwarna hijau. Di bawah diagram, panel hasil menampilkan reliabilitas dan validitas konvergen tiap konstruk.",
    shotCaption:
      "Model mediasi setelah bootstrap: loading, koefisien jalur dengan nilai p, dan R² di diagram, tabelnya di bawah.",
    plsH3: "PLS-SEM dengan diagram jalur seperti di SmartPLS",
    pls: [
      ["Gambar modelnya", "Pilih data kuesioner, seret kolom item ke kanvas untuk membuat konstruk, lalu tarik jalurnya. Konstruk reflektif atau formatif, PLS atau PLS konsisten (PLSc)."],
      ["Bootstrap", "Bawaannya 5.000 subsampel, memakai semua inti prosesor, lengkap dengan bilah kemajuan dan tombol Berhenti. Nilai t dan p satu atau dua arah, dengan interval persentil dan BCa."],
      ["Semua tabel untuk Bab 4", "Loading, Cronbach's alpha, rho_A, reliabilitas komposit, AVE, Fornell-Larcker, HTMT, cross-loading, VIF, R², f², efek tidak langsung, SRMR, dan NFI. Nilai yang di luar batas lazim (Hair dkk.) langsung ditandai."],
      ["Laporan sebagai titik awal", "Ekspor laporan gaya APA dalam Bahasa Indonesia atau Inggris (.docx) atau semua tabel sebagai .xlsx, atau sisipkan tabelnya ke lembar kerja baru. Modelnya ikut tersimpan di dalam buku kerja."],
      ["Minta Paparan AI menjelaskan", 'Tombol Jelaskan hasil mengirim model, data, dan hasilnya ke <a href="#paparan">Paparan AI</a>, yang lalu menuliskan pembahasan hasilnya (berbayar, dan tidak wajib).'],
    ],
    plsCheck:
      "Sudah dicocokkan dengan keluaran SmartPLS 4 untuk 14 model dari buku teks: angkanya sama sampai tiga desimal yang dicetak SmartPLS. Jumlah responden tidak dibatasi.",
    citeH3: "Sitasi yang tetap jalan di Word",
    cite: [
      "Docs membuka skripsi yang ditulis dengan Zotero, Mendeley, Mendeley Cite, Word, atau EndNote tanpa merusak sitasinya, lalu menyimpannya sehingga plug-in Anda tetap jalan saat kembali ke Word.",
      "Selama Zotero terbuka, <strong>Sisipkan Sitasi</strong> dan <strong>Sisipkan Daftar Pustaka</strong> menambahkan yang baru. <strong>Gaya Sitasi</strong> mengganti gaya seluruh dokumen ke APA, IEEE, Harvard, Chicago, Vancouver, MLA, atau sekitar 80 gaya lain, dalam Bahasa Indonesia atau Inggris.",
      "Tidak pakai Zotero? Impor berkas .bib, RIS, atau CSL-JSON ke dokumen, lalu kutip dari situ.",
    ],
  },

  navPaparan: "Paparan AI",
  pap: {
    h2: "Biar Paparan AI yang membuat paparan, dokumen, atau buku kerjanya",
    lede:
      "Paparan AI adalah AI berbayar di dalam Kantor. Tulis apa yang Anda perlukan dan lampirkan berkasnya, lalu Paparan AI membuat presentasi PowerPoint, dokumen Word, atau buku kerja Excel sungguhan. Sebelum berkasnya diserahkan, Paparan AI memeriksanya dengan pemeriksaan Kantor sendiri dan melihat halamannya.",
    shot: "paparan-id",
    shotAlt:
      "Kantor dengan panel obrolan Paparan AI di kanan. Permintaannya mengubah penjualan-2026.csv menjadi buku kerja berisi total bulanan per kota dan grafik garis. Panel menampilkan gambar halaman cetak yang diperiksa, ringkasan hasilnya, dan berkas kopi-senja-2026.xlsx dengan tombol Buka. Di kiri buku kerjanya terbuka: penjualan tiap kota per bulan sepanjang 2026, kolom dan baris Total, dan grafik garis total bulanan di bawahnya.",
    shotCaption:
      "CSV masuk, buku kerja yang sudah dicek keluar: rumus yang tetap hidup, total, dan grafik, dibuka di Sheets di samping obrolan.",
    asksH3: "Yang bisa Anda minta",
    asks: [
      {
        app: "slides",
        ask: "Paparan 6 slide tentang penjualan kuartal ini untuk direksi, dari sales.xlsx",
        get: "Berkas .pptx yang bisa diedit, berisi angka utama, grafik dari angka Anda, dan catatan pembicara.",
      },
      {
        app: "docs",
        ask: "Rapikan proposal.docx dan tandai perubahannya dengan lacak perubahan",
        get: "Dokumen Anda kembali dengan setiap suntingan terlacak, untuk diterima atau ditolak di Kantor atau Word.",
      },
      {
        app: "sheets",
        ask: "Rekap.xlsx ini sudah benar? Perbaiki totalnya.",
        get: "Daftar yang salah (rumus galat, total yang diketik manual, SUM yang melewatkan baris, angka yang tersimpan sebagai teks) dan salinan yang sudah diperbaiki. Berkas asli Anda tidak pernah diubah.",
      },
    ],
    startH3: "Cara mulai",
    start: [
      'Pasang Kantor 0.25.0 atau yang lebih baru dari <a href="#download">unduhan</a> di atas.',
      "Buka obrolan dengan tombol ikon kilau di kanan atas jendela, atau cari <strong>Obrolan Paparan AI</strong> di palet perintah <span class=\"nowrap\">(<span class=\"kbd\">Ctrl K</span>)</span>.",
      "Pilih <strong>Masuk</strong> dan hubungkan akun Paparan (bisa masuk dengan Google). Koneksi pertama mendapat 300 kredit gratis.",
      "Jelaskan pekerjaannya, lampirkan berkas CSV, Excel, Word, PowerPoint, atau gambar, lalu kirim. Minta perubahan di obrolan yang sama dan Paparan AI mengerjakan berkas yang sama.",
    ],
    priceH3: "Biayanya",
    priceText:
      "Anda membayar per pekerjaan dengan kredit, dan 1 kredit sama dengan Rp10. Dalam uji coba kami, paparan 6 slide memakai sekitar 120 kredit (sekitar Rp1.200), dan buku kerja dengan grafik dari berkas CSV yang sudah dicek sekitar 100. Pekerjaan yang lebih besar memakai lebih banyak; panel menampilkan sisa kredit Anda setelah setiap pekerjaan.",
    packsLabel: "Paket kredit",
    packs: [
      ["5.000 kredit", "Rp50.000", "US$3"],
      ["22.000 kredit", "Rp200.000", "US$12"],
      ["60.000 kredit", "Rp500.000", "US$30"],
    ],
    packsNote: "Bayar dalam rupiah lewat QRIS, virtual account, atau dompet digital, atau dengan kartu dalam dolar AS. Tanpa langganan.",
    privacy:
      "Paparan AI hanya menerima yang diperlukan tiap langkah pekerjaan, misalnya teks dari berkas Anda dan gambar halaman yang diperiksanya, dan tidak menyimpan dokumen Anda. Berkas pekerjaannya tetap di folder di komputer Anda.",
    free:
      "Kantor tetap gratis tanpa Paparan AI. Fitur AI bawaannya juga jalan dengan kunci Anda sendiri untuk layanan yang kompatibel dengan OpenAI, atau dengan Ollama di komputer Anda.",
    more: "Paket, akun Anda, dan info lain tentang Paparan AI",
  },

  limitsH2: "Yang belum ada di versi awal ini",
  limitsIntro:
    "Kantor masih baru. Kantor sudah dicoba dengan beberapa ratus file dari dunia nyata, tetapi tampilan halamannya belum dibandingkan dengan Office. Simpan salinan file penting, dan perhatikan kekurangan berikut:",
  limits: [
    "Pencetakan lewat penampil PDF: Kantor menyiapkan halamannya, lalu penampil PDF mengirimnya ke printer.",
    "PDF: Kantor bisa menambahkan komentar, stempel, isian formulir, dan tanda tangan (gambar, atau tanda tangan digital dengan sertifikat Anda sendiri), menyusun halaman, serta memeriksa tanda tangan digital (juga pada PDF yang dilindungi kata sandi), tetapi tidak mengedit teks di halaman. e-Meterai dan tanda tangan tersertifikasi memerlukan akun Anda sendiri di penyedia dan belum dicoba pada layanan penyedia yang sebenarnya, dan PDF yang dilindungi kata sandi hanya dapat dibubuhi e-meterai sebagai salinan tanpa kata sandi; penyedia yang dokumentasi API-nya tidak terbuka (Peruri, Privy, Tilaka…) belum tersedia.",
    "Word: persamaan belum tampil dengan benar (Kantor menyimpannya tanpa diubah); teks SmartArt bisa diedit (gambarnya juga ditampilkan), tata letaknya bisa diubah (daftar, proses, siklus, hierarki, Venn, matriks, piramida, dan tata letak gambar), dan item bisa ditambah atau dihapus pada tata letak tersebut, termasuk asisten dan tata letak menggantung pada Bagan Organisasi (tata letak lain tetap dengan itemnya).",
    "Word: bidang (field) menampilkan hasilnya dan F9 memperbaruinya (tanggal, referensi silang, keterangan, daftar isi, rumus, IF, STYLEREF, LISTNUM), Alt+F9 menampilkan dan mengedit kode bidang, dan Sisipkan Bidang, Penanda, serta Referensi Silang bekerja seperti di Word; indeks (INDEX) berisi entri, halaman, referensi silang, dan judul huruf, dalam kolom-kolomnya. Surat massal (mail merge) mengisi surat, sertifikat, dan label dari buku kerja atau file CSV seperti tab Mailings di Word, termasuk Blok Alamat dan Baris Salam (satu dokumen, atau satu .docx atau PDF per penerima); penerima yang tidak dicentang tidak disimpan di file, dan amplop, lembar label, serta email belum bisa dibuat.",
    "Word: Track Changes mencatat suntingan Anda (teks, format, baris dan kolom tabel, sel yang digabung dan dipecah, kode dan pembaruan field, header, footer, dan catatan kaki) dan Anda bisa menerima atau menolaknya. Komentar bisa dibalas dan ditandai selesai.",
    "Word 97-2003 (.doc): gambar mengambang (termasuk WMF dan EMF), bentuk, grup bentuk, dan kotak teks (juga di header dan footer), komentar, tabel bertingkat, serta perubahan terlacak ikut terbuka, tetapi file dari Word 6 atau Word 95 serta file berkata sandi tidak bisa dibuka.",
    "Excel: aturan format bersyarat, tabel (Format as Table) beserta gaya tabel kustomnya, validasi data, dan pengaturan halaman bisa dibuat dan diubah di Kantor; file yang diubah dengan cara ini belum dicoba dibuka di Excel sendiri.",
    "Excel: makro di buku kerja .xlsm tetap tersimpan saat Anda menyimpan (Kantor tidak menjalankannya, dan memberi tahu Anda); menyimpan sebagai .xlsx memperingatkan bahwa makro akan dibuang, seperti di Excel. Pivot table bisa dibuat, disusun ulang, dikelompokkan, di-refresh, dan dihapus, lengkap dengan calculated field dan calculated item (item juga bisa disebut menurut posisinya), Show Values As, filter (beberapa per bidang bila pivot mengizinkannya), dan gaya.",
    "OpenDocument (.ods): grafik ikut terbuka sebagai grafik kolom, batang, garis, area, pai, donat, sebar, radar, gelembung, dan saham; bentuk gambar tangan bebas, grup bentuk (beserta grafik di dalamnya), isian arsir (sebagai pola), dan kontrol formulir juga ikut terbuka (kontrol hanya digambar, tidak dijalankan).",
    "Excel: grafik bisa disisipkan (kolom, batang, garis, pai, area, sebar) dan diubah lewat panel Grafik, termasuk grafik bawaan file (juga grafik kombinasi dengan sumbu sekunder, grafik 3-D, dan jenis Office 2016); chart sheet sudah ditampilkan, dicetak, dan bisa diubah (grafiknya lewat panel Grafik; Ganti Nama, Hapus, Pindah atau Salin, Warna Tab, Sembunyikan, dan Tampilkan dari tabnya), dan Pindahkan Grafik memindahkan grafik ke chart sheet sendiri dan sebaliknya, dengan tautan data dan bentuk penggunanya; bentuk (shape), kotak teks, dan grup (beserta grafik di dalamnya) sudah ditampilkan, dicetak, dan bisa ditambahkan (kotak teks dan beberapa bentuk umum), dan teks, isian (juga gambar atau pola), garis tepi, serta bayangannya bisa diubah (Format Bentuk); efek 3-D digambar secara mendekati, dan teks dengan bidang atau persamaan ditampilkan tetapi tidak bisa diedit. Sparkline (garis, kolom, dan menang/kalah) ditampilkan dan dicetak, tetapi belum bisa dibuat atau diubah.",
    "PLS-SEM: PLSpredict, IPMA, analisis multigrup, moderasi dengan suku interaksi, dan konstruk orde tinggi belum tersedia.",
    "Sitasi: sitasi Word dan EndNote tetap tersimpan dan terdaftar tetapi gayanya tidak diubah, gaya catatan kaki menaruh sitasi di dalam teks, dan tanda bacanya bisa sedikit berbeda dari keluaran Zotero sendiri, jadi Perbarui Sitasi hanya berjalan saat Anda memintanya.",
    "PowerPoint: data grafik (termasuk menyisipkan, menghapus, atau mengurutkan ulang seri dan kategori di posisi mana pun) diedit lewat panel, bukan langsung di slide; teks SmartArt bisa diedit dan bentuknya bisa dipindah serta diubah ukurannya; tata letaknya bisa diubah ke daftar, proses, siklus, hierarki, Venn, matriks, piramida, atau tata letak gambar, dan item bisa ditambah atau dihapus pada tata letak tersebut, termasuk asisten dan tata letak menggantung pada Bagan Organisasi (sebagian SmartArt tampil sebagai kotak polos); grafik peta menampilkan daftar nilainya bila data petanya tersimpan dalam bentuk yang belum bisa dibaca Kantor, dan seri jenis grafik baru (waterfall, treemap…) belum bisa ditambah atau dihapus (datanya bisa diedit).",
  ],
  reportH3: "Menemukan file yang salah tampil di Kantor?",
  reportText:
    "Ceritakan apa yang terjadi. Lampirkan file-nya hanya jika tidak berisi data pribadi.",
  reportButton: "Laporkan masalah di GitHub",

  footSource: "Kode sumber",
  footChangelog: "Catatan perubahan",
  footLicense: "Lisensi AGPL-3.0",
  footPrivacy: "Privasi",
  footBy: "oleh",
};
