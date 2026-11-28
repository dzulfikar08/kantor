//! Kantor's scripting core.
//!
//! - [`object`]: the host-neutral object layer. [`Value`] is a Variant, [`Dispatch`] is an
//!   IDispatch-like object interface (case-insensitive member names, named arguments, a default
//!   member, enumeration), [`Host`] supplies the global names (`Range`, `Cells`, `ActiveSheet`),
//!   UI callbacks (`MsgBox`, `InputBox`), `Debug.Print` output and cancellation. Nothing in it is
//!   VBA-specific: a JavaScript or Python binding can wrap the same objects.
//! - [`vba`]: a VBA interpreter (lexer, parser, tree-walking runtime, the VBA standard library,
//!   `Collection` and `Scripting.Dictionary`). Input is module source text.
//! - [`forms`]: UserForms as a host shows them: the state of a shown form ([`forms::FormView`])
//!   and what the user did ([`forms::FormInput`]), exchanged through [`Host::form_event`].
//! - [`binding`]: what the JavaScript and Python bindings share: how a member name
//!   resolves to a property or a method, and the functions a module defines.
//! - [`date`]: OLE Automation dates (days since 1899-12-30), shared by every language binding.
//!
//! See `docs/scripting.md` for the design and how the Sheets object model plugs in.

pub mod binding;
pub mod date;
pub mod forms;
pub mod object;
pub mod vba;

pub use object::{
    Arg, Array, CancelToken, Dispatch, ErrorKind, Host, HostCall, Location, NullHost, ObjectRef,
    Record, ScriptError, ScriptMember, Value, find_arg,
};
