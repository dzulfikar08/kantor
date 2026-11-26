//! A chat's job off the UI thread: its work folder, the files the user
//! attached, the skill, the model and the harness run. The panel starts
//! [`work`] on a worker thread and gets [`Msg`]s back.
//!
//! A job keeps its folder and its conversation ([`Session`]) across the
//! chat's follow-ups: "make slide 3 shorter" goes on where the deck was
//! made, with the same tools (so the files it made stay its own to change).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::RenderImage;
use kantor_ai::agent::{
    self, Checked, Event, Limits, LocalSkill, Model, PromptSource, ToolOutput, ToolSpec, Tools,
};
use kantor_ai::paparan::{Paparan, SkillError};
use kantor_ai::{AiError, Api, Cancel, Client, ProviderConfig, ProviderId, UreqTransport};
use kantor_core::settings::AiSettings;
use serde_json::Value;

use super::model::JobKind;
use crate::harness::{App, DocTools};

/// A picture a tool showed the model, decoded for the panel.
pub struct Picture {
    pub label: String,
    /// About 560 px wide, for the row.
    pub thumb: Arc<RenderImage>,
    /// As rendered (at most 1600 px wide), for the enlarged view.
    pub full: Arc<RenderImage>,
}

/// What the worker tells the panel, in order.
pub enum Msg {
    /// The work folder is ready and the attachments are in it (their names
    /// there).
    Ready {
        root: PathBuf,
        files: Vec<String>,
    },
    Event(Event),
    Pictures(Vec<Picture>),
}

/// A job between runs: its tools (folder and the user's own files) and
/// the conversation so far.
pub struct Session {
    pub kind: JobKind,
    tools: DocTools,
    history: Vec<Value>,
}

/// Where a job's skill comes from: Paparan AI's served skills
/// ([`served_skills`]) or local folders for bring-your-own-key and
/// development ([`local_skills`]). Called on the worker (it may fetch).
pub type SkillLoader =
    Arc<dyn Fn(JobKind) -> Result<Box<dyn PromptSource + Send>, SkillError> + Send + Sync>;

/// Skills from folders named after them under `dir`
/// (`dir/paparan-deck/SKILL.md`).
pub fn local_skills(dir: PathBuf) -> SkillLoader {
    Arc::new(move |kind: JobKind| {
        let folder = dir.join(kind.skill());
        let skill = LocalSkill::open(&folder).map_err(SkillError::Other)?;
        Ok(Box::new(skill) as Box<dyn PromptSource + Send>)
    })
}

/// [`local_skills`] for another provider than Paparan AI with no skills
/// folder set: a skill that isn't there means the job needs Paparan AI
/// (its skills are the paid part), not a missing folder.
pub fn local_or_paparan(dir: PathBuf) -> SkillLoader {
    let local = local_skills(dir.clone());
    Arc::new(move |kind: JobKind| {
        if !dir.join(kind.skill()).join("SKILL.md").is_file() {
            return Err(SkillError::NeedsPaparan {
                skill: kind.skill().to_owned(),
            });
        }
        local(kind)
    })
}

/// Credits one request in the panel may spend with Paparan AI (a
/// follow-up gets as many again). The jobs that finish well cost 50 to 250.
pub const MAX_CREDITS: f64 = 500.0;

/// The limits of a job run with `ai`'s provider: with Paparan AI, counted
/// in credits, with a ceiling and short reasoning.
pub fn limits(ai: &AiSettings) -> Limits {
    let mut limits = Limits::default();
    if ProviderId::from_key(&ai.provider) == Some(ProviderId::Paparan) {
        limits.prices = Some(kantor_ai::paparan::AGENT_PRICES);
        limits.max_cost = Some(MAX_CREDITS);
        limits.reasoning = Some(kantor_ai::paparan::AGENT_REASONING.to_owned());
    }
    limits
}

/// Paparan AI's skills, fetched for each job with the sign-in (`token`, or
/// the one saved in the keychain) and kept in memory only.
pub fn served_skills(api: Arc<Paparan>, token: Option<String>) -> SkillLoader {
    Arc::new(move |kind: JobKind| {
        let token = token
            .clone()
            .or_else(kantor_ai::paparan::load_token)
            .ok_or(SkillError::SignedOut)?;
        let skill = kantor_ai::paparan::load_skill(&api, &token, kind.skill())?;
        Ok(Box::new(skill) as Box<dyn PromptSource + Send>)
    })
}

