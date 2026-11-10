//! `kantor mcp`: an MCP server on stdio that lets AI agents (Claude Code,
//! Claude Desktop, Cursor, …) read and edit the documents open in the
//! running Kantor, live. It forwards tool calls to the app over its local
//! socket ([`crate::ipc`]); the app runs them like the user's own edits.

mod client;
pub mod protocol;
pub mod tools;

use std::io;

pub const USAGE: &str = "usage: kantor mcp [--no-launch] [--print-config]\n\
    \n\
    Run an MCP server on stdin/stdout for AI agents to read and edit the\n\
    documents open in Kantor. It starts Kantor if it isn't running\n\
    (unless --no-launch). --print-config prints setup for Claude Code,\n\
    Claude Desktop and Cursor.";

/// The executable that starts Kantor: the `.AppImage` file itself when
/// running from one (the binary inside lives on a mount that disappears when
/// this process exits), else this binary.
fn exe_path() -> std::io::Result<std::path::PathBuf> {
    if let Some(appimage) = std::env::var_os("APPIMAGE").map(std::path::PathBuf::from)
        && appimage.is_file()
    {
        return Ok(appimage);
    }
    std::env::current_exe()
}

/// The `kantor` binary as the agent should start it.
fn exe() -> String {
    exe_path()
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "kantor".to_owned())
}

/// `path` quoted for a POSIX shell when it needs it.
fn shell_quote(path: &str) -> String {
    let plain = path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+:@".contains(c));
    if plain && !path.is_empty() {
        path.to_owned()
    } else {
        format!("'{}'", path.replace('\'', r"'\''"))
    }
}

/// The command that adds Kantor to Claude Code.
pub fn claude_code_command() -> String {
    format!("claude mcp add kantor -- {} mcp", shell_quote(&exe()))
}

/// Setup text for `--print-config`.
pub fn config_text() -> String {
    let desktop = serde_json::json!({
        "mcpServers": {"kantor": {"command": exe(), "args": ["mcp"]}}
    });
    format!(
        "Claude Code:\n  {}\n\n\
         Claude Desktop (claude_desktop_config.json), Cursor (~/.cursor/mcp.json)\n\
         and other MCP clients:\n{}\n",
        claude_code_command(),
        serde_json::to_string_pretty(&desktop).unwrap_or_default()
    )
}

/// Run the subcommand; returns the exit code.
pub fn main(args: &[String]) -> i32 {
    let mut launch = true;
    for arg in args {
        match arg.as_str() {
            "--no-launch" => launch = false,
            "--print-config" => {
                print!("{}", config_text());
                return 0;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            other => {
                eprintln!("kantor mcp: unknown option {other}\n\n{USAGE}");
                return 2;
            }
        }
    }
    let mut server = protocol::Server::new(client::AppClient::new(launch));
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut output = io::stdout().lock();
    match server.run(&mut input, &mut output) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("kantor mcp: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn quotes_paths_for_the_shell() {
        assert_eq!(shell_quote("/usr/bin/kantor"), "/usr/bin/kantor");
        assert_eq!(
            shell_quote("/Applications/My Apps/kantor"),
            "'/Applications/My Apps/kantor'"
        );
        assert_eq!(shell_quote("/a'b"), r"'/a'\''b'");
        assert!(claude_code_command().starts_with("claude mcp add kantor -- "));
        assert!(config_text().contains("\"mcpServers\""));
    }
}
