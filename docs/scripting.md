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
  `Project::set_step_limit` adds a hard step budget. `take_calls` is asked before every
  statement for `HostCall`s to run there: event handlers for what the script just did
  (`HostCall::Proc`, a procedure by name) or a host object's event for `WithEvents` variables
  (`HostCall::Event`); keep it cheap.
- **Calls back into the script.** `Dispatch::script_member` marks members the runtime answers
  itself (`ScriptMember::Run`: Excel's `Application.Run` calls a procedure of the running
  project). `Dispatch::document_module` names the document module an object is (`Sheet1`,
  `ThisWorkbook`), so that module's public variables and procedures are members of the
  object too (`ThisWorkbook.Total`, `Sheet1.Recalc`).

### The VBA interpreter (`kantor_script::vba`)

`Project::new()`, `add_module(name, kind, source)`, `procedures()`, `run(name, args, host)`,
`call_function(name, args, host)` (for worksheet UDFs), `run_by_ref(name, args, host)` (any
procedure of a standard or document module, private ones too, with `ByRef` results back in
`args`: an event handler's `Cancel`), `raise_event(source_type, event, args, host)` (a host
object's event to the `WithEvents` variables holding one), `with_events_handlers()`,
`register_constant`, `reset`.

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
- **Events and interfaces** (`vba/events.rs`). `Event Name(params)` declares an event of a
  class; `RaiseEvent Name(args)` calls `<variable>_Name` in every module whose `WithEvents`
  variable holds this instance (arguments `ByRef` where the handler says so; an undeclared
  event is a compile error). A `WithEvents` variable is a *sink*, registered when it comes to
  exist: a document module's on first use of its variables, a class instance's when it is
  created (kept weakly, so the sink doesn't keep its instance alive). `Private WithEvents App
  As Application` works the same for host objects: the host raises `SheetChange`… with
  `raise_event("Application", …)`, matched by type name. `Implements IShape` makes a class
  answer for an interface: a member called through a variable declared `As IShape` runs
  `IShape_Member` (private, as VBA writes them); through a variable of the class itself the
  class's own public member wins, and an interface member is used when the class has none of
  that name; `TypeOf x Is IShape` is true. `On n GoTo a, b, c` / `On n GoSub …` jump to the
  n-th label (0 or past the list falls through, below 0 or above 255 is error 5, MS-VBAL
  §5.4.2.10).
- **Handlers between statements.** Before each statement the interpreter runs what
  `Host::take_calls` returns. An error in such a handler stops the run (it isn't the caller's
  to trap with `On Error`, as VBA reports it in the handler). Handlers that fire themselves
  again (a `Worksheet_Change` that writes its own cells without `EnableEvents = False`) end
  at the call depth limit with "Out of stack space" (error 28), not in a hang.

## The Sheets object model (`crates/sheets_script`)

A crate of its own because it needs both `sheets_engine` and `kantor_script`, and neither
should know the other. `sheets_ui`, the `kantor sheets macros run` command and the tests use it
the same way.

```
 sheets_ui (Run, buttons) ─┐                         ┌─► sheets_engine::Workbook
 kantor sheets macros run ─┼─► sheets_script ─────────┤    (edits, undo, recalculation)
 tests                    ─┘   run_macro · objects   └─► kantor_script::vba::Project
```

- **The live project** (`engine::MacroEngine`). While content is enabled a workbook keeps
  one compiled project, so module-level and global variables keep their values between macro
  runs and event handlers, as in Excel. It is reset (variables cleared) by an `End` statement,
  by a run that ends with an unhandled error (the error dialog's End; Stop too), and it goes
  away with the engine: content disabled or the workbook closed. The engine also keeps one
  `Session` for all its runs (`Session::reload` puts the next run's workbook, view and
  dialogs in), so objects a macro keeps in module variables (`Set ws = Sheet1`, a `WithEvents`
  `Application`) stay valid from run to run, and what Excel's application keeps:
  `Application.EnableEvents` (a macro that turns it off and ends leaves events off, as in
  Excel), `Application.OnTime` schedules and the paths the user allowed. Worksheet UDFs keep
  their own per-thread copies of the project (below): module variables aren't shared between
  macros and UDFs.
- **The engine and threads.** The interpreter is built on `Rc` and isn't `Send`, so an engine
  lives on one thread for its whole life. The desktop keeps a *macro thread* per pane
  (`sheets_ui::macro_thread`), made at Enable Content and ended when the pane closes (its job
  channel drops). Each macro run, event or due `OnTime` is a job that carries the workbook
  there (as before, the grid holds a stand-in) and a channel back for dialogs and the result;
  the workbook comes back with the outcome. The UI thread never touches the project: it keeps
  the project's `EventIndex` (plain data, `Send`) to tell whether an event has a handler.
  The command line and the tests use an engine on their own thread.
- **A run** (`run::run_macro`, `MacroEngine::run`) takes the workbook, runs one macro and gives the workbook back
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
  Yes and takes input box answers from a list. `MacroUi` also lists the window's other
  workbooks (`other_books`), lends one to the run (`lend_book`) and answers whether a path may
  be used (`allow_path`).

### Events (`sheets_script::events`)

Only with content enabled and `Application.EnableEvents` True. An event's handlers run in
Excel's order: the sheet module's (matched by code name), then `ThisWorkbook`'s, then each
`WithEvents` variable holding the `Application`:

| Event | Sheet module | `ThisWorkbook` | `Application` (`WithEvents`) |
|---|---|---|---|
| Enable Content (or a trusted file opening) | | `Workbook_Open` | `WorkbookOpen(Wb)` |
| Close | | `Workbook_BeforeClose(Cancel)` | `WorkbookBeforeClose(Wb, Cancel)` |
| Save / Save As | | `Workbook_BeforeSave(SaveAsUI, Cancel)`, `Workbook_AfterSave(Success)` | `WorkbookBeforeSave`, `WorkbookAfterSave` |
| Cells changed | `Worksheet_Change(Target)` | `Workbook_SheetChange(Sh, Target)` | `SheetChange(Sh, Target)` |
| Selection moved | `Worksheet_SelectionChange(Target)` | `Workbook_SheetSelectionChange` | `SheetSelectionChange` |
| Another sheet picked | `Worksheet_Deactivate`, `Worksheet_Activate` | `Workbook_SheetDeactivate/Activate(Sh)` | `SheetDeactivate/Activate` |
| Sheet inserted | | `Workbook_NewSheet(Sh)` | `WorkbookNewSheet(Wb, Sh)` |
| Double-click | `Worksheet_BeforeDoubleClick(Target, Cancel)` | `Workbook_SheetBeforeDoubleClick` | `SheetBeforeDoubleClick` |
| Right-click | `Worksheet_BeforeRightClick(Target, Cancel)` | `Workbook_SheetBeforeRightClick` | `SheetBeforeRightClick` |
| Recalculation | `Worksheet_Calculate` | `Workbook_SheetCalculate(Sh)` | `SheetCalculate` |

- **Two ways in.** The host's (a user edit, double-click, save, close) go through
  `MacroEngine::fire`, which returns `RunOutcome::cancel`. A macro's own changes fire events
  too, as in Excel: between two statements `events::poll` compares the workbook with what it
  saw last (the undo history's length, the active sheet, its selection, the sheet ids) and
  hands the interpreter the handlers for the difference. The changed cells come from the undo
  entries the statement added (`Workbook::edited_since`, KANTOR_PATCHES §37's
  `kantor_history_changes`), so every way of editing fires `Worksheet_Change` without each
  object knowing about events. While `EnableEvents` is False nothing fires and nothing is
  compared; turning it back on takes the state as seen.
- **Change from the grid.** Typing, paste, fill, clear, sort, undo and redo all end in
  `update_title`, which compares the undo history's lengths with what events last saw: more
  undo steps (an edit or redo) give the cells of those steps, fewer (an undo) the cells the
  undone steps changed. One `Change` per sheet, `Target` holding every changed area; a sheet
  changed as a whole (rows inserted) gives its used range. In automatic calculation
  `Worksheet_Calculate` follows; F9 fires it too.
- **Undo.** The user's edit and what its `Change` handlers do are one undo step
  (`MacroEngine::fire` with `fold_from`, the undo mark before the edit): Ctrl+Z takes back the
  typing and the handler's reaction together, where Excel would clear the undo list.
  `Application.Undo` in a handler that hasn't changed anything yet takes the user's edit back
  for good (`Workbook::revert_edit`): the classic "validate and undo" handler works. Handlers
  after an undo are a step of their own. Other events' handlers are one step each, as a macro
  run is.
- **Cancel** stops what the event was for: the double-click's edit, the right-click menu, the
  save, the close. Double-click, save and close wait for their handlers (with dialogs and the
  busy pane as for a macro) and go on afterwards only if no handler set `Cancel`. The close
  goes through the workspace: `Workspace::request_close` asks each closing workbook with a
  `Workbook_BeforeClose` first and closes again when it lets the close go on. The right-click
  menu is built while the grid holds the workbook, so the handler runs in place with the
  150 ms wait: when it ends in time its `Cancel` decides; a slower handler goes on with the
  busy pane and the menu stays shut.
- **Cost when nothing handles an event: none.** The UI thread keeps the project's
  `EventIndex`, built once at Enable Content from the procedure names: which of the event
  kinds any handler exists for (sheet modules, `ThisWorkbook`, `<variable>_<Event>`
  procedures of modules with a `WithEvents` variable). Each hook asks it first (a hash set
  lookup), so without a handler typing does exactly what it did before; `update_title` doesn't
  even schedule the check. Inside a run, the per-statement poll is one flag check when the
  project handles none of the events a macro's changes fire. With a handler, an event is a
  job on the macro thread: the 150 ms rule applies (a quick handler never shows the busy pane).

### Other workbooks (`sheets_script::books`)

Every workbook a run touches has a `Session` of its own, so `Workbooks("Data.xlsx").Sheets(1)
.Range("A1").Value` is the same object model over another workbook. The sessions share an
`App`: the list of workbooks and the active one. Unqualified `Range`, `Cells`, `ActiveSheet`
follow the active workbook as in Excel; `ThisWorkbook` (and code names such as `Sheet1`) stay
the macro's own.

- `Workbooks`: `Count`, `Item` by number or name (with or without the extension), `For Each`
  over the workbooks open in the window: the run's own, then the other tabs. A tab's workbook
  is lent to the run when a macro first touches it (the tab shows it's busy) and goes back when
  the run ends.
- `Workbooks.Open(Filename)` reads `.xlsx`/`.xlsm`/`.xls`/`.xlsb`/`.ods`/`.csv`/`.tsv` on the
  macro's thread and makes it active (a workbook already open is returned); `Workbooks.Add`
  makes a new one. Both come out of the run (`BookOut::New`) and open in new tabs; the command
  line drops them with a note if they have unsaved changes.
- `Workbook.Save` (the macro's own is saved by the host when the run ends; another one at
  once), `SaveAs Filename, FileFormat` (`xlOpenXMLWorkbook` 51 → .xlsx,
  `xlOpenXMLWorkbookMacroEnabled` 52 → .xlsm, `xlCSV` 6 → .csv of the active sheet; the
  extension is added when missing, a different one is error 1004), `SaveCopyAs`,
  `Close [SaveChanges]` (another workbook closes, a lent one's tab too; `ThisWorkbook.Close`
  closes the pane after the run: saved first with `SaveChanges:=True`, without asking with
  False, and kept open with a note when it has unsaved changes and no answer), `Activate`.
- **Paths.** A macro must not read or write files the user didn't mean it to. `Open`,
  `SaveAs`, `SaveCopyAs` and `Close Filename:=` of a path in the macro's workbook's own folder
  go ahead on the desktop; any other path asks the user (a Yes/No dialog naming the path, No
  by default) once per path for the workbook's session. Relative paths are taken from the
  workbook's folder; `..` is worked out before the check. Headless (`kantor sheets macros
  run`, tests) only paths given with `--allow-path` (a file, or a folder and everything in it)
  work, the workbook's folder included; anything else is error 1004 ("Kantor didn't let the
  macro open …"). On the sandboxed Mac App Store build a path also needs a sandbox grant,
  which macros can't ask for yet.

### Application.Run, OnTime, Wait, Undo

- `Application.Run "Macro", args…` (and `Run` unqualified) calls a procedure of the project:
  `"Macro"`, `"Module1.Macro"`, or `"'Book.xlsm'!Macro"` (the workbook part is ignored:
  only the running workbook's macros can be called). Arguments by value; a Function's result
  comes back. Unknown: error 1004 with Excel's text.
- `Application.OnTime EarliestTime, "Macro"[, LatestTime][, Schedule]` keeps a schedule in
  the engine; `Schedule:=False` removes the one with the same time and macro (error 1004
  when there is none). On the desktop a timer task waits for the earliest schedule while the
  pane is open and runs it as a macro (a busy workbook retries a second later); past
  `LatestTime` a schedule is dropped. `MacroEngine::due(now)` takes the next due one (the
  tests drive it with their own clock). The command line ends with the macro and notes
  schedules that didn't run.
- `Application.Wait Time` sleeps in 50 ms slices, so Stop (and Esc) ends it with error 18.
- `Application.Undo` (see Events).

### Coverage

- **Globals / `Application`**: `Range`, `Cells`, `Rows`, `Columns`, `ActiveSheet`, `ActiveCell`,
  `Selection`, `ActiveWorkbook`, `ThisWorkbook`, `Workbooks`, `Worksheets`/`Sheets`, `Names`,
  `ActiveWindow`, `Intersect`, `Union`, `Evaluate` (and `[A1]`), `WorksheetFunction`, `Calculate`,
  `Goto`, `InputBox` (Type 0/1/2/4/8; `False` when cancelled), `UserName`, `Version` (16.0),
  `Name`, `OperatingSystem`, `PathSeparator`; `ScreenUpdating`, `DisplayAlerts`, `EnableEvents`,
  `StatusBar`, `Cursor`, `CutCopyMode` (False clears the clipboard) are kept; `Calculation`
  switches the workbook's calculation mode. `Application.Sum(...)` returns an error value
  instead of raising. `Run`, `OnTime`, `Wait`, `Undo` (above). `OnKey`, `Quit`, `SendKeys`,
  file dialogs and `Caller` outside a UDF fail with error 445.
- **`Workbook`**: `Name`, `FullName`, `Path`, `CodeName`, `Worksheets`, `Sheets`, `Names`,
  `ActiveSheet`, `Saved`, `Save`, `SaveAs`, `SaveCopyAs`, `Close`, `Activate` (see Other
  workbooks); `RefreshAll`, printing: 445. `Workbooks`: see Other workbooks.
- **`Worksheets`/`Sheets`** (worksheets; chart sheets aren't in the object model): `Count`,
  `Item` by number or name (default member), `Add` with `Before`/`After`/`Count` (the new sheet
  becomes active), `For Each`.
- **`Worksheet`**: `Name` (get/let), `CodeName`, `Index`, `Range`, `Cells`, `Rows`, `Columns`,
  `UsedRange`, `Activate`, `Select`, `Delete`, `Copy`/`Move` with `Before`/`After` (to another
  workbook: 445), `Visible` (`xlSheetVeryHidden` hides), `Next`/`Previous`, `Paste`,
  `AutoFilter`, `AutoFilterMode` (False removes it), `FilterMode`, `ShowAllData`, `Sort`
  (`SortFields.Clear/Add`, `SetRange`, `Header`, `Apply`: what recorded macros write),
  `StandardWidth`/`StandardHeight`, `Evaluate`, `Calculate`. `Protect`/`Unprotect` are
  accepted and do nothing (Kantor doesn't protect sheets). `Shapes`, `ChartObjects`,
  `ListObjects`, `PivotTables`, `Comments`: 445.
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
  `RemoveDuplicates`, `AutoFit` (columns by the longest shown text; rows back to automatic),
  `NumberFormat`, `HorizontalAlignment`, `VerticalAlignment`, `WrapText`, `IndentLevel`,
  `Orientation`, `MergeCells`/`Merge`/`UnMerge`, `ColumnWidth` (characters), `RowHeight`
  (points), `Width`/`Height`/`Left`/`Top` (points), `Hidden`, `HasFormula`, `Font` (`Bold`,
  `Italic`, `Underline`, `Strikethrough`, `Size`, `Name`, `Color`, `ColorIndex`, `FontStyle`),
  `Interior` (`Color`, `ColorIndex`, `Pattern` `xlNone`), `Borders`/`Borders(edge)`
  (`LineStyle`, `Weight`, `Color`, `ColorIndex`), `BorderAround`,
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
- **`ActiveWindow`**: `FreezePanes` (at the active cell), `SplitRow`/`SplitColumn`,
  `DisplayGridlines`, `Zoom`, `View` (page break preview); scrolling is accepted.
- **Constants**: the `xl*` constants macros use (directions, paste and find options, sort and
  filter, alignment, borders, colours, calculation, sheet visibility, file formats, error
  codes), registered with `Project::register_constant` (`consts.rs`).

Not covered: UserForms, charts, shapes
and pivot tables as objects, comments/notes, hyperlinks, conditional formats and validation from
macros, printing; removing a single border edge with
`xlNone` is ignored (the engine removes all of a range's borders or none) and diagonal borders
aren't drawn; `AutoFit` measures characters, not glyphs; `RemoveDuplicates` moves values, not
formats.

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
  stack, so a run can't be cut into slices on the UI thread. It runs on the pane's macro
  thread (32 MB stack, see "The engine and threads"), with the workbook moved there as a big
  recalculation does. The grid holds a stand-in meanwhile; the UI thread waits up to 150 ms first, so a quick macro never
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
  that recalculates compiles the project once (thread-local) and calls run with a 5 million
  statement budget. A reference argument comes as a read-only `Range` (`udf_range`, as Excel
  passes a `Range` to `Function F(r As Range)` and to Variant parameters): `Value`, `Value2`,
  `Text`, `Count`, `Cells`, `Item`, `Rows`, `Columns`, `Address`, `Row`, `Column`, `Offset`
  and `Resize` reading inside the cells passed, `Worksheet.Name`, `For Each`; its values are
  the ones the engine read for the formula (so they are dependencies as before). Other
  arguments are values. `Application.Caller` is the calling cell (`Address`, `Row`,
  `Column`), `Application.Volatile` is accepted. The engine is recalculating, so nothing else
  of the workbook is reachable: reading outside the cells passed, any write (`r.Value = 1`)
  and `Range(...)` fail, and the cell shows `#VALUE!`, as Excel shows for a function that
  writes. A function named like a worksheet function (`Info`, `Second`) is the worksheet
  function, as in Excel. With content disabled they are `#NAME?`, as in Excel with macros off.
  The formula text is saved as written.

### Headless: `kantor sheets macros run`

`kantor sheets macros run FILE --macro NAME [--out OUT.xlsm] [--input V]... [--events]
[--allow-path PATH]...` runs a macro and saves the workbook (in place unless `--out`). With
`--events`, `Workbook_Open` runs first and `Workbook_BeforeSave` (which may cancel the save)
and `Workbook_AfterSave` around the save, through one engine, so module variables `Workbook_Open`
sets are there for the macro. `--allow-path` names files or folders macros may open or save
(`Workbooks.Open`, `SaveAs`); nothing else is allowed headless. `MsgBox` prints to standard error and answers OK
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
- Not implemented: UserForms, `Decimal` (mapped to Double), `LSet` between user-defined types,
  file I/O, `Declare`d DLL calls, `Shell`, `SendKeys`, registry settings. Each raises an error
  naming the feature.
- Static types are known only from declarations: a member call goes through an interface when
  the object is in a variable or parameter declared as that interface; through a `Variant` it
  uses the class's own public member first. Of host objects only the `Application`'s events reach
  `WithEvents` variables (a `WithEvents wb As Workbook` or `As Worksheet` hears nothing; the
  same events reach `ThisWorkbook` and the sheet modules). `Workbook_BeforeClose` doesn't run for `ThisWorkbook.Close` called
  by a macro.
- `Format` covers the common number, date and text patterns and the named formats, en-US only.
- Recursion is limited to 100 nested calls by default (`set_max_call_depth`): the tree-walking
  interpreter uses the native stack, and Windows gives the main thread 1 MB.
