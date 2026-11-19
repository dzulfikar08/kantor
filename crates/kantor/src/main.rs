//! Kantor: one binary for Docs, Sheets, Slides and the PDF viewer.
//!
//! ```text
//! kantor                    open the Home screen
//! kantor FILE...            open each FILE in the app matching its extension
//! kantor --sheets [FILE]    start Sheets (likewise --docs, --slides)
//! kantor mcp                MCP server on stdio for AI agents (see mcp/)
//! kantor pdf TOOL ...       PDF tools without a window (see cli/pdf.rs)
//! kantor convert IN OUT     convert without a window (see cli/convert.rs)
//! kantor slides build ...   build and edit .pptx from JSON (see cli/slides.rs)
//! ```
//!
//! On Windows a release kantor.exe is a GUI program (no console window pops up
//! when it starts), so in a terminal its output goes nowhere unless redirected.
//! Terminals run `kantor.com` instead (crates/kantor_console), which `cmd` and
//! PowerShell pick before kantor.exe for a bare `kantor`.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

extern crate gpui_kit as gpui;

mod actions;
mod agent;
mod ai;
mod cli;
mod ipc;
mod mcp;
mod open_files;
mod theme;
mod workspace;

use std::path::PathBuf;

use anyhow::{Result, bail};
use gpui_kit::*;
use workspace::{AppKind, Workspace};

struct Args {
    app: Option<AppKind>,
    /// Files to open, each in its own tab (`kantor a.xlsx b.docx`; the
    /// Linux launchers pass every selected file, `Exec=kantor %F`).
    files: Vec<PathBuf>,
}

const USAGE: &str = "usage: kantor [--docs | --sheets | --slides] [FILE...]\n\
    \x20      kantor mcp [--no-launch] [--print-config]\n\
    \x20      kantor pdf <tool> [options]\n\
    \x20      kantor convert [options] INPUT OUTPUT\n\
    \x20      kantor slides <build|list|edit|schema> ...\n\
    \n\
    Kantor is a native office suite. With no arguments it opens the Home screen.\n\
    Docs opens .docx (and imports .rtf and .doc), Sheets .xlsx/.csv/.ods, Slides .pptx/.ppt,\n\
    and .pdf files open in the PDF viewer (see plan.md).\n\
    `kantor mcp` lets AI agents edit the open documents (kantor mcp --help).\n\
    `kantor pdf` merges, splits, rotates, watermarks and numbers PDFs (kantor pdf --help).\n\
    `kantor convert` converts .docx to .pdf and between spreadsheet formats\n\
    without opening a window (kantor convert --help).\n\
    `kantor slides` builds and edits presentations from JSON (kantor slides --help).";

fn parse_args() -> Result<Args> {
    let mut app = None;
    let mut files = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            Some("-V" | "--version") => {
                // `kantor 0.2.0`, nightlies `kantor 0.2.0 (nightly 2026-11-10 14:30)`.
                println!("kantor {}", kantor_update::current_build().display());
                std::process::exit(0);
            }
            Some("--docs") => app = Some(AppKind::Docs),
            Some("--sheets") => app = Some(AppKind::Sheets),
            Some("--slides") => app = Some(AppKind::Slides),
            Some(s) if s.starts_with('-') => bail!("unknown option {s}\n\n{USAGE}"),
            // A launcher that passes URLs (`%U`) sends file:// ones.
            Some(s) if s.contains("://") => match open_files::path_from_url(s) {
                Some(path) => files.push(path),
                None => bail!("{s} is not a file\n\n{USAGE}"),
            },
            _ => files.push(PathBuf::from(arg)),
        }
    }
    Ok(Args { app, files })
}

