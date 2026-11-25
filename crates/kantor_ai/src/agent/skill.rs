//! Where the agent's instructions come from.
//!
//! A skill is a folder: `SKILL.md` (sent as the system prompt) plus
//! `references/` and `examples/` the model reads on demand with
//! `read_skill_file`. Only SKILL.md goes up front: the rest costs tokens on
//! every step once read, so it is read only when the job needs it.
//!
//! The source is pluggable: [`LocalSkill`] reads a folder on disk (BYOK and
//! development); [`ServedSkill`] holds a skill served by the Paparan
//! backend, which keeps the paid prompts on the server (never compiled into
//! the binary, never cached on disk).

use std::path::{Component, Path, PathBuf};

/// One file of a skill, as the model gets it.
#[derive(Clone, Debug, PartialEq)]
pub enum SkillFile {
    Text(String),
    /// A PNG or JPEG picture (shown to the model as an image).
    Image {
        mime: &'static str,
        bytes: Vec<u8>,
    },
}

/// Instructions for the agent: the system prompt and the files it may read.
pub trait PromptSource {
    /// The skill's name (`paparan-deck`).
    fn name(&self) -> String;
    /// The main instructions (SKILL.md without its front matter).
    fn system(&self) -> Result<String, String>;
    /// The other files the model may ask for, with a size hint
    /// (`references/charts.md (6 KB)`).
    fn files(&self) -> Vec<String>;
    /// One file, by a path relative to the skill (paths as the skill text
    /// writes them, such as `~/.claude/skills/NAME/references/x.md`, work
    /// too).
    fn read(&self, path: &str) -> Result<SkillFile, String>;
}

/// The longest skill file read (text or picture).
const MAX_FILE: u64 = 4 * 1024 * 1024;

/// A skill folder on disk.
pub struct LocalSkill {
    dir: PathBuf,
    name: String,
}

impl LocalSkill {
    /// The skill in `dir` (which must hold SKILL.md).
    pub fn open(dir: &Path) -> Result<Self, String> {
        let dir = dir
            .canonicalize()
            .map_err(|e| format!("can't open the skill folder {}: {e}", dir.display()))?;
        let md = dir.join("SKILL.md");
        let text = std::fs::read_to_string(&md)
            .map_err(|e| format!("{} has no readable SKILL.md: {e}", dir.display()))?;
        let name = front_matter_name(&text)
            .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "skill".to_owned());
        Ok(Self { dir, name })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file `path` names, inside this skill or a sibling skill
    /// (`~/.claude/skills/paparan-deck/references/charts.md` from the
    /// sheet skill), never outside them.
    fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        let mut p = path.trim().replace('\\', "/");
        for prefix in ["~/.claude/skills/", ".claude/skills/"] {
            if let Some(i) = p.find(prefix) {
                p = p[i + prefix.len()..].to_owned();
                break;
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            let home = format!("{}/.claude/skills/", home.to_string_lossy());
            if let Some(rest) = p.strip_prefix(&home) {
                p = rest.to_owned();
            }
        }
        let own = format!("{}/", self.name);
        let rel: PathBuf = if let Some(rest) = p.strip_prefix(&own) {
            PathBuf::from(rest)
        } else {
            PathBuf::from(p.trim_start_matches("./"))
        };
        if rel.as_os_str().is_empty()
            || rel.components().any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(format!(
                "\"{path}\" is not a file of this skill; give a path like references/x.md"
            ));
        }
        let mine = self.dir.join(&rel);
        if mine.is_file() {
            return Ok(mine);
        }
        // A sibling skill's file (`paparan-deck/references/charts.md`).
        if let Some(root) = self.dir.parent() {
            let mut comps = rel.components();
            if let Some(Component::Normal(first)) = comps.next()
                && root.join(first).join("SKILL.md").is_file()
            {
                let other = root.join(&rel);
                if other.is_file() {
                    return Ok(other);
                }
            }
        }
        Err(format!(
            "no file \"{path}\" in the skill; its files are: {}",
            self.files().join(", ")
        ))
    }
}

/// `name:` in SKILL.md's front matter.
fn front_matter_name(text: &str) -> Option<String> {
    let body = text.strip_prefix("---")?;
    let end = body.find("\n---")?;
    body[..end].lines().find_map(|l| {
        l.trim()
            .strip_prefix("name:")
            .map(|n| n.trim().trim_matches('"').to_owned())
            .filter(|n| !n.is_empty())
    })
}

/// SKILL.md without its front matter.
pub fn strip_front_matter(text: &str) -> &str {
    if let Some(body) = text.strip_prefix("---")
        && let Some(end) = body.find("\n---")
    {
        let rest = &body[end + 4..];
        return rest.trim_start_matches(|c| c != '\n').trim_start();
    }
    text
}

