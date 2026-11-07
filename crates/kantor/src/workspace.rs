//! The suite shell: one window that shows the Home screen or an app.
//!
//! Kantor is one binary for Docs, Sheets and Slides. A window starts on Home
//! (pick an app or open a file) unless it was launched with a file or an app flag.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::*;
use sheets_ui::SheetsView;

const LOGO_SVG: &[u8] = include_bytes!("../../../assets/icons/kantor.svg");

/// Which app a file or flag asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppKind {
    Docs,
    Sheets,
    Slides,
}

impl AppKind {
    /// Guess the app from a file extension.
    pub fn for_path(path: &std::path::Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "xlsx" | "xlsm" | "xls" | "ods" | "csv" => Some(Self::Sheets),
            "docx" | "doc" | "odt" | "rtf" => Some(Self::Docs),
            "pptx" | "ppt" | "odp" => Some(Self::Slides),
            _ => None,
        }
    }
}

/// One app card on the Home screen.
struct Card {
    id: &'static str,
    app: AppKind,
    title: &'static str,
    subtitle: &'static str,
    accent: Hsla,
    ready: bool,
}

enum Screen {
    Home,
    Sheets(Entity<SheetsView>),
}

pub struct Workspace {
    screen: Screen,
    logo: Arc<Image>,
    status: SharedString,
}

impl Workspace {
    pub fn new(
        app: Option<AppKind>,
        file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            screen: Screen::Home,
            logo: Arc::new(Image::from_bytes(ImageFormat::Svg, LOGO_SVG.to_vec())),
            status: SharedString::default(),
        };
        let app = app.or_else(|| file.as_deref().and_then(AppKind::for_path));
        match app {
            Some(app) => this.launch(app, file, window, cx),
            None => window.set_window_title("Kantor"),
        }
        this
    }

    fn launch(
        &mut self,
        app: AppKind,
        file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match app {
            AppKind::Sheets => {
                let view = cx.new(|cx| SheetsView::new(file, window, cx));
                let title = view.read(cx).title(cx);
                window.set_window_title(&title);
                self.screen = Screen::Sheets(view);
            }
            AppKind::Docs | AppKind::Slides => {
                let name = if app == AppKind::Docs {
                    "Docs"
                } else {
                    "Slides"
                };
                self.status = format!("{name} is not built yet — it's on the roadmap.").into();
                self.screen = Screen::Home;
            }
        }
        cx.notify();
    }

    fn open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open document".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| match AppKind::for_path(&path) {
                Some(app) => this.launch(app, Some(path), window, cx),
                None => {
                    this.status = format!("Kantor can't open {}", path.display()).into();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn render_card(&self, card: Card, cx: &mut Context<Self>) -> Stateful<Div> {
        let Card {
            id,
            app,
            title,
            subtitle,
            accent,
            ready,
        } = card;
        let theme = cx.theme();
        let (card_bg, border, muted) = (theme.secondary, theme.border, theme.muted_foreground);
        let view = cx.entity().downgrade();
        v_flex()
            .id(id)
            .w(px(200.))
            .p_4()
            .gap_2()
            .rounded(px(14.))
            .bg(card_bg)
            .border_1()
            .border_color(border)
            .child(div().w(px(36.)).h(px(6.)).rounded(px(3.)).bg(accent))
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().text_sm().text_color(muted).child(subtitle))
            .child({
                let button = Button::new((id, 1usize)).small();
                let button = if ready {
                    button.label("New").primary()
                } else {
                    button.label("Coming soon").ghost()
                };
                button.on_click(move |_, window, cx| {
                    let _ = view.update(cx, |this, cx| this.launch(app, None, window, cx));
                })
            })
    }

    fn render_home(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (bg, fg, muted, blue, green, orange) = (
            theme.background,
            theme.foreground,
            theme.muted_foreground,
            theme.blue,
            theme.green,
            theme.yellow,
        );
        let view = cx.entity().downgrade();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_6()
            .bg(bg)
            .text_color(fg)
            .child(img(self.logo.clone()).size(px(96.)))
            .child(
                v_flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::BOLD)
                            .child("Kantor"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child("Docs, Sheets and Slides — native, fast, yours."),
                    ),
            )
            .child(
                h_flex()
                    .gap_4()
                    .child(self.render_card(
                        Card {
                            id: "docs",
                            app: AppKind::Docs,
                            title: "Docs",
                            subtitle: "Write .docx documents",
                            accent: blue,
                            ready: false,
                        },
                        cx,
                    ))
                    .child(self.render_card(
                        Card {
                            id: "sheets",
                            app: AppKind::Sheets,
                            title: "Sheets",
                            subtitle: "Edit .xlsx spreadsheets",
                            accent: green,
                            ready: true,
                        },
                        cx,
                    ))
                    .child(self.render_card(
                        Card {
                            id: "slides",
                            app: AppKind::Slides,
                            title: "Slides",
                            subtitle: "Present .pptx decks",
                            accent: orange,
                            ready: false,
                        },
                        cx,
                    )),
            )
            .child(
                Button::new("open-file")
                    .label("Open file…")
                    .ghost()
                    .on_click(move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| this.open_file(window, cx));
                    }),
            )
            .child(
                div()
                    .h(px(20.))
                    .text_sm()
                    .text_color(muted)
                    .child(self.status.clone()),
            )
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.screen {
            Screen::Home => self.render_home(cx).into_any_element(),
            Screen::Sheets(view) => view.clone().into_any_element(),
        }
    }
}
