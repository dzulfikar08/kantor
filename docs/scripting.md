# Scripting: macros on one object layer

Kantor Sheets runs the VBA macros of `.xlsm` files. `crates/kantor_script` is the language
core: a host-neutral object layer and a VBA interpreter. `crates/sheets_script` is the Sheets
object model on top of it (`Application`, `Workbook`, `Worksheet`, `Range`…) and the host that
runs a workbook's macros; `sheets_ui` runs them after Enable Content, and `kantor sheets macros
run` runs them headless. JavaScript and Python are meant to bind to the same objects later.

## Layers

```
 VBA source ─► kantor_script::vba ─┐
 (later) JS ─► rquickjs binding  ──┼─► kantor_script::object ◄── sheets_script (Sheets object model)
 (later) Py ─► Python binding    ──┘     Value · Dispatch · Host
```

### The object layer (`kantor_script::object`)

It follows OLE Automation, which VBA, Office and Windows Script Host all use, so VBA semantics
map onto it one to one. Nothing in it is VBA-specific.

- **`Value`** is a VARIANT: `Empty`, `Null`, `Missing` (an omitted optional argument),
  `Boolean`, `Byte`, `Integer` (i16), `Long` (i32), `LongLong`, `Single`, `Double`,
  `Currency` (i64 × 10⁴), `Date` (OLE date: days since 1899-12-30), `String`, `Error` (an
  Excel `CVErr` code), `Array` (multi-dimensional, per-dimension lower bounds, column-major
  like a SAFEARRAY), `Record` (a user-defined type) and `Object(Option<ObjectRef>)`.
  Arrays and records have value semantics (`Rc` + copy on write); objects are references.
- **`Dispatch`** is IDispatch: `get`, `put` (Let), `put_ref` (Set), `call` (a method used as a
  statement), `enumerate` (`For Each`) and `type_name`. Member names are case-insensitive, and
  `""` is the default member (DISPID_VALUE), so `Range("A1") = 5` is `put("", [], 5)` on the
  `Range`. Arguments are `Arg { name: Option<String>, value }`, which carries named arguments
  (`Copy Destination:=x`); `find_arg(args, position, name)` resolves either form.
- **`ScriptError`** has an Automation error number and description (`Err.Number`,
  `Err.Description`), a source, a location (module, procedure, line) and a kind:
  `Runtime` and `Unsupported` are trappable by `On Error`; `Syntax` and `Interrupted` aren't.
  An unknown member should fail with `ScriptError::no_member` (438): interpreters rely on that
  number to fall back to other meanings of a name.
- **`Host`** is everything outside the script: `globals()` returns the object whose members
  are visible without qualification (Excel's `Application` globals: `Range`, `Cells`,
  `ActiveSheet`), `constant()` supplies the object model's constants (`xlUp`), and there are
  `create_object`, `msg_box`, `input_box`, `debug_print`, `now` (local time) and `cancelled`
  (polled every 1024 statements; `CancelToken` is a ready-made flag for the UI thread).
  `Project::set_step_limit` adds a hard step budget.

### The VBA interpreter (`kantor_script::vba`)

`Project::new()`, `add_module(name, kind, source)`, `procedures()`, `run(name, args, host)`,
`call_function(name, args, host)` (for worksheet UDFs), `register_constant`, `reset`.

- **Front end.** A preprocessor (line endings, the `VERSION … BEGIN … END` header of exported
  `.cls`/`.frm` files, `#If`/`#Const`), a lexer (continuations, `:` separators, `'`/`Rem`,
  date literals, `&H`/`&O`, type suffixes, `[A1]`), and a recursive-descent parser to an AST.
  A simple statement that doesn't parse becomes an "invalid" statement and an unsupported one
  (file I/O, `Load`, `RaiseEvent`…) an "unsupported" statement: both parse, and fail only when
  they run, so one bad line doesn't block a module. Broken block structure fails the module
  with its line.
- **Compiler.** Each procedure becomes a flat list of operations with jumps; expressions stay
  trees. That makes `GoTo`, `GoSub`, `On Error GoTo`, `Resume`, `Resume Next` and `Exit`
  plain jumps. Local `Dim`/`Static`/`Const` are hoisted (VBA allocates locals on entry).
