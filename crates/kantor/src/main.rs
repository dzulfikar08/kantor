//! Kantor: one binary for Docs, Sheets and Slides.
//!
//! ```text
//! kantor [FILE]            open FILE in the app matching its extension
//! kantor --sheets [FILE]   start Sheets
//! ```

extern crate gpui_kit as gpui;

mod theme;

use std::path::PathBuf;

use anyhow::{Result, bail};
use gpui_kit::*;

actions!(kantor, [Quit]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Sheets,
}

struct Args {
    app: Mode,
    file: Option<PathBuf>,
}

const USAGE: &str = "usage: kantor [--sheets] [FILE]\n\
    \n\
    Kantor is a native office suite. Only Sheets exists so far; .docx/.pptx support\n\
    is planned (see plan.md).";

fn parse_args() -> Result<Args> {
    let mut app = None;
    let mut file = None;
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            Some("-V" | "--version") => {
                println!("kantor {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            Some("--sheets") => app = Some(Mode::Sheets),
            Some(s) if s.starts_with('-') => bail!("unknown option {s}\n\n{USAGE}"),
            _ if file.is_none() => file = Some(PathBuf::from(arg)),
            _ => bail!("only one file can be opened at a time\n\n{USAGE}"),
        }
    }
    let app = match (app, &file) {
        (Some(app), _) => app,
        (None, Some(f)) => match f
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
        {
            Some(e) if matches!(e.as_str(), "xlsx" | "xlsm") => Mode::Sheets,
            Some(e) if matches!(e.as_str(), "docx" | "pptx") => {
                bail!(".{e} files are not supported yet (Docs and Slides are planned)")
            }
            _ => Mode::Sheets,
        },
        (None, None) => Mode::Sheets,
    };
    Ok(Args { app, file })
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("kantor: {e:#}");
            std::process::exit(2);
        }
    };

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        theme::init(cx);
        sheets_ui::init(cx);

        cx.bind_keys([
            KeyBinding::new("ctrl-q", Quit, None),
            KeyBinding::new("super-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        let options = WindowOptions {
            app_id: Some("kantor-sheets".into()),
            titlebar: Some(TitlebarOptions {
                title: Some("Kantor Sheets".into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        match args.app {
            Mode::Sheets => {
                let file = args.file.clone();
                let opened = gpui_kit::open_window(options, cx, move |window, cx| {
                    let view = cx.new(|cx| sheets_ui::SheetsView::new(file, window, cx));
                    let title = view.read(cx).title(cx);
                    window.set_window_title(&title);
                    view
                });
                if let Err(err) = opened {
                    eprintln!("kantor: failed to open a window: {err:#}");
                    std::process::exit(1);
                }
            }
        }
        cx.activate(true);
    });
}
