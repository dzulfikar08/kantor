//! The protocol's data as Kantor uses it. Parsing is lenient: unknown
//! update kinds, content types and enum values come through as `Other`
//! instead of failing the whole message, because agents add fields and
//! variants faster than the spec's versions move.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub type SessionId = String;

/// A JSON-RPC error the agent answered with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// ACP's "authentication required" error code.
pub const AUTH_REQUIRED: i64 = -32000;

#[derive(Debug)]
pub enum AcpError {
    /// The agent's program couldn't be started.
    Spawn(std::io::Error),
    /// Writing to the agent failed (it has closed its input).
    Io(std::io::Error),
    /// The agent answered with an error.
    Rpc(RpcError),
    /// The agent wants the user to sign in first: offer
    /// [`AgentInfo::auth_methods`] and call [`crate::AcpAgent::authenticate`].
    AuthRequired(RpcError),
    /// The agent exited (or was shut down) before answering.
    Exited,
    /// The agent's answer didn't make sense, or it speaks another version.
    Protocol(String),
    /// No pending permission request has this id (answered or cancelled).
    UnknownPermission(PermissionId),
    /// The agent didn't offer this choice; the request stays pending.
    ChoiceNotOffered(PermissionChoice),
}

impl AcpError {
    pub(crate) fn from_rpc(e: RpcError) -> Self {
        if e.code == AUTH_REQUIRED {
            AcpError::AuthRequired(e)
        } else {
            AcpError::Rpc(e)
        }
    }
}

impl fmt::Display for AcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AcpError::Spawn(e) => write!(f, "couldn't start the agent: {e}"),
            AcpError::Io(e) => write!(f, "couldn't talk to the agent: {e}"),
            AcpError::Rpc(e) => write!(f, "the agent answered with an error: {}", e.message),
            AcpError::AuthRequired(_) => write!(f, "the agent needs you to sign in first"),
            AcpError::Exited => write!(f, "the agent stopped"),
            AcpError::Protocol(m) => write!(f, "unexpected answer from the agent: {m}"),
            AcpError::UnknownPermission(id) => {
                write!(f, "permission request {} is no longer pending", id.0)
            }
            AcpError::ChoiceNotOffered(c) => write!(f, "the agent didn't offer {c:?}"),
        }
    }
}

impl std::error::Error for AcpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AcpError::Spawn(e) | AcpError::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// What the agent reported from `initialize`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentInfo {
    pub protocol_version: u64,
    /// `agentInfo.name` / `.title` / `.version`, when the agent sends them.
    pub name: Option<String>,
    pub title: Option<String>,
    pub version: Option<String>,
    /// Ways to sign in. Empty when the agent needs none (or handles it
    /// itself, e.g. Claude Code's own login).
    pub auth_methods: Vec<AuthMethod>,
    pub load_session: bool,
    pub prompt_image: bool,
    pub prompt_audio: bool,
    pub prompt_embedded_context: bool,
}

impl AgentInfo {
    pub(crate) fn from_value(v: &Value) -> Self {
        let caps = &v["agentCapabilities"];
        let prompt = &caps["promptCapabilities"];
        let info = &v["agentInfo"];
        AgentInfo {
            protocol_version: v["protocolVersion"].as_u64().unwrap_or(0),
            name: opt_str(info, "name"),
            title: opt_str(info, "title"),
            version: opt_str(info, "version"),
            auth_methods: v["authMethods"]
                .as_array()
                .map(|a| a.iter().filter_map(AuthMethod::from_value).collect())
                .unwrap_or_default(),
            load_session: caps["loadSession"].as_bool().unwrap_or(false),
            prompt_image: prompt["image"].as_bool().unwrap_or(false),
            prompt_audio: prompt["audio"].as_bool().unwrap_or(false),
            prompt_embedded_context: prompt["embeddedContext"].as_bool().unwrap_or(false),
        }
    }
}

/// A way to sign in, passed through to the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// `"agent"` (the agent signs in itself when asked to authenticate, e.g.
    /// by opening a browser), `"terminal"` (run the agent's program with
    /// [`Self::terminal_args`] in a terminal), or a newer kind.
    pub kind: String,
    pub terminal_args: Vec<String>,
    pub terminal_env: Vec<(String, String)>,
}

