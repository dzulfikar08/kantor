//! Kantor Notes: a Markdown note editor with live preview.
//!
//! The `.md` text is the document ([`notes_engine`]). The editor is a drawn
//! element, not a text input: [`layout`] shapes each buffer line with
//! GPUI's text system, formatted with its markup hidden or, on the lines
//! the caret is on, *revealed* as typed; [`paint`] draws it; typing and
//! input methods come through [`EntityInputHandler`] ([`input`]); keys are
//! actions in the [`KEY_CONTEXT`] key context.
//!
//! Three modes ([`Mode`]): live preview (the default), source (every line
//! as typed) and reading (formatted, not editable).

extern crate gpui_kit as gpui;

mod find;
mod input;
pub mod layout;
mod paint;
mod toolbar;

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{t, tf};
use kantor_ui::{metrics::*, tokens};
use notes_engine::{Editor, Merge, Selection};

pub use layout::Mode;
use layout::{Body, ImageSizes, Laid, LineBox, Look, MarkerDeco};

/// Key context of the note editor.
pub const KEY_CONTEXT: &str = "NotesEditor";

/// Widest text column (unzoomed px): lines stay readable on wide windows.
const MAX_COLUMN: f32 = 760.;
/// Space around the text column.
const PAD_X: f32 = 32.;
const PAD_TOP: f32 = 28.;
/// Room under the last line, so the end of a note isn't stuck to the edge.
const PAD_BOTTOM: f32 = 160.;
/// Body text size at 100 %.
const BODY_SIZE: f32 = 15.;

actions!(
    notes,
    [
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        MoveWordLeft,
        MoveWordRight,
        SelectWordLeft,
        SelectWordRight,
        MoveLineStart,
        MoveLineEnd,
        SelectLineStart,
        SelectLineEnd,
        MoveDocStart,
        MoveDocEnd,
        SelectDocStart,
        SelectDocEnd,
        PageUp,
        PageDown,
        SelectPageUp,
        SelectPageDown,
        SelectAll,
        Collapse,
        Backspace,
        Delete,
        DeleteWordBack,
        DeleteWordForward,
        Newline,
        PlainNewline,
        Indent,
        Outdent,
        Copy,
        Cut,
        Paste,
        ToggleBold,
        ToggleItalic,
        ToggleStrike,
        ToggleCode,
        InsertLink,
        Heading0,
        Heading1,
        Heading2,
        Heading3,
        Undo,
        Redo,
        Save,
        SaveAs,
        ToggleReading,
        ToggleSource,
        ZoomIn,
        ZoomOut,
        ZoomReset,
    ]
);

pub fn init(cx: &mut App) {
    kantor_ui::find_bar::bind_app_keys(KEY_CONTEXT, cx);
    cx.bind_keys(key_bindings());
}

fn key_bindings() -> Vec<KeyBinding> {
    let c = Some(KEY_CONTEXT);
    let mut keys = vec![
        KeyBinding::new("left", MoveLeft, c),
        KeyBinding::new("right", MoveRight, c),
        KeyBinding::new("up", MoveUp, c),
        KeyBinding::new("down", MoveDown, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new(kantor_ui::keys::primary("ctrl-left"), MoveWordLeft, c),
        KeyBinding::new(kantor_ui::keys::primary("ctrl-right"), MoveWordRight, c),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-shift-left"),
            SelectWordLeft,
            c,
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-shift-right"),
            SelectWordRight,
            c,
        ),
        KeyBinding::new("home", MoveLineStart, c),
        KeyBinding::new("end", MoveLineEnd, c),
        KeyBinding::new("shift-home", SelectLineStart, c),
        KeyBinding::new("shift-end", SelectLineEnd, c),
        KeyBinding::new(kantor_ui::keys::primary("ctrl-home"), MoveDocStart, c),
        KeyBinding::new(kantor_ui::keys::primary("ctrl-end"), MoveDocEnd, c),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-shift-home"),
            SelectDocStart,
            c,
        ),
        KeyBinding::new(kantor_ui::keys::primary("ctrl-shift-end"), SelectDocEnd, c),
        KeyBinding::new("pageup", PageUp, c),
        KeyBinding::new("pagedown", PageDown, c),
        KeyBinding::new("shift-pageup", SelectPageUp, c),
        KeyBinding::new("shift-pagedown", SelectPageDown, c),
        KeyBinding::new("escape", Collapse, c),
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-backspace"),
            DeleteWordBack,
            c,
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-delete"),
            DeleteWordForward,
            c,
        ),
        KeyBinding::new("enter", Newline, c),
        KeyBinding::new("shift-enter", PlainNewline, c),
        KeyBinding::new("tab", Indent, c),
        KeyBinding::new("shift-tab", Outdent, c),
    ];
    macro_rules! primary {
        ($($key:literal => $action:expr),* $(,)?) => {
            $(keys.extend(kantor_ui::keys::primary_pair(concat!("ctrl-", $key), $action, c));)*
        };
    }
    primary!(
        "a" => SelectAll,
        "c" => Copy,
        "x" => Cut,
        "v" => Paste,
        "b" => ToggleBold,
        "i" => ToggleItalic,
        "shift-x" => ToggleStrike,
        "`" => ToggleCode,
        "k" => InsertLink,
        "0" => ZoomReset,
        "1" => Heading1,
        "2" => Heading2,
        "3" => Heading3,
        "z" => Undo,
        "shift-z" => Redo,
        "y" => Redo,
        "s" => Save,
        "shift-s" => SaveAs,
        "e" => ToggleReading,
        "=" => ZoomIn,
        "+" => ZoomIn,
        "shift-=" => ZoomIn,
        "-" => ZoomOut,
    );
    keys
}

