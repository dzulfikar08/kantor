//! Known agents and how to start them. Detection only looks for files: no
//! process runs and nothing goes over the network until the user starts an
//! agent (the `npx` fallback downloads the adapter on its first start).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// How to start an ACP agent.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentCommand {
    /// Shown in the UI ("Claude Code").
    pub label: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Set on top of Kantor's own environment.
    pub env: Vec<(String, String)>,
}

impl AgentCommand {
    /// Any ACP agent the user names. A bare program name is looked up on
    /// [`SearchPath::from_env`], which also becomes the agent's `PATH`.
    pub fn custom(
        label: impl Into<String>,
        program: impl Into<PathBuf>,
        args: Vec<String>,
    ) -> Self {
        let search = SearchPath::from_env();
        let program: PathBuf = program.into();
        let bare = program.components().count() == 1 && !program.is_absolute();
        let program = match program.to_str() {
            Some(name) if bare => search.find(name).unwrap_or(program),
            _ => program,
        };
        AgentCommand {
            label: label.into(),
            env: search.path_env(&program).into_iter().collect(),
            program,
            args,
        }
    }
}

/// Agents Kantor knows how to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentPreset {
    /// Claude Code through Anthropic's ACP adapter (`claude-agent-acp`).
    ClaudeCode,
    /// OpenAI Codex through its ACP adapter (`codex-acp`).
    Codex,
    /// Gemini CLI, which speaks ACP itself (`gemini --acp`, 0.33+).
    Gemini,
}

/// What the UI shows for an agent that isn't installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallHint {
    pub summary: &'static str,
    /// Commands to run in a terminal, in order.
    pub commands: Vec<&'static str>,
    pub url: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Detection {
    /// Can start. `via_npx`: the adapter isn't installed and `npx` fetches
    /// it on the first start (say so in the UI: that start needs the
    /// network and takes a while).
    Ready {
        command: AgentCommand,
        via_npx: bool,
    },
    Missing {
        hint: InstallHint,
    },
}

impl AgentPreset {
    pub const ALL: [AgentPreset; 3] = [
        AgentPreset::ClaudeCode,
        AgentPreset::Codex,
        AgentPreset::Gemini,
    ];

    /// Stable id for settings.
    pub fn id(self) -> &'static str {
        match self {
            AgentPreset::ClaudeCode => "claude-code",
            AgentPreset::Codex => "codex",
            AgentPreset::Gemini => "gemini",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            AgentPreset::ClaudeCode => "Claude Code",
            AgentPreset::Codex => "Codex",
            AgentPreset::Gemini => "Gemini CLI",
        }
    }

    /// How the user signs in to the agent itself (once, outside Kantor).
    pub fn sign_in_hint(self) -> &'static str {
        match self {
            AgentPreset::ClaudeCode => {
                "Sign in once by running `claude` in a terminal, or set ANTHROPIC_API_KEY."
            }
            AgentPreset::Codex => "Sign in once with `codex login`, or set OPENAI_API_KEY.",
            AgentPreset::Gemini => "Run `gemini` once in a terminal to sign in.",
        }
    }

    /// Installed adapter binaries, preferred first (older names after).
    fn binaries(self) -> &'static [&'static str] {
        match self {
            AgentPreset::ClaudeCode => &["claude-agent-acp", "claude-code-acp"],
            AgentPreset::Codex => &["codex-acp"],
            AgentPreset::Gemini => &["gemini"],
        }
    }

    fn args(self) -> &'static [&'static str] {
        match self {
            AgentPreset::Gemini => &["--acp"],
            _ => &[],
        }
    }

    /// The npm package `npx` can run when no binary is installed.
    fn npx_package(self) -> Option<&'static str> {
        match self {
            AgentPreset::ClaudeCode => Some("@agentclientprotocol/claude-agent-acp"),
            AgentPreset::Codex => Some("@agentclientprotocol/codex-acp"),
            // The whole CLI, and it needs a sign-in first anyway.
            AgentPreset::Gemini => None,
        }
    }

    pub fn install_hint(self) -> InstallHint {
        match self {
            AgentPreset::ClaudeCode => InstallHint {
                summary: "Install Node.js and Claude Code's ACP adapter.",
                commands: vec!["npm install -g @agentclientprotocol/claude-agent-acp"],
                url: "https://github.com/agentclientprotocol/claude-agent-acp",
            },
            AgentPreset::Codex => InstallHint {
                summary: "Install Node.js and Codex's ACP adapter.",
                commands: vec!["npm install -g @agentclientprotocol/codex-acp"],
                url: "https://github.com/agentclientprotocol/codex-acp",
            },
            AgentPreset::Gemini => InstallHint {
                summary: "Install Gemini CLI 0.33 or newer.",
                commands: vec!["npm install -g @google/gemini-cli"],
                url: "https://github.com/google-gemini/gemini-cli",
            },
        }
    }

    /// Looks on `PATH` and in the usual install folders.
    pub fn detect(self) -> Detection {
        self.detect_in(&SearchPath::from_env())
    }

    pub fn detect_in(self, search: &SearchPath) -> Detection {
        let args = || self.args().iter().map(|a| (*a).to_owned()).collect();
        let command = |program: PathBuf, args: Vec<String>| AgentCommand {
            label: self.name().to_owned(),
            env: search.path_env(&program).into_iter().collect(),
            program,
            args,
        };
        if let Some(program) = self.binaries().iter().find_map(|b| search.find(b)) {
            return Detection::Ready {
                command: command(program, args()),
                via_npx: false,
            };
        }
        if let Some(package) = self.npx_package()
            && let Some(npx) = search.find("npx")
        {
            return Detection::Ready {
                command: command(npx, vec!["-y".to_owned(), package.to_owned()]),
                via_npx: true,
            };
        }
        Detection::Missing {
            hint: self.install_hint(),
        }
    }
}