/// The skills for a job: a skills folder set in settings.toml wins (an
/// override for development); otherwise Paparan AI serves its own when it's
/// the provider, and other providers (BYOK, Ollama) read `~/.claude/skills`.
pub fn skill_loader(ai: &AiSettings) -> SkillLoader {
    let set = ai
        .skills_dir
        .as_deref()
        .is_some_and(|d| !d.trim().is_empty());
    if !set && ProviderId::from_key(&ai.provider) == Some(ProviderId::Paparan) {
        let transport = UreqTransport::with_timeouts(
            std::time::Duration::from_secs(30),
            std::time::Duration::from_secs(60),
        );
        served_skills(Arc::new(Paparan::new(Arc::new(transport))), None)
    } else if set {
        local_skills(skills_dir(ai))
    } else {
        local_or_paparan(skills_dir(ai))
    }
}

/// The skills folder: the setting, else `~/.claude/skills`.
pub fn skills_dir(ai: &AiSettings) -> PathBuf {
    match ai.skills_dir.as_deref().map(str::trim) {
        Some(d) if !d.is_empty() => expand_home(d),
        _ => kantor_core::dirs::user_home_dir()
            .unwrap_or_default()
            .join(".claude")
            .join("skills"),
    }
}

fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => kantor_core::dirs::user_home_dir()
            .unwrap_or_default()
            .join(rest),
        None => PathBuf::from(p),
    }
}

/// Where new jobs' folders go: `<data dir>/paparan/jobs`.
pub fn jobs_root() -> PathBuf {
    kantor_core::dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("paparan")
        .join("jobs")
}

/// What the user asked for, to run on the worker.
pub struct Request {
    pub prompt: String,
    pub kind: JobKind,
    /// The folder the user picked for the first run; `None` makes one
    /// under [`jobs_root`].
    pub folder: Option<PathBuf>,
    pub attachments: Vec<PathBuf>,
    pub ai: AiSettings,
    pub skills: SkillLoader,
    pub limits: Limits,
    /// The job so far (follow-ups).
    pub session: Option<Session>,
    /// A scripted model instead of the provider (`KANTOR_CHAT_SCRIPT`).
    pub script: Option<PathBuf>,
}

/// How a run ended: why it failed (for the panel's Top up / Sign in), and
/// the job to go on with.
pub struct Done {
    pub error: Option<AiError>,
    pub session: Option<Session>,
    /// The job needs Paparan AI's skills (another provider, no local
    /// skill): the panel says so and offers Sign in.
    pub needs_paparan: bool,
}

