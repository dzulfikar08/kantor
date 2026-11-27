//! VBA projects (`vbaProject.bin`, [MS-OVBA]): the modules of a workbook's
//! (or document's) macros, their source code and procedures, read without
//! running anything; and the source of a module replaced, for an editor.
//!
//! A project is a compound file (MS-CFB). `VBA/dir` (compressed) lists the
//! project's code page, references and modules; each module's stream holds
//! a compiled cache (the "PerformanceCache", which Kantor ignores) and,
//! from its text offset on, the compressed source. The `PROJECT` stream
//! tells document modules (`Document=Sheet1/&H00000000`), classes and
//! forms (`BaseClass=UserForm1`) apart.
//!
//! Reading: [`VbaProject::read`], [`VbaProject::modules`],
//! [`Module::procedures`]. What isn't read: the compiled caches
//! (`_VBA_PROJECT`, `__SRP_*` and module PerformanceCaches, a
//! version-specific format with no public spec), UserForm designers (the
//! form's storage with its controls: a form is listed with its code only),
//! the project's password protection (`DPB`; the source isn't encrypted by
//! it, so it reads), digital signatures (they live outside this file),
//! and `.frx`-style binary resources.
//!
//! Writing: [`VbaProject::with_module_code`] replaces one module's source
//! and drops every compiled cache, as MS-OVBA describes for a project
//! without a cache (the `_VBA_PROJECT` stream reduced to its 7-byte
//! version-independent header, every module's text offset 0, `__SRP_*`
//! streams removed), so the host compiles the project from source when it
//! opens it. Whether Excel accepts such a rewritten project was not checked
//! with Excel itself: the layout follows MS-OVBA 2.3.4.1 and the public
//! "VBA purging" research that showed Office recompiling cache-less
//! projects; our own reader reads it back.
//!
//! [MS-OVBA]: https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-ovba/

use std::io::{Cursor, Read, Seek, Write};

pub mod codepage;
pub mod compression;
#[doc(hidden)]
pub mod dir;
mod procedures;
pub mod synth;

pub use procedures::{Procedure, ProcedureKind, scan as scan_procedures};

/// Why a project couldn't be read or written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    pub(crate) fn new(msg: impl Into<String>) -> Self {
        Error(msg.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// What a module is, as the Visual Basic Editor groups them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleKind {
    /// A standard module (`Module1`): macros and functions.
    Standard,
    /// A class module.
    Class,
    /// A document's own module: `ThisWorkbook`, `Sheet1` (events).
    Document,
    /// A UserForm's code (its designer, the form itself, isn't read).
    Form,
}

impl ModuleKind {
    /// English name, as the Visual Basic Editor's folders say.
    pub fn label(self) -> &'static str {
        match self {
            ModuleKind::Standard => "Module",
            ModuleKind::Class => "Class Module",
            ModuleKind::Document => "Document",
            ModuleKind::Form => "UserForm",
        }
    }

    /// Short lowercase name, for JSON.
    pub fn key(self) -> &'static str {
        match self {
            ModuleKind::Standard => "standard",
            ModuleKind::Class => "class",
            ModuleKind::Document => "document",
            ModuleKind::Form => "form",
        }
    }
}

/// One module of a project.
#[derive(Clone, Debug)]
pub struct Module {
    pub name: String,
    /// Its stream in the `VBA` storage (usually the name).
    pub stream_name: String,
    pub kind: ModuleKind,
    /// The source as the Visual Basic Editor shows it: without
    /// `Attribute` lines, lines ending in `\n`.
    pub code: String,
    /// The `Attribute …` lines (`Attribute VB_Name = "Module1"`, and
    /// procedures' such as `Attribute Macro1.VB_ProcData.VB_Invoke_Func`),
    /// in order.
    pub attributes: Vec<String>,
    /// The source as stored (attribute lines included, CRLF).
    pub source: String,
    /// The module's doc string (often empty).
    pub doc_string: String,
    /// `Option Private Module`-style flag from `dir` (MODULEPRIVATE).
    pub private: bool,
    pub read_only: bool,
    /// Why the source couldn't be read (the module is listed, empty).
    pub error: Option<String>,
    /// Where its MODULEOFFSET value is in the decompressed `dir`.
    offset_at: Option<usize>,
    text_offset: u32,
}

