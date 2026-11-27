# Scripting: macros on one object layer

Kantor keeps the VBA projects of `.xlsm` files but couldn't run them. `crates/kantor_script`
is the language core for running macros: a host-neutral object layer and a VBA interpreter.
There is no Excel object model in it yet. A Sheets object model (`Range`, `Cells`,
`Worksheets`…) comes next, and JavaScript and Python are meant to bind to the same objects
later.

## Layers

```
 VBA source ─► kantor_script::vba ─┐
 (later) JS ─► rquickjs binding  ──┼─► kantor_script::object ◄── sheets object model (later)
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

## How the Sheets object model plugs in

The object model is a set of `Dispatch` types in a new crate (or a module of `sheets_engine`)
over the workbook model, plus a `Host`:

1. `Host::globals()` returns an `Application` object. Its `get` answers `Range`, `Cells`,
   `Rows`, `Columns`, `ActiveSheet`, `ActiveCell`, `Selection`, `ActiveWorkbook`,
   `Worksheets`, `Sheets`, `WorksheetFunction`, `Application` and `Evaluate` (the `[A1]`
   syntax arrives as `Evaluate`).
2. `Range` implements `get`/`put` for `Value`, `Value2`, `Formula`, `Text`, `NumberFormat`,
   `Offset`, `Resize`, `Cells`, `Row`, `Column`, `End(xlUp)`, `CurrentRegion`… and `""` as an
   alias of `Value` (with arguments, `""` is `Item`). Multi-cell `Value` is a 2-D
   `Value::Array` with lower bounds 1 (`Array::from_rows`/`rows`). `enumerate` yields the cells.
3. `Host::constant` maps `xl*` names to values (or the model calls
   `Project::register_constant` once). `Dim r As XlDirection` already works: unknown type
   names starting with `vb`/`xl`/`mso` are treated as Long enumerations.
4. `Host::msg_box`/`input_box` show Kantor dialogs; `debug_print` feeds an Immediate pane;
   `now` gives local time; `cancelled` returns the UI's `CancelToken`.
5. Writes go through Sheets' normal edit path so a macro is one undo step and recalculation
   runs at the end (or when the script reads a dependent cell). `ScreenUpdating` and
   `Calculation` become flags on the host.
6. UDFs: the formula engine calls `Project::call_function(name, args, host)` with ranges as
   `Range` objects (or values), on the UI thread, with a small step budget.

Events (`Workbook_Open`, `Worksheet_Change`) are procedures in document modules; the host
calls `Project::run("ThisWorkbook.Workbook_Open", …)` when it wants them to fire.

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