/// What the note view asks the workspace for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotesEvent {
    /// Convert the note (it is saved first) and write the result.
    Export(Export),
    /// Open a linked file (another note, a document) in Kantor.
    Open(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Export {
    Word,
    Pdf,
    Slides,
}

impl EventEmitter<NotesEvent> for NotesView {}

/// Note facts, for the inspector.
pub struct NoteInfo {
    pub name: String,
    pub path: Option<PathBuf>,
    pub words: usize,
    pub chars: usize,
    pub lines: usize,
    pub headings: usize,
    pub links: usize,
    pub line_ending: &'static str,
}

/// A mouse drag selecting text: by characters, words or lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragUnit {
    Char,
    Word,
    Line,
}

pub struct NotesView {
    editor: Editor,
    path: Option<PathBuf>,
    untitled_name: Option<String>,
    disk_guard: Option<kantor_ui::disk_guard::DiskGuard>,
    /// The file couldn't be read as a note: shown empty, never saved over.
    broken: bool,
    mode: Mode,
    zoom: f32,
    focus: FocusHandle,
    status: SharedString,
    laid: Rc<Laid>,
    cache: HashMap<u64, Rc<LineBox>>,
    image_sizes: ImageSizes,
    images: HashMap<PathBuf, Option<Arc<Image>>>,
    /// Canvas bounds from the last paint.
    viewport: Rc<Cell<Bounds<Pixels>>>,
    scroll_y: Pixels,
    /// Scroll the caret into view at the next render.
    reveal_caret: bool,
    /// Where vertical moves aim (kept over a run of Up/Down).
    goal_x: Option<Pixels>,
    /// The input method's composition (buffer range).
    marked: Option<Range<usize>>,
    drag: Option<(DragUnit, Range<usize>)>,
    find: Entity<kantor_ui::find_bar::FindBar>,
    found: Rc<Vec<Range<usize>>>,
    found_current: Option<usize>,
    _find_sub: Subscription,
}

impl NotesView {
    /// A view of `path` (or a new empty note).
    pub fn new(path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (editor, status, broken) = match path.as_deref().map(std::fs::read) {
            None => (Editor::default(), String::new(), false),
            Some(Ok(bytes)) => match Editor::open(&bytes) {
                Ok(e) => (e, String::new(), false),
                Err(e) => (
                    Editor::default(),
                    tf("Can't open this note: {}", &[&e]),
                    true,
                ),
            },
            Some(Err(e)) => (
                Editor::default(),
                tf("Can't open this note: {}", &[&e]),
                true,
            ),
        };
        let mut this = Self::with_editor(editor, path, window, cx);
        this.status = status.into();
        this.broken = broken;
        this
    }

    /// A note restored from a recovery copy (unsaved). The copy must be
    /// UTF-8 (check with `notes_engine::Buffer::from_bytes`).
    pub fn new_recovered(
        bytes: Vec<u8>,
        original: Option<PathBuf>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The original's bytes are what "saved" means; the copy's are the edits.
        let mut editor = match original.as_deref().map(std::fs::read) {
            Some(Ok(b)) => Editor::open(&b).unwrap_or_default(),
            _ => Editor::default(),
        };
        let copy = Editor::open(&bytes).unwrap_or_default();
        let all = editor.buffer().len();
        let text = copy.text().to_owned();
        if text != editor.text() {
            editor.edit(vec![(0..all, text)], Some(Selection::caret(0)), Merge::No);
        }
        let mut this = Self::with_editor(editor, original.clone(), window, cx);
        if original.is_none() {
            this.untitled_name = Some(name.clone());
        }
        this.status = tf("Restored unsaved changes to {}", &[&name]).into();
        this
    }

    fn with_editor(
        editor: Editor,
        path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (find, find_sub) = find::new_bar(window, cx);
        Self {
            editor,
            path,
            untitled_name: None,
            disk_guard: None,
            broken: false,
            mode: Mode::Live,
            zoom: 1.0,
            focus: cx.focus_handle(),
            status: SharedString::default(),
            laid: Rc::default(),
            cache: HashMap::new(),
            image_sizes: HashMap::new(),
            images: HashMap::new(),
            viewport: Rc::default(),
            scroll_y: px(0.),
            reveal_caret: false,
            goal_x: None,
            marked: None,
            drag: None,
            find,
            found: Rc::default(),
            found_current: None,
            _find_sub: find_sub,
        }
    }

    // ── shell API (tabs, status bar, inspector, close prompt) ──────────────

    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    pub fn doc_name(&self, _: &App) -> String {
        match (&self.path, &self.untitled_name) {
            (Some(p), _) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            (None, Some(n)) => n.clone(),
            (None, None) => t("Untitled.md").to_owned(),
        }
    }

    pub fn set_untitled_name(&mut self, name: String, cx: &mut Context<Self>) {
        self.untitled_name = Some(name);
        cx.notify();
    }

    pub fn path(&self, _: &App) -> Option<PathBuf> {
        self.path.clone()
    }

    pub fn is_dirty(&self, _: &App) -> bool {
        self.editor.is_dirty()
    }

    pub fn revision(&self, _: &App) -> Option<u64> {
        Some(self.editor.revision())
    }

    /// The note as Save would write it (a recovery copy).
    pub fn recovery_bytes(&self, _: &App) -> Vec<u8> {
        self.editor.to_bytes()
    }

    pub fn can_autosave(&self, _: &App) -> bool {
        !self.broken
            && self.mode != Mode::Reading
            && self
                .path
                .as_ref()
                .is_some_and(|p| !self.disk_guard.as_ref().is_some_and(|g| g.changed(p)))
    }

    pub fn set_disk_guard(&mut self, guard: Option<kantor_ui::disk_guard::DiskGuard>) {
        self.disk_guard = guard;
    }