impl Module {
    /// The procedures in [`Module::code`] (lines count in it).
    pub fn procedures(&self) -> Vec<Procedure> {
        procedures::scan(&self.code)
    }

    /// The value of attribute `name` (`VB_Name`, `VB_Base`…), quotes
    /// removed.
    pub fn attribute(&self, name: &str) -> Option<String> {
        self.attributes.iter().find_map(|line| {
            let rest = line.trim_start().get(10..)?.trim_start();
            let (key, value) = rest.split_once('=')?;
            key.trim()
                .eq_ignore_ascii_case(name)
                .then(|| value.trim().trim_matches('"').to_owned())
        })
    }
}

/// What a reference points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceKind {
    /// A registered type library (`stdole`, `Office`, `Excel`…).
    Registered,
    /// Another VBA project (an add-in, a workbook).
    Project,
    /// An ActiveX control library (`MSForms`…).
    Control,
}

/// One entry of Tools > References.
#[derive(Clone, Debug)]
pub struct Reference {
    pub name: String,
    pub kind: ReferenceKind,
    /// The library id, e.g.
    /// `*\G{00020430-0000-0000-C000-000000000046}#2.0#0#…stdole2.tlb#OLE Automation`.
    pub libid: String,
}

/// A VBA project read from `vbaProject.bin`.
#[derive(Clone, Debug)]
pub struct VbaProject {
    bytes: Vec<u8>,
    /// The `VBA` storage's parent: `""` in `.xlsm`/`.docm` projects,
    /// `/_VBA_PROJECT_CUR` in `.xls` files.
    root: String,
    dir: Vec<u8>,
    pub name: String,
    /// The project's Windows code page (source, names).
    pub codepage: u16,
    /// 0 16-bit Windows, 1 32-bit Windows, 2 Macintosh, 3 64-bit Windows.
    pub sys_kind: u32,
    pub references: Vec<Reference>,
    modules: Vec<Module>,
}

/// The version-independent `_VBA_PROJECT` stream: no compiled cache
/// (MS-OVBA 2.3.4.1).
pub const EMPTY_VBA_PROJECT_STREAM: [u8; 7] = [0xCC, 0x61, 0xFF, 0xFF, 0x00, 0x00, 0x00];

fn read_stream<F: Read + Seek>(cfb: &mut cfb::CompoundFile<F>, path: &str) -> Result<Vec<u8>> {
    let mut s = cfb
        .open_stream(path)
        .map_err(|e| Error::new(format!("{path}: {e}")))?;
    let mut out = Vec::new();
    s.read_to_end(&mut out)
        .map_err(|e| Error::new(format!("{path}: {e}")))?;
    Ok(out)
}

/// A reference being read from `dir`: name (code page), name (UTF-16),
/// kind, library id.
type RawReference = (Vec<u8>, Option<String>, ReferenceKind, Vec<u8>);

/// Module being read from `dir`.
#[derive(Default)]
struct Raw {
    name: Vec<u8>,
    name_unicode: Option<String>,
    stream: Vec<u8>,
    stream_unicode: Option<String>,
    doc: Vec<u8>,
    procedural: bool,
    private: bool,
    read_only: bool,
    offset_at: Option<usize>,
    text_offset: u32,
}

