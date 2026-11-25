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
    self, Event, Limits, LocalSkill, Model, PromptSource, ToolOutput, ToolSpec, Tools,
};
use kantor_ai::{AiError, Api, Cancel, Client, ProviderConfig, ProviderId, UreqTransport};
use kantor_core::settings::AiSettings;
use serde_json::Value;

use super::model::JobKind;
use crate::harness::{App, DocTools};

/// A picture a tool showed the model, decoded for the panel.
pub struct Picture {
    pub label: String,
    /// About 240 px wide, for the row.
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

/// Where a job's skill comes from. Local folders for bring-your-own-key and
/// development ([`local_skills`]); Paparan AI's served skills plug in here
/// as another loader once the server has them.
pub type SkillLoader =
    Arc<dyn Fn(JobKind) -> Result<Box<dyn PromptSource + Send>, String> + Send + Sync>;

/// Skills from folders named after them under `dir`
/// (`dir/paparan-deck/SKILL.md`).
pub fn local_skills(dir: PathBuf) -> SkillLoader {
    Arc::new(move |kind: JobKind| {
        let folder = dir.join(kind.skill());
        let skill = LocalSkill::open(&folder)?;
        Ok(Box::new(skill) as Box<dyn PromptSource + Send>)
    })
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
        Err(e) => return fail(e, session(tools, history)),
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
    }
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

    fn check_outputs(&mut self, outputs: &[String]) -> Result<Vec<String>, String> {
        self.inner.check_outputs(outputs)
    }
}

/// Width of the pictures in the message list.
const THUMB_WIDTH: u32 = 240;
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
            copy_into(&job, &[src.clone()]).is_err(),
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
        let skills: SkillLoader = Arc::new(|_| Err("no skills here".to_owned()));
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
        assert!(agent_config(cfg(ProviderId::Anthropic, "https://api.anthropic.com")).is_err());
    }

    #[std::prelude::v1::test]
    fn skills_folder_setting() {
        let mut ai = AiSettings::default();
        assert!(skills_dir(&ai).ends_with(".claude/skills"));
        ai.skills_dir = Some("/opt/skills".into());
        assert_eq!(skills_dir(&ai), PathBuf::from("/opt/skills"));
    }
}