    pub fn title(&self, cx: &App) -> String {
        let dirty = if self.editor.is_dirty() { "• " } else { "" };
        format!("{dirty}{} — Kantor Notes", self.doc_name(cx))
    }

    pub fn status(&self) -> SharedString {
        self.status.clone()
    }

    /// `Line 4 · 120 words` for the status bar.
    pub fn location(&self) -> String {
        let buf = self.editor.buffer();
        let line = buf.line_of(self.editor.selection().head) + 1;
        let sel = self.editor.selected_text();
        let (words, _) = if sel.is_empty() {
            notes_engine::count(self.editor.text())
        } else {
            notes_engine::count(sel)
        };
        let words = if sel.is_empty() {
            tf("{} words", &[&words])
        } else {
            tf("{} words selected", &[&words])
        };
        format!("{} · {words}", tf("Line {}", &[&line]))
    }

    pub fn info(&self, cx: &App) -> NoteInfo {
        let (words, chars) = notes_engine::count(self.editor.text());
        let p = self.editor.parsed();
        NoteInfo {
            name: self.doc_name(cx),
            path: self.path.clone(),
            words,
            chars,
            lines: self.editor.buffer().line_count(),
            headings: p.headings.len(),
            links: p.links.len(),
            line_ending: match self.editor.buffer().eol() {
                notes_engine::Eol::Lf => "LF",
                notes_engine::Eol::CrLf => "CRLF",
            },
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    pub fn editor_focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.marked = None;
        self.reveal_caret = true;
        cx.notify();
    }

    pub fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.zoom = (self.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        cx.notify();
    }

    pub fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        cx.notify();
    }

    fn editable(&self) -> bool {
        self.mode != Mode::Reading && !self.broken
    }

    /// The note's folder (relative links and pictures).
    pub fn dir(&self) -> Option<&Path> {
        self.path.as_deref().and_then(Path::parent)
    }

    // ── agent / MCP ────────────────────────────────────────────────────────