impl VbaProject {
    /// Read a project from the bytes of `vbaProject.bin` (or an `.xls`
    /// file's compound file, whose project is under `_VBA_PROJECT_CUR`).
    /// A module whose source can't be read is listed with
    /// [`Module::error`]; a project without a readable `dir` is an error.
    pub fn read(bytes: &[u8]) -> Result<VbaProject> {
        let mut cfb = cfb::CompoundFile::open(Cursor::new(bytes))
            .map_err(|e| Error::new(format!("not a VBA project: {e}")))?;
        let root = if cfb.is_stream("/VBA/dir") {
            ""
        } else if cfb.is_stream("/_VBA_PROJECT_CUR/VBA/dir") {
            "/_VBA_PROJECT_CUR"
        } else {
            return Err(Error::new("not a VBA project: no VBA/dir stream"));
        };
        let dir = compression::decompress(&read_stream(&mut cfb, &format!("{root}/VBA/dir"))?)
            .map_err(|e| Error::new(format!("VBA/dir: {e}")))?;
        let records = dir::records(&dir)?;

        let mut code_page = 1252u16;
        let mut sys_kind = 0;
        let mut name = Vec::new();
        let mut references: Vec<RawReference> = Vec::new();
        let mut ref_name: (Vec<u8>, Option<String>) = (Vec::new(), None);
        let mut raws: Vec<Raw> = Vec::new();
        let mut current: Option<Raw> = None;
        let mut previous = 0u16;
        let mut extended_name = false;
        // A length-prefixed string at the start of `data`.
        let sized = |data: &[u8]| -> Vec<u8> {
            let n = dir::u32_at(data).unwrap_or(0) as usize;
            data.get(4..4 + n).unwrap_or_default().to_vec()
        };
        for r in &records {
            let data = &dir[r.start..r.start + r.len];
            match (r.id, current.as_mut()) {
                (dir::SYS_KIND, _) => sys_kind = dir::u32_at(data).unwrap_or(0),
                (dir::CODE_PAGE, _) => code_page = dir::u16_at(data).unwrap_or(1252),
                (dir::NAME, None) => name = data.to_vec(),
                // A control's extended name repeats the reference's.
                (dir::REFERENCE_NAME, None) => {
                    extended_name = previous == dir::REFERENCE_CONTROL;
                    if !extended_name {
                        ref_name = (data.to_vec(), None);
                    }
                }
                (dir::REFERENCE_NAME_UNICODE, None) if !extended_name => {
                    ref_name.1 = Some(dir::utf16(data));
                }
                (dir::REFERENCE_REGISTERED, None) => {
                    let n = std::mem::take(&mut ref_name);
                    references.push((n.0, n.1, ReferenceKind::Registered, sized(data)));
                }
                (dir::REFERENCE_PROJECT, None) => {
                    let n = std::mem::take(&mut ref_name);
                    references.push((n.0, n.1, ReferenceKind::Project, sized(data)));
                }
                // A control reference: its original library (when there is one,
                // first), then the control record with a "twiddled" libid.
                (dir::REFERENCE_ORIGINAL, None) => {
                    let n = std::mem::take(&mut ref_name);
                    references.push((n.0, n.1, ReferenceKind::Control, data.to_vec()));
                }
                (dir::REFERENCE_CONTROL, None) if previous != dir::REFERENCE_ORIGINAL => {
                    let n = std::mem::take(&mut ref_name);
                    references.push((n.0, n.1, ReferenceKind::Control, sized(data)));
                }
                (dir::MODULE_NAME, _) => {
                    if let Some(m) = current.take() {
                        raws.push(m);
                    }
                    current = Some(Raw {
                        name: data.to_vec(),
                        ..Raw::default()
                    });
                }
                (dir::MODULE_NAME_UNICODE, Some(m)) => m.name_unicode = Some(dir::utf16(data)),
                (dir::MODULE_STREAM_NAME, Some(m)) => m.stream = data.to_vec(),
                (dir::MODULE_STREAM_NAME_UNICODE, Some(m)) => {
                    m.stream_unicode = Some(dir::utf16(data));
                }
                (dir::MODULE_DOC_STRING, Some(m)) => m.doc = data.to_vec(),
                (dir::MODULE_OFFSET, Some(m)) => {
                    m.offset_at = Some(r.start);
                    m.text_offset = dir::u32_at(data).unwrap_or(0);
                }
                (dir::MODULE_TYPE_PROCEDURAL, Some(m)) => m.procedural = true,
                (dir::MODULE_TYPE_OTHER, Some(m)) => m.procedural = false,
                (dir::MODULE_PRIVATE, Some(m)) => m.private = true,
                (dir::MODULE_READ_ONLY, Some(m)) => m.read_only = true,
                (dir::MODULE_TERMINATOR, Some(_)) => raws.extend(current.take()),
                _ => {}
            }
            previous = r.id;
        }
        raws.extend(current.take());

        let text = |b: &[u8]| codepage::decode(code_page, b);
        // Module kinds from the PROJECT stream.
        let mut kinds: Vec<(String, ModuleKind)> = Vec::new();
        if let Ok(project) = read_stream(&mut cfb, &format!("{root}/PROJECT")) {
            for line in text(&project).lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    break;
                }
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                let kind = match key.trim() {
                    "Document" => ModuleKind::Document,
                    "Module" => ModuleKind::Standard,
                    "Class" => ModuleKind::Class,
                    "BaseClass" => ModuleKind::Form,
                    _ => continue,
                };
                let name = value.split('/').next().unwrap_or(value).trim();
                kinds.push((name.to_lowercase(), kind));
            }
        }

        let mut modules = Vec::with_capacity(raws.len());
        for raw in raws {
            let name = raw.name_unicode.clone().unwrap_or_else(|| text(&raw.name));
            let stream_name = raw
                .stream_unicode
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| text(&raw.stream));
            let kind = if raw.procedural {
                ModuleKind::Standard
            } else {
                let lower = name.to_lowercase();
                match kinds.iter().find(|(n, _)| *n == lower) {
                    Some((_, ModuleKind::Standard)) | None => ModuleKind::Class,
                    Some((_, k)) => *k,
                }
            };
            let mut module = Module {
                name,
                stream_name,
                kind,
                code: String::new(),
                attributes: Vec::new(),
                source: String::new(),
                doc_string: text(&raw.doc),
                private: raw.private,
                read_only: raw.read_only,
                error: None,
                offset_at: raw.offset_at,
                text_offset: raw.text_offset,
            };
            match module_source(&mut cfb, root, &module.stream_name, raw.text_offset) {
                Ok(bytes) => {
                    module.source = text(&bytes);
                    let (code, attributes) = split_attributes(&module.source);
                    module.code = code;
                    module.attributes = attributes;
                }
                Err(e) => module.error = Some(e.to_string()),
            }
            modules.push(module);
        }

        Ok(VbaProject {
            bytes: bytes.to_vec(),
            root: root.to_owned(),
            dir,
            name: text(&name),
            codepage: code_page,
            sys_kind,
            references: references
                .into_iter()
                .map(|(name, unicode, kind, libid)| Reference {
                    name: unicode.unwrap_or_else(|| text(&name)),
                    kind,
                    libid: text(&libid),
                })
                .collect(),
            modules,
        })
    }

    /// The modules, in the project's order.
    pub fn modules(&self) -> &[Module] {
        &self.modules
    }

    /// The module called `name` (any case).
    pub fn module(&self, name: &str) -> Option<&Module> {
        self.modules
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
    }

    /// The project's bytes as read.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The project with module `name`'s source replaced by `code` (as the
    /// editor shows it: lines ending in `\n` or `\r\n`, attribute lines
    /// optional), every compiled cache dropped (see the crate docs).
    ///
    /// `code` without attribute lines keeps the module's own: the header
    /// ones (`VB_Name`, `VB_Base`…) go first, and a procedure's (its
    /// description, its Alt+F8 shortcut) follow its header line when a
    /// procedure of that name is still there. Code with an `Attribute
    /// VB_Name` line is written as given.
    pub fn with_module_code(&self, name: &str, code: &str) -> Result<Vec<u8>> {
        let index = self
            .modules
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| Error::new(format!("there is no module {name}")))?;
        let module = &self.modules[index];
        let source = merge_attributes(module, code);
        let encoded = codepage::encode(self.codepage, &source).ok_or_else(|| {
            Error::new(format!(
                "the code has characters code page {} can't hold",
                self.codepage
            ))
        })?;

        let mut cfb = cfb::CompoundFile::open(Cursor::new(self.bytes.clone()))?;
        let vba = format!("{}/VBA", self.root);
        let mut dir = self.dir.clone();
        for (i, m) in self.modules.iter().enumerate() {
            if m.error.is_some() && i != index {
                continue;
            }
            let path = format!("{vba}/{}", m.stream_name);
            let compressed = if i == index {
                compression::compress(&encoded)
            } else {
                let stream = read_stream(&mut cfb, &path)?;
                stream
                    .get(m.text_offset as usize..)
                    .unwrap_or_default()
                    .to_vec()
            };
            write_stream(&mut cfb, &path, &compressed)?;
            if let Some(at) = m.offset_at {
                dir[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
            }
        }
        write_stream(
            &mut cfb,
            &format!("{vba}/dir"),
            &compression::compress(&dir),
        )?;
        write_stream(
            &mut cfb,
            &format!("{vba}/_VBA_PROJECT"),
            &EMPTY_VBA_PROJECT_STREAM,
        )?;
        let srp: Vec<String> = cfb
            .read_storage(&vba)?
            .filter(|e| e.is_stream() && e.name().to_ascii_uppercase().starts_with("__SRP_"))
            .map(|e| e.name().to_owned())
            .collect();
        for s in srp {
            cfb.remove_stream(format!("{vba}/{s}"))?;
        }
        cfb.flush()?;
        Ok(cfb.into_inner().into_inner())
    }
}

