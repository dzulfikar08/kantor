//! One open document of any app, as the workspace sees it: tabs, splits,
//! the close prompt, the session and the status bar only use these methods.

use std::path::PathBuf;

use docs_ui::DocsView;
use gpui_kit::*;
use sheets_ui::{Format, HAlign, SheetsView};
use slides_ui::SlidesView;

use super::AppKind;

/// A document view: Sheets, Docs or Slides.
#[derive(Clone)]
pub(super) enum DocView {
    Sheets(Entity<SheetsView>),
    Docs(Entity<DocsView>),
    Slides(Entity<SlidesView>),
}

/// Formatting commands both apps understand (palette entries).
#[derive(Clone, Copy, Debug)]
pub(super) enum CommonFormat {
    Bold,
    Italic,
    Underline,
    AlignLeft,
    AlignCenter,
    AlignRight,
}

/// Call a method that every view has with the same signature.
macro_rules! both {
    ($self:expr, $cx:expr, |$v:ident| $body:expr) => {
        match $self {
            DocView::Sheets(e) => {
                let $v = e.read($cx);
                $body
            }
            DocView::Docs(e) => {
                let $v = e.read($cx);
                $body
            }
            DocView::Slides(e) => {
                let $v = e.read($cx);
                $body
            }
        }
    };
}

macro_rules! both_mut {
    ($self:expr, $window:expr, $cx:expr, |$v:ident, $w:ident, $c:ident| $body:expr) => {
        match $self {
            DocView::Sheets(e) => e.update($cx, |$v, $c| {
                let $w = $window;
                $body
            }),
            DocView::Docs(e) => e.update($cx, |$v, $c| {
                let $w = $window;
                $body
            }),
            DocView::Slides(e) => e.update($cx, |$v, $c| {
                let $w = $window;
                $body
            }),
        }
    };
}