    /// Replace the whole text (one undo step); the bytes not changed stay.
    pub fn agent_set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let all = self.editor.buffer().len();
        let text = self.editor.buffer().normalize(text);
        // Only the part that differs is replaced, so line endings and
        // untouched lines stay byte for byte.
        let old = self.editor.text();
        let pre = common_prefix(old, &text);
        let suf = common_suffix(&old[pre..], &text[pre..]);
        let edit = (pre..all - suf, text[pre..text.len() - suf].to_owned());
        self.editor.edit(vec![edit], None, Merge::No);
        self.edited(cx);
    }

    /// Save for the assistant: to the note's own file, or to `path`.
    pub fn agent_save(
        &mut self,
        path: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<String> {
        if self.broken {
            anyhow::bail!("this note couldn't be read, so it isn't saved over");
        }
        let target = match path {
            Some(p) => p.to_path_buf(),
            None => self
                .path
                .clone()
                .ok_or_else(|| anyhow::anyhow!("this note has no file yet: pass `path`"))?,
        };
        self.write_to(&target)?;
        self.path = Some(target);
        self.after_save(Ok(()), window, cx);
        Ok(self.status.to_string())
    }

    /// Replace `find` (every occurrence, or the first) with `with`.
    pub fn agent_replace(
        &mut self,
        find: &str,
        with: &str,
        all: bool,
        cx: &mut Context<Self>,
    ) -> usize {
        if find.is_empty() {
            return 0;
        }
        let with = self.editor.buffer().normalize(with);
        let mut edits: Vec<(Range<usize>, String)> = self
            .editor
            .text()
            .match_indices(find)
            .map(|(i, m)| (i..i + m.len(), with.clone()))
            .collect();
        if !all {
            edits.truncate(1);
        }
        let n = edits.len();
        if n > 0 {
            self.editor.edit(edits, None, Merge::No);
            self.edited(cx);
        }
        n
    }

    // ── editing ────────────────────────────────────────────────────────────

    /// Bold / italic from the palette: wrap the selection in `mark`.
    pub fn wrap_doc(&mut self, mark: &'static str, cx: &mut Context<Self>) {
        self.type_or_command(|e| e.toggle_wrap(mark), cx);
    }

    /// After a change: scroll to the caret, search again, redraw.
    fn edited(&mut self, cx: &mut Context<Self>) {
        self.reveal_caret = true;
        self.goal_x = None;
        self.refresh_find(cx);
        cx.notify();
    }

    fn moved(&mut self, cx: &mut Context<Self>) {
        self.reveal_caret = true;
        self.marked = None;
        cx.notify();
    }

    fn set_sel(&mut self, sel: Selection, cx: &mut Context<Self>) {
        self.editor.set_selection(sel);
        self.moved(cx);
    }

    fn move_to(&mut self, at: usize, select: bool, cx: &mut Context<Self>) {
        let sel = self.editor.selection();
        let anchor = if select { sel.anchor } else { at };
        self.set_sel(Selection::new(anchor, at), cx);
    }

    fn horizontal(&mut self, forward: bool, word: bool, select: bool, cx: &mut Context<Self>) {
        self.goal_x = None;
        let sel = self.editor.selection();
        let buf = self.editor.buffer();
        if !select && !sel.is_empty() && !word {
            let r = sel.range();
            let at = if forward { r.end } else { r.start };
            return self.move_to(at, false, cx);
        }
        let at = match (forward, word) {
            (true, false) => buf.next_grapheme(sel.head),
            (false, false) => buf.prev_grapheme(sel.head),
            (true, true) => buf.next_word(sel.head),
            (false, true) => buf.prev_word(sel.head),
        };
        self.move_to(at, select, cx);
    }

    /// The caret's place in the layout (document space, column-relative).
    fn caret_pos(&self) -> Option<layout::Caret> {
        let buf = self.editor.buffer();
        let head = self.editor.selection().head;
        let line = buf.line_of(head);
        if self.laid.lines.len() != buf.line_count() {
            return None;
        }
        self.laid.caret(buf, line, head)
    }

    fn vertical(&mut self, down: bool, select: bool, cx: &mut Context<Self>) {
        let Some(c) = self.caret_pos() else {
            return;
        };
        let x = *self.goal_x.get_or_insert(c.at.x);
        let y = if down {
            c.at.y + c.height + px(1.)
        } else {
            c.at.y - px(1.)
        };
        let buf = self.editor.buffer();
        let at = if y < px(0.) {
            0
        } else {
            self.laid.hit(point(x, y), buf)
        };
        let sel = self.editor.selection();
        let anchor = if select { sel.anchor } else { at };
        self.editor.set_selection(Selection::new(anchor, at));
        self.reveal_caret = true;
        self.marked = None;
        cx.notify();
    }

    fn line_edge(&mut self, end: bool, select: bool, cx: &mut Context<Self>) {
        self.goal_x = None;
        let buf = self.editor.buffer();
        let head = self.editor.selection().head;
        let line = buf.line_of(head);
        let lr = buf.line_range(line);
        let at = if end {
            lr.end
        } else {
            // Home: the text's start (after `- `, `> `, `# `), then the line's.
            let content = lr.start + notes_engine::prefix::line_prefix(buf.line(line)).len;
            let content = match self.editor.parsed().lines.get(line).map(|l| &l.kind) {
                Some(notes_engine::LineKind::Heading(_)) => {
                    let s = buf.line(line);
                    lr.start + s.len() - s.trim_start_matches(['#', ' ']).len()
                }
                _ => content,
            };
            if head == content { lr.start } else { content }
        };
        self.move_to(at, select, cx);
    }

    fn page(&mut self, down: bool, select: bool, cx: &mut Context<Self>) {
        let Some(c) = self.caret_pos() else {
            return;
        };
        let h = self.viewport.get().size.height.max(px(100.)) * 0.9;
        let y = if down { c.at.y + h } else { c.at.y - h };
        let buf = self.editor.buffer();
        let at = if y < px(0.) {
            0
        } else {
            self.laid.hit(point(c.at.x, y), buf)
        };
        self.scroll_y = (self.scroll_y + if down { h } else { -h }).max(px(0.));
        self.move_to(at, select, cx);
    }

    fn type_or_command(&mut self, f: impl FnOnce(&mut Editor), cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        self.marked = None;
        f(&mut self.editor);
        self.edited(cx);
    }

    pub fn undo_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        if self.editor.undo() {
            self.edited(cx);
        } else {
            self.status = t("Nothing to undo").into();
            cx.notify();
        }
    }

    pub fn redo_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        if self.editor.redo() {
            self.edited(cx);
        } else {
            self.status = t("Nothing to redo").into();
            cx.notify();
        }
    }

    pub fn copy_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let s = self.editor.selected_text();
        if !s.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(s.to_owned()));
        }
    }

    pub fn cut_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable() {
            return self.copy_doc(window, cx);
        }
        let s = self.editor.selected_text().to_owned();
        if !s.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(s));
            self.type_or_command(|e| e.insert("", Merge::No), cx);
        }
    }

    /// Paste text, or a picture: saved next to the note in `attachments/`
    /// and linked where the caret is.
    pub fn paste_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let image = item.entries().iter().find_map(|e| match e {
            ClipboardEntry::Image(img) => Some(img.clone()),
            _ => None,
        });
        if let Some(img) = image {
            self.paste_picture(&img.bytes, img.format.extension(), cx);
            return;
        }
        if let Some(text) = item.text() {
            self.type_or_command(|e| e.insert(&text, Merge::No), cx);
        }
    }

    fn paste_picture(&mut self, bytes: &[u8], ext: &str, cx: &mut Context<Self>) {
        let Some(dir) = self.dir().map(Path::to_path_buf) else {
            self.status = t("Save the note first: pictures are saved next to it").into();
            cx.notify();
            return;
        };
        match save_attachment(&dir, bytes, ext) {
            Ok(rel) => {
                let link = format!("![]({})", notes_engine::link_target(&rel));
                self.type_or_command(|e| e.insert(&link, Merge::No), cx);
                self.status = tf("Picture saved as {}", &[&rel]).into();
            }
            Err(e) => {
                self.status = tf("Couldn't save the picture: {}", &[&e]).into();
                cx.notify();
            }
        }
    }

    /// Pictures dropped on the note: copied into `attachments/` (unless
    /// they are in the note's folder already) and linked at the caret.
    /// Returns whether they were taken.
    pub fn drop_pictures(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) -> bool {
        if !self.editable() || paths.is_empty() {
            return false;
        }
        let Some(dir) = self.dir().map(Path::to_path_buf) else {
            self.status = t("Save the note first: pictures are saved next to it").into();
            cx.notify();
            return true;
        };
        let mut links = Vec::new();
        for p in paths {
            let rel = match p.strip_prefix(&dir) {
                Ok(r) => Ok(r.to_string_lossy().into_owned()),
                Err(_) => std::fs::read(p).map_err(anyhow::Error::from).and_then(|b| {
                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("png");
                    save_attachment(&dir, &b, ext)
                }),
            };
            match rel {
                Ok(rel) => links.push(format!("![]({})", notes_engine::link_target(&rel))),
                Err(e) => self.status = tf("Couldn't save the picture: {}", &[&e]).into(),
            }
        }
        if !links.is_empty() {
            let eol = self.editor.buffer().eol().as_str();
            let text = links.join(eol);
            self.type_or_command(|e| e.insert(&text, Merge::No), cx);
        }
        true
    }

    // ── saving ─────────────────────────────────────────────────────────────

    pub fn save_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.broken {
            self.status = t("This note couldn't be read, so it isn't saved over").into();
            cx.notify();
            return;
        }
        let Some(target) = self.path.clone() else {
            self.save_as_task(window, cx).detach();
            return;
        };
        if self.disk_guard.as_ref().is_some_and(|g| g.changed(&target)) {
            let view = cx.entity().downgrade();
            kantor_ui::disk_guard::confirm_overwrite(
                &self.doc_name(cx),
                window,
                cx,
                move |choice, window, cx| {
                    let _ = view.update(cx, |this, cx| match choice {
                        kantor_ui::disk_guard::Overwrite::Replace => {
                            this.disk_guard = None;
                            this.save_doc(window, cx);
                        }
                        kantor_ui::disk_guard::Overwrite::SaveAs => {
                            this.save_as_task(window, cx).detach()
                        }
                    });
                },
            );
            return;
        }
        let result = self.write_to(&target);
        self.after_save(result, window, cx);
    }

    fn write_to(&mut self, path: &Path) -> anyhow::Result<()> {
        kantor_core::fsx::write_atomic(path, &self.editor.to_bytes())?;
        self.editor.mark_saved();
        Ok(())
    }

    fn after_save(
        &mut self,
        result: anyhow::Result<()>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if result.is_ok() {
            self.disk_guard = None;
        }
        self.status = match result {
            Ok(()) => tf("Saved {}", &[&self.doc_name(cx)]).into(),
            Err(err) => tf("Save failed: {}", &[&format!("{err:#}")]).into(),
        };
        let title = self.title(cx);
        window.set_window_title(&title);
        cx.notify();
    }

    pub fn save_doc_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_as_task(window, cx).detach();
    }

    /// Save before the note closes; resolves to whether it is saved.
    pub fn save_for_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        if self.path.is_none() {
            return self.save_as_task(window, cx);
        }
        self.save_doc(window, cx);
        Task::ready(!self.editor.is_dirty())
    }

    fn save_as_task(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        let dir = self
            .dir()
            .map(Path::to_path_buf)
            .or_else(kantor_core::dirs::user_home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let name = self.doc_name(cx);
        let path = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = path.await else {
                return false;
            };
            if path.extension().is_none() {
                path.set_extension("md");
            }
            this.update_in(cx, |this, window, cx| {
                let result = this.write_to(&path);
                if result.is_ok() {
                    this.path = Some(path);
                    this.broken = false;
                }
                this.after_save(result, window, cx);
                !this.editor.is_dirty()
            })
            .unwrap_or(false)
        })
    }

    /// Autosave: write the note to its own file.
    pub fn autosave(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let Some(path) = self.path.clone() else {
            return Task::ready(Err(anyhow::anyhow!("not saved yet")));
        };
        let result = self.write_to(&path);
        let ok = result.is_ok();
        self.after_save(result, window, cx);
        Task::ready(if ok {
            Ok(())
        } else {
            Err(anyhow::anyhow!("{}", self.status))
        })
    }

    /// Print: as Save as PDF, then open in the system viewer (the workspace
    /// does the converting).
    pub fn export(&mut self, kind: Export, cx: &mut Context<Self>) {
        cx.emit(NotesEvent::Export(kind));
    }

    pub fn set_status(&mut self, status: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = status.into();
        cx.notify();
    }

    // ── links ──────────────────────────────────────────────────────────────

    /// Follow the link at `offset`: web addresses in the browser, notes and
    /// documents in Kantor, other files with the system.
    fn follow_link(&mut self, offset: usize, cx: &mut Context<Self>) -> bool {
        let Some(link) = self.editor.parsed().link_at(offset).cloned() else {
            return false;
        };
        let dest = link.dest.trim().to_owned();
        if link.kind == notes_engine::parse::LinkKind::Wiki {
            // Wiki links resolve in the note's folder (a folder of notes
            // and backlinks come later).
            if let Some(dir) = self.dir() {
                let name = dest.split(['#', '|']).next().unwrap_or("").trim();
                let p = if notes_engine::is_note_path(Path::new(name)) {
                    dir.join(name)
                } else {
                    dir.join(format!("{name}.md"))
                };
                if p.is_file() {
                    cx.emit(NotesEvent::Open(p));
                } else {
                    self.status = tf("No note named {} in this folder", &[&name]).into();
                    cx.notify();
                }
            }
            return true;
        }
        if let Some(anchor) = dest.strip_prefix('#') {
            // A heading in this note.
            let want = anchor.replace('-', " ").to_lowercase();
            let hit = self
                .editor
                .parsed()
                .headings
                .iter()
                .find(|h| h.text.to_lowercase() == want)
                .map(|h| h.line);
            if let Some(line) = hit {
                let at = self.editor.buffer().line_start(line);
                self.move_to(at, false, cx);
            }
            return true;
        }
        if dest.contains("://") || dest.starts_with("mailto:") {
            cx.open_url(&dest);
            return true;
        }
        let Some(path) = layout::resolve(self.dir(), dest.split('#').next().unwrap_or("")) else {
            return true;
        };
        if notes_engine::is_note_path(&path)
            || matches!(
                path.extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("docx" | "xlsx" | "pptx" | "pdf" | "csv")
            )
        {
            cx.emit(NotesEvent::Open(path));
        } else {
            cx.open_with_system(&path);
        }
        true
    }

    // ── mouse ──────────────────────────────────────────────────────────────

    /// Column origin in window coordinates (where document point 0,0 is).
    fn origin(&self) -> Point<Pixels> {
        let b = self.viewport.get();
        let (left, _) = column(b.size.width, self.zoom);
        point(b.origin.x + left, b.origin.y + px(PAD_TOP) - self.scroll_y)
    }

    fn doc_point(&self, p: Point<Pixels>) -> Point<Pixels> {
        let o = self.origin();
        point(p.x - o.x, p.y - o.y)
    }

    /// The task check box under a window point: its line.
    fn task_box_at(&self, p: Point<Pixels>) -> Option<usize> {
        let d = self.doc_point(p);
        if self.laid.lines.is_empty() || d.y < px(0.) || d.y >= self.laid.height() {
            return None;
        }
        let i = self.laid.line_at_y(d.y);
        let b = &self.laid.lines[i];
        let Body::Text { deco, .. } = &b.body else {
            return None;
        };
        let (MarkerDeco::Task { .. }, slot) = deco.marker.as_ref()? else {
            return None;
        };
        let w = self.look_size() * 1.6;
        (d.x >= *slot - px(4.) && d.x <= *slot + w).then_some(i)
    }

    fn look_size(&self) -> Pixels {
        px(BODY_SIZE * self.zoom)
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if let Some(line) = self.task_box_at(ev.position) {
            if !self.broken {
                self.editor.toggle_task(line);
                self.edited(cx);
                self.reveal_caret = false;
            }
            return;
        }
        let buf = self.editor.buffer();
        let at = self.laid.hit(self.doc_point(ev.position), buf);
        let follow = self.mode == Mode::Reading || ev.modifiers.secondary(cx);
        if ev.click_count == 1 && follow && self.follow_link(at, cx) {
            return;
        }
        if self.mode == Mode::Reading {
            return;
        }
        self.goal_x = None;
        self.marked = None;
        let buf = self.editor.buffer();
        let (unit, range) = match ev.click_count {
            2 => (DragUnit::Word, buf.word_at(at)),
            n if n >= 3 => {
                let l = buf.line_of(at);
                (DragUnit::Line, buf.line_full_range(l))
            }
            _ => (DragUnit::Char, at..at),
        };
        let sel = if ev.modifiers.shift {
            Selection::new(self.editor.selection().anchor, at)
        } else {
            Selection::new(range.start, range.end)
        };
        self.drag = Some((unit, range));
        self.editor.set_selection(sel);
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((unit, origin)) = self.drag.clone() else {
            return;
        };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let buf = self.editor.buffer();
        let at = self.laid.hit(self.doc_point(ev.position), buf);
        let r = match unit {
            DragUnit::Char => at..at,
            DragUnit::Word => buf.word_at(at),
            DragUnit::Line => buf.line_full_range(buf.line_of(at)),
        };
        let sel = if r.start < origin.start {
            Selection::new(origin.end, r.start)
        } else {
            Selection::new(origin.start, r.end.max(origin.end))
        };
        if sel != self.editor.selection() {
            self.editor.set_selection(sel);
            cx.notify();
        }
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = ev.delta.pixel_delta(window.line_height());
        if ev.modifiers.control || ev.modifiers.platform {
            let dy = f32::from(delta.y);
            if dy != 0.0 {
                self.zoom_by(if dy > 0.0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP }, cx);
            }
            return;
        }
        self.scroll_y -= delta.y;
        self.clamp_scroll();
        cx.notify();
    }

    fn max_scroll(&self) -> Pixels {
        let view_h = self.viewport.get().size.height;
        (self.laid.height() + px(PAD_TOP + PAD_BOTTOM) - view_h).max(px(0.))
    }

    fn clamp_scroll(&mut self) {
        self.scroll_y = self.scroll_y.clamp(px(0.), self.max_scroll());
    }

    fn scroll_to_caret(&mut self) {
        let view_h = self.viewport.get().size.height;
        if view_h <= px(0.) {
            return;
        }
        let Some(c) = self.caret_pos() else {
            return;
        };
        let margin = self.look_size() * 2.;
        let top = c.at.y + px(PAD_TOP);
        let bottom = top + c.height;
        if top - margin < self.scroll_y {
            self.scroll_y = top - margin;
        } else if bottom + margin > self.scroll_y + view_h {
            self.scroll_y = bottom + margin - view_h;
        }
        self.clamp_scroll();
    }

    // ── layout ─────────────────────────────────────────────────────────────

    fn look(&self, cx: &App) -> Look {
        let t = tokens(cx);
        let theme = cx.theme();
        Look {
            font: theme.font_family.clone(),
            mono: theme.mono_font_family.clone(),
            size: self.look_size(),
            fg: t.fg,
            muted: t.fg_muted,
            dim: t.fg_dim,
            accent: t.accent,
            code_bg: t.surface,
            border: t.border,
        }
    }

    /// Lines shown as typed: the selection's, each with its group.
    fn revealed_lines(&self) -> Range<usize> {
        let buf = self.editor.buffer();
        let r = self.editor.selection().range();
        let p = self.editor.parsed();
        let a = buf.line_of(r.start);
        let b = buf.line_of(r.end);
        let start = p.lines.get(a).map_or(a, |l| l.group.start.min(a));
        let end = p.lines.get(b).map_or(b + 1, |l| l.group.end.max(b + 1));
        start..end
    }

    fn relayout(&mut self, window: &mut Window, cx: &App) {
        let look = self.look(cx);
        let (_, width) = column(self.viewport.get().size.width, self.zoom);
        let revealed = self.revealed_lines();
        let is_revealed = move |l: usize| revealed.contains(&l);
        let dir = self
            .path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let input = layout::Input {
            buf: self.editor.buffer(),
            parsed: self.editor.parsed(),
            look: &look,
            mode: self.mode,
            revealed: &is_revealed,
            width,
            dir: dir.as_deref(),
            image_sizes: &mut self.image_sizes,
        };
        self.laid = Rc::new(layout::layout(window.text_system(), input, &mut self.cache));
    }

    /// Decoded pictures on screen.
    fn visible_images(
        &mut self,
        lines: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> HashMap<PathBuf, Arc<RenderImage>> {
        let mut out = HashMap::new();
        let laid = self.laid.clone();
        for b in &laid.lines[lines] {
            let img = match &b.body {
                Body::Image(i) => i,
                Body::Text { image: Some(i), .. } => i,
                _ => continue,
            };
            let entry = self.images.entry(img.path.clone()).or_insert_with(|| {
                let format = image_format(&img.path)?;
                let bytes = std::fs::read(&img.path).ok()?;
                Some(Arc::new(Image::from_bytes(format, bytes)))
            });
            if let Some(image) = entry.clone()
                && let Some(data) = image.use_render_image(window, cx)
            {
                out.insert(img.path.clone(), data);
            }
        }
        out
    }
}