fn write_stream<F: Read + Write + Seek>(
    cfb: &mut cfb::CompoundFile<F>,
    path: &str,
    data: &[u8],
) -> Result<()> {
    let mut s = cfb
        .create_stream(path)
        .map_err(|e| Error::new(format!("{path}: {e}")))?;
    s.write_all(data)?;
    s.flush()?;
    Ok(())
}

/// A module's decompressed source bytes (trailing padding removed).
fn module_source<F: Read + Seek>(
    cfb: &mut cfb::CompoundFile<F>,
    root: &str,
    stream: &str,
    offset: u32,
) -> Result<Vec<u8>> {
    let data = read_stream(cfb, &format!("{root}/VBA/{stream}"))?;
    let Some(compressed) = data.get(offset as usize..) else {
        return Err(Error::new(format!(
            "text offset {offset} is past the end of stream {stream} ({} bytes)",
            data.len()
        )));
    };
    if compressed.is_empty() {
        return Ok(Vec::new());
    }
    let mut bytes = compression::decompress(compressed)?;
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    Ok(bytes)
}

fn is_attribute(line: &str) -> bool {
    let t = line.trim_start();
    t.len() > 10
        && t.get(..10)
            .is_some_and(|p| p.eq_ignore_ascii_case("Attribute "))
}