/// Folders searched for agents: `PATH`, then where Node version managers
/// and package managers put binaries. Apps started from the macOS Finder
/// or a Linux desktop launcher get a short `PATH` without them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPath {
    dirs: Vec<PathBuf>,
}

impl SearchPath {
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        let mut unique: Vec<PathBuf> = Vec::new();
        for d in dirs {
            if !d.as_os_str().is_empty() && !unique.contains(&d) {
                unique.push(d);
            }
        }
        SearchPath { dirs: unique }
    }

    pub fn from_env() -> Self {
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        dirs.extend(extra_dirs());
        SearchPath::new(dirs)
    }

    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }

    /// The executable `name` in the first folder that has it (on Windows
    /// with the `PATHEXT` extensions: `npx.cmd`, `codex-acp.exe`, …).
    pub fn find(&self, name: &str) -> Option<PathBuf> {
        let exts = exe_extensions();
        self.dirs.iter().find_map(|dir| {
            exts.iter().find_map(|ext| {
                let mut file = OsString::from(name);
                file.push(ext);
                let path = dir.join(file);
                is_executable(&path).then_some(path)
            })
        })
    }

    /// `PATH` for an agent started from `program`: its own folder first
    /// (an npm-installed script finds `node` next to it), then these.
    fn path_env(&self, program: &Path) -> Option<(String, String)> {
        let mut dirs = Vec::with_capacity(self.dirs.len() + 1);
        if let Some(parent) = program.parent().filter(|p| !p.as_os_str().is_empty()) {
            dirs.push(parent.to_path_buf());
        }
        dirs.extend(
            self.dirs
                .iter()
                .filter(|d| Some(d.as_path()) != program.parent())
                .cloned(),
        );
        let joined = std::env::join_paths(dirs).ok()?;
        Some(("PATH".to_owned(), joined.into_string().ok()?))
    }
}

#[cfg(windows)]
fn exe_extensions() -> Vec<String> {
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
    // An extensionless file isn't runnable on Windows (npm also writes a
    // POSIX shell script next to the .cmd).
    pathext
        .split(';')
        .filter(|e| !e.is_empty())
        .map(|e| e.to_ascii_lowercase())
        .collect()
}