fn size_hint(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

fn walk(dir: &Path, rel: &Path, depth: usize, out: &mut Vec<(PathBuf, u64)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = e.path();
        let rel = rel.join(&name);
        if path.is_dir() {
            if depth < 3 {
                walk(&path, &rel, depth + 1, out);
            }
        } else if let Ok(m) = e.metadata() {
            out.push((rel, m.len()));
        }
    }
}

impl PromptSource for LocalSkill {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn system(&self) -> Result<String, String> {
        let text = std::fs::read_to_string(self.dir.join("SKILL.md"))
            .map_err(|e| format!("can't read SKILL.md: {e}"))?;
        Ok(strip_front_matter(&text).to_owned())
    }

    fn files(&self) -> Vec<String> {
        let mut all = Vec::new();
        walk(&self.dir, Path::new(""), 0, &mut all);
        all.into_iter()
            .filter(|(p, _)| p != Path::new("SKILL.md"))
            .take(80)
            .map(|(p, n)| {
                format!(
                    "{} ({})",
                    p.to_string_lossy().replace('\\', "/"),
                    size_hint(n)
                )
            })
            .collect()
    }

    fn read(&self, path: &str) -> Result<SkillFile, String> {
        let file = self.resolve(path)?;
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if len > MAX_FILE {
            return Err(format!("{path} is too large to read ({})", size_hint(len)));
        }
        let ext = file
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let bytes = std::fs::read(&file).map_err(|e| format!("can't read {path}: {e}"))?;
        match ext.as_str() {
            "png" => Ok(SkillFile::Image {
                mime: "image/png",
                bytes,
            }),
            "jpg" | "jpeg" => Ok(SkillFile::Image {
                mime: "image/jpeg",
                bytes,
            }),
            "pdf" | "pptx" | "docx" | "xlsx" | "zip" => Err(format!(
                "{path} is a binary file; read its source or its PNG instead"
            )),
            _ => Ok(SkillFile::Text(
                String::from_utf8_lossy(&bytes).into_owned(),
            )),
        }
    }
}

/// A skill served by the Paparan backend (`GET skills/NAME`): the paid
/// prompts stay on the server and are fetched per job. The files are kept in
/// memory for the run only (never written to disk) and are all text.
///
/// Paths are relative to the skill (`references/charts.md`). A file of
/// another skill that this one points at is served under that skill's name
/// (`paparan-deck/references/charts.md` in the sheet skill).
pub struct ServedSkill {
    name: String,
    version: String,
    files: std::collections::BTreeMap<String, String>,
}

impl ServedSkill {
    /// A skill from its files (`SKILL.md` required).
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        files: std::collections::BTreeMap<String, String>,
    ) -> Result<Self, String> {
        let name = name.into();
        if !files.contains_key("SKILL.md") {
            return Err(format!("the served skill {name} has no SKILL.md"));
        }
        Ok(Self {
            name,
            version: version.into(),
            files,
        })
    }

    /// The server's content hash of the skill.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The key `path` names: `~/.claude/skills/NAME/x`, `NAME/x`, `./x`
    /// and `x` all name `x` of this skill.
    fn resolve(&self, path: &str) -> Result<&str, String> {
        let mut p = path.trim().replace('\\', "/");
        for prefix in ["~/.claude/skills/", ".claude/skills/"] {
            if let Some(i) = p.find(prefix) {
                p = p[i + prefix.len()..].to_owned();
                break;
            }
        }
        let mut p = p.trim_start_matches("./");
        if let Some(rest) = p.strip_prefix(&format!("{}/", self.name)) {
            p = rest;
        }
        if p.is_empty()
            || p.starts_with('/')
            || p.split('/').any(|c| c.is_empty() || c == "." || c == "..")
        {
            return Err(format!(
                "\"{path}\" is not a file of this skill; give a path like references/x.md"
            ));
        }
        match self.files.get_key_value(p) {
            Some((k, _)) => Ok(k.as_str()),
            None => Err(format!(
                "no file \"{path}\" in the skill; its files are: {}",
                self.files
                    .keys()
                    .filter(|k| *k != "SKILL.md")
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

impl PromptSource for ServedSkill {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn system(&self) -> Result<String, String> {
        Ok(strip_front_matter(&self.files["SKILL.md"]).to_owned())
    }

    fn files(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|(p, _)| *p != "SKILL.md")
            .take(80)
            .map(|(p, t)| format!("{p} ({})", size_hint(t.len() as u64)))
            .collect()
    }

    fn read(&self, path: &str) -> Result<SkillFile, String> {
        let key = self.resolve(path)?;
        Ok(SkillFile::Text(self.files[key].clone()))
    }
}