/// Where the text column goes in a pane `view_w` wide: its left edge and
/// its width.
fn column(view_w: Pixels, zoom: f32) -> (Pixels, Pixels) {
    let view_w = if view_w <= px(0.) { px(900.) } else { view_w };
    let w = (view_w - px(PAD_X * 2.))
        .min(px(MAX_COLUMN * zoom))
        .max(px(120.));
    let left = ((view_w - w) / 2.).max(px(PAD_X.min(f32::from(view_w) / 8.)));
    (left, w)
}

fn image_format(path: &Path) -> Option<ImageFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "gif" => ImageFormat::Gif,
        "webp" => ImageFormat::Webp,
        "svg" => ImageFormat::Svg,
        "bmp" => ImageFormat::Bmp,
        "tif" | "tiff" => ImageFormat::Tiff,
        "ico" => ImageFormat::Ico,
        _ => return None,
    })
}

/// Write a picture into `dir/attachments/`; returns its path relative to `dir`.
fn save_attachment(dir: &Path, bytes: &[u8], ext: &str) -> anyhow::Result<String> {
    let folder = dir.join("attachments");
    std::fs::create_dir_all(&folder)?;
    let stamp = chrono_stamp();
    let name = notes_engine::attachment_name(&folder, &stamp, ext);
    kantor_core::fsx::write_atomic(&folder.join(&name), bytes)?;
    Ok(format!("attachments/{name}"))
}