#[cfg(not(windows))]
fn exe_extensions() -> Vec<String> {
    vec![String::new()]
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Usual install folders beyond `PATH`.
fn extra_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(windows)]
    {
        for (var, sub) in [
            ("APPDATA", "npm"),
            ("ProgramFiles", "nodejs"),
            ("LOCALAPPDATA", "Volta\\bin"),
            ("USERPROFILE", ".bun\\bin"),
            ("USERPROFILE", ".cargo\\bin"),
        ] {
            if let Some(base) = std::env::var_os(var) {
                dirs.push(PathBuf::from(base).join(sub));
            }
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            for sub in [
                ".local/bin",
                ".npm-global/bin",
                ".volta/bin",
                ".bun/bin",
                ".cargo/bin",
                ".local/share/mise/shims",
                ".asdf/shims",
            ] {
                dirs.push(home.join(sub));
            }
            dirs.extend(nvm_dirs(&home.join(".nvm/versions/node")));
            dirs.extend(nvm_dirs(&home.join(".local/share/fnm/node-versions")));
        }
        for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
            dirs.push(PathBuf::from(d));
        }
    }
    dirs.retain(|d| d.is_dir());
    dirs
}

/// `<versions>/<vX.Y.Z>/bin` (nvm) or `…/installation/bin` (fnm), newest
/// version first.
#[cfg(not(windows))]
fn nvm_dirs(versions: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(versions) else {
        return Vec::new();
    };
    let mut found: Vec<(Vec<u64>, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let key = name
                .trim_start_matches('v')
                .split('.')
                .map(|n| n.parse().unwrap_or(0))
                .collect();
            let base = e.path();
            let bin = [base.join("bin"), base.join("installation/bin")]
                .into_iter()
                .find(|b| b.is_dir())?;
            Some((key, bin))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_exe(dir: &Path, name: &str) -> PathBuf {
        #[cfg(windows)]
        let name = format!("{name}.cmd");
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn detects_installed_npx_and_missing() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let search = SearchPath::new(vec![a.path().to_path_buf(), b.path().to_path_buf()]);

        for p in AgentPreset::ALL {
            assert!(matches!(p.detect_in(&search), Detection::Missing { .. }));
            assert_eq!(AgentPreset::from_id(p.id()), Some(p));
        }

        let npx = touch_exe(b.path(), "npx");
        let Detection::Ready {
            command,
            via_npx: true,
        } = AgentPreset::ClaudeCode.detect_in(&search)
        else {
            panic!("claude via npx")
        };
        assert_eq!(command.program, npx);
        assert_eq!(
            command.args,
            ["-y", "@agentclientprotocol/claude-agent-acp"]
        );
        let path = &command.env.iter().find(|(k, _)| k == "PATH").unwrap().1;
        let first = std::env::split_paths(path).next().unwrap();
        assert_eq!(first, b.path());
        // Gemini has no npx fallback.
        assert!(matches!(
            AgentPreset::Gemini.detect_in(&search),
            Detection::Missing { .. }
        ));

        // An installed adapter wins over npx, the old name still counts.
        let old = touch_exe(a.path(), "claude-code-acp");
        let Detection::Ready {
            command,
            via_npx: false,
        } = AgentPreset::ClaudeCode.detect_in(&search)
        else {
            panic!("claude installed")
        };
        assert_eq!(command.program, old);
        let new = touch_exe(b.path(), "claude-agent-acp");
        let Detection::Ready { command, .. } = AgentPreset::ClaudeCode.detect_in(&search) else {
            panic!("claude installed")
        };
        assert_eq!(command.program, new);

        touch_exe(a.path(), "gemini");
        let Detection::Ready { command, .. } = AgentPreset::Gemini.detect_in(&search) else {
            panic!("gemini")
        };
        assert_eq!(command.args, ["--acp"]);
        assert_eq!(command.label, "Gemini CLI");
    }

    #[cfg(unix)]
    #[test]
    fn skips_files_that_are_not_executable() {
        let a = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("codex-acp"), "").unwrap();
        let search = SearchPath::new(vec![a.path().to_path_buf()]);
        assert_eq!(search.find("codex-acp"), None);
    }
}
