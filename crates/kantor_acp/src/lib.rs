//! Kantor as an [Agent Client Protocol](https://agentclientprotocol.com)
//! client: people who already pay for Claude Code, Codex or Gemini CLI run
//! that agent on the documents open in Kantor, at no extra cost. No GPUI
//! dependency; the chat panel sits on top of this crate.
//!
//! The agent is a subprocess speaking JSON-RPC on stdin/stdout. Kantor hands
//! it Kantor's own MCP server (`kantor mcp --no-launch`, see [`McpServer`])
//! in `session/new`, so the agent reads and edits the open documents through
//! the same tools as any other MCP client, live in the running app.
//!
//! ```no_run
//! use kantor_acp::{AcpAgent, AgentPreset, ClientOptions, Detection, McpServer, PromptBlock};
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let Detection::Ready { command, .. } = AgentPreset::ClaudeCode.detect() else {
//!     return Ok(()); // show the install hint instead
//! };
//! let dir = std::path::Path::new("/home/me/Documents");
//! let (agent, events) = AcpAgent::spawn(&command, dir, ClientOptions::default())?;
//! let info = agent.initialize()?;
//! let session = agent.new_session(dir, &[McpServer::kantor_current()?])?;
//! agent.prompt(
//!     &session.id,
//!     vec![
//!         PromptBlock::text("Make the title bold"),
//!         PromptBlock::file(&dir.join("report.docx")),
//!     ],
//! )?;
//! // Read `events` (AcpEvent::Update, ::Permission, ::TurnEnded, ::Exited)
//! // from a GPUI task; answer permissions with `agent.answer_permission`.
//! # drop((info, events)); Ok(()) }
//! ```
//!
//! Threads: one reader thread per agent parses its output in order and sends
//! [`AcpEvent`]s down a `futures` channel; one more drains its stderr. The
//! request methods ([`AcpAgent::initialize`], [`AcpAgent::new_session`], …)
//! block until the agent answers or exits, so call them off the UI thread.
//! [`AcpAgent::prompt`] returns at once; the turn's end is an event.
//!
//! Privacy, in line with `kantor_ai`'s rules:
//! - nothing starts until the user picks an agent: [`AgentPreset::detect`]
//!   only looks at files on disk (no network, no process runs);
//! - the client offers no file system or terminal access by default
//!   ([`ClientOptions::fs`]); the agent edits documents only through
//!   Kantor's MCP tools, which run like the user's own edits;
//! - every tool call the agent asks permission for goes to the user
//!   ([`AcpEvent::Permission`]); nothing is approved automatically;
//! - prompt text, agent output and file contents are never logged; logs
//!   carry method names only.
//!
//! # Use Claude Code / Codex with Kantor (ACP) — README draft
//!
//! If you already use Claude Code, Codex or Gemini CLI, Kantor can run it
//! on your open documents: no API key and no extra cost, the agent uses your
//! own subscription or sign-in.
//!
//! 1. Install the agent's ACP adapter (Kantor shows what's missing):
//!    - **Claude Code**: `npm install -g @agentclientprotocol/claude-agent-acp`,
//!      and sign in once by running `claude` in a terminal (or set
//!      `ANTHROPIC_API_KEY`). Without the global install Kantor starts it
//!      through `npx` (Node.js needed; the first start downloads it).
//!    - **Codex**: `npm install -g @agentclientprotocol/codex-acp`, and sign
//!      in once with `codex login` (or set `OPENAI_API_KEY`).
//!    - **Gemini CLI**: `npm install -g @google/gemini-cli` (0.33 or newer),
//!      then run `gemini` once to sign in.
//!    - **Any other ACP agent**: give Kantor its command line.
//! 2. Open the AI panel in Kantor and pick the agent.
//! 3. Ask. The agent sees the open document and edits it through Kantor's
//!    document tools; every action that needs permission is shown to you
//!    first (allow once, always, or reject).
//!
//! Kantor never sends your documents anywhere itself: the agent you chose
//! talks to its own service, under that service's terms.

mod client;
mod fs;
mod preset;
mod process;
mod types;
mod wire;

pub use client::{AcpAgent, ClientOptions, Events};
pub use fs::{DiskFs, FsHandler};
pub use preset::{AgentCommand, AgentPreset, Detection, InstallHint, SearchPath};
pub use types::{
    AcpError, AcpEvent, AgentInfo, AuthMethod, AvailableCommand, Content, McpServer,
    PermissionChoice, PermissionId, PermissionKind, PermissionOption, PermissionRequest, PlanEntry,
    PlanPriority, PlanStatus, PromptBlock, RpcError, Session, SessionId, SessionMode, SessionModes,
    SessionUpdate, StopReason, ToolCall, ToolCallUpdate, ToolContent, ToolKind, ToolLocation,
    ToolStatus,
};

/// The ACP version this client speaks.
pub const PROTOCOL_VERSION: u64 = 1;
