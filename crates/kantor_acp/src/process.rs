//! The agent's process: started in a process group of its own so shutdown
//! also ends what it started (`npx` → node → the adapter → Claude Code and
//! its MCP servers).

use std::io;
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

use crate::preset::AgentCommand;

/// Environment variables the agent must not inherit: Claude Code refuses to
/// start "inside another Claude Code session" when Kantor itself was started
/// from one.
const SCRUBBED_ENV: &[&str] = &["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"];

pub(crate) struct AgentProcess {
    child: Child,
    done: bool,
}

pub(crate) struct Pipes {
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
}

pub(crate) fn spawn(command: &AgentCommand, cwd: &Path) -> io::Result<(AgentProcess, Pipes)> {
    let mut cmd = Command::new(&command.program);
    cmd.args(&command.args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for key in SCRUBBED_ENV {
        cmd.env_remove(key);
    }
    for (k, v) in &command.env {
        cmd.env(k, v);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn()?;
    let missing = || io::Error::other("agent pipes missing");
    let pipes = Pipes {
        stdin: child.stdin.take().ok_or_else(missing)?,
        stdout: child.stdout.take().ok_or_else(missing)?,
        stderr: child.stderr.take().ok_or_else(missing)?,
    };
    Ok((AgentProcess { child, done: false }, pipes))
}

impl AgentProcess {
    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }

    /// Ends the process and everything in its group, and returns its exit
    /// code (`None` when a signal ended it). Asks politely first: `grace`
    /// to exit after SIGTERM (Unix) before SIGKILL. Idempotent.
    pub(crate) fn stop(&mut self, grace: Duration) -> Option<i32> {
        if self.done {
            return self.child.try_wait().ok().flatten().and_then(|s| s.code());
        }
        self.done = true;
        let status = self.stop_tree(grace);
        status.and_then(|s| s.code())
    }

    #[cfg(unix)]
    fn stop_tree(&mut self, grace: Duration) -> Option<std::process::ExitStatus> {
        use rustix::process::{Pid, Signal, kill_process_group};
        use std::time::Instant;
        // The leader isn't reaped before this (no wait/try_wait so far), so
        // its pid, which is the group id, can't have been reused.
        let group = Pid::from_child(&self.child);
        let _ = kill_process_group(group, Signal::TERM);
        let deadline = Instant::now() + grace;
        let mut status = None;
        while Instant::now() < deadline {
            if let Ok(Some(s)) = self.child.try_wait() {
                status = Some(s);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Group members that ignored SIGTERM (or outlived the leader).
        let _ = kill_process_group(group, Signal::KILL);
        match status {
            Some(s) => Some(s),
            None => self.child.wait().ok(),
        }
    }

    #[cfg(windows)]
    fn stop_tree(&mut self, _grace: Duration) -> Option<std::process::ExitStatus> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // No console to send Ctrl+Break to (CREATE_NO_WINDOW), so end the
        // tree: npx.cmd → cmd.exe → node → … The pid can't be reused while
        // `child` holds the process handle, even when it has exited.
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &self.child.id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        let _ = self.child.kill();
        self.child.wait().ok()
    }

    #[cfg(not(any(unix, windows)))]
    fn stop_tree(&mut self, _grace: Duration) -> Option<std::process::ExitStatus> {
        let _ = self.child.kill();
        self.child.wait().ok()
    }
}

impl Drop for AgentProcess {
    fn drop(&mut self) {
        self.stop(Duration::from_millis(300));
    }
}
