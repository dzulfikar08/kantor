//! Writing a new VBA project from module sources, laid out as Excel writes
//! one (`PROJECT`, `PROJECTwm`, `VBA/dir`, `VBA/_VBA_PROJECT`, a stream per
//! module, a storage per UserForm). Tests use it in place of binary
//! fixtures; with [`Builder::with_caches`] the modules carry stand-in
//! compiled caches (a PerformanceCache before the source, `__SRP_*`
//! streams), as real projects do.

use std::io::{Cursor, Write};

use crate::dir::{self, push, utf16_bytes};
use crate::{Error, ModuleKind, Result, codepage, compression};

/// The CLSID projects written here carry (`ID=` in `PROJECT`).
pub const PROJECT_ID: &str = "{5F3C1A2B-7D4E-4C8B-9A6F-0E1D2C3B4A59}";

/// The `dir` records of one module (MS-OVBA 2.3.4.2.3.2), its source at
/// `offset` in its stream (named like the module).
pub(crate) fn module_records(
    d: &mut Vec<u8>,
    mbcs: &[u8],
    name: &str,
    kind: ModuleKind,
    offset: u32,
) {
    push(d, dir::MODULE_NAME, mbcs);
    push(d, dir::MODULE_NAME_UNICODE, &utf16_bytes(name));
    push(d, dir::MODULE_STREAM_NAME, mbcs);
    push(d, dir::MODULE_STREAM_NAME_UNICODE, &utf16_bytes(name));
    push(d, dir::MODULE_DOC_STRING, b"");
    push(d, dir::MODULE_DOC_STRING_UNICODE, b"");
    push(d, dir::MODULE_OFFSET, &offset.to_le_bytes());
    push(d, dir::MODULE_HELP_CONTEXT, &0u32.to_le_bytes());
    push(d, dir::MODULE_COOKIE, &0xFFFFu16.to_le_bytes());
    let ty = if kind == ModuleKind::Standard {
        dir::MODULE_TYPE_PROCEDURAL
    } else {
        dir::MODULE_TYPE_OTHER
    };
    push(d, ty, b"");
    push(d, dir::MODULE_TERMINATOR, b"");
}

/// A `PROJECT` stream line naming a module (`Module=Module1`).
pub(crate) fn project_line(name: &str, kind: ModuleKind) -> String {
    match kind {
        ModuleKind::Standard => format!("Module={name}"),
        ModuleKind::Class => format!("Class={name}"),
        ModuleKind::Document => format!("Document={name}/&H00000000"),
        ModuleKind::Form => format!("BaseClass={name}"),
    }
}

/// A project to write.
#[derive(Clone, Debug)]
pub struct Builder {
    name: String,
    codepage: u16,
    caches: bool,
    modules: Vec<(String, ModuleKind, String)>,
    forms: Vec<crate::forms::FormDesign>,
}