/// `YYYYMMDDhhmmss` in UTC (Obsidian's pasted-picture pattern).
fn chrono_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn common_prefix(a: &str, b: &str) -> usize {
    let mut n = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    while !a.is_char_boundary(n) || !b.is_char_boundary(n) {
        n -= 1;
    }
    n
}

fn common_suffix(a: &str, b: &str) -> usize {
    let mut n = a
        .bytes()
        .rev()
        .zip(b.bytes().rev())
        .take_while(|(x, y)| x == y)
        .count();
    while !a.is_char_boundary(a.len() - n) || !b.is_char_boundary(b.len() - n) {
        n -= 1;
    }
    n
}

macro_rules! listeners {
    ($el:expr, $cx:expr, [$($action:ident => |$this:ident, $window:ident, $c:ident| $body:expr),* $(,)?]) => {
        $el$(.on_action($cx.listener(|$this: &mut NotesView, _: &$action, $window: &mut Window, $c: &mut Context<NotesView>| { $body; })))*
    };
}

impl Render for NotesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens(cx);
        self.relayout(window, cx);
        if self.reveal_caret && self.viewport.get().size.height > px(0.) {
            self.reveal_caret = false;
            self.scroll_to_caret();
        }
        self.clamp_scroll();
        let view_h = self.viewport.get().size.height.max(px(600.));
        let y0 = self.scroll_y - px(PAD_TOP);
        let visible = self.laid.visible(y0, y0 + view_h);
        let images = self.visible_images(visible.clone(), window, cx);
        let frame = paint::Frame::new(self, visible, images, window, cx);
        let viewport = self.viewport.clone();
        let entity = cx.entity().downgrade();
        let canvas = canvas(
            move |bounds, _, cx| {
                let old = viewport.replace(bounds);
                // First paint or a resize: lay out again at the real width.
                if old.size != bounds.size {
                    let _ = entity.update(cx, |_, cx| cx.notify());
                }
            },
            move |bounds, _, window, cx| paint::paint(&frame, bounds, window, cx),
        )
        .size_full();

        let area = div()
            .id("notes-editor")
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .cursor(if self.mode == Mode::Reading {
                CursorStyle::Arrow
            } else {
                CursorStyle::IBeam
            })
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.drag = None),
            )
            .on_scroll(cx.listener(Self::on_scroll))
            .child(canvas)
            .when(self.find.read(cx).is_open(), |el| {
                el.child(self.find.clone())
            });

        let root = v_flex()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(t.bg)
            .text_color(t.fg)
            .text_size(FONT_BODY)
            .child(toolbar::render(self, cx))
            .child(area);
        let root = listeners!(root, cx, [
            MoveLeft => |this, _w, cx| this.horizontal(false, false, false, cx),
            MoveRight => |this, _w, cx| this.horizontal(true, false, false, cx),
            SelectLeft => |this, _w, cx| this.horizontal(false, false, true, cx),
            SelectRight => |this, _w, cx| this.horizontal(true, false, true, cx),
            MoveWordLeft => |this, _w, cx| this.horizontal(false, true, false, cx),
            MoveWordRight => |this, _w, cx| this.horizontal(true, true, false, cx),
            SelectWordLeft => |this, _w, cx| this.horizontal(false, true, true, cx),
            SelectWordRight => |this, _w, cx| this.horizontal(true, true, true, cx),
            MoveUp => |this, _w, cx| this.vertical(false, false, cx),
            MoveDown => |this, _w, cx| this.vertical(true, false, cx),
            SelectUp => |this, _w, cx| this.vertical(false, true, cx),
            SelectDown => |this, _w, cx| this.vertical(true, true, cx),
            MoveLineStart => |this, _w, cx| this.line_edge(false, false, cx),
            MoveLineEnd => |this, _w, cx| this.line_edge(true, false, cx),
            SelectLineStart => |this, _w, cx| this.line_edge(false, true, cx),
            SelectLineEnd => |this, _w, cx| this.line_edge(true, true, cx),
            MoveDocStart => |this, _w, cx| this.move_to(0, false, cx),
            MoveDocEnd => |this, _w, cx| { let e = this.editor.buffer().len(); this.move_to(e, false, cx) },
            SelectDocStart => |this, _w, cx| this.move_to(0, true, cx),
            SelectDocEnd => |this, _w, cx| { let e = this.editor.buffer().len(); this.move_to(e, true, cx) },
            PageUp => |this, _w, cx| this.page(false, false, cx),
            PageDown => |this, _w, cx| this.page(true, false, cx),
            SelectPageUp => |this, _w, cx| this.page(false, true, cx),
            SelectPageDown => |this, _w, cx| this.page(true, true, cx),
            SelectAll => |this, _w, cx| { this.editor.select_all(); this.moved(cx) },
            Collapse => |this, _w, cx| {
                let h = this.editor.selection().head;
                this.move_to(h, false, cx)
            },
            Backspace => |this, _w, cx| this.type_or_command(Editor::backspace, cx),
            Delete => |this, _w, cx| this.type_or_command(Editor::delete, cx),
            DeleteWordBack => |this, _w, cx| this.type_or_command(Editor::delete_word_back, cx),
            DeleteWordForward => |this, _w, cx| this.type_or_command(Editor::delete_word_forward, cx),
            Newline => |this, _w, cx| this.type_or_command(Editor::newline, cx),
            PlainNewline => |this, _w, cx| this.type_or_command(Editor::plain_newline, cx),
            Indent => |this, _w, cx| this.type_or_command(Editor::indent, cx),
            Outdent => |this, _w, cx| this.type_or_command(Editor::outdent, cx),
            Copy => |this, w, cx| this.copy_doc(w, cx),
            Cut => |this, w, cx| this.cut_doc(w, cx),
            Paste => |this, w, cx| this.paste_doc(w, cx),
            ToggleBold => |this, _w, cx| this.type_or_command(|e| e.toggle_wrap("**"), cx),
            ToggleItalic => |this, _w, cx| this.type_or_command(|e| e.toggle_wrap("*"), cx),
            ToggleStrike => |this, _w, cx| this.type_or_command(|e| e.toggle_wrap("~~"), cx),
            ToggleCode => |this, _w, cx| this.type_or_command(|e| e.toggle_wrap("`"), cx),
            InsertLink => |this, _w, cx| this.type_or_command(Editor::insert_link, cx),
            Heading0 => |this, _w, cx| this.type_or_command(|e| e.set_heading(0), cx),
            Heading1 => |this, _w, cx| this.type_or_command(|e| e.set_heading(1), cx),
            Heading2 => |this, _w, cx| this.type_or_command(|e| e.set_heading(2), cx),
            Heading3 => |this, _w, cx| this.type_or_command(|e| e.set_heading(3), cx),
            Undo => |this, w, cx| this.undo_doc(w, cx),
            Redo => |this, w, cx| this.redo_doc(w, cx),
            Save => |this, w, cx| this.save_doc(w, cx),
            SaveAs => |this, w, cx| this.save_doc_as(w, cx),
            ToggleReading => |this, _w, cx| {
                let m = if this.mode == Mode::Reading { Mode::Live } else { Mode::Reading };
                this.set_mode(m, cx)
            },
            ToggleSource => |this, _w, cx| {
                let m = if this.mode == Mode::Source { Mode::Live } else { Mode::Source };
                this.set_mode(m, cx)
            },
            ZoomIn => |this, _w, cx| this.zoom_by(ZOOM_STEP, cx),
            ZoomOut => |this, _w, cx| this.zoom_by(1.0 / ZOOM_STEP, cx),
            ZoomReset => |this, _w, cx| this.set_zoom(1.0, cx),
        ]);
        use kantor_ui::find_bar::{FindNext, FindPrev, OpenFind, OpenReplace};
        root.on_action(cx.listener(|this, _: &OpenFind, w, cx| this.open_find(false, w, cx)))
            .on_action(cx.listener(|this, _: &OpenReplace, w, cx| this.open_find(true, w, cx)))
            .on_action(cx.listener(|this, _: &FindNext, w, cx| this.find_next_doc(true, w, cx)))
            .on_action(cx.listener(|this, _: &FindPrev, w, cx| this.find_next_doc(false, w, cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn agent_edits_keep_untouched_bytes() {
        let old = "a\r\nb\r\nc";
        let new = "a\r\nB\r\nc";
        let p = common_prefix(old, new);
        let s = common_suffix(&old[p..], &new[p..]);
        assert_eq!((p, s), (3, 3));
    }

    #[std::prelude::v1::test]
    fn stamps_look_like_obsidians() {
        let s = chrono_stamp();
        assert_eq!(s.len(), 14);
        assert!(s.starts_with("20"));
    }

    #[std::prelude::v1::test]
    fn column_is_centred_and_bounded() {
        let (left, w) = column(px(2000.), 1.0);
        assert_eq!(w, px(MAX_COLUMN));
        assert_eq!(left, px((2000. - MAX_COLUMN) / 2.));
        let (_, w) = column(px(400.), 1.0);
        assert_eq!(w, px(400. - PAD_X * 2.));
    }
}
