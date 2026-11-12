//! `kantor.com`: Kantor's command line on Windows.
//!
//! A release `kantor.exe` is a GUI-subsystem program, so Windows gives it no
//! console: `kantor --version` or `kantor convert` typed in a terminal would
//! print nothing. Packaging installs this console program next to it as
//! `kantor.com`; `cmd` and PowerShell try `.com` before `.exe` for a bare
//! `kantor`, so terminals get this, and it runs `kantor.exe` with the same
//! arguments and this console's standard handles.
//!
//! Subcommands (`convert`, `pdf`, `mcp`, `--version`, `--help`) are waited
//! for and their exit code passed on. Anything else opens the app: that is
//! started and left running, so the terminal isn't held until it quits.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

/// Arguments that make kantor.exe a command-line tool rather than the app.
fn is_cli(first: Option<&str>) -> bool {
    matches!(
        first,
        Some("convert" | "pdf" | "mcp" | "-V" | "--version" | "-h" | "--help")
    )
}

/// `kantor.exe` (or `kantor` elsewhere) in this program's folder.
fn gui_exe() -> std::io::Result<PathBuf> {
    let me = std::env::current_exe()?;
    let dir = me
        .parent()
        .ok_or_else(|| std::io::Error::other("no folder"))?;
    Ok(dir.join(format!("kantor{}", std::env::consts::EXE_SUFFIX)))
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let exe = match gui_exe() {
        Ok(p) if p.is_file() => p,
        Ok(p) => {
            eprintln!("kantor: {} not found", p.display());
            return ExitCode::from(1);
        }
        Err(e) => {
            eprintln!("kantor: can't find kantor.exe: {e}");
            return ExitCode::from(1);
        }
    };
    let mut cmd = Command::new(&exe);
    cmd.args(&args);
    if is_cli(args.first().and_then(|a| a.to_str())) {
        return match cmd.status() {
            Ok(status) => ExitCode::from(status.code().map_or(1, |c| u8::try_from(c).unwrap_or(1))),
            Err(e) => {
                eprintln!("kantor: can't run {}: {e}", exe.display());
                ExitCode::from(1)
            }
        };
    }
    // The app: detached from this console.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match cmd.spawn() {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kantor: can't start {}: {e}", exe.display());
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_the_subcommands() {
        for a in ["convert", "pdf", "mcp", "--version", "-V", "--help", "-h"] {
            assert!(is_cli(Some(a)), "{a}");
        }
        for a in ["report.xlsx", "--sheets", "--docs"] {
            assert!(!is_cli(Some(a)), "{a}");
        }
        assert!(!is_cli(None));
    }
}