- **Runtime.** Variables are shared cells, so `ByRef` passes the caller's cell; array elements
  and user-defined-type fields are passed by copy-in/copy-out to the exact element or field
  (MS-VBAL §5.3.1.11). Objects are replaced by their default member for operators and `Let`
  (§5.6.2.3). Operators, coercions, banker's rounding, Null propagation and `Option Compare`
  follow MS-VBAL §5.5–5.6; comments in `vba/convert.rs` cite the sections.
- **Library.** The VBA functions macros use most (strings, conversion, `Format`, dates, math,
  `Array`/`IIf`/`Choose`/`Switch`, `MsgBox`/`InputBox` via the host, `CallByName`,
  `CreateObject("Scripting.Dictionary")`), the `vb*` constants, `Collection` and
  `Scripting.Dictionary` as built-in objects, and class modules with `Class_Initialize`,
  `Class_Terminate`, `Property Get/Let/Set`, a default member (`VB_UserMemId = 0`) and an
  enumerator (`VB_UserMemId = -4`).

## The Sheets object model (`crates/sheets_script`)

A crate of its own because it needs both `sheets_engine` and `kantor_script`, and neither
should know the other. `sheets_ui`, the `kantor sheets macros run` command and the tests use it
the same way.

```
 sheets_ui (Run, buttons) ─┐                         ┌─► sheets_engine::Workbook
 kantor sheets macros run ─┼─► sheets_script ─────────┤    (edits, undo, recalculation)
 tests                    ─┘   run_macro · objects   └─► kantor_script::vba::Project
```

- **A run** (`run::run_macro`) takes the workbook, runs one macro and gives the workbook back
  with a `RunOutcome`: the result (`MacroError` with the number, description, module, procedure
  and the line as the Visual Basic Editor shows it, attribute lines left out), where the macro
  left the active sheet and selection, and whether `ThisWorkbook.Save` was called (the caller
  saves). The workbook's own undo history gets one entry for the whole run
  (`Workbook::undo_mark`/`fold_undo`); a multi-step undo recalculates once.
- **Edits** go through the engine's normal edit operations (`set_input`, `set_range_style_value`,
  `sort_range_by`, `set_column_filter`, `insert_rows`, `copy_cells`…), so recalculation, undo
  and save behave as for edits made by hand. While a macro runs, automatic recalculation is
  deferred (`Workbook::script_begin`); a read after writes brings values up to date first
  (`script_sync`), as Excel's automatic calculation does; `script_end` resumes.
- **Objects** hold the run's `Session` (`Rc`): the workbook (lent to the run), the active sheet
  and each sheet's selection, the clipboard of `Copy`/`Cut`, `Find`'s last search, the pending
  `Worksheet.Sort`, and the application settings a macro changes. Sheets are kept by `sheetId`,
  so a `Worksheet` or `Range` object stays right when sheets are added, moved or renamed; a
  deleted sheet's objects fail with error 424. Document modules map to sheets by the file's
  code names (`<sheetPr codeName>`, read by `Workbook::sheet_code_names`), or in order for a
  workbook without them, so `Sheet1.Range(...)` and `Me` in a sheet module work.
- **Errors** are Excel's: 1004 (with Excel's wording where it has one, "Application-defined or
  object-defined error" otherwise) for object-model failures, 9 for an unknown sheet or name,
  13 for type mismatches, 91/424 for missing objects, 438 for unknown members, 445 for what
  Kantor doesn't do (it names the feature), 18 when stopped.
- **Dialogs** go through the `MacroUi` trait, which is *lent the workbook* while a `MsgBox` or
  `InputBox` shows, so a UI can show the cells as the macro left them. `HeadlessUi` answers OK /
  Yes and takes input box answers from a list.

### Coverage