fn main() {
    if std::env::args_os().nth(1).is_some_and(|a| a == "pdf") {
        std::process::exit(cli::pdf::main(std::env::args_os().skip(2).collect()));
    }
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.first().map(String::as_str) == Some("mcp") {
        // stdout carries the protocol; logs go to stderr.
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
        std::process::exit(mcp::main(&raw[1..]));
    }
    if raw.first().map(String::as_str) == Some("slides") {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
        let args: Vec<_> = std::env::args_os().skip(2).collect();
        std::process::exit(cli::slides::main(&args));
    }
    if raw.first().map(String::as_str) == Some("convert") {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
        let args: Vec<_> = std::env::args_os().skip(2).collect();
        std::process::exit(cli::convert::main(&args));
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

    // Finder's Open With, double-clicks and Dock drops (macOS): queued until
    // the window can take them (see open_files).
    let open_requests = open_files::OpenRequests::default();
    let application = gpui_kit::application().with_assets(kantor_ui::KantorAssets);
    application.on_open_urls(open_requests.handler());
    application.run(move |cx| {
        gpui_kit::init(cx);
        kantor_ui::widget_strings::init();
        theme::init(cx);
        kantor_ui::perf::watch_main_thread(cx);
        kantor_ui::find_bar::init(cx);
        sheets_ui::init(cx);
        docs_ui::init(cx);
        slides_ui::init(cx);
        pdf_ui::init(cx);

        ai::init(cx);
        workspace::init_cloud_panels(cx);
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

        let Args { app, mut files } = args;
        // A cold start by Open With: the files the system sent before
        // the window existed open in it (no Home screen first).
        files.extend(open_requests.take_pending());
        let opened = gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| Workspace::new(app, files, window, cx))
        });
        let (window, workspace) = match opened {
            Ok(opened) => opened,
            Err(err) => {
                eprintln!("kantor: failed to open a window: {err:#}");
                std::process::exit(1);
            }
        };
        start_agent_endpoint(window, workspace.clone(), cx);
        open_later_requests(&open_requests, window, workspace.clone(), cx);
        // A downloaded update goes in place when Kantor quits. Cloud
        // locks are released by every way of quitting: the Quit command
        // and closing the window do it before this (then there's nothing
        // left here), the system's quit (macOS Dock or logout, Windows
        // shutdown or logout) gets here directly. This runs before
        // GPUI's short shutdown timeout starts.
        let updating = workspace.clone();
        cx.on_app_quit(move |cx| {
            updating.update(cx, |ws, _| {
                ws.apply_update_on_quit();
                ws.cloud_quit();
            });
            async {}
        })
        .detach();
        #[cfg(unix)]
        quit_on_signals(window, workspace.clone(), cx);
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

/// SIGTERM (logout, shutdown, `kill`), SIGINT (Ctrl+C in a terminal) and
/// SIGHUP (the terminal closed) quit Kantor as the Quit command would,
/// minus the questions nobody is there to answer: unsaved documents keep
/// their recovery copies (offered at the next start), cloud documents keep
/// their working copies, and cloud locks are released. A second signal, or
/// [`SIGNAL_QUIT_LIMIT`] without having quit, ends the process at once.
#[cfg(unix)]
fn quit_on_signals(window: AnyWindowHandle, workspace: Entity<Workspace>, cx: &mut App) {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

    let mut signals = match signal_hook::iterator::Signals::new([SIGTERM, SIGINT, SIGHUP]) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("can't handle quit signals: {e}");
            return;
        }
    };
    let (tx, rx) = async_channel::bounded::<i32>(1);
    let _ = std::thread::Builder::new()
        .name("kantor-signals".into())
        .spawn(move || {
            let mut first = true;
            for sig in signals.forever() {
                if !first {
                    std::process::exit(128 + sig);
                }
                first = false;
                log::info!("signal {sig}: quitting");
                let _ = tx.send_blocking(sig);
                std::thread::spawn(move || {
                    std::thread::sleep(SIGNAL_QUIT_LIMIT);
                    log::warn!(
                        "still running {} s after signal {sig}: exiting",
                        SIGNAL_QUIT_LIMIT.as_secs()
                    );
                    std::process::exit(128 + sig);
                });
            }
        });
    cx.spawn(async move |cx| {
        if rx.recv().await.is_ok() {
            let quit = window.update(cx, |_, _, cx| {
                workspace.update(cx, |ws, cx| ws.quit_unattended(cx))
            });
            if quit.is_err() {
                // The window is gone already: quit anyway (the app-quit hook
                // releases the locks).
                cx.update(|cx| cx.quit());
            }
        }
    })
    .detach();
}

/// How long quitting after a signal may take before the process just ends
/// (lock release waits at most a few seconds of it).
#[cfg(unix)]
const SIGNAL_QUIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Open the files the system sends once Kantor is running (Open With,
/// double-clicks and Dock drops on macOS), in the window's tabs.
fn open_later_requests(
    requests: &open_files::OpenRequests,
    window: AnyWindowHandle,
    workspace: Entity<Workspace>,
    cx: &mut App,
) {
    let rx = requests.receiver();
    cx.spawn(async move |cx| {
        while let Ok(paths) = rx.recv().await {
            let _ = window.update(cx, |_, window, cx| {
                window.activate_window();
                workspace.update(cx, |ws, cx| ws.open_paths(None, paths, window, cx))
            });
        }
    })
    .detach();
}

/// Listen for `kantor mcp` (see [`ipc`]) and run its tool calls on the main
/// thread against `workspace`. The socket is removed when Kantor quits.
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