/// Run one request: make or reuse the folder, copy the attachments, load
/// the skill, run (or resume) the agent. Blocking: on a worker thread.
pub fn work(req: Request, cancel: &Cancel, send: &dyn Fn(Msg)) -> Done {
    let fail = |message: String, session: Option<Session>| {
        send(Msg::Event(Event::Error {
            message: message.clone(),
        }));
        Done {
            error: Some(AiError::BadReply(message)),
            session,
            needs_paparan: false,
        }
    };
    let (tools, history, kind) = match req.session {
        // A follow-up: files attached to it join the user's own files.
        Some(mut s) => match copy_into(s.tools.root(), &req.attachments) {
            Ok(copied) => {
                for f in &copied {
                    s.tools.protect(Path::new(f));
                }
                send(Msg::Ready {
                    root: s.tools.root().to_path_buf(),
                    files: copied,
                });
                (s.tools, s.history, s.kind)
            }
            Err(e) => return fail(e, Some(s)),
        },
        None => {
            let dir = match req.folder {
                Some(d) => d,
                None => match new_job_dir(&jobs_root(), &req.prompt, chrono::Local::now()) {
                    Ok(d) => d,
                    Err(e) => return fail(format!("can't make the work folder: {e}"), None),
                },
            };
            // The attachments are the user's files too (copied before the
            // tools list the folder's files as the user's).
            let copied = match copy_into(&dir, &req.attachments) {
                Ok(c) => c,
                Err(e) => return fail(e, None),
            };
            let apps = App::for_skill(req.kind.skill());
            let tools = match DocTools::new(&dir, apps, true) {
                Ok(t) => t,
                Err(e) => return fail(format!("{e:#}"), None),
            };
            send(Msg::Ready {
                root: tools.root().to_path_buf(),
                files: copied,
            });
            (tools, Vec::new(), req.kind)
        }
    };
    let session = |tools, history| {
        Some(Session {
            kind,
            tools,
            history,
        })
    };
    let skill = match (req.skills)(kind) {
        Ok(s) => s,
        Err(e) => {
            send(Msg::Event(Event::Error {
                message: e.to_string(),
            }));
            return Done {
                error: Some(
                    e.as_ai_error()
                        .unwrap_or_else(|| AiError::BadReply(e.to_string())),
                ),
                session: session(tools, history),
                needs_paparan: matches!(e, SkillError::NeedsPaparan { .. }),
            };
        }
    };
    let model: Box<dyn Model> = match &req.script {
        Some(path) => match super::script::Played::load(path) {
            Ok(m) => Box::new(m),
            Err(e) => return fail(e, session(tools, history)),
        },
        None => match client(&req.ai) {
            Ok(c) => Box::new(c),
            Err(e) => {
                send(Msg::Event(Event::Error {
                    message: e.to_string(),
                }));
                return Done {
                    error: Some(e),
                    session: session(tools, history),
                    needs_paparan: false,
                };
            }
        },
    };
    let mut tools = tools;
    let outcome = {
        let mut recorder = Recorder {
            inner: &mut tools,
            send,
        };
        let mut on_event = |e: &Event| send(Msg::Event(e.clone()));
        let skill = Some(skill.as_ref() as &dyn PromptSource);
        if history.is_empty() {
            agent::run(
                model.as_ref(),
                &mut recorder,
                skill,
                &req.prompt,
                &req.limits,
                cancel,
                &mut on_event,
            )
        } else {
            agent::resume(
                model.as_ref(),
                &mut recorder,
                skill,
                history,
                &req.prompt,
                &req.limits,
                cancel,
                &mut on_event,
            )
        }
    };
    let error = match &outcome.ending {
        agent::Ending::Failed(e) => Some(e.clone()),
        _ => None,
    };
    Done {
        session: session(tools, outcome.messages),
        error,
        needs_paparan: false,
    }
}

/// Whether trying the same request again may work (offline, a time-out,
/// the server's own trouble).
pub fn retryable(e: Option<&AiError>) -> bool {
    matches!(
        e,
        Some(
            AiError::Network(_)
                | AiError::Timeout
                | AiError::Http {
                    status: 500..=599,
                    ..
                }
        )
    )
}

/// The client for the chosen provider, as the agent needs it: an
/// OpenAI-compatible API with tool calls. Ollama is reached through its
/// OpenAI-compatible endpoint.
fn client(ai: &AiSettings) -> Result<Client, AiError> {
    let cfg = ProviderConfig::from_settings(ai, |p| kantor_ai::keys::load_key(p.key()))?;
    let cfg = agent_config(cfg)?;
    // Long steps (a whole deck's JSON streamed back) need long time-outs.
    let transport = UreqTransport::with_timeouts(
        std::time::Duration::from_secs(300),
        std::time::Duration::from_secs(1200),
    );
    Ok(Client::new(cfg, Arc::new(transport)))
}

/// `cfg` as the agent can use it (see [`client`]).
pub fn agent_config(cfg: ProviderConfig) -> Result<ProviderConfig, AiError> {
    // Paparan AI's tier for agent jobs (vision and tool calls), whatever
    // model the other AI features use.
    if cfg.provider == ProviderId::Paparan {
        return Ok(ProviderConfig {
            model: kantor_ai::paparan::AGENT_MODEL.to_owned(),
            ..cfg
        });
    }
    match cfg.api() {
        Api::OpenAi => Ok(cfg),
        Api::Ollama => Ok(ProviderConfig {
            provider: ProviderId::Custom,
            base_url: format!("{}/v1", cfg.base_url.trim_end_matches('/')),
            ..cfg
        }),
        _ => Err(AiError::BadReply(format!(
            "{} can't run Paparan AI jobs yet: choose Paparan AI, an OpenAI-compatible provider or Ollama in AI Settings",
            cfg.provider.info().label
        ))),
    }
}

/// The harness's tools, telling the panel about the pictures they hand
/// the model (decoded here, off the UI thread).
struct Recorder<'a> {
    inner: &'a mut DocTools,
    send: &'a dyn Fn(Msg),
}