- **Globals / `Application`**: `Range`, `Cells`, `Rows`, `Columns`, `ActiveSheet`, `ActiveCell`,
  `Selection`, `ActiveWorkbook`, `ThisWorkbook`, `Workbooks`, `Worksheets`/`Sheets`, `Names`,
  `ActiveWindow`, `Intersect`, `Union`, `Evaluate` (and `[A1]`), `WorksheetFunction`, `Calculate`,
  `Goto`, `InputBox` (Type 0/1/2/4/8; `False` when cancelled), `UserName`, `Version` (16.0),
  `Name`, `OperatingSystem`, `PathSeparator`; `ScreenUpdating`, `DisplayAlerts`, `EnableEvents`,
  `StatusBar`, `Cursor`, `CutCopyMode` (False clears the clipboard) are kept; `Calculation`
  switches the workbook's calculation mode. `Application.Sum(...)` returns an error value
  instead of raising. `Wait`, `OnTime`, `OnKey`, `Run`, `Quit`, `SendKeys`, file dialogs and
  `Caller` fail with error 445.
- **`Workbook`**: `Name`, `FullName`, `Path`, `CodeName`, `Worksheets`, `Sheets`, `Names`,
  `ActiveSheet`, `Saved`, `Save` (saved when the run ends); `SaveAs`, `SaveCopyAs`, `Close`,
  printing: 445. `RefreshAll` refreshes every pivot table (not web formulas or external
  connections); `PivotCaches`, `Charts` (chart sheets), `IconSets`, `ExportAsFixedFormat`
  (below). `Workbooks(1)`/`Workbooks("Book.xlsm")` is this workbook;
  `Workbooks.Open`/`Add`: 445.