impl DocView {
    /// Open `file` (or a new empty document) in the app for `kind`.
    pub fn new(
        kind: AppKind,
        file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Self> {
        Some(match kind {
            AppKind::Sheets => Self::Sheets(cx.new(|cx| SheetsView::new(file, window, cx))),
            AppKind::Docs => Self::Docs(cx.new(|cx| DocsView::new(file, window, cx))),
            AppKind::Slides => Self::Slides(cx.new(|cx| SlidesView::new(file, window, cx))),
        })
    }

    pub fn kind(&self) -> AppKind {
        match self {
            Self::Sheets(_) => AppKind::Sheets,
            Self::Docs(_) => AppKind::Docs,
            Self::Slides(_) => AppKind::Slides,
        }
    }

    pub fn any_view(&self) -> AnyView {
        match self {
            Self::Sheets(e) => e.clone().into(),
            Self::Docs(e) => e.clone().into(),
            Self::Slides(e) => e.clone().into(),
        }
    }

    pub fn doc_name(&self, cx: &App) -> String {
        both!(self, cx, |v| v.doc_name(cx))
    }

    /// Name the document shows while it has no file (`Untitled 2.xlsx`).
    pub fn set_untitled_name(&self, name: String, cx: &mut App) {
        match self {
            Self::Sheets(e) => e.update(cx, |v, cx| v.set_untitled_name(name, cx)),
            Self::Docs(e) => e.update(cx, |v, cx| v.set_untitled_name(name, cx)),
            Self::Slides(e) => e.update(cx, |v, cx| v.set_untitled_name(name, cx)),
        }
    }

    pub fn path(&self, cx: &App) -> Option<PathBuf> {
        both!(self, cx, |v| v.path(cx))
    }

    pub fn is_dirty(&self, cx: &App) -> bool {
        both!(self, cx, |v| v.is_dirty(cx))
    }

    pub fn title(&self, cx: &App) -> String {
        both!(self, cx, |v| v.title(cx))
    }

    pub fn status(&self, cx: &App) -> SharedString {
        both!(self, cx, |v| v.status())
    }

    /// Where the user is: the cell (`B7`), the page (`Page 2 of 5 · …`) or
    /// the slide (`Slide 3 of 12`).
    pub fn location(&self, cx: &App) -> String {
        match self {
            Self::Sheets(e) => e.read(cx).cell_ref(),
            Self::Docs(e) => e.read(cx).location(),
            Self::Slides(e) => e.read(cx).location(),
        }
    }

    /// Sum / Average / Count of a spreadsheet's selected numbers.
    pub fn selection_summary(&self, cx: &App) -> Option<String> {
        match self {
            Self::Sheets(e) => e.read(cx).selection_summary(cx),
            _ => None,
        }
    }

    /// A spreadsheet's calculation state, when worth showing (manual mode,
    /// calculating).
    pub fn calc_status(&self, cx: &App) -> Option<String> {
        match self {
            Self::Sheets(e) => e.read(cx).calc_status(cx),
            _ => None,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        both_mut!(self, window, cx, |v, w, c| v.focus(w, c))
    }

    pub fn save(&self, window: &mut Window, cx: &mut App) {
        both_mut!(self, window, cx, |v, w, c| v.save_doc(w, c))
    }

    pub fn save_as(&self, window: &mut Window, cx: &mut App) {
        both_mut!(self, window, cx, |v, w, c| v.save_doc_as(w, c))
    }

    /// Save before closing; resolves to whether the document is saved.
    pub fn save_for_close(&self, window: &mut Window, cx: &mut App) -> Task<bool> {
        both_mut!(self, window, cx, |v, w, c| v.save_for_close(w, c))
    }

    pub fn undo(&self, window: &mut Window, cx: &mut App) {
        both_mut!(self, window, cx, |v, w, c| v.undo_doc(w, c))
    }

    pub fn redo(&self, window: &mut Window, cx: &mut App) {
        both_mut!(self, window, cx, |v, w, c| v.redo_doc(w, c))
    }

    pub fn copy(&self, window: &mut Window, cx: &mut App) {
        both_mut!(self, window, cx, |v, w, c| v.copy_doc(w, c))
    }

    pub fn cut(&self, window: &mut Window, cx: &mut App) {
        both_mut!(self, window, cx, |v, w, c| v.cut_doc(w, c))
    }

    pub fn paste(&self, window: &mut Window, cx: &mut App) {
        both_mut!(self, window, cx, |v, w, c| v.paste_doc(w, c))
    }

    /// Open the Find bar (`replace`: with the replace row).
    pub fn find(&self, replace: bool, window: &mut Window, cx: &mut App) {
        both_mut!(self, window, cx, |v, w, c| v.open_find(replace, w, c))
    }

    pub fn format(&self, format: CommonFormat, window: &mut Window, cx: &mut App) {
        use docs_engine::{Align, Toggle};
        match self {
            Self::Sheets(e) => {
                let f = match format {
                    CommonFormat::Bold => Format::Bold,
                    CommonFormat::Italic => Format::Italic,
                    CommonFormat::Underline => Format::Underline,
                    CommonFormat::AlignLeft => Format::Align(HAlign::Left),
                    CommonFormat::AlignCenter => Format::Align(HAlign::Center),
                    CommonFormat::AlignRight => Format::Align(HAlign::Right),
                };
                e.update(cx, |v, cx| v.format_doc(f, window, cx));
            }
            Self::Docs(e) => e.update(cx, |v, cx| match format {
                CommonFormat::Bold => v.toggle_doc(Toggle::Bold, window, cx),
                CommonFormat::Italic => v.toggle_doc(Toggle::Italic, window, cx),
                CommonFormat::Underline => v.toggle_doc(Toggle::Underline, window, cx),
                CommonFormat::AlignLeft => v.set_align_doc(Align::Left, window, cx),
                CommonFormat::AlignCenter => v.set_align_doc(Align::Center, window, cx),
                CommonFormat::AlignRight => v.set_align_doc(Align::Right, window, cx),
            }),
            // Paragraph alignment isn't editable in Slides yet.
            Self::Slides(e) => e.update(cx, |v, cx| match format {
                CommonFormat::Bold => v.toggle_doc(Toggle::Bold, window, cx),
                CommonFormat::Italic => v.toggle_doc(Toggle::Italic, window, cx),
                CommonFormat::Underline => v.toggle_doc(Toggle::Underline, window, cx),
                _ => {}
            }),
        }
    }
}
