//! Kantor: one binary for Docs, Sheets and Slides.
//!
//! ```text
//! kantor                    open the Home screen
//! kantor FILE               open FILE in the app matching its extension
//! kantor --sheets [FILE]    start Sheets (likewise --docs, --slides)
//! kantor mcp                MCP server on stdio for AI agents (see mcp/)
//! ```

extern crate gpui_kit as gpui;

mod actions;
mod agent;
mod ai;
#[cfg(unix)]
mod ipc;
#[cfg(unix)]
mod mcp;
mod theme;
mod workspace;

use std::path::PathBuf;

use anyhow::{Result, bail};
use gpui_kit::*;
use workspace::{AppKind, Workspace};

struct Args {
    app: Option<AppKind>,
    file: Option<PathBuf>,
}

const USAGE: &str = "usage: kantor [--docs | --sheets | --slides] [FILE]\n\
    \x20      kantor mcp [--no-launch] [--print-config]\n\
    \n\
    Kantor is a native office suite. With no arguments it opens the Home screen.\n\
    Docs opens .docx, Sheets .xlsx/.csv/.ods, Slides .pptx (see plan.md).\n\
    `kantor mcp` lets AI agents edit the open documents (kantor mcp --help).";

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
            Some("--docs") => app = Some(AppKind::Docs),
            Some("--sheets") => app = Some(AppKind::Sheets),
            Some("--slides") => app = Some(AppKind::Slides),
            Some(s) if s.starts_with('-') => bail!("unknown option {s}\n\n{USAGE}"),
            _ if file.is_none() => file = Some(PathBuf::from(arg)),
            _ => bail!("only one file can be opened at a time\n\n{USAGE}"),
        }
    }
    Ok(Args { app, file })
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.first().map(String::as_str) == Some("mcp") {
        // stdout carries the protocol; logs go to stderr.
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
        #[cfg(unix)]
        std::process::exit(mcp::main(&raw[1..]));
        #[cfg(not(unix))]
        {
            eprintln!("kantor mcp: not supported on this platform");
            std::process::exit(2);
        }
    }
    env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("warn,kantor=info,sheets_ui=info,docs_ui=info,slides_ui=info"),
    )
    .init();
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("kantor: {e:#}");
            std::process::exit(2);
        }
    };

    gpui_kit::application()
        .with_assets(kantor_ui::KantorAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::init(cx);
            kantor_ui::find_bar::init(cx);
            sheets_ui::init(cx);
            docs_ui::init(cx);
            slides_ui::init(cx);

            ai::init(cx);
            actions::bind_keys(cx);

            let options = WindowOptions {
                app_id: Some("kantor".into()),
                titlebar: Some(TitlebarOptions {
                    title: Some("Kantor".into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1280.), px(800.)),
                    cx,
                ))),
                window_min_size: Some(size(px(720.), px(480.))),
                ..Default::default()
            };

            let Args { app, file } = args;
            let opened = gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| Workspace::new(app, file, window, cx))
            });
            let (window, workspace) = match opened {
                Ok(opened) => opened,
                Err(err) => {
                    eprintln!("kantor: failed to open a window: {err:#}");
                    std::process::exit(1);
                }
            };
            #[cfg(unix)]
            start_agent_endpoint(window, workspace.clone(), cx);
            // Ctrl+Q while focus is outside the workspace (a dialog): still
            // goes through the unsaved-changes check.
            cx.on_action(move |_: &actions::Quit, cx| {
                let workspace = workspace.clone();
                let _ = window.update(cx, |_, window, cx| {
                    workspace.update(cx, |ws, cx| ws.request_quit(window, cx))
                });
            });
            cx.activate(true);
        });
}

/// Listen for `kantor mcp` (see [`ipc`]) and run its tool calls on the main
/// thread against `workspace`. The socket is removed when Kantor quits.
#[cfg(unix)]
fn start_agent_endpoint(window: AnyWindowHandle, workspace: Entity<Workspace>, cx: &mut App) {
    use futures::StreamExt as _;

    let Some(dir) = ipc::socket_dir() else {
        log::warn!("MCP endpoint off: HOME is not set");
        return;
    };
    let (server, mut requests) = match ipc::serve(&dir) {
        Ok(s) => s,
        Err(err) => {
            log::warn!("MCP endpoint off: can't listen in {}: {err}", dir.display());
            return;
        }
    };
    log::info!("MCP endpoint at {}", server.path.display());
    cx.spawn(async move |cx| {
        while let Some(req) = requests.next().await {
            let (tool, args) = (req.tool.clone(), req.args.clone());
            let result = window
                .update(cx, |_, window, cx| {
                    workspace.update(cx, |ws, cx| ws.agent_call(&tool, args, window, cx))
                })
                .unwrap_or_else(|_| Err("Kantor's window is closed".into()));
            req.respond(result);
        }
    })
    .detach();
    cx.on_app_quit(move |_| {
        server.remove();
        async {}
    })
    .detach();
}