- **`Worksheets`/`Sheets`** (worksheets; chart sheets aren't in the object model): `Count`,
  `Item` by number or name (default member), `Add` with `Before`/`After`/`Count` (the new sheet
  becomes active), `For Each`.
- **`Worksheet`**: `Name` (get/let), `CodeName`, `Index`, `Range`, `Cells`, `Rows`, `Columns`,
  `UsedRange`, `Activate`, `Select`, `Delete`, `Copy`/`Move` with `Before`/`After` (to another
  workbook: 445), `Visible` (`xlSheetVeryHidden` hides), `Next`/`Previous`, `Paste`,
  `AutoFilter`, `AutoFilterMode` (False removes it), `FilterMode`, `ShowAllData`, `Sort`
  (`SortFields.Clear/Add`, `SetRange`, `Header`, `Apply`: what recorded macros write),
  `StandardWidth`/`StandardHeight`, `Evaluate`, `Calculate`, `Shapes`, `Pictures`,
  `ChartObjects`, `PivotTables`, `ListObjects`, `Hyperlinks`, `Comments`, `PageSetup`,
  `HPageBreaks`/`VPageBreaks`, `ResetAllPageBreaks`, `ExportAsFixedFormat` (objects below).
  `Protect`/`Unprotect` are accepted and do nothing: the engine has no sheet protection (it
  only carries a file's `<sheetProtection>` through a save); `CommentsThreaded`, `PrintOut`,
  `PrintPreview`: 445.
- **`Range`**: `Value` (2-D arrays in and out, base 1; dates as `Date` where the cell has a date
  format), `Value2`, `Formula`, `FormulaR1C1` (converted both ways), `FormulaLocal`, `Text`,
  `Address` (`RowAbsolute`, `ColumnAbsolute`, `ReferenceStyle`, `External`, `RelativeTo`), `Row`,
  `Column`, `Rows`, `Columns`, `Count`, `CountLarge`, `Cells`, `Item`, `Offset`, `Resize`,
  `End(xlUp/xlDown/xlToLeft/xlToRight)`, `CurrentRegion`, `EntireRow`, `EntireColumn`,
  `Areas`, `MergeArea`, `Clear`, `ClearContents`, `ClearFormats`, `Delete`/`Insert` (`Shift`; whole
  rows and columns insert and delete them), `Copy [Destination]`, `Cut [Destination]`,
  `PasteSpecial` (`xlPasteAll`, `xlPasteValues`, `xlPasteFormats`, `xlPasteFormulas`, with number
  formats, `xlPasteColumnWidths`), `Select`, `Activate`, `Find`/`FindNext`/`FindPrevious` (`What`,
  `After`, `LookIn`, `LookAt`, `SearchOrder`, `SearchDirection`, `MatchCase`), `Replace`, `Sort`
  (`Key1..3`, `Order1..3`, `Header` including `xlGuess`), `AutoFilter` (`Field`, `Criteria1`,
  `Operator` `xlAnd`/`xlOr`/`xlFilterValues`/top 10, `Criteria2`; no arguments toggles it),
  `RemoveDuplicates` (kept rows move up with their formats, by `copy_cells`), `AutoFit`
  (columns measured with the cells' fonts through `sheets_pdf`'s shaper, headless too, so the
  result can differ from the app's GPUI measurement by a few pixels; rows fitted the same way),
  `NumberFormat`, `HorizontalAlignment`, `VerticalAlignment`, `WrapText`, `IndentLevel`,
  `Orientation`, `MergeCells`/`Merge`/`UnMerge`, `ColumnWidth` (characters), `RowHeight`
  (points), `Width`/`Height`/`Left`/`Top` (points), `Hidden`, `HasFormula`, `Font` (`Bold`,
  `Italic`, `Underline`, `Strikethrough`, `Size`, `Name`, `Color`, `ColorIndex`, `FontStyle`),
  `Interior` (`Color`, `ColorIndex`, `Pattern` `xlSolid`/`xlNone`; other patterns: 445, the
  engine's fill is solid only), `Borders`/`Borders(edge)` (`LineStyle`, `Weight`, `Color`,
  `ColorIndex`; `xlNone` on one edge removes just that edge, also on the facing side of the
  neighbouring cell; `xlDiagonalDown`/`xlDiagonalUp` are set and saved, but the grid and PDF
  don't draw them yet), `BorderAround`, `Locked`/`FormulaHidden` (the cell style's protection,
  saved), `PageBreak`, `ExportAsFixedFormat` (that range), `Comment`, `AddComment`, `NoteText`,
  `ClearComments`, `Hyperlinks`, `ClearHyperlinks`, `Validation`, `FormatConditions`,
  `Characters`, `ListObject`,
  `SpecialCells(xlCellTypeConstants/Formulas/Blanks/LastCell/Visible[, value types])`,
  `FillDown`/`FillRight`, `Name` (defines a name), the default member (`Value`, or `Item` with
  arguments) and `For Each` over cells, rows or columns. Whole rows and columns read, write and
  format their used part.
- **`WorksheetFunction.X(...)`**: any function the engine knows, evaluated on the active sheet
  (`Workbook::evaluate_formula`, KANTOR_PATCHES §37): ranges go in as references, arrays as array
  constants; an Excel error raises 1004 ("Unable to get the X property of the WorksheetFunction
  class"); an unknown name is 438.
- **`Names`**: `Count`, `Item`, `Add Name, RefersTo`, `For Each`; `Name`: `Name`, `RefersTo`,
  `RefersToRange`, `Value`, `Delete`.
- **Shapes** (`shapes.rs`, engine `script_shapes.rs`): `Shapes` `Count`, `Item`/default (index or
  name), `For Each`, `AddShape` (the `msoShape*` presets 1–108), `AddTextbox` (horizontal),
  `AddLine`, `AddPicture` (always embedded; `-1` sizes are the picture's own), `AddChart2`/
  `AddChart`, `Range(...)` (`Delete`, `Select`, one-shape pass-through), `SelectAll`. `Shape`:
  `Name`, `Type`, `AutoShapeType`, `Left`/`Top`/`Width`/`Height` (points), `IncrementLeft/Top`,
  `Visible`, `ID`, `AlternativeText`, `OnAction` (stored as `[0]!Macro`; a click runs it at
  once), `Placement`, `TopLeftCell`/`BottomRightCell`, `ZOrderPosition`, `HasChart`/`Chart`,
  `TextFrame.Characters([Start, Length]).Text`, `TextFrame2.TextRange.Text`, `Fill.ForeColor.RGB`,
  `Fill.Visible`/`Transparency`, `Line.ForeColor.RGB`/`Weight`/`Visible`, `Delete`.
  `Pictures`: `Count`, `Item`, `Insert`, `Delete`, `For Each`. 445: `ZOrder`, `Duplicate`,
  `Copy`, `Flip`, `Group`, scaling, `Characters.Font` in shapes, theme/scheme colours, fill and
  line on pictures and charts.
- **Charts** (`charts.rs`, engine `script_charts.rs`): `ChartObjects` (`Count`, `Item`, `Add`,
  `Delete`, `For Each`), `ChartObject` (`Name` read, position and size, `Placement`,
  `TopLeftCell`/`BottomRightCell`, `Chart`, `Delete`), `Chart`: `ChartType` (column, bar, line,
  pie, area, scatter and their stacked and 3-D forms; 100% stacked, doughnut, radar, bubble,
  stock, surface: 445; `xlLineMarkers` makes a line chart without markers), `SetSourceData`
  (`PlotBy`), `HasTitle`/`ChartTitle.Text`, `HasLegend`/`Legend.Position`, `SeriesCollection`
  (`Count`, `Item`, `NewSeries`, `For Each`; `Series.Name`/`Values`/`XValues`/`Formula`/`Delete`),
  `Axes(xlCategory|xlValue)` (`HasTitle`, `AxisTitle.Text`, `MinimumScale`/`MaximumScale`/
  `MajorUnit` and `…IsAuto`, `ReversePlotOrder`, `ScaleType`, `TickLabels.NumberFormat`),
  `Export` (PNG, the PDF export's chart drawing rasterised at 96 dpi), `Location`,
  `ActiveChart`; `Workbook.Charts` (chart sheets: `Count`, `Item`, `Add`, `Delete`, `For Each`).
  `Series.Values = Range(...)` stores the values, not a link (the interpreter passes a Range's
  default value on `Let`); an address string or `SetSourceData` keeps the link. 445: chart and
  plot area formatting, data labels, points, gridlines, secondary axes, renaming a chart object.
- **PivotTables** (`pivots.rs`, engine `script_pivots.rs`): `PivotCaches.Create(xlDatabase,
  SourceData).CreatePivotTable(TableDestination, TableName)`, `PivotTables.Add`; `PivotTable`:
  `Name`, `RefreshTable`, `PivotCache.Refresh`, `TableRange1`/`TableRange2`, `DataBodyRange`,
  `RowRange`, `ColumnRange`, `PageRange`, `PivotFields`/`RowFields`/`ColumnFields`/
  `PageFields`/`DataFields`, `ClearAllFilters`, `ClearTable`, `AddDataField`, `AddFields`,
  `RowAxisLayout`, `RowGrand`/`ColumnGrand`, `TableStyle2`; `PivotField`: `Orientation`
  (moves the field), `Position`, `CurrentPage`, `PivotItems(...).Visible`, `Function`,
  `NumberFormat` (formats the workbook already has), `Caption` (value fields),
  `ClearAllFilters`. Edits go through the PivotTable Fields list's own layout edit. 445:
  `GetPivotData`, `ShowPages`, `CalculatedFields`, `PivotTableWizard`, captions of row/column
  fields and items.
- **ListObjects** (`tables.rs`, engine `script_tables.rs`): `ListObjects.Add` (range source),
  `Name`, `Range`, `DataBodyRange`, `HeaderRowRange`, `TotalsRowRange`, `ShowTotals`,
  `ShowHeaders`, `ShowAutoFilter`, `TableStyle`, stripes and first/last column, `Resize`,
  `Unlist`, `Delete`, `ListRows` (`Add`, `Item`, `Delete`), `ListColumns` (`Add`, `Item` by name,
  `Name`, `DataBodyRange`, `TotalsCalculation`, `Delete`), `AutoFilter`, `Sort`, and structured
  references in `Range("T[Col]")`. `ListRows.Add`/`ListColumns.Add` in the middle of a table
  insert whole sheet rows or columns.
- **Hyperlinks** (`hyperlinks.rs`, engine `script_links.rs`): `Hyperlinks.Add(Anchor, Address,
  SubAddress, ScreenTip, TextToDisplay)`, `Count`, `Item`, `Delete`, `For Each` on sheets and
  ranges; `Hyperlink.Address`/`SubAddress`/`ScreenTip`/`TextToDisplay`/`Range`/`Delete`. A
  link on several cells goes on the first (Kantor keeps links per cell); `Follow`: 445.
- **Notes** (`comments.rs`): Excel's `Comment` is a Kantor note: `AddComment`, `Comment.Text`
  (with `Start`/`Overwrite`), `Author`, `Visible`, `Delete`, `Next`/`Previous`, `Parent`,
  `ClearComments`, `NoteText`, `Worksheet.Comments`. Threaded comments
  (`AddCommentThreaded`, `CommentThreaded`): 445, the engine only carries them through a save.
- **Validation** (`validation.rs`): `Add`, `Modify`, `Delete`, `Type`, `Operator`,
  `AlertStyle`, `Formula1`/`Formula2`, `IgnoreBlank`, `InCellDropdown`, input and error titles
  and messages, `ShowInput`/`ShowError`, `IMEMode`, `Value`. `Add` on a range that has a rule
  raises 1004, as in Excel.
- **Conditional formats** (`cond_formats.rs`): `FormatConditions.Add` (cell value, expression,
  text, blanks, time period…), `AddColorScale`, `AddDatabar`, `AddIconSetCondition`, `AddTop10`,
  `AddAboveAverage`, `AddUniqueValues`, `Delete`, `Count`, `Item`, `For Each`;
  `FormatCondition`: `Interior`, `Font`, `Borders`, `NumberFormat`, `StopIfTrue`, `Priority`,
  `SetFirstPriority`, `Formula1/2`, `ModifyAppliesToRange`, `Delete`; color scale criteria,
  data bar points and colours, icon criteria. Error-value rules: 445.
- **`Characters(Start, Length)`**: `Text`, `Count`, `Insert`, `Delete`; `Font` applies to the
  whole cell when the characters cover all of it. Rich text runs inside a cell: 445 (the
  engine keeps one font per cell).
- **`PageSetup`** (`page_setup.rs`): orientation, paper size, `PrintArea`,
  `PrintTitleRows`/`Columns`, `Zoom`/`FitToPagesWide`/`Tall`, margins, centring, gridlines,
  headings, headers and footers (also `FirstPage`/`EvenPage`), `FirstPageNumber`, `Order`,
  `BlackAndWhite`, `PrintComments`; `Draft`, `PrintQuality`, `PrintErrors` are accepted and
  ignored. Manual page breaks: `HPageBreaks`/`VPageBreaks.Add`, `Location`, `Delete`.
  `Application.InchesToPoints`/`CentimetersToPoints`.
- **`ExportAsFixedFormat xlTypePDF`** on a sheet, the workbook (visible sheets) or a range,
  through `sheets_pdf`, with `From`/`To`; without `Filename`, the workbook's name with `.pdf`
  in its folder. XPS: 445. `PrintOut`/`PrintPreview` raise 445 on purpose: a macro shouldn't
  print to a real printer unattended.
- **Files a macro reads or writes** (`AddPicture`, `ExportAsFixedFormat`, `Chart.Export`) go
  through `paths::checked`: inside the workbook's folder they're allowed; anything else asks
  the user in the app (Yes/No) and is refused by `kantor sheets macros run`. A workbook that
  was never saved has no folder, so every path asks (and fails headless).
- **`ActiveWindow`**: `FreezePanes` (at the active cell), `SplitRow`/`SplitColumn`,
  `DisplayGridlines`, `Zoom`, `View` (page break preview); scrolling is accepted.
- **Constants**: the `xl*` constants macros use (directions, paste and find options, sort and
  filter, alignment, borders, colours, calculation, sheet visibility, file formats, error
  codes), registered with `Project::register_constant` (`consts.rs`).

Not covered: events (`Workbook_Open`, `Worksheet_Change` don't fire), UserForms,
`Application.Run`, other workbooks, printing to a printer, sheet protection, threaded comments,
rich text runs in cells, pattern fills other than solid; diagonal borders are kept but not
drawn; module-level variables start fresh on each run (the project is compiled per run).

### Running macros in Sheets

- **Trust.** A workbook with macros opens with them disabled (`MacroTrust::Disabled`): the bar
  says so and has **Enable Content**, which enables them for this pane until it closes; nothing
  is remembered. `vba_ui::initial_trust` is where trusted locations or "always trust this file"
  would decide otherwise. With content disabled, Run is greyed out and a click on a macro
  button shows the bar again.
- **Run.** The Macros dialog (Alt+F8) lists the modules and procedures; Run starts the selected
  public parameterless `Sub` (what Excel's Alt+F8 lists). Buttons and shapes with an assigned
  macro (`macro="[0]!Macro1"` on the drawing object, or a form control's `controlPr macro` /
  VML `x:FmlaMacro`, read by `sheets_engine::drawing`) run it on click; Ctrl+click selects them.
- **Where it runs.** The interpreter is built on `Rc` (not `Send`) and recurses on the native
  stack, so a run can't be cut into slices on the UI thread. It runs on a worker thread of its
  own instead (32 MB stack), with the workbook moved there as a big recalculation does, and the
  project compiled there from the module sources (`MacroProject`, which is `Send`). The grid
  holds a stand-in meanwhile; the UI thread waits up to 150 ms first, so a quick macro never
  shows it. A longer run shows the busy pane with **Stop** (and Esc), which sets the
  interpreter's cancel flag (polled every 1024 statements): the macro stops with error 18 and
  what it did so far stays, as one undo step.
- **Dialogs.** `MsgBox` and `InputBox` are modal dialogs (buttons and icons from `Buttons`,
  the default button, Esc for Cancel). While one shows, the worker hands the workbook back to
  the UI thread, so the grid shows the cells as the macro left them; the answer takes it back.
  A run-time error shows "Run-time error '1004':", the description and "in Module1.Macro1,
  line 12", with **End**.
- **Output.** `Debug.Print` lines go to the Macros dialog's Output list (the last 500).
- **UDFs.** With content enabled, the workbook's VBA `Public Function`s work in cell formulas
  (`=MyTax(A2)`): `udf::VbaFunctions` implements the engine's user functions (§37); each thread
  that recalculates compiles the project once (thread-local), calls get their arguments as
  values (ranges as 2-D arrays) with a 5 million statement budget and no workbook objects, and
  an error shows `#VALUE!`. With content disabled they are `#NAME?`, as in Excel with macros off.
  The formula text is saved as written.

### Headless: `kantor sheets macros run`

`kantor sheets macros run FILE --macro NAME [--out OUT.xlsm] [--input V]...` runs a macro and
saves the workbook (in place unless `--out`). `MsgBox` prints to standard error and answers OK
(or Yes); `InputBox` takes the `--input` values in order and fails clearly when none is left;
`Debug.Print` goes to standard error. Running the file's macros trusts it, so its VBA functions
work in its formulas too. On a run-time error nothing is saved (exit status 1).

### AI tools don't run macros

The MCP tool `sheets_macros` and Paparan AI's `macros` tool read macros; neither runs them, and
no AI tool can press Enable Content or Run. A macro is code that came with the file: running it
is the user's decision. The tools' descriptions say so, so the assistant explains a macro or
redoes its work with formulas and the sheets editing tools instead.

## JavaScript and Python on the same layer

A binding needs four things: wrap a `Dispatch` object as a native object of the language
(property get/set and method calls go to `get`/`put`/`call`, iteration to `enumerate`),
convert `Value`s both ways, expose `Host::globals()` as the global scope, and map
`ScriptError` to the language's exceptions. Named arguments become a trailing options object
(JS) or keyword arguments (Python).

### JavaScript: rquickjs vs boa

| | rquickjs (QuickJS-NG, C) | boa (pure Rust) |
|---|---|---|
| Size | QuickJS is a few C files; ~370 KiB of x86 code for hello world | Larger: the CLI is ~25 MB stripped, mostly ICU data; ~11 MB in a 2022 embedding test vs 4.6 MB for QuickJS |
| Build | Compiles bundled C with `cc`: needs a C compiler on each CI runner (present on all three). docs.rs builds the `-sys` crate in ~10–25 s | Pure Rust, no C toolchain; a large crate graph to compile |
| Windows | x86_64 MSVC bindings shipped and tested; marked "experimental" upstream | Same as any Rust crate |
| Speed | Fast interpreter; Boa's own benchmark page puts QuickJS ~5× ahead | ~1/5 of QuickJS, improving each release |
| Conformance | Nearly all of test262 (ES2025) | ~95–96% of test262 |
| Sandboxing | Memory limit, interrupt handler for timeouts, no I/O unless we add it | No I/O unless we add it; limits through the runtime API |

### Python: RustPython vs embedded CPython

| | RustPython | CPython (python-build-standalone) |
|---|---|---|
| Size | ~18 MB release binary without the stdlib, ~25–30 MB with `freeze-stdlib` (2021 figures) | A full interpreter and stdlib, tens of MB unpacked: more than Kantor's whole 60 MB install budget allows |
| Startup | Interpreter in-process; freezing the stdlib avoids file reads | Fast; frozen startup modules help (~20%) |
| Packaging | A Rust crate in the same binary; nothing extra in the dmg, AppImage or installer | A separate runtime directory per platform, its own signing/notarisation on macOS, updates |
| Stdlib | Most of the pure-Python stdlib; C-extension packages (numpy, pandas) don't run | Everything, including pip packages, if we ship or download them |
| Compatibility | Targets CPython 3.15 syntax; slower than CPython | Reference implementation |
| Sandboxing | Feasible: leave `os`/`socket`/`subprocess` out of the frozen stdlib | Not possible in-process; would need a separate process with OS sandboxing |

### Recommendation

- **JavaScript: rquickjs.** Small, fast, near-complete ES conformance, and it can be sandboxed
  with a memory limit and an interrupt handler. Boa is the fallback if a C toolchain
  dependency becomes a problem.
- **Python: RustPython with a frozen, curated stdlib** (no `os`, `socket`, `subprocess`),
  opt-in at build time. If users need numpy or pandas, offer CPython later as a separately
  downloaded add-on that runs out of process; don't bundle it.

Sources: [rquickjs on docs.rs](https://docs.rs/crate/rquickjs/latest),
[rquickjs-sys on docs.rs](https://docs.rs/crate/rquickjs-sys/latest),
[rquickjs on GitHub](https://github.com/delskayn/rquickjs),
[QuickJS](https://bellard.org/quickjs/),
[Boa v0.19 release](https://boajs.dev/blog/2024/07/09/boa-release-19),
[Boa v0.21 release](https://boajs.dev/blog/2025/10/22/boa-release-21),
[Boa embedding size discussion](https://github.com/boa-dev/boa/discussions/2163),
[Boa 0.22 performance article](https://www.issoh.co.jp/tech/details/3100/),
[RustPython on GitHub](https://github.com/RustPython/RustPython),
[RustPython frozen stdlib sizes, PR #2403](https://github.com/RustPython/RustPython/pull/2403),
[CPython frozen startup modules, bpo-45020](https://bugs.python.org/issue45020).

## Known gaps versus real VBA

- No static types on values: Integer arithmetic widens to Long (as for Variants) instead of
  overflowing until the result is stored; a number compared with a non-numeric string orders
  the number first instead of raising Type mismatch; `TypeName` of an array reports the common
  element type (`Array("a")` gives `String()`, VBA says `Variant()`).
- String positions count Unicode scalar values, not UTF-16 code units (differs only outside the
  BMP). Locale-dependent conversions are en-US.
- `Class_Terminate` runs when a variable releases the last reference (assignment, `Set … =
  Nothing`, procedure exit), not when a collection or array drops it.
- `ByRef` on an object's property passes a copy (as VBA does for properties); `ByRef` to a
  `ParamArray` element doesn't write back.
- Not implemented: `Implements` dispatch through interfaces, `Event`/`RaiseEvent`/`WithEvents`,
  UserForms, `Decimal` (mapped to Double), `LSet` between user-defined types, `On … GoTo`
  lists, file I/O, `Declare`d DLL calls, `Shell`, `SendKeys`, registry settings. Each raises
  an error naming the feature.
- `Format` covers the common number, date and text patterns and the named formats, en-US only.
- Recursion is limited to 100 nested calls by default (`set_max_call_depth`): the tree-walking
  interpreter uses the native stack, and Windows gives the main thread 1 MB.
