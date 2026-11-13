<p align="center"><img src="assets/icons/kantor.svg" width="128" alt="Kantor"></p>

# Kantor

A small, fast, native office suite for Linux, macOS and Windows, written in Rust on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) (the UI framework behind Zed). Three apps in one binary: **Sheets**, **Docs** and **Slides**, for `.xlsx`, `.docx` and `.pptx` files. Builds: Linux on x86_64 and aarch64 (made with [Omarchy](https://omarchy.org) in mind), macOS on Apple Silicon and Intel, and Windows 10/11 on x64 and Arm (new, see below).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="site/src/img/sheets-dark-1550.webp">
  <img src="site/src/img/sheets-light-1550.webp" width="100%" alt="Kantor Sheets showing a quarterly sales workbook: a merged title row on a dark blue fill, column headers on a light blue fill with borders, amounts in Rupiah such as Rp412.500.000, a vs-target column in green and red percentages, and a yellow note block. A range of cells is selected and the status bar shows its sum, average and count.">
</picture>

<table>
  <tr>
    <td width="33%"><picture><source media="(prefers-color-scheme: dark)" srcset="site/src/img/sheets-dark-800.webp"><img src="site/src/img/sheets-light-800.webp" alt="Sheets: a formatted sales workbook with fills, borders, merged cells and Rupiah and percent number formats."></picture></td>
    <td width="33%"><picture><source media="(prefers-color-scheme: dark)" srcset="site/src/img/docs-dark-800.webp"><img src="site/src/img/docs-light-800.webp" alt="Docs: a one-page report on an A4 page with a title, headings, a bulleted list, a bar chart picture and a table, below the toolbar and ruler."></picture></td>
    <td width="33%"><picture><source media="(prefers-color-scheme: dark)" srcset="site/src/img/slides-dark-800.webp"><img src="site/src/img/slides-light-800.webp" alt="Slides: a slide with a gradient title band, a table of regions and a clustered column chart, with slide thumbnails on the left and the speaker notes below."></picture></td>
  </tr>
  <tr>
    <td align="center">Sheets</td>
    <td align="center">Docs</td>
    <td align="center">Slides</td>
  </tr>
</table>

Real screenshots of the current build on Linux, in the light or dark theme to match your GitHub setting. The sample files come from [site/tools/make_samples.py](site/tools/make_samples.py).

> **Status: early, all three apps work.** Sheets opens, edits and saves `.xlsx` (formulas via [IronCalc](https://github.com/ironcalc/IronCalc)), imports `.csv`, `.ods`, `.xls`/`.xlt` (with their formatting) and `.xlsb`, and prints to PDF. Docs opens, edits and saves `.docx` and exports PDF. Slides opens, edits, presents and saves `.pptx` and exports PDF. Saving keeps what Kantor doesn't show yet, byte for byte. Not yet checked against a large set of real-world files; see [Known gaps](#known-gaps) and [plan.md](plan.md).

## Install

**Most people:** download Kantor from **<https://kantor.paparanai.id>** and open it.

- **macOS** (Apple Silicon and Intel, macOS 11 or later): open `Kantor.dmg` and drag Kantor to Applications. The app is signed and notarised by Apple.
- **Linux** (x86_64 or aarch64): download the AppImage, make it executable and run it. No install step:

  ```sh
  chmod +x Kantor-x86_64.AppImage
  ./Kantor-x86_64.AppImage budget.xlsx
  ```

  Most file managers can also mark the file as executable (Properties → Permissions) and open it with a double-click. The AppImage needs glibc 2.39 or newer (Ubuntu 24.04, Debian 13, Fedora 40, current Arch or later) and FUSE 3; without FUSE, run it with `--appimage-extract-and-run`. It doesn't add a launcher to your app menu; the install script below does.

The same files are on every [GitHub Release](https://github.com/dzulfikar08/kantor/releases/latest): `Kantor.dmg`, `Kantor-x86_64.AppImage`, `Kantor-aarch64.AppImage`, plus versioned copies and `SHA256SUMS`. Changes per release: [CHANGELOG.md](CHANGELOG.md).

**From a terminal** (Linux or macOS, no compiling, no sudo):

```sh
curl -fsSL https://raw.githubusercontent.com/dzulfikar08/kantor/main/scripts/install.sh | sh
kantor budget.xlsx
```

This installs the rolling **nightly** build of `main`. Pin a tagged release with `curl -fsSL …/install.sh | KANTOR_VERSION=v0.2.0 sh` (macOS builds start with v0.2.0). On Linux it installs `~/.local/bin/kantor`, a **Kantor** launcher (the suite's Home screen) and a **Kantor Sheets** launcher. On macOS it installs `~/Applications/Kantor.app` and links `~/.local/bin/kantor` to it (the nightly is Apple Silicon only and not notarised; tagged releases are universal and notarised). If `~/.local/bin` is not on your `PATH`, the script prints the line to add for your shell. On macOS, the `Ctrl` shortcuts below are `⌘`.

**Uninstall:** delete the AppImage, or drag Kantor from Applications to the Bin. To remove what the install script installed:

```sh
curl -fsSL https://raw.githubusercontent.com/dzulfikar08/kantor/main/scripts/install.sh | sh -s -- --uninstall
```

Settings (`~/.config/kantor`) and the saved session (`~/.local/state/kantor`) are kept; delete them by hand if you want them gone.

**Windows 10 and 11** (x64 or Arm), from the first release with Windows downloads on. Until
then, try the nightly builds `Kantor-Setup-nightly-x64.exe` / `Kantor-nightly-x64.zip` from the
[nightly release](https://github.com/dzulfikar08/kantor/releases/tag/nightly).

- **Installer:** run `Kantor-Setup-x64.exe` (or `Kantor-Setup-arm64.exe`). It installs for you
  alone in `%LOCALAPPDATA%\Programs\Kantor`, with no administrator prompt, and adds a Start menu
  entry, an uninstall entry (Settings → Apps) and **Open with → Kantor** for `.xlsx`, `.xlsm`,
  `.xls`, `.xlt`, `.ods`, `.csv`, `.docx` and `.pptx`. Your default apps stay as they are. An optional
  task adds Kantor to your `PATH`.
- **Portable:** unzip `Kantor-x64.zip` anywhere and run `kantor.exe`.
- The builds aren't code-signed yet, so SmartScreen says *Windows protected your PC* the first
  time: choose **More info → Run anyway** (see [docs/windows-signing.md](docs/windows-signing.md)).
- In a terminal, `kantor` runs `kantor.com`, the console front that comes with both. `kantor.exe`
  is a windowed program and prints nothing to a console. Silent install for IT:
  `Kantor-Setup-x64.exe /VERYSILENT /SUPPRESSMSGBOXES`.
- Settings are in `%APPDATA%\Kantor\settings.toml`, the session and update state in
  `%LOCALAPPDATA%\Kantor`. Uninstalling keeps both.

Arch Linux: a PKGBUILD for a future `kantor-bin` AUR package lives in [packaging/aur/](packaging/aur/); it is not published yet.

## What works

`kantor` opens the Home screen (Docs · Sheets · Slides); `kantor file.xlsx` (or `.docx`, `.pptx`) opens the file, and `kantor --sheets`, `--docs` or `--slides` starts a new document. Documents open in tabs that you can split side by side; Find & Replace (`Ctrl F` / `Ctrl H`) and undo work the same in all three apps.

### Sheets

- **Formatted workbooks look as they do in Excel:** cell fills (theme and indexed colours), text colour, font size and family, bold/italic/underline, horizontal and vertical alignment, wrapped text, borders (thin to thick, double, dotted, dashed), merged cells and gridlines off per sheet. The toolbar and palette set fill and text colour, font size, borders, wrap, merge & center and alignment, each as one undo step.
- **Number formats as Excel shows them** (`crates/kantor_numfmt`): thousands separators, decimals, percentages, currency, dates and times, fractions, elapsed time (`[h]:mm:ss`), conditions, colours (`[Red]`), text sections and accounting fill. Locale tags work: `[$Rp-421]#,##0` shows `Rp1.500.000` with Indonesian separators, and `[$-421]d mmmm yyyy` shows `17 Agustus 2025`.
- **Conditional formatting as Excel shows it** (display; rules are edited in Excel for now): highlight rules (greater than, between, equal to, text that contains / begins / ends with, dates in a period, duplicates, blanks, errors), formula rules, top/bottom N and %, above/below average, 2- and 3-colour scales, data bars (with a negative axis) and icon sets (arrows, traffic lights, flags, symbols, stars, quarters, ratings), with priority and *Stop If True*. Rules follow edits and recalculation, are worked out only for the cells on screen, and are saved unchanged.
- **Formulas** through IronCalc. While you type one, clicking or arrowing to cells inserts their references, which are coloured in the editor and outlined in the same colour on the grid; `F4` cycles `$` anchors, and typing a function name lists matching functions.

  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/sheets-formula-dark.webp">
    <img src="docs/images/sheets-formula-light.webp" width="720" alt="A SUMIF formula being typed in cell F16 of the sales workbook. Its references H5:H10, F5:F10 and F11 are coloured in the cell and in the formula bar, and the same ranges are outlined in those colours on the grid.">
  </picture>

- **Grid work:** select rows, columns or everything, `Ctrl`+arrows to the edge of the data, fill handle and `Ctrl D` / `Ctrl R` (series continue), autofit columns, insert/delete rows and columns, freeze panes, copy and paste ranges as tab-separated text. The status bar shows Sum, Average and Count of the selection.
- **Big workbooks:** files open in the background with a Cancel button and show the values Excel saved instead of recalculating on open. A 180,000-row, 32 MB `.xlsx` uses about 323 MB with the app open (377 MB after a full recalculation), and closing it gives the memory back. A real workbook with 767,000 formulas recalculates in about 3 seconds (patched IronCalc with cached lookups). Workbooks with more than 50,000 formulas switch to **manual calculation**: `F9` recalculates everything (`Shift F9` the sheet), and the palette switches between automatic and manual.
- **Tables, sort and filter:** Excel tables in their table style, structured references (`Sales[@Units]`), sort by one or more columns and AutoFilter by values or conditions, saved as Excel saves them.
- **Charts and pictures** on a sheet are drawn over the grid and follow their cells' values live; click to select, drag to move or resize, `Delete` removes (undoable, saved).
- **Chart panel:** change a chart's type, title, legend, axis titles, series colours and data range, for charts made in Kantor and charts that came with the file (changed in place: the rest of the chart part is saved as it was).
- **Export as PDF… and Print…** (toolbar printer menu and the palette; `kantor convert book.xlsx book.pdf` without a window) print the way Excel does: each sheet's paper (A4, Letter, Legal, F4/Folio and the other Excel sizes), orientation, scale or *fit to N pages wide/tall*, margins, centring, print area, print titles repeated on every page, manual page breaks, page order, gridlines and row/column headings, headers and footers (`&P`, `&N`, `&D`, `&T`, `&F`, `&A`, fonts and colours). Cells keep their number formats, fonts, fills, all 13 border styles, alignment (indent, rotation, shrink to fit, centre across selection, wrap), merged cells, text running into empty neighbours, `####` for numbers too wide, conditional formatting (data bars and icons too) and table styles; hidden rows, columns and sheets are left out. Fonts are embedded and the text can be searched and copied. *Print…* opens the PDF in your PDF viewer to print. The 90,000-order sample (6,183 pages) exports in about 10–17 s after opening, under 220 MB.
- **Saving keeps what Kantor doesn't edit:** chart sheets, charts, pictures, sparklines, data validation and comments are carried over, and their references follow inserted or deleted rows and columns.
- Imports `.csv`/`.tsv`, `.ods`, `.xls` and `.xlsb` (saving those goes to a new `.xlsx`). Excel 97-2003 workbooks and templates (`.xls`, `.xlt`) open as they look in Excel: fonts, fills, borders, number formats, alignment, merged cells, column widths, row heights, frozen panes, hidden rows, columns and sheets, tab colours, hyperlinks, names and page setup.

### Docs

`.docx` editing: paragraph and character styles, headings, fonts, colour, alignment, spacing and indents, bulleted and numbered lists (9 levels, with Word's Enter/Tab/Backspace behaviour), tables with merged cells and borders, inline pictures, page breaks, hyperlinks, tab stops (left, centre, right and decimal, and the default stops), page setup (A4, Letter, Legal, A5, F4; margins in cm) with a ruler, and a word count. **Headers and footers** show on every page as Word lays them out (letterhead logos included, different first page, different odd and even pages, a tall header pushes the text down) and are edited from the toolbar's header button, *Ctrl K → Header and Footer…* or a double-click in the top or bottom margin; **page numbers** (`{PAGE}` and `{PAGES}`, or *Add Page Numbers* for "Page 1 of 9" in the footer) show the right number on each page and in the PDF. **Sections** (*Insert Section Break*) each have their own paper size, orientation and margins: Page setup changes the section holding the caret. **Review:** comments show as cards beside the pages (the commented text highlighted); add one with `Ctrl Alt M` or the toolbar's Review menu, edit or delete it on its card. Tracked changes from Word show in the author's colour (insertions underlined, deletions struck through, on screen and in the PDF); accept or reject the change at the caret, or all of them, from the Review menu or the palette. Both are saved back as they were (untouched comments byte for byte). Turn on **Track Changes** (`Ctrl Shift E`, or the Review menu) and your typing and deletions are recorded under your name: deleted text stays, struck through, until the change is accepted, and the setting is saved in the file as Word's. **Footnotes and endnotes** show as Word lays them out (numbers in the text, footnotes at the bottom of their page, endnotes after the text, numbering restarting per section or page as the file says); *Insert Footnote* (`Ctrl Alt F`) and *Insert Endnote* (`Ctrl Alt D`) add one, and a double-click on a note edits it. Table rows taller than what's left of a page break across pages at a line (unless the row is set not to), and header rows repeat at the top of each page. *Export as PDF…* and *Print…* (`Ctrl P`, the toolbar's printer menu) write a PDF with the same layout as the screen (fonts embedded, text selectable); Print opens it in your PDF viewer. Floating images, shapes, fields and other parts Kantor can't show yet are kept as placeholders and saved unchanged. **Spell check** works offline in English (en_US) and Indonesian (id_ID): misspelled words get a red wavy underline, and right-clicking one offers up to five suggestions, *Ignore All* and *Add to Dictionary* (the personal dictionary is `dictionary.txt` in Kantor's config folder). Text marked with a language in the file uses that language; other text uses `[spelling] language` in settings (default: Indonesian on Indonesian systems, else English). Switch it off with *Check Spelling as You Type* in the Review menu or the palette. Slides checks the text box being edited the same way.

### Slides

`.pptx` editing: text boxes and placeholders (with the master's bullets, numbering and spacing), shapes and pictures you can move, resize, rotate and flip, fill and outline, a thumbnail strip to reorder slides, speaker notes, copy and paste of shapes, and a slide show with notes (`F5`). Slides are drawn as in PowerPoint: all 187 preset shapes (arrows, chevrons, callouts, flowchart symbols…) and freeform shapes, gradient and picture fills, transparency, dashed lines and arrowheads, groups, tables with PowerPoint's table styles, SmartArt, and charts (column, bar, line, area, pie, doughnut, scatter and combinations) from the values saved in the file. *Export as PDF…* writes one page per slide, hidden slides left out. *Print…* (`Ctrl P`, the toolbar's printer menu) prints slides, notes pages or handouts (1, 2, 3, 4, 6 or 9 slides per page) through your PDF viewer. **Tables and chart data can be edited:** double-click a table or chart (or the toolbar's edit button) for a panel with its cells: type into any cell, add or remove rows and columns next to the cell you're on, fill a cell with a theme colour; for charts, edit category labels, series names and values and add or remove series and categories (an embedded table covering the chart's cells grows or shrinks with them). Charts are redrawn at once, and the chart's embedded workbook gets the same values (so PowerPoint's *Edit Data* agrees). Everything else in the table, the chart part and the workbook is saved byte for byte. SmartArt is shown but can't be edited yet.

## Keyboard

On macOS, use `⌘` where this table says `Ctrl` (`Ctrl Tab` stays).

| Action | Keys |
|---|---|
| Move | arrows, `Tab`/`Shift+Tab`, `Home`/`End`, `PgUp`/`PgDn` |
| Edit a cell | just type (replaces), or `Enter`/`F2`/double-click (keeps content) |
| Commit / cancel | `Enter` (moves down) / `Esc`; clicking another cell also commits |
| Clear a cell | `Delete` |
| Anchor a reference / recalculate | `F4` while editing a formula / `F9` |
| Undo / redo | `Ctrl Z` / `Ctrl Shift Z` |
| Find / replace | `Ctrl F` / `Ctrl H` |
| Open / Save / Save as | `Ctrl O` / `Ctrl S` / `Ctrl Shift S` |
| Print | `Ctrl P` (a PDF opens in your PDF viewer) |
| Command palette | `Ctrl K` (or `Ctrl Shift P`) |
| New spreadsheet / document / presentation | `Ctrl N` / `Ctrl Shift N` / `Ctrl Alt N` |
| Slide show | `F5` (`Shift F5` from the current slide) |
| Close pane / close tab | `Ctrl W` / `Ctrl Shift W` |
| Split right / down | `Ctrl \` / `Ctrl Alt \` (drag the divider to resize) |
| Sidebar / inspector | `Ctrl Shift E` / `Ctrl Shift I` (drag the edge to resize) |
| Theme / UI font | `Ctrl K` → *Choose Theme…* / *Choose UI Font…* |
| Next / previous tab | `Ctrl Tab` / `Ctrl Shift Tab` |
| Quit | `Ctrl Q` |

Edits happen in the cell itself (synced live with the formula bar). With an input method such as fcitx5, press `Enter` first, then compose.

By default the UI follows your Omarchy theme (live-reloading on `omarchy theme set`); *Choose Theme…* offers your Omarchy themes plus 900+ Ghostty themes. The UI font is SF Pro when installed (never bundled), else Inter / Adwaita Sans. Open tabs, splits and panel sizes are restored on the next launch. Settings: `~/.config/kantor/settings.toml`; session: `~/.local/state/kantor/workspace.json`.

## Command line

```sh
kantor convert report.docx report.pdf          # also .ods/.xls → .xlsx, a sheet → .csv
kantor convert deck.pptx deck.pdf              # one page per slide, hidden slides left out
kantor convert --handouts 6 deck.pptx h.pdf    # 1, 2, 3, 4, 6 or 9 slides per A4 page; --notes
kantor convert book.xlsx book.pdf              # every visible sheet, as Excel prints them
kantor convert --sheet Sales --fit-width --landscape --paper f4 book.xlsx sales.pdf
                                               # also --fit-page, --scale N, --portrait,
                                               # --no-print-area (the sheet's page setup otherwise)
kantor convert -o out/ --to pdf a.docx b.docx
kantor pdf merge a.pdf b.pdf -o both.pdf       # also split, extract, delete, reorder, rotate,
                                               # info, images, watermark, page-numbers, compress
```

Neither opens a window. On Windows these run through `kantor.com` (what a bare `kantor` finds
in `cmd` and PowerShell); `kantor.exe` itself is a windowed program with no console.

## AI agents (MCP)

`kantor mcp` is an [MCP](https://modelcontextprotocol.io) server: Claude Code, Claude Desktop,
Cursor and other agents can read and edit the documents open in the **running** Kantor while you
watch. Every change is one undo step (`Ctrl Z` takes it back), the changed cells, paragraphs or
slide are selected, and the status bar says what the agent did. Nothing is saved until the agent
(or you) saves.

```sh
claude mcp add kantor -- kantor mcp      # Claude Code (palette: "Copy MCP Setup for Claude Code")
kantor mcp --print-config                # JSON for Claude Desktop, Cursor and others
```

```json
{ "mcpServers": { "kantor": { "command": "/home/you/.local/bin/kantor", "args": ["mcp"] } } }
```

If Kantor isn't running, the first tool call starts it (`kantor mcp --no-launch` returns an error
instead). Tools: `list_documents`, `open_document`, `new_document`, `save_document`,
`activate_document`, `get_selection`; Sheets `sheets_read_range`, `sheets_write_range`,
`sheets_select`, `sheets_format`, insert/delete rows and columns; Docs `docs_get_paragraphs`,
`docs_insert_paragraphs`, `docs_replace_paragraph`, `docs_delete_paragraphs`, `docs_find_replace`;
Slides `slides_list`, `slides_add_slide`, `slides_set_shape_text`, `slides_delete_slide`,
`slides_set_notes`, `slides_go_to`. The app listens only on a private Unix socket
(`$XDG_RUNTIME_DIR/kantor/`, on macOS `~/Library/Application Support/Kantor/run/`), readable by
you alone. On Windows it listens on `127.0.0.1` with a random port, and `kantor mcp` must first
send a random token that only your user can read (`%LOCALAPPDATA%\Kantor\run\kantor-<pid>.port`).
Use the full path in MCP configs, e.g. `"command": "C:\\Users\\you\\AppData\\Local\\Programs\\Kantor\\kantor.exe"`
(`kantor mcp --print-config` prints it).

## AI features (optional) and privacy

The short version for the whole app is in [PRIVACY.md](PRIVACY.md).

AI is **off** until you turn it on in `Ctrl K` → *AI Settings* and choose a provider: Anthropic, OpenAI, OpenRouter, Groq, DeepSeek, Google Gemini, any OpenAI-compatible server, or **Ollama** running on your own computer (nothing leaves it). Kantor has no AI service of its own: your data goes **only to the provider you chose**, and only when you run an AI command. Each command sends only what it needs:

- *Ask AI for a Formula…* (Sheets): the column headers, up to 20 sample rows near the selection and the active cell's address — never the whole sheet;
- *Explain This Formula* (Sheets): the formula and the column headers;
- *Rewrite / Translate Selection…* (Docs): the selected paragraphs' text.

API keys are stored in the system keychain (Secret Service on Linux, Keychain on macOS, Credential Manager on Windows); if none is available Kantor says so and keeps the key in `~/.config/kantor/ai-keys.toml`, readable only by you. Keys are never written to `settings.toml` or logs. What the provider does with your requests is governed by its own terms.

## Updates

Kantor downloaded from the site, the GitHub releases or the install script keeps itself up to date:

- **When it checks:** at most once a day, at launch, and when you choose `Ctrl K` → *Check for Updates…*. A check fetches a small manifest from `https://kantor.paparanai.id/update/stable.json` (redirected to the GitHub release) and sends **only Kantor's version and your OS and CPU type**, in the User-Agent (`Kantor/0.2.1 (linux; x86_64)`). No identifier, no usage data, nothing about your files. `kantor mcp`, `kantor convert` and `kantor pdf` never check.
- **How it's protected:** the manifest is signed (Ed25519) and Kantor checks the signature with a key built into the app before trusting anything in it; every download must then match the manifest's size and SHA-256, and the new build must start and report the promised version before it is used. On macOS the new `Kantor.app` must also pass `codesign` (Developer ID team `F6Q6UFUK69`) and Gatekeeper (notarised) checks.
- **What you see:** nothing while it downloads in the background. When the new version is ready, the status bar says *Kantor X.Y.Z is ready — Restart to update*. Click it (or `Ctrl K` → *Restart to Update*): Kantor asks about unsaved documents as when quitting, installs the update and reopens. If you just quit, the update is installed then and you get it next time.
- **How each install updates:**
  - **macOS app** (`Kantor.dmg` or the install script): the new notarised `Kantor.app` replaces the old one where it is. If Kantor can't write there (an Applications folder you don't own), it offers the download page instead.
  - **AppImage:** the new AppImage replaces the file you run, under the same name.
  - **Install script on Linux** (`~/.local/bin/kantor`): the binary, launchers and icons are replaced.
  - **Windows installer** (`Kantor-Setup-*.exe`): the new installer is downloaded and checked
    against the signed manifest's SHA-256. When Kantor quits, it runs silently: it waits for
    Kantor to exit, updates the files and, after *Restart to update*, starts Kantor again.
  - **Windows portable zip:** the files in the folder are replaced (the running `kantor.exe` is
    renamed aside and removed on the next start).
  - **Packages and source builds** (AUR, `cargo build`, …) are never changed: Kantor only tells you a new version exists. Update those the usual way.

  Each replacement is all-or-nothing: if any step fails, the old version stays as it was.
- **Channels:** releases follow `stable`; nightly builds (the install script's default) follow `nightly`. The nightly macOS build is not notarised, so on macOS nightly Kantor only tells you about new nightlies.
- **Turn it off** in `~/.config/kantor/settings.toml`; Kantor then makes no update requests at all, not even from the palette:

  ```toml
  [update]
  enabled = false
  # channel = "nightly"   # or "stable"; unset follows the build
  ```

For testing, `KANTOR_UPDATE_URL` replaces the manifest URL and `KANTOR_UPDATE_CURRENT=0.1.0` makes Kantor compare as if it were that version; signatures, checksums and code-signing checks still apply.

## Your work is safe

- **Recovery copies, always on.** While a document has unsaved changes, Kantor keeps a copy of it
  in your data folder: `~/.local/share/kantor/recovery` (Linux), `~/Library/Application
  Support/Kantor/recovery` (macOS), `%LOCALAPPDATA%\Kantor\recovery` (Windows). A copy is made two
  minutes after the first unsaved change, at a pause in typing, and whenever you switch to another
  window. Copies are written in the background in the document's own format (`.xlsx`, `.docx`,
  `.pptx`), never over your file, and are deleted when you save, choose *Don't Save*, or quit.
- **After a crash or a power cut** Kantor shows *Recover unsaved work* when it starts: each
  document with the time of its copy. *Restore* opens it with your changes, still unsaved and still
  named after the original file; *Save* writes the original, and asks first if that file was
  changed by someone else in the meantime. *Discard* deletes the copy. *Not Now* keeps the copies
  for later (`Ctrl K` → *Recover Unsaved Work…*).
- **Autosave (off by default).** Turn it on with the *Autosave* switch in the status bar, `Ctrl K` →
  *Turn Autosave On or Off*, or in `~/.config/kantor/settings.toml`:

  ```toml
  [autosave]
  enabled = true
  idle_seconds = 30   # save this long after your last change (at least 5)
  ```

  It is off by default, as in Office and LibreOffice for files on your computer: saving over your
  file without asking would also keep changes you meant to throw away with *Don't Save*, and the
  recovery copies already cover crashes. Autosave only saves `.xlsx`, `.docx` and `.pptx` files
  that already have a name. Files opened from `.csv`, `.ods`, `.xls` or `.xlsb`, files with content
  Kantor can't keep, and files someone else changed since Kantor opened them are never autosaved;
  they get recovery copies only.
- **Several windows:** each Kantor process locks its own copies, so two windows never offer or
  delete each other's work; only copies whose Kantor is no longer running are offered.
- **Damaged copies** (a disk that filled up, a copy cut short) are never deleted: they are moved to
  `recovery/damaged/` and Kantor tells you how many it skipped.
- **Known gaps:** up to two minutes of typing can be lost if the power goes before the next copy
  (a copy is also made whenever the window loses focus). A big workbook's copy takes as long to
  write as a Save (≈13 s for 90,000 rows with 810,000 formulas, in the background), so a crash in
  that time leaves the previous copy. Undo history, the selection and scroll position aren't kept
  in a copy, and a restored workbook comes back with Excel's last calculated values for formulas
  in manual calculation. With autosave on, Docs and Slides save on the window's thread (quick for
  normal files).

## Known gaps

- Not yet checked against a large set of real-world files; expect layout differences from Excel, Word and PowerPoint.
- Sheets: chart sheets (a whole sheet that is one chart) aren't shown, shapes and text boxes show as a box, charts are inserted with Insert Chart and changed in the Chart panel, which also restyles charts from a file in place (type, title, legend, axis titles, series colours, data range; combo, 3-D and other types Kantor can't make keep their type and data range; no data label or axis scale options yet); frozen rows taller than the window leave no room to scroll. Macros are not carried over on save (Kantor says so). Pivot tables are kept, drawn with their style, refreshed, made (Insert PivotTable…), rearranged in the PivotTable Fields panel and deleted; there is no grouping of dates or numbers, calculated fields, value filters, "show values as", per-field subtotal functions or PivotTable style picker in Kantor yet, a new pivot always gets its own cache, and Kantor-made pivots haven't been opened in Excel itself yet (the file is written to Excel's schema and asks Excel to refresh on load). Refresh and the Fields panel say so and change nothing for pivots with calculated fields or items, grouped dates or numbers, value filters, "show values as" or an external or OLAP source, and inner items of a compact pivot aren't indented. Slicers and timelines are kept on save but not shown or usable in Kantor. IronCalc can't evaluate 3-D references (`SUM(Jan:Mar!B2)`). Tables are made with Format as Table and changed in Table Design; renaming a table that came from a file drops its column formats and calculated column formulas from the saved part (Excel then stops filling new rows with the formula), custom table styles can't be made, and a table's header cell formatting isn't applied from the style's dxfs on cells with their own style. Filters by colour, icon, top 10 and dynamic dates are kept but can't be edited; sorting doesn't move cell comments or remember the sort (`sortState`). `.ods`, `.xls` and `.xlsb` are import-only. `.xls` imports leave out comments, data validation, conditional formatting, charts and pictures, show fill patterns as one mixed colour, and Excel 5/95 files bring values only; `.ods` and `.xlsb` imports bring values and formulas without formatting, merged cells or column widths. Printing: shapes and text boxes are left out of the PDF (charts and pictures print), comments aren't printed, fill patterns print solid, and `&G` header pictures and theme colours in headers are skipped; Page Setup edits the odd-page header and footer only (different first/even pages from a file are kept), and manual page breaks can't be added in Kantor. Conditional formatting rules are made and managed in Kantor (Kantor can't edit icon set thresholds, data bar lengths or axis, or rules with borders, number formats or "text equal to"; those can be deleted and reordered), and a sheet whose rules were changed is saved from IronCalc's model, which drops Excel 2010 extras such as custom icon sets and data bar borders on that sheet; its borders aren't drawn, data bars ignore minimum/maximum lengths, and "between" date periods don't highlight. Data validation works from the file and is made with Data Validation… (no "apply these changes to all other cells with the same settings" and no IME mode); a typed time such as `9:30` is stored as text (IronCalc), so time rules see text unless the value is a formula like `=TIME(9,30,0)`.
- Sheets functions not available (`#NAME?`): the `CUBE*` family, `WEBSERVICE`, `FILTERXML`, `RTD`, `STOCKHISTORY`, `IMAGE`, `EUROCONVERT` and Excel's online functions (`TRANSLATE`, `DETECTLANGUAGE`, `COPILOT`, `PY`). `FORECAST.ETS` and its variants give `#N/IMPL`. `GROUPBY`/`PIVOTBY` don't take a vector of functions (`HSTACK(SUM,AVERAGE)`).
- Docs: tab leaders (dot leaders) aren't drawn and tab stops can't be set in Kantor yet; fields other than page numbers and floating objects are kept in the file but shown only as placeholders and left out of the PDF; footnotes and endnotes are edited as plain text in a popover (ones with pictures or tables are shown but can't be edited), a note too long for its page doesn't continue onto the next one, and endnotes always go at the end of the document; headers and footers are edited as plain text in a panel (not on the page itself), and ones with pictures, tables or other objects are shown but can't be edited yet; page number formats (i, ii, a, b) and restarts from the file are honoured but can't be set in Kantor. Files with a table of contents or cross-references are saved under a new name. With Track Changes on, formatting changes, Replace All and table and picture edits aren't tracked; formatting changes from Word are resolved only by Accept All / Reject All; comments can't be replied to or resolved.
- Slides: groups and SmartArt can't be edited; table cells are edited in a panel (not on the slide), rows and columns can't be added or removed in tables with merged cells, chart categories are only added or removed at the end and a new series goes after the last one; chart types other than those above show a placeholder; on screen, rotated text stays upright (the PDF turns it).
- The Linux builds need glibc 2.39 or newer.

## Building

All builds run on **GitHub Actions** (`ubuntu-24.04-arm`, `ubuntu-24.04` and `macos-15`); contributors don't need a powerful machine. If you do want to build locally:

```sh
./scripts/ci-deps.sh            # Ubuntu packages; on Arch: wayland libxkbcommon vulkan-icd-loader fontconfig; macOS needs only Xcode's tools
cargo run -p kantor -- file.xlsx
cargo test --workspace
```

Expect `target/` to reach 10–20 GB and the first build to need plenty of RAM.

## Layout

```
crates/
  kantor         binary: CLI (`convert`, `pdf`, `mcp`), suite shell (tabs, Home, palette, status bar)
  kantor_ui      design system: metrics, colour tokens from Omarchy, cached views
  kantor_core    shared non-GPUI pieces (fuzzy search, find, settings, session)
  kantor_ai      AI providers, streaming HTTP clients, prompts, keychain (no GPUI)
  kantor_theme   Omarchy palette parser (no GPUI)
  kantor_update  in-app updates: signed manifests, download, verify, swap the install (no GPUI)
  kantor_mem     gives freed heap memory back to the system after big documents close (no GPUI)
  kantor_pdf     PDF page tools behind `kantor pdf` (no GPUI)
  sheets_engine  IronCalc wrapper: open/save xlsx, imports, part preservation (no GPUI)
  kantor_numfmt  Excel number formats, adapted from GridCraft (MIT OR Apache-2.0; no GPUI)
  sheets_pdf     workbook → PDF as Excel prints it: page setup, pagination, cells (krilla)
  sheets_ui      grid, formula bar, sheet tabs
  docs_engine    document model, styles, undo, .docx read/write and preservation (no GPUI)
  docs_layout    parley text layout into pages (no GPUI)
  docs_pdf       .docx → PDF with the same layout as the screen
  docs_ui        page view, caret, selection, IME, toolbar, ruler
  slides_engine  slide/shape/text model, undo, .pptx read/write and preservation (no GPUI)
  slides_scene   slide → display list (paths, gradients, pictures, glyph runs), shared by screen and PDF
  slides_pdf     slide scenes → PDF (krilla)
  slides_ui      slide canvas, thumbnails, notes, slide show
vendor/
  gpui-pre-wgpu  GPUI's wgpu renderer (Linux) with memory patches for low-memory GPUs
  ironcalc_base  IronCalc's engine with cached lookups for large workbooks (KANTOR_PATCHES.md)
  ironcalc       IronCalc's xlsx reader and writer, patched to match (KANTOR_PATCHES.md)
site/            the download site (kantor.paparanai.id); site/tools/ takes the screenshots
```

## Credits

- [IronCalc](https://github.com/ironcalc/IronCalc) (MIT OR Apache-2.0) does the formulas and xlsx import/export; Kantor carries patched copies in `vendor/`.
- [GPUI](https://github.com/zed-industries/zed) (Apache-2.0) draws the UI, through [GPUI Kit](https://github.com/longbridge/gpui-kit) (Apache-2.0).
- Cell values are formatted by `crates/kantor_numfmt`, adapted from [GridCraft](https://github.com/storytold/gridcraft)'s `gridcraft-numfmt` (MIT OR Apache-2.0, by the ArtCraft Team and the GridCraft contributors); see its [PROVENANCE.md](crates/kantor_numfmt/PROVENANCE.md).
- [parley](https://github.com/linebender/parley) lays out text and [krilla](https://github.com/LaurenzV/krilla) writes the PDFs.

Third-party notices are in [NOTICE](NOTICE). Every download also ships `THIRD_PARTY_LICENSES.txt` with the licence texts of all linked crates (generated by `scripts/third-party-licenses.py`).

The shell's structure and styling follow [Voltrus Tools](https://github.com/VoltrusID/voltrus-explorers) (see `docs/reference-voltrus-tools.md`).

## License

[GNU Affero General Public License v3.0 or later](LICENSE) (AGPL-3.0-or-later). Modified versions, including ones offered over a network, must publish their source under the same licence.
