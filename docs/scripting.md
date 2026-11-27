# Scripting: macros on one object layer

Kantor Sheets runs the VBA macros of `.xlsm` files, and JavaScript macros stored in any
`.xlsx`/`.xlsm`. `crates/kantor_script` is the language core: a host-neutral object layer and a
VBA interpreter; `crates/kantor_script_js` binds JavaScript (QuickJS) to the same objects.
`crates/sheets_script` is the Sheets object model on top of it (`Application`, `Workbook`,
`Worksheet`, `Range`…) and the host that runs a workbook's macros; `sheets_ui` runs them after
Enable Content, and `kantor sheets macros run` runs them headless. Python was built and measured
and is left out for now (see [JavaScript and Python](#javascript-and-python-macros)).

## Layers

```
 VBA source ─► kantor_script::vba ──┐
 JavaScript ─► kantor_script_js   ──┼─► kantor_script::object ◄── sheets_script (Sheets object model)
 (Python)   ─► kantor_script_py   ──┘     Value · Dispatch · Host
               (not built: +16 MB)
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
- **UserForms** (below).

## UserForms

### The designer (`kantor_vba::forms`)

A form module's designer lives next to the `VBA` storage in `vbaProject.bin`, in a storage
named after the form ([MS-OFORMS]): `f` is the form (a FormControl: caption, client size,
colours, font) followed by one *site* per control (name, ID, position, tab index, flags such as
visible / Default / Cancel, ControlSource, RowSource, and the control's type as a
ClsidCacheIndex); `o` holds the controls' own data one after another (each as long as its site
says); `\x03VBFrame` is the designer's text header (client size in twips, StartUpPosition); a
Frame, a MultiPage and each of its Pages have a storage `i<ID>` with their own `f` and `o`
(a MultiPage's `o` is the TabStrip with the page captions).

Every structure is a version, a size, a property mask and the stored properties in mask order:
those of four bytes or less first (each aligned to its size), then sizes, positions and
strings, then pictures and fonts. `forms::read_props` reads any of them from a table of
(bit, name, kind); `write_props` writes them (`synth::Builder::form` writes whole designers for
tests). A property that doesn't decode keeps its default and a control that doesn't decode
keeps its site, so a damaged form still opens. `VbaProject::form(name)` returns a
`FormDesign`: caption, size in points (HIMETRIC × 72 / 2540), colours as OLE_COLORs
(`ole_rgb` resolves system colours), font, StartUpPosition, and a flat list of controls
(parents before children, `parent` pointing at the Frame / MultiPage / Page) with: kind
(Label, TextBox, CommandButton, CheckBox, OptionButton, ToggleButton, ComboBox, ListBox,
Frame, MultiPage, Page, Image, SpinButton, ScrollBar, TabStrip; other ActiveX controls as
`Other`), name, position and size, caption, value, tab index / tab stop, visible, enabled,
locked, Default / Cancel, accelerator, font, fore / back / border colours, back style, border
style, special effect, MaxLength, MultiLine, WordWrap, AutoSize, PasswordChar, text alignment,
ColumnCount / BoundColumn / TextColumn / ColumnHeads, ListStyle, MultiSelect, Style (drop-down
list), RowSource, ControlSource, GroupName, Min / Max / value / SmallChange / LargeChange /
Orientation, the Image's picture (bytes as stored; a bare DIB gets a BMP header) and size mode,
tab captions, tag and tip text. Not read: column widths (`rgColumnInfo`), mouse icons, the
design-time extender, ActiveX controls outside the cache (only their sites). Fixtures: two
Excel-made forms from MIT-licensed public repositories (`crates/kantor_vba/tests/fixtures`).

[MS-OFORMS]: https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-oforms/

### Running forms (`kantor_script::vba::forms`)

A form module compiles like a class module whose instances are forms. `Project::add_form`
gives it its design. `UserForm1` used as an object is the form's default instance (created and
initialised on first use: module variables, lists from RowSource, values from ControlSource,
then `UserForm_Initialize`); `Dim f As New UserForm1` / `Set f = New UserForm1` make others.
`Load`, `Unload` (QueryClose with CloseMode 1, which can cancel, then Terminate; the default
instance is forgotten, so the next use loads a fresh one), `Show` (modal by default),
`Show vbModeless` (the macro goes on; the form's loop runs once the macro has finished),
`Hide`, `Me`. Inside the form's module its controls and properties are in scope (`TextBox1`,
`Caption`, `Hide`), after the module's own names and before other modules' (VBA functions win
over form properties of the same name: `Left(s, 1)` is the function, `Me.Left` the form's).

Controls are objects (`TypeName` gives `TextBox`…) with `Value` (default member; a Label's or
Frame's default is `Caption`), `Text`, `Caption`, `Enabled`, `Visible`, `Locked`, `Left` / `Top`
/ `Width` / `Height` (points), `Tag`, `ControlTipText`, `TabIndex`, `TabStop`, colours,
`Font` (Name, Size, Bold, Italic, Underline, Strikethrough), `ControlSource`, `RowSource`
(filled from the sheet through the host's `Range`; `Sheet!Name` falls back to the workbook
name), `GroupName`, `MaxLength`, `MultiLine`, `PasswordChar`, `Default`, `Cancel`,
`Accelerator`; lists: `AddItem item[, index]`, `RemoveItem`, `Clear` (error 70 with a
RowSource, as in Excel), `List` (the whole 2-D array, or `List(row[, col])`, read and
written), `Column`, `ListCount`, `ListIndex`, `Selected(i)`, `MultiSelect`, `ColumnCount`,
`BoundColumn`, `TextColumn`, `Style`; `Min` / `Max` / `SmallChange` / `LargeChange`; a
MultiPage's `Value`, `Pages(i)`, `SelectedItem`; containers' `Controls`; methods `SetFocus`
(runs Exit / Enter), `Move`, `ZOrder`. The form has `Controls` (`Me.Controls("Name")`,
`.Count`, `For Each`; also the form's default member), `Caption`, `Width` / `Height` /
`InsideWidth` / `InsideHeight`, `Left` / `Top`, `BackColor`, `ForeColor`, `Enabled`, `Tag`,
`Font`, `ActiveControl`, `StartUpPosition`; `Repaint`, `PrintForm` and the like do nothing.

Changing a value (from the user or from code) queues the events it causes, which run right
after the statement, as in VBA: `Change` for every value change, `Click` too for CheckBoxes,
OptionButtons, ToggleButtons and list picks; picking an OptionButton clears the others of its
group (GroupName, or same parent); a value with a ControlSource is written to its cell. Moving
the focus runs `BeforeUpdate(Cancel)` / `AfterUpdate` for a changed control, then
`Exit(Cancel)` and `Enter`; clicking a CommandButton moves the focus to it first. Form events:
`UserForm_Initialize`, `Activate`, `Click`, `QueryClose(Cancel, CloseMode)`, `Terminate`;
control events: `Click`, `DblClick(Cancel)`, `Change`, `BeforeUpdate`, `AfterUpdate`, `Enter`,
`Exit`, `KeyPress(KeyAscii)` (`KeyAscii = 0` drops the key) and `KeyDown(KeyCode, Shift)`.
`Cancel`, `KeyAscii` and `KeyCode` are `MSForms.ReturnBoolean` / `ReturnInteger` objects:
`Cancel = True` assigns their default member (Let to an object variable).

`Show` runs the form loop: the host gets a `forms::FormView` (the design as the code changed
it, each control's state, the focus, which controls the code watches live) through
`Host::form_event` and returns a `forms::FormInput` (edits: typed text, a ticked box, a picked
row, a number; and an action: a click, a focus change, Enter / Esc for the Default / Cancel
button, a key press, the close box). The loop runs the handlers and asks again until the form
is hidden or unloaded, then calls `Host::form_closed`. Handlers run in the same project as the
macro, so module variables are shared. An error in a handler ends the run (the form closes).
`forms::scripted_input` turns "set these controls by name, click that one" into an input: the
tests and `--form-answers` use it.

### Forms in Sheets

`sheets_script` passes the designs on (`MacroProject::forms`) and `MacroUi::form_event` is lent
the workbook while a form shows, as for `MsgBox`. In `sheets_ui` (`macro_form.rs`) the form is
a modal card over the grid drawn from its design: positions and sizes from the file (points ×
4/3), the form's and controls' colours, captions and font sizes and styles, Kantor's look
otherwise (the system font). TextBoxes and editable ComboBoxes are Kantor inputs kept between
steps; typed text goes with the next action unless the code handles that control's `Change` or
`KeyPress` (then each change goes at once). A MsgBox shown by a handler appears over the form.
Forms run on the workbook's macro thread with the live project, like any macro or event handler:
each step reaches the UI over the run's channel and the answer goes back the same way, so a form
shown from an event handler (`Workbook_Open`, `Worksheet_BeforeDoubleClick`…) works as one shown
from Run, and the form's code sees the module variables the other runs left. While a form shows
the run isn't over, so grid events wait (a modeless form too: its loop runs after the macro or
handler that showed it). `Private WithEvents App As Application` in a form's code hears
application events while the form is loaded.
The Macros dialog shows a form module's layout read-only under its code.

Headless (`kantor sheets macros run`), a form fails the run with error 445 naming it, unless
`--form-answers FILE.json` gives its steps:
`{"UserForm1": [{"set": {"txtName": "Ann", "chkVip": true, "lstCity": "Rome"}, "click": "cmdOK"}]}`
(a list takes a row number or a row's text; `"click": "close"` is the close box; each step is
one round of the form's loop).

Not done: modeless forms don't let the user edit the grid while they show (their loop runs
after the macro, holding the workbook as a modal form does); multi-line TextBoxes are drawn
single-line; no Tab-order navigation of its own (the inputs' focus order), no KeyDown from the
UI, no dragging ScrollBar thumbs, no column widths in lists, no mouse icons or `MouseMove` /
`MouseDown` events, no controls added at run time (`Controls.Add`), WMF/EMF pictures in Image
controls aren't drawn.

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
  `Worksheet_Calculate` follows; F9 fires it too. An edit ended with Enter or Tab fires its
  `Change` before the `SelectionChange` of the move, as in Excel, and an edit not yet seen
  when another run starts fires after that run.
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
  workbooks); printing: 445. `RefreshAll` refreshes every pivot table (not web formulas or
  external connections); `PivotCaches`, `Charts` (chart sheets), `IconSets`,
  `ExportAsFixedFormat` (below). `Workbooks`: see Other workbooks.
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
  through `paths::checked`: inside the workbook's folder (or below it) they're allowed;
  anything else goes through the same check as `Workbooks.Open` (see Paths): asked once in the
  app (Yes/No), and headless only paths given with `--allow-path`. A workbook that was never
  saved has no folder, so every path is checked.
- **`ActiveWindow`**: `FreezePanes` (at the active cell), `SplitRow`/`SplitColumn`,
  `DisplayGridlines`, `Zoom`, `View` (page break preview); scrolling is accepted.
- **Constants**: the `xl*` constants macros use (directions, paste and find options, sort and
  filter, alignment, borders, colours, calculation, sheet visibility, file formats, error
  codes), registered with `Project::register_constant` (`consts.rs`).

Not covered: printing to a printer, sheet protection, threaded comments, rich text runs in
cells, pattern fills other than solid; diagonal borders are kept but not drawn; `AutoFit`
measures characters, not glyphs; `RemoveDuplicates` moves values, not formats.

### Running macros in Sheets

- **Trust.** A workbook with macros opens with them disabled (`MacroTrust::Disabled`): the bar
  says so and has **Enable Content**, which enables them for this pane until it closes; nothing
  is remembered. `vba_ui::initial_trust` is where trusted locations or "always trust this file"
  would decide otherwise. With content disabled, Run is greyed out and a click on a macro
  button shows the bar again.
- **Run.** The Macros dialog (Alt+F8) lists the modules and procedures, then the workbook's
  JavaScript and Python modules with their functions; Run starts the selected public
  parameterless `Sub` (what Excel's Alt+F8 lists) or script function. Buttons and shapes with an assigned
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

### The Macro Editor (`sheets_ui::vba_editor`)

Excel's Visual Basic Editor, simpler: Alt+F11, the palette's "Macro Editor", the menu item
under Macros…, or Edit in the Macros dialog open it as a panel beside the grid (Alt+F11 again
moves between it and the grid).

- **Left**, the project: `ThisWorkbook` and the sheet modules, standard modules, class modules,
  UserForms (their code; the designer is kept as it is, there is no form designer), then the
  JavaScript and Python scripts. A workbook without a VBA project shows an empty
  `ThisWorkbook` and a module per sheet. Insert Module / Class Module / JavaScript, Rename and
  Remove (not for document modules or forms).
- **Right**, gpui-base's code editor (`EditorState`): line numbers, monospace, Tab indents
  (4 spaces), undo/redo, Ctrl+F search. Colours come from `code_highlight.rs`, a small lexer
  for VBA and JavaScript (keywords, strings, comments, numbers) behind the editor's
  `InputHighlighter` hook, styled by the theme's syntax styles.
- **Problems** (`sheets_script::code::diagnostics`, `kantor_script::vba::Project::check`):
  the error that keeps a module from compiling, lines that don't parse and statements Kantor
  doesn't run, shown at their lines and listed under the code (click to go there). A fatal
  one stops F5 with its line. A run-time error of a module shown in the editor marks its
  line too.
- **F5 / Run** runs the macro the cursor is in (a public `Sub` without arguments, or a
  script's function without parameters); outside every procedure, the module's only macro.
- **Into the workbook** (`sheets_script::code::apply`) 0.4 s after typing stops, and before a
  run, a save, a module switch or closing: `kantor_vba::VbaProject::with_modules` rewrites the
  project (kept modules keep their `dir` records and attributes, new ones get Excel's default
  attributes, removed ones lose their stream, form storage and `PROJECT` lines; renaming is a
  remove plus an add, so a renamed class gets default attributes), or
  `kantor_vba::new_project` makes one (with `ThisWorkbook` and a module per sheet, and the
  `CMG`/`DPB`/`GC` lines the `PROJECT` grammar requires, written with MS-OVBA's data
  encryption: not protected, no password, visible). Every compiled cache is dropped
  (`_VBA_PROJECT` reduced to its 7-byte header, `__SRP_*` streams gone, text offsets 0), so
  Excel compiles from source. The bytes go to `Workbook::set_vba_project`, which makes the
  workbook unsaved (not an undo step: Undo in the grid never touches the code; the editor
  has its own undo). Scripts go to `Workbook::set_scripts`.
- **Saving.** A save as `.xlsm` writes the edited project (`preserve::VbaSave`) with the
  workbook's `codeName` and, for worksheets without one, the names of the document modules in
  sheet order (what macros map them by). Ctrl+S or Save As to `.xlsx` with VBA asks as Excel
  does (save without macros, or as `.xlsm`); JavaScript scripts stay in an `.xlsx`. An
  autosave of a plain `.xlsx` leaves an edited project out (it would make the file a
  macro-enabled one under an `.xlsx` name).
- **The live project** is rebuilt after each change that reaches the workbook (a new macro
  thread with the new code; module variables start over, as after an edit in Excel), so the
  next run or event handler uses it.
- **Trust.** Editing never needs Enable Content; running does. One exception: code typed in
  a workbook that opened *without* macros is the user's own, so the first code that reaches
  the workbook enables the pane's macros (and its VBA functions in formulas). A workbook that
  opened with macros keeps them disabled until Enable Content (the editor shows the button).
  Only the user starts a run (F5, Run, the Macros dialog, buttons): no MCP tool or Paparan AI
  action reaches the editor or runs code.
- **Command line:** `kantor sheets macros add BOOK.xlsm Module.bas|Class.cls` puts an exported
  VBA module into the project the same way.

Not done: a form designer (forms' code only), breakpoints, stepping and the Immediate and
Locals windows, IntelliSense, Tools ▸ References, project passwords, `Option Explicit`
checks before a run (undeclared variables fail when they run), JavaScript syntax checks before
a run.

### Headless: `kantor sheets macros run`

`kantor sheets macros run FILE --macro NAME [--out OUT.xlsm] [--input V]... [--events]
[--allow-path PATH]... [--form-answers F.json]` runs a macro (a VBA `Sub`, or a script's
function: `Report.go`) and saves the workbook (in place
unless `--out`). With `--events`, `Workbook_Open` runs first and `Workbook_BeforeSave` (which may
cancel the save) and `Workbook_AfterSave` around the save, through one engine, so module
variables `Workbook_Open` sets are there for the macro. `--allow-path` names files or folders
macros may open or save (`Workbooks.Open`, `SaveAs`); nothing else is allowed headless. `MsgBox`
prints to standard error and answers OK (or Yes); `InputBox` takes the `--input` values in order
and fails clearly when none is left; a UserForm takes its steps from `--form-answers` (see
UserForms), in `Workbook_Open` too;
`Debug.Print` goes to standard error. Running the file's macros trusts it, so its VBA functions
work in its formulas too. On a run-time error nothing is saved (exit status 1).

### AI tools don't run macros

The MCP tool `sheets_macros` and Paparan AI's `macros` tool read macros (VBA modules and
scripts); neither runs them, and
no AI tool can press Enable Content or Run. A macro is code that came with the file: running it
is the user's decision. The tools' descriptions say so, so the assistant explains a macro or
redoes its work with formulas and the sheets editing tools instead.

## JavaScript and Python macros

Scripts are modules of JavaScript (or Python) kept in the workbook, run on the same object
model as VBA: the same `Application`, `Range`, `Worksheet`… objects, the same Enable Content,
the same macro thread, busy pane and Stop, the same `MsgBox`/`InputBox` dialogs, and one run is
one undo step. A module's **top-level functions without parameters** are its macros (as a VBA
`Sub` without parameters is): the Macros dialog lists them under the module with the VBA
modules, Run starts one, and `kantor sheets macros run FILE --macro Report.go` runs one
headless. A function whose name starts with `_` is private (not listed). A macro button can
name one (`Report.go`). Events don't call scripts (only VBA handlers); UDFs are VBA only.

### Where scripts live

In the workbook, in one **custom XML part** (`customXml/itemN.xml` with its
`customXml/itemPropsN.xml`, linked from the workbook part by the standard `customXml`
relationship):

```xml
<scripts xmlns="https://kantor.paparanai.id/2026/scripts">
  <script name="Report" language="javascript">function go() { … }</script>
</scripts>
```

Why a custom XML part: it is the place OOXML gives an application for its own data. Excel
keeps custom XML parts through its own saves (they are its `Workbook.CustomXMLParts`) and
ignores what it doesn't know, so the scripts travel with the file through Excel, in `.xlsx`
and `.xlsm` alike; a part of an unknown relationship type would be dropped by Excel's next save.
The part is ordinary XML (content type `application/xml`, the properties part
`customXmlProperties+xml`), so the package stays valid (`package_problems` checks it in the
tests). Carriage returns are kept as `&#13;`.

- `sheets_engine::scripts`: `Script { name, language, source }`, the XML.
- `Workbook::scripts()` reads them from the file the workbook was opened from (or the list set
  since); `Workbook::set_scripts` replaces them (not an undo step; the workbook becomes
  unsaved); a save writes the set list in place of the old part, or carries the original's part
  untouched (`preserve::with_scripts`, `read_scripts`).
- A workbook with scripts opens with macros disabled, as one with VBA does (the bar, Enable
  Content). Running a script trusts the file the same way.
- There is no editor yet: `kantor sheets macros add FILE report.js [--module Report]` stores a
  file as a module (replacing one of that name), `kantor sheets macros remove FILE --module
  Report` takes it out, `kantor sheets macros FILE` lists modules and functions (scripts after
  the VBA modules), `--module Report` prints its code. The MCP tool `sheets_macros` and Paparan
  AI's `macros` tool list and show scripts too, and don't run them.

### The binding (`kantor_script::binding`, `kantor_script_js`)

JavaScript separates reading a property (`r.value`) from calling a method (`r.clear()`), and a
property access can't know whether a call follows. Naming a member therefore:

- gives a **method** (a function that runs it with the call's arguments) when the host lists it
  as one (`Host::is_method`: `sheets_script::scripts::METHODS`, members that act or take
  optional arguments that change what they give: `Clear`, `Select`, `Copy`, `Offset`,
  `Resize`, `Address`, `Add`, `Find`…, and every `WorksheetFunction` member), or when reading
  it without arguments fails for want of them (errors 449, 450: `ws.Range`, `End`, `Item`);
- else reads the **property** right away (`r.value`, `ws.name`, `ws.cells`).

So properties are read and assigned as properties, methods are called, and objects are called
for their default member: `Worksheets("Data")`, `ws.cells(2, 1)`, `Range("A1:B2").cells(1, 1)`.
A method's function also turns into its value where a primitive is needed (`"At " +
r.address`), and an object turns into its default member's value (`Range("A1") + 1`).

Member names are case-insensitive, as in VBA: `Range`, `range` and `RANGE` are one member. The
idiomatic JavaScript spelling is camelCase (`ws.range("A1").numberFormat`); VBA's PascalCase
works too.

| | VBA | JavaScript |
|---|---|---|
| Property | `x = r.Value`, `r.Value = 5` | `x = r.value`, `r.value = 5` |
| Default member | `Range("A1") = 5` | `Range("A1").value = 5` |
| Method | `r.ClearContents` | `r.clearContents()` |
| Method with arguments | `r.Offset(1, 0)` | `r.offset(1, 0)` |
| Collection item | `Worksheets("Data")`, `ws.Cells(2, 1)` | the same |
| Named arguments | `r.Copy Destination:=ws.Range("D1")` | `r.copy({ destination: ws.range("D1") })` |
| Omitted argument | `r.Find("x", , xlValues)` | `r.find("x", undefined, xlValues)` |
| `For Each` | `For Each ws In Worksheets` | `for (const ws of Worksheets)` |
| `Debug.Print` | `Debug.Print x` | `console.log(x)` or `print(x)` |
| Constants | `xlUp`, `vbYesNo` | the same |
| Errors | `On Error`, `Err.Number` | `try { } catch (e) { e.number }` |

Values:

| Workbook (`Value`) | JavaScript |
|---|---|
| Empty cell, `Null`, `Nothing` | `null` (an omitted argument is `undefined`) |
| Numbers (`Long`, `Double`, `Currency`…) | `number` (a whole number goes back as `Long`) |
| `String`, `Boolean` | `string`, `boolean` |
| `Date` | `Date`, local time (and back) |
| Error value (`#N/A`) | `CellError` (`e.code` 2042; `String(e)` is `#N/A`); `CVErr(2042)` makes one |
| 2-D array (`Range.Value`) | array of rows: `r.value[0][1]`; writing an array of arrays fills a range |
| 1-D array | array |
| Object | a proxy (`typeof` is `function`, it can be called); `TypeName(x)` gives `Range` |
| `LongLong` beyond 2^53 | `number`; a `BigInt` goes in as `LongLong` |

Globals: `Application`, `ActiveWorkbook`, `ActiveSheet`, `ActiveCell`, `ActiveChart`,
`Selection`, `ThisWorkbook`, `Workbooks`, `Worksheets`, `Sheets`, `Charts`, `Range`, `Cells`,
`Rows`, `Columns`, `WorksheetFunction`, `Evaluate`, `Intersect`, `Union` (each also in
camelCase: `activeSheet`), read when used, so `ActiveSheet` is always the active sheet. Any
other name the script doesn't define is looked up among the object model's constants (`xlUp`,
`xlValues`, `msoShapeRectangle`), VBA's `vb…` constants, and the `Application`'s members
(through a proxy as the global object's prototype). `MsgBox`/`msgBox(prompt, buttons, title)`
returns the button (`vbYes`…), `InputBox`/`inputBox(prompt, title, default)` returns the text or
`null`; `console.log`/`print` write to the Macros dialog's output (the Immediate window).

Errors: a member that fails throws a `KantorError` (`e.number` 1004, `e.description`), which
`try`/`catch` can catch. An uncaught one ends the run with Excel's dialog: "Run-time error
'1004':" and "in Report.go, line 12". JavaScript's own errors (`TypeError`) show as "Run-time
error:" with their message; a syntax error as "Compile error:".

Running: each run starts a fresh QuickJS runtime (no state is kept between runs, unlike VBA's
module variables), evaluates every JavaScript module of the workbook as a classic script (the
target's module last, so its definitions win; functions are shared by all modules, like Public
Subs), then calls the function. Sandbox: QuickJS has no file, network or process access
unless the embedder adds it, and none is added; memory is capped (1 GiB) and so is the stack
(512 KiB). Stop sets the run's cancel token: QuickJS's interrupt handler checks it (so a loop
that calls nothing stops too), and every call into the object model checks it and the host.
`async` functions aren't awaited (there is no event loop).

Example, side by side:

```vba
Sub Report()
    Dim ws As Worksheet, r As Long, total As Double
    Set ws = ActiveSheet
    For r = 2 To 5
        total = total + ws.Cells(r, 3).Value
    Next r
    ws.Range("E1").Value = "Total"
    ws.Range("F1").Value = total
    ws.Range("E1:F1").Font.Bold = True
    ws.Range("F1").NumberFormat = "#,##0.00"
    If MsgBox("Add a chart?", vbYesNo) = vbYes Then
        With ws.ChartObjects.Add(Left:=200, Top:=20, Width:=360, Height:=216).Chart
            .SetSourceData Source:=ws.Range("A1:C5")
            .ChartType = xlLine
        End With
    End If
    Debug.Print "total", total
End Sub
```

```javascript
function report() {
  const ws = ActiveSheet;
  let total = 0;
  for (let r = 2; r <= 5; r++) total += ws.cells(r, 3).value;
  ws.range("E1").value = "Total";
  ws.range("F1").value = total;
  ws.range("E1:F1").font.bold = true;
  ws.range("F1").numberFormat = "#,##0.00";
  if (msgBox("Add a chart?", vbYesNo) === vbYes) {
    const ch = ws.chartObjects.add({ left: 200, top: 20, width: 360, height: 216 }).chart;
    ch.setSourceData(ws.range("A1:C5"));
    ch.chartType = xlLine;
  }
  console.log("total", total);
}
```

And the same in Python, as `kantor_script_py` (not built, below) runs it:

```python
def report():
    ws = ActiveSheet
    total = sum(ws.Cells(r, 3).Value for r in range(2, 6))
    ws.Range("E1").Value = "Total"
    ws.Range("F1").Value = total
    ws.Range("E1:F1").Font.Bold = True
    ws.Range("F1").number_format = "#,##0.00"
    if MsgBox("Add a chart?", vbYesNo) == vbYes:
        ch = ws.ChartObjects.Add(Left=200, Top=20, Width=360, Height=216).Chart
        ch.SetSourceData(ws.Range("A1:C5"))
        ch.ChartType = xlLine
    print("total", total)
```

### Python: built, measured, left out

`kantor_script_py` (branch `t3/sheets-python-wip`, not merged, never built in CI) binds
RustPython 0.6 the same way: `_Obj` wrappers over object handles (`__getattr__`,
`__setattr__`, `__call__`/`__getitem__` for the default member, `__iter__`, `len()` is `Count`),
keyword arguments for named ones, underscores dropped so `r.number_format` is `NumberFormat`,
globals as live objects (`ActiveSheet` looked up on each use; `Range(...)` and `Cells(...)` call
the global), the constants defined up front (`Host::constants`), `Date` (a `float` of Excel's
serial with `.year`…), `CellError`, `KantorError` with `number`, Stop through RustPython's user
signal channel (raises `KeyboardInterrupt`, which `except Exception` doesn't catch), and no
`host_env` (no `os`, `socket`, `subprocess`, `open`). It was left out because a size probe
(a hello-world binary built release, fat LTO, one codegen unit, stripped, as Kantor's release
profile; 2026-11-27, branch `t3/size-probe-js-py`) measured:

| Binary | Linux x86_64 | macOS arm64 | Windows x64 |
|---|---|---|---|
| Hello world | 0.29 MB | 0.30 MB | 0.12 MB |
| + rquickjs 0.14 (QuickJS-NG) | 1.49 MB (**+1.2 MB**) | 1.32 MB (**+1.0 MB**) | 1.21 MB (**+1.1 MB**) |
| + rustpython-vm 0.6 (compiler, no stdlib, no `host_env`) | 16.5 MB (**+16.2 MB**) | 13.9 MB (**+13.6 MB**) | doesn't build |

That is over the 15 MB this was allowed on Linux and a quarter of Kantor's 60 MB install
budget, without
even `math` (which needs `rustpython-stdlib`, more again). RustPython 0.6.0 also doesn't build
as published: `rustpython-codegen` fails against the newest `rustpython-ruff_python_ast` (pin
`=0.16.5`), and `rustpython-host_env` fails on Windows without its `ctypes` feature. Python
scripts are therefore kept, listed and shown (`sheets_script::scripts::can_run` is false for
them; running one says "Running Python macros is not supported in Kantor"). Ways forward: a
smaller interpreter, or CPython as a separately downloaded add-on run out of process.

The design notes below are what the choice was made from.

### Choosing the engines

#### JavaScript: rquickjs vs boa

| | rquickjs (QuickJS-NG, C) | boa (pure Rust) |
|---|---|---|
| Size | QuickJS is a few C files; ~370 KiB of x86 code for hello world | Larger: the CLI is ~25 MB stripped, mostly ICU data; ~11 MB in a 2022 embedding test vs 4.6 MB for QuickJS |
| Build | Compiles bundled C with `cc`: needs a C compiler on each CI runner (present on all three). docs.rs builds the `-sys` crate in ~10–25 s | Pure Rust, no C toolchain; a large crate graph to compile |
| Windows | x86_64 MSVC bindings shipped and tested; marked "experimental" upstream | Same as any Rust crate |
| Speed | Fast interpreter; Boa's own benchmark page puts QuickJS ~5× ahead | ~1/5 of QuickJS, improving each release |
| Conformance | Nearly all of test262 (ES2025) | ~95–96% of test262 |
| Sandboxing | Memory limit, interrupt handler for timeouts, no I/O unless we add it | No I/O unless we add it; limits through the runtime API |

#### Python: RustPython vs embedded CPython

| | RustPython | CPython (python-build-standalone) |
|---|---|---|
| Size | ~18 MB release binary without the stdlib, ~25–30 MB with `freeze-stdlib` (2021 figures) | A full interpreter and stdlib, tens of MB unpacked: more than Kantor's whole 60 MB install budget allows |
| Startup | Interpreter in-process; freezing the stdlib avoids file reads | Fast; frozen startup modules help (~20%) |
| Packaging | A Rust crate in the same binary; nothing extra in the dmg, AppImage or installer | A separate runtime directory per platform, its own signing/notarisation on macOS, updates |
| Stdlib | Most of the pure-Python stdlib; C-extension packages (numpy, pandas) don't run | Everything, including pip packages, if we ship or download them |
| Compatibility | Targets CPython 3.15 syntax; slower than CPython | Reference implementation |
| Sandboxing | Feasible: leave `os`/`socket`/`subprocess` out of the frozen stdlib | Not possible in-process; would need a separate process with OS sandboxing |

#### Recommendation (2026-11-27, before the size probe)

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
- Not implemented: `Decimal` (mapped to Double), `LSet` between user-defined types,
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