impl Builder {
    /// An empty project called `VBAProject` in code page `codepage`.
    pub fn new(codepage: u16) -> Self {
        Builder {
            name: "VBAProject".into(),
            codepage,
            caches: false,
            modules: Vec::new(),
            forms: Vec::new(),
        }
    }

    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_owned();
        self
    }

    /// Add a module. Its default attribute lines (`VB_Name`, a document's
    /// `VB_Base`…) go first unless `code` starts with `Attribute VB_Name`.
    pub fn module(mut self, name: &str, kind: ModuleKind, code: &str) -> Self {
        self.modules.push((name.to_owned(), kind, code.to_owned()));
        self
    }

    /// A UserForm with its designer: a `Form` module called `design.name`
    /// with `code`, and the form's storage written by
    /// [`crate::forms::write_streams`].
    pub fn form(mut self, design: &crate::forms::FormDesign, code: &str) -> Self {
        self.modules
            .push((design.name.clone(), ModuleKind::Form, code.to_owned()));
        self.forms.push(design.clone());
        self
    }

    /// Stand-in compiled caches: each module's stream starts with bytes
    /// before its source (a non-zero text offset), and `__SRP_0`/`__SRP_1`
    /// streams are written.
    pub fn with_caches(mut self) -> Self {
        self.caches = true;
        self
    }

    /// The stored source of a module (attribute lines, CRLF).
    pub fn source(name: &str, kind: ModuleKind, code: &str) -> String {
        let code = code.replace("\r\n", "\n");
        let mut lines: Vec<String> = Vec::new();
        if !code.trim_start().starts_with("Attribute VB_Name") {
            lines.push(format!("Attribute VB_Name = \"{name}\""));
            let extra: &[&str] = match kind {
                ModuleKind::Standard => &[],
                ModuleKind::Class => &[
                    "Attribute VB_GlobalNameSpace = False",
                    "Attribute VB_Creatable = False",
                    "Attribute VB_PredeclaredId = False",
                    "Attribute VB_Exposed = False",
                ],
                // The workbook's module (ThisWorkbook) is based on Excel's
                // Workbook class, the others on its Worksheet class.
                ModuleKind::Document if name.eq_ignore_ascii_case("ThisWorkbook") => &[
                    "Attribute VB_Base = \"0{00020819-0000-0000-C000-000000000046}\"",
                    "Attribute VB_GlobalNameSpace = False",
                    "Attribute VB_Creatable = False",
                    "Attribute VB_PredeclaredId = True",
                    "Attribute VB_Exposed = True",
                    "Attribute VB_TemplateDerived = False",
                    "Attribute VB_Customizable = True",
                ],
                ModuleKind::Document => &[
                    "Attribute VB_Base = \"0{00020820-0000-0000-C000-000000000046}\"",
                    "Attribute VB_GlobalNameSpace = False",
                    "Attribute VB_Creatable = False",
                    "Attribute VB_PredeclaredId = True",
                    "Attribute VB_Exposed = True",
                    "Attribute VB_TemplateDerived = False",
                    "Attribute VB_Customizable = True",
                ],
                ModuleKind::Form => &[
                    "Attribute VB_Base = \"0{6E4A2C1F-0B57-4F8A-9C3D-1A2B3C4D5E6F}{0F1E2D3C-4B5A-6978-8796-A5B4C3D2E1F0}\"",
                    "Attribute VB_GlobalNameSpace = False",
                    "Attribute VB_Creatable = False",
                    "Attribute VB_PredeclaredId = True",
                    "Attribute VB_Exposed = False",
                    "Attribute VB_TemplateDerived = False",
                    "Attribute VB_Customizable = False",
                ],
            };
            lines.extend(extra.iter().map(|s| (*s).to_owned()));
        }
        lines.extend(code.lines().map(str::to_owned));
        let mut out = lines.join("\r\n");
        out.push_str("\r\n");
        out
    }

    /// The `vbaProject.bin` bytes.
    pub fn build(&self) -> Result<Vec<u8>> {
        let cp = self.codepage;
        let mbcs = |s: &str| -> Result<Vec<u8>> {
            codepage::encode(cp, s)
                .ok_or_else(|| Error::new(format!("{s:?} doesn't fit code page {cp}")))
        };
        let mut cfb = cfb::CompoundFile::create(Cursor::new(Vec::new()))?;
        cfb.create_storage("/VBA")?;

        // dir: PROJECTINFORMATION
        let mut d = Vec::new();
        push(&mut d, dir::SYS_KIND, &3u32.to_le_bytes());
        push(&mut d, dir::LCID, &0x409u32.to_le_bytes());
        push(&mut d, dir::LCID_INVOKE, &0x409u32.to_le_bytes());
        push(&mut d, dir::CODE_PAGE, &cp.to_le_bytes());
        push(&mut d, dir::NAME, &mbcs(&self.name)?);
        push(&mut d, dir::DOC_STRING, b"");
        push(&mut d, dir::DOC_STRING_UNICODE, b"");
        push(&mut d, dir::HELP_FILE, b"");
        push(&mut d, dir::HELP_FILE_2, b"");
        push(&mut d, dir::HELP_CONTEXT, &0u32.to_le_bytes());
        push(&mut d, dir::LIB_FLAGS, &0u32.to_le_bytes());
        // PROJECTVERSION: a reserved 4 where the size would be, then 6 bytes.
        d.extend_from_slice(&dir::VERSION.to_le_bytes());
        d.extend_from_slice(&4u32.to_le_bytes());
        d.extend_from_slice(&1_234_567_890u32.to_le_bytes());
        d.extend_from_slice(&7u16.to_le_bytes());
        push(&mut d, dir::CONSTANTS, b"");
        push(&mut d, dir::CONSTANTS_UNICODE, b"");
        // PROJECTREFERENCES: stdole, as every project has.
        let stdole = b"*\\G{00020430-0000-0000-C000-000000000046}#2.0#0#C:\\Windows\\System32\\stdole2.tlb#OLE Automation";
        push(&mut d, dir::REFERENCE_NAME, b"stdole");
        push(&mut d, dir::REFERENCE_NAME_UNICODE, &utf16_bytes("stdole"));
        let mut reg = Vec::new();
        reg.extend_from_slice(&(stdole.len() as u32).to_le_bytes());
        reg.extend_from_slice(stdole);
        reg.extend_from_slice(&0u32.to_le_bytes());
        reg.extend_from_slice(&0u16.to_le_bytes());
        push(&mut d, dir::REFERENCE_REGISTERED, &reg);
        // PROJECTMODULES
        push(
            &mut d,
            dir::MODULES,
            &(self.modules.len() as u16).to_le_bytes(),
        );
        push(&mut d, dir::COOKIE, &0xFFFFu16.to_le_bytes());

        let mut project = format!("ID=\"{PROJECT_ID}\"\r\n");
        let mut wm = Vec::new();
        let mut workspace = String::new();
        let mut has_form = false;
        for (n, (name, kind, code)) in self.modules.iter().enumerate() {
            let source = mbcs(&Self::source(name, *kind, code))?;
            let cache: Vec<u8> = if self.caches {
                (0..200 + n * 37).map(|i| (i * 31 + n) as u8).collect()
            } else {
                Vec::new()
            };
            let mut stream = cache.clone();
            stream.extend_from_slice(&compression::compress(&source));
            {
                let mut s = cfb.create_stream(format!("/VBA/{name}"))?;
                s.write_all(&stream)?;
            }

            module_records(&mut d, &mbcs(name)?, name, *kind, cache.len() as u32);

            project.push_str(&project_line(name, *kind));
            project.push_str("\r\n");
            if *kind == ModuleKind::Form {
                has_form = true;
                // The designer: a storage named after the form.
                cfb.create_storage(format!("/{name}"))?;
                let design = self
                    .forms
                    .iter()
                    .find(|d| d.name == *name)
                    .cloned()
                    .unwrap_or_else(|| crate::forms::FormDesign::new(name));
                for (path, data) in crate::forms::write_streams(&design) {
                    if let Some((dir, _)) = path.rsplit_once('/') {
                        let mut at = format!("/{name}");
                        for part in dir.split('/') {
                            at = format!("{at}/{part}");
                            if !cfb.is_storage(&at) {
                                cfb.create_storage(&at)?;
                            }
                        }
                    }
                    let mut s = cfb.create_stream(format!("/{name}/{path}"))?;
                    s.write_all(&data)?;
                }
            }
            wm.extend_from_slice(&mbcs(name)?);
            wm.push(0);
            wm.extend_from_slice(&utf16_bytes(name));
            wm.extend_from_slice(&[0, 0]);
            workspace.push_str(&format!("{name}=0, 0, 0, 0, C\r\n"));
        }
        d.extend_from_slice(&dir::TERMINATOR.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        wm.extend_from_slice(&[0, 0]);

        if has_form {
            project.push_str("Package={AC9F2F90-E877-11CE-9F68-00AA00574A4F}\r\n");
        }
        project.push_str(&format!(
            "Name=\"{}\"\r\nHelpContextID=\"0\"\r\nVersionCompatible32=\"393222000\"\r\n\
             CMG=\"{}\"\r\nDPB=\"{}\"\r\nGC=\"{}\"\r\n\r\n\
             [Host Extender Info]\r\n&H00000001={{3832D640-CF90-11CF-8E43-00A0C911005A}};VBE;&H00000000\r\n\r\n\
             [Workspace]\r\n{workspace}",
            self.name,
            // Not protected, no password, visible (MS-OVBA 2.3.1.15–17).
            crate::crypt::encrypt(PROJECT_ID, 0x2C, &[0; 4]),
            crate::crypt::encrypt(PROJECT_ID, 0x47, &[0]),
            crate::crypt::encrypt(PROJECT_ID, 0x9A, &[0xFF]),
        ));

        let mut vba_project = crate::EMPTY_VBA_PROJECT_STREAM.to_vec();
        if self.caches {
            // A version-specific header and cache, as Excel writes.
            vba_project[2..4].copy_from_slice(&0x00B2u16.to_le_bytes());
            vba_project.extend((0..300).map(|i| (i % 251) as u8));
        }
        let mut streams: Vec<(String, Vec<u8>)> = vec![
            ("/PROJECT".into(), mbcs(&project)?),
            ("/PROJECTwm".into(), wm),
            ("/VBA/dir".into(), compression::compress(&d)),
            ("/VBA/_VBA_PROJECT".into(), vba_project),
        ];
        if self.caches {
            streams.push(("/VBA/__SRP_0".into(), vec![1; 120]));
            streams.push(("/VBA/__SRP_1".into(), vec![2; 40]));
        }
        for (path, data) in streams {
            let mut s = cfb.create_stream(&path)?;
            s.write_all(&data)?;
        }
        cfb.flush()?;
        Ok(cfb.into_inner().into_inner())
    }
}