impl AuthMethod {
    fn from_value(v: &Value) -> Option<Self> {
        let id = v["id"].as_str()?.to_owned();
        let name = v["name"].as_str().unwrap_or(&id).to_owned();
        Some(AuthMethod {
            name,
            description: opt_str(v, "description"),
            kind: v["type"].as_str().unwrap_or("agent").to_owned(),
            terminal_args: str_list(&v["args"]),
            terminal_env: v["env"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                        .collect()
                })
                .unwrap_or_default(),
            id,
        })
    }
}

/// An MCP server handed to the agent in `session/new` (stdio transport,
/// which every ACP agent supports).
#[derive(Debug, Clone, PartialEq)]
pub struct McpServer {
    pub name: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl McpServer {
    /// Kantor's own MCP server run from `exe` (the `kantor` binary, or the
    /// `.AppImage` file): `kantor mcp --no-launch`, attaching to the running
    /// app. `KANTOR_SOCKET_DIR`, when set, is passed on so the server finds
    /// this instance.
    pub fn kantor(exe: impl Into<PathBuf>) -> Self {
        let env = std::env::var("KANTOR_SOCKET_DIR")
            .ok()
            .filter(|d| !d.is_empty())
            .map(|d| vec![("KANTOR_SOCKET_DIR".to_owned(), d)])
            .unwrap_or_default();
        McpServer {
            name: "kantor".to_owned(),
            command: exe.into(),
            args: vec!["mcp".to_owned(), "--no-launch".to_owned()],
            env,
        }
    }

    /// [`Self::kantor`] for the running executable: the `.AppImage` file
    /// when running from one (the binary inside lives on a mount that goes
    /// away with this process), else this binary.
    pub fn kantor_current() -> std::io::Result<Self> {
        if let Some(appimage) = std::env::var_os("APPIMAGE").map(PathBuf::from)
            && appimage.is_file()
        {
            return Ok(Self::kantor(appimage));
        }
        Ok(Self::kantor(std::env::current_exe()?))
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "command": self.command,
            "args": self.args,
            "env": self.env.iter()
                .map(|(name, value)| json!({"name": name, "value": value}))
                .collect::<Vec<_>>(),
        })
    }
}