/// `(code, attribute lines)` of stored source.
fn split_attributes(source: &str) -> (String, Vec<String>) {
    let mut code = Vec::new();
    let mut attributes = Vec::new();
    for line in source.lines() {
        if is_attribute(line) {
            attributes.push(line.to_owned());
        } else {
            code.push(line);
        }
    }
    let mut code = code.join("\n");
    if !code.is_empty() {
        code.push('\n');
    }
    (code, attributes)
}

/// The source to store for `code` in `module` (CRLF line ends).
fn merge_attributes(module: &Module, code: &str) -> String {
    let code = code.replace("\r\n", "\n");
    let mut lines: Vec<String> = Vec::new();
    let has_own = code
        .lines()
        .find(|l| !l.trim().is_empty())
        .is_some_and(|l| is_attribute(l) && l.contains("VB_Name"));
    if has_own {
        lines.extend(code.lines().map(str::to_owned));
    } else {
        // The header: the attribute lines before the first code line.
        let mut header = Vec::new();
        let mut per_proc: Vec<(String, String)> = Vec::new();
        let mut in_header = true;
        for line in module.source.lines() {
            if is_attribute(line) {
                if in_header {
                    header.push(line.to_owned());
                } else {
                    let rest = line.trim_start()[10..].trim_start();
                    if let Some((proc_name, _)) = rest.split_once('.') {
                        per_proc.push((proc_name.trim().to_lowercase(), line.to_owned()));
                    }
                }
            } else {
                in_header = false;
            }
        }
        lines.extend(header);
        let procs = procedures::scan(&code);
        for (i, line) in code.lines().enumerate() {
            lines.push(line.to_owned());
            // After a procedure's header (its last physical line).
            if let Some(p) = procs.iter().find(|p| header_end(&code, p.line) == i) {
                let lower = p.name.to_lowercase();
                lines.extend(
                    per_proc
                        .iter()
                        .filter(|(n, _)| *n == lower)
                        .map(|(_, l)| l.clone()),
                );
            }
        }
    }
    let mut out = lines.join("\r\n");
    out.push_str("\r\n");
    out
}

/// The last physical line of the header starting at `line` (continued
/// with ` _`).
fn header_end(code: &str, line: usize) -> usize {
    let mut at = line;
    for l in code.lines().skip(line) {
        if l.trim_end().ends_with(" _") {
            at += 1;
        } else {
            break;
        }
    }
    at
}