impl Tools for Recorder<'_> {
    fn specs(&self) -> Vec<ToolSpec> {
        self.inner.specs()
    }

    fn primer(&self) -> String {
        self.inner.primer()
    }

    fn call(&mut self, name: &str, args: &Value, cancel: &Cancel) -> ToolOutput {
        let out = self.inner.call(name, args, cancel);
        let pictures: Vec<Picture> = out
            .images
            .iter()
            .filter_map(|i| decode(&i.label, &i.bytes))
            .collect();
        if !pictures.is_empty() {
            (self.send)(Msg::Pictures(pictures));
        }
        out
    }

    fn check_outputs(&mut self, outputs: &[String]) -> Result<Checked, String> {
        self.inner.check_outputs(outputs)
    }

    fn made_outputs(&mut self) -> Checked {
        self.inner.made_outputs()
    }
}

/// Width of the pictures in the message list (twice what they show at,
/// for high-density screens).
const THUMB_WIDTH: u32 = 560;
const FULL_WIDTH: u32 = 1600;

/// A PNG or JPEG as pictures GPUI paints (BGRA).
fn decode(label: &str, bytes: &[u8]) -> Option<Picture> {
    let rgba = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let scaled = |max: u32| {
        if w <= max {
            rgba.clone()
        } else {
            let nh = ((h as f32 * max as f32 / w as f32).round() as u32).max(1);
            image::imageops::thumbnail(&rgba, max, nh)
        }
    };
    let render = |mut img: image::RgbaImage| {
        for px in img.pixels_mut() {
            px.0.swap(0, 2);
        }
        Arc::new(RenderImage::new(vec![image::Frame::new(img)]))
    };
    Some(Picture {
        label: label.to_owned(),
        thumb: render(scaled(THUMB_WIDTH)),
        full: render(scaled(FULL_WIDTH)),
    })
}

/// A new, empty folder for a job under `root`, named after when it started
/// and what it's about (`20261125-143000-buat-paparan-kinerja`).
pub fn new_job_dir(
    root: &Path,
    prompt: &str,
    now: chrono::DateTime<chrono::Local>,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let stamp = now.format("%Y%m%d-%H%M%S").to_string();
    let slug = slug(prompt);
    let base = if slug.is_empty() {
        stamp
    } else {
        format!("{stamp}-{slug}")
    };
    for n in 1..1000 {
        let name = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let dir = root.join(name);
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("too many jobs with the same name"))
}

/// The first words of `prompt` as a folder name: lower case letters and
/// digits joined by `-`, at most 40 characters.
pub fn slug(prompt: &str) -> String {
    let mut out = String::new();
    for word in prompt
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(6)
    {
        let word = word.to_ascii_lowercase();
        if out.len() + word.len() + 1 > 40 {
            break;
        }
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(&word);
    }
    out
}

/// Copy `files` into `dir`; returns their names there. A name already
/// taken gets a number (`data (2).xlsx`); a file already in `dir` stays
/// where it is.
pub fn copy_into(dir: &Path, files: &[PathBuf]) -> Result<Vec<String>, String> {
    let real_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let mut names = Vec::new();
    for f in files {
        if !f.is_file() {
            return Err(format!("{} is not a file", f.display()));
        }
        if let Ok(real) = f.canonicalize()
            && let Ok(rel) = real.strip_prefix(&real_dir)
        {
            names.push(rel.to_string_lossy().replace('\\', "/"));
            continue;
        }
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| format!("{} has no file name", f.display()))?;
        let dest = free_name(dir, &name);
        std::fs::copy(f, dir.join(&dest))
            .map_err(|e| format!("can't copy {}: {e}", f.display()))?;
        names.push(dest);
    }
    Ok(names)
}