/// A session made by `session/new`.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: SessionId,
    pub modes: Option<SessionModes>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionModes {
    pub current: String,
    pub available: Vec<SessionMode>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionMode {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

impl Session {
    pub(crate) fn from_value(v: &Value) -> Option<Self> {
        let id = v["sessionId"].as_str()?.to_owned();
        let modes = v
            .get("modes")
            .filter(|m| m.is_object())
            .map(|m| SessionModes {
                current: m["currentModeId"].as_str().unwrap_or_default().to_owned(),
                available: m["availableModes"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|m| {
                                let id = m["id"].as_str()?.to_owned();
                                Some(SessionMode {
                                    name: m["name"].as_str().unwrap_or(&id).to_owned(),
                                    description: opt_str(m, "description"),
                                    id,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        Some(Session { id, modes })
    }
}

/// One piece of a prompt.
#[derive(Debug, Clone, PartialEq)]
pub enum PromptBlock {
    Text(String),
    /// A link to a file the agent may look at (the open document): ACP's
    /// baseline `resource_link`, which every agent accepts.
    ResourceLink {
        uri: String,
        name: String,
        mime_type: Option<String>,
    },
}

impl PromptBlock {
    pub fn text(text: impl Into<String>) -> Self {
        PromptBlock::Text(text.into())
    }

    /// A link to the document at `path` (absolute).
    pub fn file(path: &Path) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        PromptBlock::ResourceLink {
            uri: file_uri(path),
            name,
            mime_type: mime_for(path).map(str::to_owned),
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        match self {
            PromptBlock::Text(text) => json!({"type": "text", "text": text}),
            PromptBlock::ResourceLink {
                uri,
                name,
                mime_type,
            } => {
                let mut v = json!({"type": "resource_link", "uri": uri, "name": name});
                if let Some(m) = mime_type {
                    v["mimeType"] = json!(m);
                }
                v
            }
        }
    }
}

/// `file://` URI for an absolute path, percent-encoded; Windows drive paths
/// become `file:///C:/…`.
pub(crate) fn file_uri(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut p = raw.replace('\\', "/");
    if let Some(unc) = p.strip_prefix("//") {
        // \\server\share\… -> file://server/share/…
        return format!("file://{}", encode_path(unc));
    }
    if !p.starts_with('/') {
        p.insert(0, '/');
    }
    format!("file://{}", encode_path(&p))
}

fn encode_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn mime_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsm" => "application/vnd.ms-excel.sheet.macroEnabled.12",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "doc" => "application/msword",
        "xls" => "application/vnd.ms-excel",
        "ppt" => "application/vnd.ms-powerpoint",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "pdf" => "application/pdf",
        "csv" => "text/csv",
        "txt" => "text/plain",
        "md" => "text/markdown",
        _ => return None,
    })
}

/// Content in a message chunk or a tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Text(String),
    Image {
        mime_type: String,
    },
    Audio {
        mime_type: String,
    },
    ResourceLink {
        uri: String,
        name: String,
    },
    /// An embedded resource; `text` for text resources.
    Resource {
        uri: String,
        text: Option<String>,
    },
    Other(Value),
}

impl Content {
    pub(crate) fn from_value(v: &Value) -> Self {
        match v["type"].as_str() {
            Some("text") => match v["text"].as_str() {
                Some(t) => Content::Text(t.to_owned()),
                None => Content::Other(v.clone()),
            },
            Some("image") => Content::Image {
                mime_type: v["mimeType"].as_str().unwrap_or_default().to_owned(),
            },
            Some("audio") => Content::Audio {
                mime_type: v["mimeType"].as_str().unwrap_or_default().to_owned(),
            },
            Some("resource_link") => Content::ResourceLink {
                uri: v["uri"].as_str().unwrap_or_default().to_owned(),
                name: v["name"].as_str().unwrap_or_default().to_owned(),
            },
            Some("resource") => Content::Resource {
                uri: v["resource"]["uri"].as_str().unwrap_or_default().to_owned(),
                text: opt_str(&v["resource"], "text"),
            },
            _ => Content::Other(v.clone()),
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Content::Text(t) => Some(t),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    #[default]
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    #[default]
    #[serde(other)]
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// What a tool call shows.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolContent {
    Content(Content),
    Diff {
        path: PathBuf,
        old_text: Option<String>,
        new_text: String,
    },
    /// Output of a terminal the client made (never, while the terminal
    /// capability is off; kept for agents that send it anyway).
    Terminal {
        terminal_id: String,
    },
    Other(Value),
}

impl ToolContent {
    fn from_value(v: &Value) -> Self {
        match v["type"].as_str() {
            Some("content") => ToolContent::Content(Content::from_value(&v["content"])),
            Some("diff") => ToolContent::Diff {
                path: PathBuf::from(v["path"].as_str().unwrap_or_default()),
                old_text: opt_str(v, "oldText"),
                new_text: v["newText"].as_str().unwrap_or_default().to_owned(),
            },
            Some("terminal") => ToolContent::Terminal {
                terminal_id: v["terminalId"].as_str().unwrap_or_default().to_owned(),
            },
            _ => ToolContent::Other(v.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolLocation {
    pub path: PathBuf,
    pub line: Option<u64>,
}

/// A tool call as first reported.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolCall {
    pub id: String,
    pub title: String,
    pub kind: ToolKind,
    pub status: ToolStatus,
    pub content: Vec<ToolContent>,
    pub locations: Vec<ToolLocation>,
    pub raw_input: Option<Value>,
    pub raw_output: Option<Value>,
}

impl ToolCall {
    /// Applies an update to the same call (the fields it carries).
    pub fn apply(&mut self, u: &ToolCallUpdate) {
        if let Some(t) = &u.title {
            self.title = t.clone();
        }
        if let Some(k) = u.kind {
            self.kind = k;
        }
        if let Some(s) = u.status {
            self.status = s;
        }
        if let Some(c) = &u.content {
            self.content = c.clone();
        }
        if let Some(l) = &u.locations {
            self.locations = l.clone();
        }
        if u.raw_input.is_some() {
            self.raw_input = u.raw_input.clone();
        }
        if u.raw_output.is_some() {
            self.raw_output = u.raw_output.clone();
        }
    }

    /// A call first seen as an update (agents may skip the `tool_call`).
    pub fn from_update(u: &ToolCallUpdate) -> Self {
        let mut call = ToolCall {
            id: u.id.clone(),
            ..ToolCall::default()
        };
        call.apply(u);
        call
    }

    fn from_value(v: &Value) -> Self {
        ToolCall::from_update(&ToolCallUpdate::from_value(v))
    }
}

/// Changed fields of a tool call; `None` means unchanged.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolCallUpdate {
    pub id: String,
    pub title: Option<String>,
    pub kind: Option<ToolKind>,
    pub status: Option<ToolStatus>,
    pub content: Option<Vec<ToolContent>>,
    pub locations: Option<Vec<ToolLocation>>,
    pub raw_input: Option<Value>,
    pub raw_output: Option<Value>,
}

impl ToolCallUpdate {
    pub(crate) fn from_value(v: &Value) -> Self {
        let present = |k: &str| v.get(k).filter(|x| !x.is_null());
        ToolCallUpdate {
            id: v["toolCallId"].as_str().unwrap_or_default().to_owned(),
            title: opt_str(v, "title"),
            kind: present("kind").map(|k| serde_json::from_value(k.clone()).unwrap_or_default()),
            status: present("status")
                .map(|s| serde_json::from_value(s.clone()).unwrap_or_default()),
            content: present("content")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(ToolContent::from_value).collect()),
            locations: present("locations").and_then(Value::as_array).map(|a| {
                a.iter()
                    .filter_map(|l| {
                        Some(ToolLocation {
                            path: PathBuf::from(l["path"].as_str()?),
                            line: l["line"].as_u64(),
                        })
                    })
                    .collect()
            }),
            raw_input: present("rawInput").cloned(),
            raw_output: present("rawOutput").cloned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanPriority {
    High,
    #[default]
    #[serde(other)]
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    #[default]
    #[serde(other)]
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PlanEntry {
    pub content: String,
    #[serde(default)]
    pub priority: PlanPriority,
    #[serde(default)]
    pub status: PlanStatus,
}

/// A slash command the agent offers (`/review`, …).
#[derive(Debug, Clone, PartialEq)]
pub struct AvailableCommand {
    pub name: String,
    pub description: String,
    /// Placeholder text for the command's argument, when it takes one.
    pub input_hint: Option<String>,
}

/// A `session/update` notification.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionUpdate {
    /// The user's own message, replayed (when loading a session).
    UserMessageChunk(Content),
    AgentMessageChunk(Content),
    AgentThoughtChunk(Content),
    ToolCall(ToolCall),
    ToolCallUpdate(ToolCallUpdate),
    /// The whole plan, replacing the previous one.
    Plan(Vec<PlanEntry>),
    AvailableCommands(Vec<AvailableCommand>),
    /// The session's mode changed (by the agent, or after `set_mode`).
    CurrentMode(String),
    /// An update kind this client doesn't know (usage, session info, …).
    Other {
        kind: String,
        raw: Value,
    },
}

impl SessionUpdate {
    pub(crate) fn from_value(v: &Value) -> Self {
        let kind = v["sessionUpdate"].as_str().unwrap_or_default();
        match kind {
            "user_message_chunk" => {
                SessionUpdate::UserMessageChunk(Content::from_value(&v["content"]))
            }
            "agent_message_chunk" => {
                SessionUpdate::AgentMessageChunk(Content::from_value(&v["content"]))
            }
            "agent_thought_chunk" => {
                SessionUpdate::AgentThoughtChunk(Content::from_value(&v["content"]))
            }
            "tool_call" => SessionUpdate::ToolCall(ToolCall::from_value(v)),
            "tool_call_update" => SessionUpdate::ToolCallUpdate(ToolCallUpdate::from_value(v)),
            "plan" => SessionUpdate::Plan(
                v["entries"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|e| serde_json::from_value(e.clone()).ok())
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            "available_commands_update" => SessionUpdate::AvailableCommands(
                v["availableCommands"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|c| {
                                Some(AvailableCommand {
                                    name: c["name"].as_str()?.to_owned(),
                                    description: c["description"]
                                        .as_str()
                                        .unwrap_or_default()
                                        .to_owned(),
                                    input_hint: opt_str(&c["input"], "hint"),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            "current_mode_update" => SessionUpdate::CurrentMode(
                v["currentModeId"].as_str().unwrap_or_default().to_owned(),
            ),
            _ => SessionUpdate::Other {
                kind: kind.to_owned(),
                raw: v.clone(),
            },
        }
    }
}

/// Why a prompt turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    Other(String),
}

impl StopReason {
    pub(crate) fn from_wire(s: &str) -> Self {
        match s {
            "end_turn" => StopReason::EndTurn,
            "max_tokens" => StopReason::MaxTokens,
            "max_turn_requests" => StopReason::MaxTurnRequests,
            "refusal" => StopReason::Refusal,
            "cancelled" => StopReason::Cancelled,
            other => StopReason::Other(other.to_owned()),
        }
    }
}

/// Identifies a pending permission request in this client (the agent's own
/// JSON-RPC id stays inside).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PermissionId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    /// A kind newer than this client.
    Other,
}

impl PermissionKind {
    fn from_wire(s: &str) -> Self {
        match s {
            "allow_once" => PermissionKind::AllowOnce,
            "allow_always" => PermissionKind::AllowAlways,
            "reject_once" => PermissionKind::RejectOnce,
            "reject_always" => PermissionKind::RejectAlways,
            _ => PermissionKind::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PermissionOption {
    pub id: String,
    /// The agent's label for it ("Allow", "Always allow edits", …).
    pub name: String,
    pub kind: PermissionKind,
}

/// The agent asks before running a tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct PermissionRequest {
    pub id: PermissionId,
    pub session: SessionId,
    pub tool_call: ToolCallUpdate,
    pub options: Vec<PermissionOption>,
}

impl PermissionRequest {
    pub(crate) fn parse(id: PermissionId, params: &Value) -> Option<Self> {
        Some(PermissionRequest {
            id,
            session: params["sessionId"].as_str()?.to_owned(),
            tool_call: ToolCallUpdate::from_value(&params["toolCall"]),
            options: params["options"]
                .as_array()?
                .iter()
                .filter_map(|o| {
                    Some(PermissionOption {
                        id: o["optionId"].as_str()?.to_owned(),
                        name: o["name"].as_str().unwrap_or_default().to_owned(),
                        kind: PermissionKind::from_wire(o["kind"].as_str().unwrap_or_default()),
                    })
                })
                .collect(),
        })
    }
}

/// The user's answer to a [`PermissionRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionChoice {
    /// The agent's `allow_once` option.
    AllowOnce,
    /// The agent's `allow_always` option (never falls back to once).
    AllowAlways,
    /// `reject_once`, else `reject_always`, else the cancelled outcome.
    Reject,
    /// A specific option by id (for agents with several of one kind).
    Option(String),
    /// Answer "cancelled" (the dialog was dismissed).
    Cancel,
}

/// What a [`PermissionChoice`] answers with: `Some(option id)` or `None`
/// for the cancelled outcome.
pub(crate) fn resolve_choice(
    options: &[PermissionOption],
    choice: &PermissionChoice,
) -> Result<Option<String>, AcpError> {
    let of_kind = |k: PermissionKind| options.iter().find(|o| o.kind == k).map(|o| o.id.clone());
    let not_offered = || AcpError::ChoiceNotOffered(choice.clone());
    match choice {
        PermissionChoice::AllowOnce => of_kind(PermissionKind::AllowOnce)
            .map(Some)
            .ok_or_else(not_offered),
        PermissionChoice::AllowAlways => of_kind(PermissionKind::AllowAlways)
            .map(Some)
            .ok_or_else(not_offered),
        PermissionChoice::Reject => {
            Ok(of_kind(PermissionKind::RejectOnce)
                .or_else(|| of_kind(PermissionKind::RejectAlways)))
        }
        PermissionChoice::Option(id) => options
            .iter()
            .any(|o| &o.id == id)
            .then(|| Some(id.clone()))
            .ok_or_else(not_offered),
        PermissionChoice::Cancel => Ok(None),
    }
}

/// What the agent and its process report, in order.
#[derive(Debug)]
pub enum AcpEvent {
    Update {
        session: SessionId,
        update: SessionUpdate,
    },
    /// Needs an answer: [`crate::AcpAgent::answer_permission`]. The agent
    /// waits (its turn is paused) until then.
    Permission(PermissionRequest),
    /// The prompt turn started by [`crate::AcpAgent::prompt`] is over.
    TurnEnded {
        session: SessionId,
        result: Result<StopReason, AcpError>,
    },
    /// The agent's process is gone (crash, sign-out, shutdown). `code` is
    /// its exit code when known; `stderr_tail` its last output on stderr,
    /// for the error message (shown to the user, never logged).
    Exited {
        code: Option<i32>,
        stderr_tail: String,
    },
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn str_list(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_updates() {
        let u = SessionUpdate::from_value(&json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "Hi"}
        }));
        assert_eq!(
            u,
            SessionUpdate::AgentMessageChunk(Content::Text("Hi".into()))
        );

        let SessionUpdate::ToolCall(call) = SessionUpdate::from_value(&json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "t1",
            "title": "Edit report.docx",
            "kind": "edit",
            "status": "pending",
            "locations": [{"path": "/d/report.docx", "line": 3}],
            "content": [{"type": "diff", "path": "/d/a.txt", "oldText": null, "newText": "x"}],
            "rawInput": {"a": 1}
        })) else {
            panic!("tool call")
        };
        assert_eq!(call.id, "t1");
        assert_eq!(call.kind, ToolKind::Edit);
        assert_eq!(call.status, ToolStatus::Pending);
        assert_eq!(call.locations[0].line, Some(3));
        assert!(matches!(
            &call.content[0],
            ToolContent::Diff { old_text: None, .. }
        ));

        let SessionUpdate::ToolCallUpdate(u) = SessionUpdate::from_value(&json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "t1",
            "status": "completed",
            "kind": "brand_new_kind"
        })) else {
            panic!("update")
        };
        assert_eq!(u.status, Some(ToolStatus::Completed));
        assert_eq!(u.kind, Some(ToolKind::Other));
        assert_eq!(u.title, None);
        let mut call = call;
        call.apply(&u);
        assert_eq!(call.status, ToolStatus::Completed);
        assert_eq!(call.title, "Edit report.docx");

        let SessionUpdate::Plan(plan) = SessionUpdate::from_value(&json!({
            "sessionUpdate": "plan",
            "entries": [{"content": "Read", "priority": "high", "status": "in_progress"}]
        })) else {
            panic!("plan")
        };
        assert_eq!(plan[0].priority, PlanPriority::High);
        assert_eq!(plan[0].status, PlanStatus::InProgress);

        let SessionUpdate::AvailableCommands(cmds) = SessionUpdate::from_value(&json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [{"name": "review", "description": "Review", "input": {"hint": "what"}}]
        })) else {
            panic!("commands")
        };
        assert_eq!(cmds[0].input_hint.as_deref(), Some("what"));

        assert_eq!(
            SessionUpdate::from_value(
                &json!({"sessionUpdate": "current_mode_update", "currentModeId": "plan"})
            ),
            SessionUpdate::CurrentMode("plan".into())
        );
        assert!(matches!(
            SessionUpdate::from_value(&json!({"sessionUpdate": "usage_update", "used": 3})),
            SessionUpdate::Other { kind, .. } if kind == "usage_update"
        ));
    }

    #[test]
    fn parses_initialize_and_session() {
        let info = AgentInfo::from_value(&json!({
            "protocolVersion": 1,
            "agentCapabilities": {"loadSession": true, "promptCapabilities": {"image": true, "embeddedContext": true}},
            "authMethods": [
                {"id": "claude-login", "name": "Log in with Claude", "description": null},
                {"id": "tui", "name": "Terminal", "type": "terminal", "args": ["--login"], "env": {"A": "1"}}
            ],
            "agentInfo": {"name": "claude-agent-acp", "version": "0.110.1"}
        }));
        assert_eq!(info.protocol_version, 1);
        assert!(info.load_session && info.prompt_image && !info.prompt_audio);
        assert_eq!(info.auth_methods[0].kind, "agent");
        assert_eq!(info.auth_methods[1].terminal_args, ["--login"]);
        assert_eq!(
            info.auth_methods[1].terminal_env,
            [("A".to_owned(), "1".to_owned())]
        );
        assert_eq!(info.version.as_deref(), Some("0.110.1"));

        let s = Session::from_value(&json!({
            "sessionId": "s1",
            "modes": {"currentModeId": "default", "availableModes": [{"id": "default", "name": "Ask"}, {"id": "plan", "name": "Plan"}]}
        }))
        .unwrap();
        let modes = s.modes.unwrap();
        assert_eq!(modes.current, "default");
        assert_eq!(modes.available[1].id, "plan");
    }

    #[test]
    fn mcp_server_json_matches_the_schema() {
        let mut m = McpServer::kantor("/opt/Kantor/kantor");
        m.env = vec![("KANTOR_SOCKET_DIR".into(), "/tmp/k".into())];
        assert_eq!(
            m.to_json(),
            json!({
                "name": "kantor",
                "command": "/opt/Kantor/kantor",
                "args": ["mcp", "--no-launch"],
                "env": [{"name": "KANTOR_SOCKET_DIR", "value": "/tmp/k"}]
            })
        );
    }

    #[test]
    fn file_links() {
        #[cfg(not(windows))]
        {
            let b = PromptBlock::file(Path::new("/home/a b/Laporan Q3.docx"));
            assert_eq!(
                b.to_json(),
                json!({
                    "type": "resource_link",
                    "uri": "file:///home/a%20b/Laporan%20Q3.docx",
                    "name": "Laporan Q3.docx",
                    "mimeType": "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                })
            );
        }
        assert_eq!(
            file_uri(Path::new(r"C:\Users\Budi\Data.xlsx")),
            "file:///C:/Users/Budi/Data.xlsx"
        );
        assert_eq!(
            file_uri(Path::new(r"\\srv\share\x.pdf")),
            "file://srv/share/x.pdf"
        );
        assert_eq!(
            PromptBlock::text("hi").to_json(),
            json!({"type": "text", "text": "hi"})
        );
    }

    #[test]
    fn permission_choices() {
        let opts = |kinds: &[&str]| -> Vec<PermissionOption> {
            kinds
                .iter()
                .map(|k| PermissionOption {
                    id: format!("o-{k}"),
                    name: k.to_string(),
                    kind: PermissionKind::from_wire(k),
                })
                .collect()
        };
        let all = opts(&["allow_once", "allow_always", "reject_once"]);
        assert_eq!(
            resolve_choice(&all, &PermissionChoice::AllowAlways).unwrap(),
            Some("o-allow_always".into())
        );
        assert_eq!(
            resolve_choice(&all, &PermissionChoice::Reject).unwrap(),
            Some("o-reject_once".into())
        );
        let few = opts(&["allow_once", "reject_always"]);
        assert!(matches!(
            resolve_choice(&few, &PermissionChoice::AllowAlways),
            Err(AcpError::ChoiceNotOffered(_))
        ));
        assert_eq!(
            resolve_choice(&few, &PermissionChoice::Reject).unwrap(),
            Some("o-reject_always".into())
        );
        assert_eq!(
            resolve_choice(&opts(&["allow_once"]), &PermissionChoice::Reject).unwrap(),
            None
        );
        assert!(resolve_choice(&few, &PermissionChoice::Option("nope".into())).is_err());
        assert_eq!(
            resolve_choice(&few, &PermissionChoice::Cancel).unwrap(),
            None
        );
    }
}