/// `name`, or `stem (2).ext`, `stem (3).ext`, … when it's taken in `dir`.
fn free_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_owned();
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|n| !dir.join(n).exists())
        .unwrap_or_else(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;

    use super::*;

    #[std::prelude::v1::test]
    fn job_folders_are_new_and_named_after_the_request() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("jobs");
        let now = chrono::Local
            .with_ymd_and_hms(2026, 11, 25, 14, 30, 0)
            .single()
            .unwrap();
        let a = new_job_dir(&root, "Buat paparan: kinerja Q3 2026!", now).unwrap();
        assert_eq!(
            a.file_name().unwrap().to_string_lossy(),
            "20261125-143000-buat-paparan-kinerja-q3-2026"
        );
        assert!(a.is_dir());
        let b = new_job_dir(&root, "Buat paparan: kinerja Q3 2026!", now).unwrap();
        assert!(b.to_string_lossy().ends_with("-2026-2"), "{}", b.display());
        let c = new_job_dir(&root, "§§§", now).unwrap();
        assert_eq!(c.file_name().unwrap().to_string_lossy(), "20261125-143000");
        assert_eq!(
            slug(&"word ".repeat(30)).len(),
            "word-word-word-word-word-word".len()
        );
    }

    #[std::prelude::v1::test]
    fn attachments_are_copied_without_overwriting() {
        let tmp = tempfile::tempdir().unwrap();
        let job = tmp.path().join("job");
        std::fs::create_dir_all(&job).unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let data = src.join("data.xlsx");
        std::fs::write(&data, b"one").unwrap();
        std::fs::write(job.join("data.xlsx"), b"there").unwrap();
        std::fs::write(job.join("data (2).xlsx"), b"there too").unwrap();
        let inside = job.join("notes.md");
        std::fs::write(&inside, b"mine").unwrap();

        let names = copy_into(&job, &[data.clone(), inside]).unwrap();
        assert_eq!(
            names,
            vec!["data (3).xlsx".to_owned(), "notes.md".to_owned()]
        );
        assert_eq!(std::fs::read(job.join("data (3).xlsx")).unwrap(), b"one");
        assert_eq!(std::fs::read(job.join("data.xlsx")).unwrap(), b"there");
        assert!(
            copy_into(&job, std::slice::from_ref(&src)).is_err(),
            "folders aren't files"
        );
    }

    #[std::prelude::v1::test]
    fn a_follow_up_keeps_the_folder_and_the_users_files() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("brief.md");
        std::fs::write(&src, b"brief").unwrap();
        let job = tmp.path().join("job");
        std::fs::create_dir_all(&job).unwrap();
        let skills: SkillLoader = Arc::new(|_| Err(SkillError::Other("no skills here".to_owned())));
        let req = |session, attachments| Request {
            prompt: "buat paparan".into(),
            kind: JobKind::Deck,
            folder: Some(job.clone()),
            attachments,
            ai: AiSettings::default(),
            skills: skills.clone(),
            limits: Limits::default(),
            session,
            script: None,
        };
        let msgs = std::sync::Mutex::new(Vec::new());
        let send = |m: Msg| {
            let tag = match m {
                Msg::Ready { files, .. } => format!("ready {}", files.join(",")),
                Msg::Event(Event::Error { message }) => format!("error {message}"),
                Msg::Event(_) => "event".into(),
                Msg::Pictures(_) => "pictures".into(),
            };
            msgs.lock().unwrap().push(tag);
        };
        // No skill: the run fails, but the job (folder, files) is kept.
        let done = work(req(None, vec![src.clone()]), &Cancel::default(), &send);
        assert!(done.error.is_some());
        let mut session = done.session.expect("the job goes on");
        assert_eq!(session.tools.root(), job.canonicalize().unwrap());
        assert_eq!(
            *msgs.lock().unwrap(),
            vec![
                "ready brief.md".to_owned(),
                "error no skills here".to_owned()
            ]
        );
        // The attachment is one of the user's files: the tools won't
        // overwrite it.
        let out = session.tools.call(
            "write_file",
            &serde_json::json!({"path": "brief.md", "content": "x"}),
            &Cancel::default(),
        );
        assert!(out.error, "{}", out.text);
        // A follow-up with another attachment.
        session.history = vec![serde_json::json!({"role": "user", "content": "buat paparan"})];
        msgs.lock().unwrap().clear();
        let done = work(
            req(Some(session), vec![src.clone()]),
            &Cancel::default(),
            &send,
        );
        assert_eq!(
            msgs.lock().unwrap()[0],
            "ready brief (2).md",
            "copied beside the first"
        );
        let mut session = done.session.unwrap();
        assert_eq!(session.history.len(), 1, "a failed start keeps the history");
        let out = session.tools.call(
            "write_file",
            &serde_json::json!({"path": "brief (2).md", "content": "x"}),
            &Cancel::default(),
        );
        assert!(out.error, "{}", out.text);
    }

    #[std::prelude::v1::test]
    fn providers_for_the_agent() {
        let cfg = |provider, base: &str| ProviderConfig {
            provider,
            model: "m".into(),
            base_url: base.into(),
            api_key: None,
        };
        let ollama = agent_config(cfg(ProviderId::Ollama, "http://localhost:11434/")).unwrap();
        assert_eq!(ollama.base_url, "http://localhost:11434/v1");
        assert_eq!(ollama.api(), Api::OpenAi);
        let paparan = agent_config(cfg(ProviderId::Paparan, "https://x/api/kantor/v1")).unwrap();
        assert_eq!(paparan.provider, ProviderId::Paparan);
        assert_eq!(paparan.model, kantor_ai::paparan::AGENT_MODEL);
        assert!(agent_config(cfg(ProviderId::Anthropic, "https://api.anthropic.com")).is_err());
    }

    /// A Paparan server answering with canned replies, recording the URLs.
    struct FakeServer {
        replies: std::sync::Mutex<std::collections::VecDeque<Result<(u16, String), AiError>>>,
        seen: std::sync::Mutex<Vec<(String, bool)>>,
    }

    impl FakeServer {
        fn new(replies: Vec<Result<(u16, &str), AiError>>) -> Arc<Self> {
            Arc::new(Self {
                replies: std::sync::Mutex::new(
                    replies
                        .into_iter()
                        .map(|r| r.map(|(s, b)| (s, b.to_owned())))
                        .collect(),
                ),
                seen: std::sync::Mutex::new(Vec::new()),
            })
        }

        fn urls(&self) -> Vec<String> {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .map(|(u, _)| u.clone())
                .collect()
        }
    }

    impl kantor_ai::Transport for FakeServer {
        fn send(&self, req: &kantor_ai::HttpRequest) -> Result<kantor_ai::HttpResponse, AiError> {
            let bearer = req
                .headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v == "Bearer tok");
            self.seen.lock().unwrap().push((req.url.clone(), bearer));
            let (status, body) = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok((500, "{}".to_owned())))?;
            Ok(kantor_ai::HttpResponse::new(
                status,
                std::io::Cursor::new(body.into_bytes()),
            ))
        }
    }

    const ME: &str = r#"{"account":{"id":"u1","email":"a@b.id","name":"Ana"},"plan":"free","credits":{"remaining":300,"unit":"credit","low":false},"topup_url":"https://app.paparanai.id/topup"}"#;

    /// Run a job with Paparan's served skills and a scripted model that
    /// finishes at once.
    fn served_run(server: Arc<FakeServer>) -> (Done, Vec<String>) {
        let tmp = tempfile::tempdir().unwrap();
        let job = tmp.path().join("job");
        std::fs::create_dir_all(&job).unwrap();
        let script = tmp.path().join("script.json");
        std::fs::write(
            &script,
            r#"[{"delay_ms": 0, "calls": [{"name": "finish", "args": {"summary": "Selesai", "outputs": []}}]}]"#,
        )
        .unwrap();
        let api = Arc::new(Paparan::with_base(
            "https://p.example/api/kantor",
            server as Arc<dyn kantor_ai::Transport>,
        ));
        let req = Request {
            prompt: "buat paparan".into(),
            kind: JobKind::Deck,
            folder: Some(job),
            attachments: vec![],
            ai: AiSettings::default(),
            skills: served_skills(api, Some("tok".into())),
            limits: Limits::default(),
            session: None,
            script: Some(script),
        };
        let errors = std::sync::Mutex::new(Vec::new());
        let done = work(req, &Cancel::default(), &|m| {
            if let Msg::Event(Event::Error { message }) = m {
                errors.lock().unwrap().push(message);
            }
        });
        (done, errors.into_inner().unwrap())
    }

    #[std::prelude::v1::test]
    fn paparan_serves_the_skill_for_the_kind() {
        let skill = serde_json::json!({
            "name": "paparan-deck",
            "version": "v1",
            "files": {"SKILL.md": "---\nname: paparan-deck\n---\n# Deck\n", "references/charts.md": "c"}
        })
        .to_string();
        let server = FakeServer::new(vec![Ok((200, ME)), Ok((200, &skill))]);
        let (done, errors) = served_run(server.clone());
        assert!(done.error.is_none(), "{:?} {errors:?}", done.error);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            server.urls(),
            vec![
                "https://p.example/api/kantor/me".to_owned(),
                "https://p.example/api/kantor/skills/paparan-deck".to_owned(),
            ]
        );
        assert!(
            server
                .seen
                .lock()
                .unwrap()
                .iter()
                .all(|(_, bearer)| *bearer)
        );
        assert!(done.session.is_some(), "the job can go on");
    }

    #[std::prelude::v1::test]
    fn served_skill_errors_become_sign_in_top_up_and_retry() {
        // Signed out: Sign in.
        let (done, errors) = served_run(FakeServer::new(vec![Ok((
            401,
            r#"{"error":{"code":"unauthorized","message":"revoked"}}"#,
        ))]));
        assert_eq!(done.error, Some(AiError::SignedOut));
        assert!(errors[0].contains("sign in again"), "{errors:?}");
        // No credits: Top up, with the account's link.
        let (done, _) = served_run(FakeServer::new(vec![Ok((
            200,
            r#"{"credits":{"remaining":0},"topup_url":"https://app.paparanai.id/topup"}"#,
        ))]));
        assert_eq!(
            done.error,
            Some(AiError::InsufficientCredits {
                message: String::new(),
                topup_url: Some("https://app.paparanai.id/topup".into())
            })
        );
        // Offline: Try again.
        let (done, errors) = served_run(FakeServer::new(vec![Err(AiError::Network(
            "connection refused".into(),
        ))]));
        assert!(
            matches!(done.error, Some(AiError::Network(_))),
            "{:?}",
            done.error
        );
        assert!(errors[0].contains("offline"), "{errors:?}");
        assert!(retryable(done.error.as_ref()));
        // No such skill: says which there are; nothing to offer.
        let (done, errors) = served_run(FakeServer::new(vec![
            Ok((200, ME)),
            Ok((
                404,
                r#"{"error":{"code":"not_found","message":"No skill"}}"#,
            )),
            Ok((
                200,
                r#"{"skills":[{"name":"paparan-doc"},{"name":"paparan-sheet"}]}"#,
            )),
        ]));
        assert!(errors[0].contains("doc, sheet"), "{errors:?}");
        assert!(!retryable(done.error.as_ref()));
    }

    #[std::prelude::v1::test]
    fn paparan_gets_served_skills_unless_a_folder_is_set() {
        let ai = AiSettings {
            enabled: true,
            provider: "paparan".into(),
            ..AiSettings::default()
        };
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("paparan-deck")).unwrap();
        std::fs::write(tmp.path().join("paparan-deck/SKILL.md"), "# Deck").unwrap();
        let local = AiSettings {
            skills_dir: Some(tmp.path().to_string_lossy().into_owned()),
            ..ai.clone()
        };
        assert!(
            skill_loader(&local)(JobKind::Deck).is_ok(),
            "the folder wins"
        );
        let byok = AiSettings {
            provider: "openai".into(),
            skills_dir: local.skills_dir.clone(),
            ..ai
        };
        assert!(skill_loader(&byok)(JobKind::Deck).is_ok());
    }

    #[std::prelude::v1::test]
    fn other_providers_without_local_skills_need_paparan() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("paparan-deck")).unwrap();
        std::fs::write(tmp.path().join("paparan-deck/SKILL.md"), "# Deck").unwrap();
        let load = local_or_paparan(tmp.path().to_path_buf());
        assert!(load(JobKind::Deck).is_ok(), "a local skill works");
        let Err(e) = load(JobKind::Workbook) else {
            panic!("no paparan-sheet folder");
        };
        assert_eq!(
            e,
            SkillError::NeedsPaparan {
                skill: "paparan-sheet".into()
            }
        );
        assert!(e.to_string().contains("need Paparan AI"), "{e}");
        assert!(e.as_ai_error().is_none());

        // Paparan AI's limits count credits and keep reasoning short.
        let paparan = limits(&AiSettings {
            provider: "paparan".into(),
            ..AiSettings::default()
        });
        assert_eq!(paparan.max_cost, Some(MAX_CREDITS));
        assert_eq!(paparan.reasoning.as_deref(), Some("low"));
        let byok = limits(&AiSettings {
            provider: "openai".into(),
            ..AiSettings::default()
        });
        assert_eq!(byok.max_cost, None);
        assert_eq!(byok.reasoning, None);
    }

    #[std::prelude::v1::test]
    fn skills_folder_setting() {
        let mut ai = AiSettings::default();
        assert!(skills_dir(&ai).ends_with(".claude/skills"));
        ai.skills_dir = Some("/opt/skills".into());
        assert_eq!(skills_dir(&ai), PathBuf::from("/opt/skills"));
    }
}
