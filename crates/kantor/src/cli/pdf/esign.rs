//! `kantor pdf emeterai`, `kantor pdf sign --provider` and `kantor pdf
//! providers`: e-meterai and certified signatures through licensed
//! providers ([`kantor_esign`]).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kantor_core::settings::{EsignProviderSettings, Settings};
use kantor_esign::batch::{EMETERAI, EMeteraiPlugin, PROVIDER_SIGN, ProviderSignPlugin};
use kantor_esign::check::Kept;
use kantor_esign::emeterai::{self, StampRequest};
use kantor_esign::signer::{SignRequest, Signer};
use kantor_esign::{Account, Credentials, Error, Kind, PROVIDERS, Support, creds, providers};
use kantor_pdf::batch::{self, Anchor, Batch, Place as BoxPlace, Plugins, Step};

use super::{Args, Failure, Outcome, check_overwrite, human_size, output_path, write_file};

pub const EMETERAI_USAGE: &str = "usage: kantor pdf emeterai FILE... --provider NAME --doc-type CODE [options]\n\
\n\
Add an e-meterai (Rp 10.000, Peruri) from your account at a provider. The\n\
document goes to the provider, which stamps it; the stamped PDF is checked\n\
and saved. Earlier signatures must still verify, or nothing is saved.\n\
Set the provider up first: kantor pdf providers set NAME ...\n\
\n\
\x20 --provider NAME      vida or mekari (kantor pdf providers lists them)\n\
\x20 --doc-type CODE      the document type (kantor pdf providers doc-types NAME)\n\
\x20 --page N|last        where the meterai goes (default last)\n\
\x20 --rect X,Y,W,H       its box in points from the page's bottom left, or\n\
\x20 --position P         bottom-right (default), bottom-left, top-right, ...\n\
\x20 --size PT            the box's side for --position (default 70; 45 to 85)\n\
\x20 --number TEXT        the document number (nomor dokumen)\n\
\x20 --reason TEXT, --location TEXT\n\
\x20 --allow-breaking-signatures   save even if the provider broke earlier signatures\n\
\n\
For one run, instead of the saved setup:\n\
\x20 --credentials-file F the provider's credentials (JSON or key=value lines)\n\
\x20 --server URL         another server address\n\
\x20 --allow-network      allow sending to the provider for this run\n\
\n\
Output: -o OUT.pdf for one file, -o DIR for several (default: FILE-emeterai.pdf).";

pub const PROVIDER_SIGN_USAGE: &str = "usage: kantor pdf sign FILE... --provider NAME --signer ID [options]\n\
\n\
Sign with a certified electronic signature from your account at a licensed\n\
provider (PSrE). Hash-based providers (xignature) never receive the document:\n\
Kantor builds the PAdES signature and the provider signs its hash.\n\
\n\
\x20 --provider NAME      the provider (kantor pdf providers lists them)\n\
\x20 --signer ID          your account at the provider (e.g. your Xignature ID)\n\
\x20 --otp-file F         a one-time password, when the provider sends one\n\
\x20                      (first line of F; - for standard input; or KANTOR_ESIGN_OTP)\n\
\x20 --send-otp           only ask the provider to send the one-time password\n\
\x20 --field NAME         sign this empty signature field\n\
\x20 --page N --rect X,Y,W,H   a new visible signature; without these it is invisible\n\
\x20 --reason TEXT, --location TEXT\n\
\x20 --credentials-file F, --server URL, --allow-network   as for kantor pdf emeterai\n\
\n\
Output: -o OUT.pdf for one file, -o DIR for several (default: FILE-signed.pdf).";

pub const PROVIDERS_USAGE: &str = "usage: kantor pdf providers [list | set NAME [options] | remove NAME | test NAME | doc-types NAME]\n\
\n\
e-meterai and certified-signature providers. Credentials go to the system\n\
keychain, never to settings.toml; nothing is sent to a provider unless it\n\
is allowed (--allow-network).\n\
\n\
\x20 set NAME --credentials-file F   save the provider's credentials (JSON object\n\
\x20                      or key=value lines; list shows the fields)\n\
\x20     --allow-network | --no-network   allow (or stop) sending documents to it\n\
\x20     --production | --sandbox         which environment (default sandbox)\n\
\x20     --server URL     another server address ('' for the documented one)\n\
\x20 remove NAME          forget its credentials and settings\n\
\x20 test NAME            check the credentials with the provider\n\
\x20 doc-types NAME       the e-meterai document types it accepts";

fn fail(e: Error) -> Failure {
    Failure {
        message: e.to_string(),
        code: 1,
    }
}

fn info(id: &str) -> Result<&'static kantor_esign::Info, Failure> {
    kantor_esign::find(id).ok_or_else(|| {
        Failure::usage(format!(
            "unknown provider \"{id}\"; known: {}",
            PROVIDERS
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })
}

/// The account for `id`: saved settings and keychain, with this run's
/// overrides.
fn account(args: &Args, id: &str) -> Result<Account, Failure> {
    let info = info(id)?;
    let mut s = Settings::load()
        .pdf
        .esign_providers
        .into_iter()
        .find(|s| s.id == info.id)
        .unwrap_or(EsignProviderSettings {
            id: info.id.into(),
            ..Default::default()
        });
    if args.flag("allow-network") {
        s.allow_network = true;
    }
    if let Some(u) = args.value("server") {
        s.base_url = u.to_string();
    }
    let creds = match args.value("credentials-file") {
        Some(f) => {
            let text =
                std::fs::read_to_string(f).map_err(|e| Failure::usage(format!("{f}: {e}")))?;
            Credentials::parse(&text).map_err(|e| Failure::usage(format!("{f}: {e}")))?
        }
        None => creds::load(info.id),
    };
    Ok(Account {
        info,
        settings: s,
        creds,
        transport: Arc::new(kantor_esign::http::Ureq::new()),
    })
}

fn parse_rect(r: &str) -> Result<[f32; 4], Failure> {
    let v: Vec<f32> = r
        .split(',')
        .map(|x| x.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .map_err(|_| Failure::usage(format!("--rect: \"{r}\" is not X,Y,W,H")))?;
    if v.len() != 4 || v[2] <= 0.0 || v[3] <= 0.0 {
        return Err(Failure::usage(format!("--rect: \"{r}\" is not X,Y,W,H")));
    }
    Ok([v[0], v[1], v[0] + v[2], v[1] + v[3]])
}

fn anchor(s: &str) -> Result<Anchor, Failure> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(|_| {
        Failure::usage(format!(
            "--position: \"{s}\" is not top-left, top-center, top-right, center-left, center, center-right, bottom-left, bottom-center or bottom-right"
        ))
    })
}

/// Where each output goes: -o FILE for one input, -o DIR for several.
fn outputs(args: &Args, suffix: &str) -> Result<Vec<(PathBuf, PathBuf)>, Failure> {
    let inputs: Vec<PathBuf> = args.positional.iter().map(PathBuf::from).collect();
    let many = inputs.len() > 1;
    let out_dir = match args.value("output") {
        Some(o) if many || Path::new(o).is_dir() => {
            std::fs::create_dir_all(o).map_err(|e| Failure {
                message: format!("{o}: {e}"),
                code: 1,
            })?;
            Some(PathBuf::from(o))
        }
        _ => None,
    };
    Ok(inputs
        .into_iter()
        .map(|i| {
            let o = match &out_dir {
                Some(d) => d.join(i.file_name().unwrap_or_default()),
                None => output_path(args, &i, suffix),
            };
            (i, o)
        })
        .collect())
}

/// Run `one` on every input; report like `kantor pdf sign`.
fn each(
    args: &Args,
    suffix: &str,
    what: &str,
    mut one: impl FnMut(&Path, &[u8]) -> Result<(Vec<u8>, String), Failure>,
) -> Outcome {
    let pairs = outputs(args, suffix)?;
    let many = pairs.len() > 1;
    let mut report = String::new();
    let mut failed = 0;
    for (input, output) in &pairs {
        let result = (|| -> Result<String, Failure> {
            check_overwrite(output, args.flag("force"))?;
            if output == input {
                return Err(Failure::usage("the output would replace the input"));
            }
            let bytes = kantor_pdf::read(input)?;
            let (out, note) = one(input, &bytes)?;
            write_file(output, &out)?;
            Ok(format!(
                "{note} ({}) → {}",
                human_size(out.len()),
                output.display()
            ))
        })();
        match result {
            Ok(line) => {
                let _ = writeln!(report, "{}: {line}", input.display());
            }
            Err(f) if !many => return Err(f.prefixed(input)),
            Err(f) => {
                failed += 1;
                let _ = writeln!(report, "{}: {}", input.display(), f.message);
            }
        }
    }
    if failed > 0 {
        eprint!("{report}");
        return Err(Failure {
            message: format!("{failed} of {} files could not be {what}", pairs.len()),
            code: 1,
        });
    }
    Ok(report.trim_end().to_string())
}

fn kept_note(kept: &Kept) -> String {
    match kept {
        Kept::Unsigned => String::new(),
        Kept::Incremental => ", earlier signatures kept".into(),
        Kept::Broken(f) => format!(", earlier signatures BROKEN: {}", f.join(", ")),
    }
}

pub fn emeterai(rest: &[String], usage: &str) -> Outcome {
    let args = Args::parse(
        rest,
        &[
            "output",
            "provider",
            "doc-type",
            "page",
            "rect",
            "position",
            "size",
            "number",
            "reason",
            "location",
            "credentials-file",
            "server",
        ],
        &[
            "force",
            "help",
            "allow-network",
            "allow-breaking-signatures",
        ],
        usage,
    )?;
    if args.positional.is_empty() {
        return Err(Failure::usage(format!("missing FILE\n\n{usage}")));
    }
    let id = args
        .value("provider")
        .ok_or_else(|| Failure::usage(format!("give --provider NAME\n\n{usage}")))?;
    let doc_type = args
        .value("doc-type")
        .ok_or_else(|| Failure::usage(format!("give --doc-type CODE\n\n{usage}")))?
        .to_string();
    let p = providers::emeterai(account(&args, id)?).map_err(fail)?;
    let page_spec = args.value("page").unwrap_or("last").to_string();
    let size: f32 = args
        .parse_value("size", "a size in points")?
        .unwrap_or(emeterai::SIZE_PT);
    let rect = args.value("rect").map(parse_rect).transpose()?;
    let place = BoxPlace::Anchor {
        anchor: anchor(args.value("position").unwrap_or("bottom-right"))?,
        dx: 36.0,
        dy: 36.0,
    };
    let allow_break = args.flag("allow-breaking-signatures");
    each(&args, "emeterai", "stamped", |input, bytes| {
        let pdf = kantor_pdf::Pdf::from_bytes(bytes, None)?;
        let page = batch::one_page(&page_spec, pdf.page_count()).map_err(Failure::usage)?;
        let rect = match rect {
            Some(r) => r,
            None => {
                batch::place_box(&pdf, page, &place, size, size)
                    .ok_or_else(|| Failure::usage("the page can't be read"))?
                    .0
            }
        };
        let name = input
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut req = StampRequest::new(&name, page, rect, &doc_type);
        req.doc_number = args.value("number").unwrap_or("").to_string();
        req.reason = args.value("reason").unwrap_or("").to_string();
        req.location = args.value("location").unwrap_or("").to_string();
        let (out, kept) =
            emeterai::stamp_checked(p.as_ref(), bytes, &req, allow_break).map_err(fail)?;
        let sn = out
            .serial
            .as_deref()
            .map(|s| format!(" {s}"))
            .unwrap_or_default();
        Ok((
            out.pdf,
            format!(
                "e-meterai{sn} by {} on page {}{}",
                p.info().name,
                page + 1,
                kept_note(&kept)
            ),
        ))
    })
}

/// `kantor pdf sign --provider …`.
pub fn provider_sign(rest: &[String], usage: &str) -> Outcome {
    let args = Args::parse(
        rest,
        &[
            "output",
            "provider",
            "signer",
            "otp-file",
            "field",
            "page",
            "rect",
            "reason",
            "location",
            "credentials-file",
            "server",
        ],
        &[
            "force",
            "help",
            "allow-network",
            "send-otp",
            "allow-breaking-signatures",
        ],
        usage,
    )?;
    let id = args
        .value("provider")
        .ok_or_else(|| Failure::usage(format!("give --provider NAME\n\n{usage}")))?;
    let who_id = args
        .value("signer")
        .ok_or_else(|| Failure::usage(format!("give --signer ID\n\n{usage}")))?;
    let p = providers::signer(account(&args, id)?).map_err(fail)?;
    let mut who = Signer {
        id: who_id.to_string(),
        otp: None,
    };
    if args.flag("send-otp") {
        return match p.request_otp(&who).map_err(fail)? {
            Some(to) => Ok(format!("one-time password sent: {to}")),
            None => Ok(format!(
                "{} needs no one-time password for this signer",
                p.info().name
            )),
        };
    }
    if args.positional.is_empty() {
        return Err(Failure::usage(format!("missing FILE\n\n{usage}")));
    }
    who.otp =
        super::read_secret(args.value("otp-file"), "KANTOR_ESIGN_OTP")?.filter(|s| !s.is_empty());
    let place = match (args.value("field"), args.value("page"), args.value("rect")) {
        (Some(f), None, None) => kantor_pdf::sign::Place::Field(f.to_string()),
        (None, Some(_), Some(r)) => {
            let page: u32 = args.parse_value("page", "a page number")?.unwrap_or(1);
            if page == 0 {
                return Err(Failure::usage("--page counts from 1"));
            }
            kantor_pdf::sign::Place::New {
                page: page - 1,
                rect: parse_rect(r)?,
            }
        }
        (None, None, None) => kantor_pdf::sign::Place::Invisible,
        _ => {
            return Err(Failure::usage(format!(
                "use either --field NAME or --page N --rect X,Y,W,H\n\n{usage}"
            )));
        }
    };
    let allow_break = args.flag("allow-breaking-signatures");
    each(&args, "signed", "signed", |input, bytes| {
        let name = input
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut req = SignRequest::new(&name, place.clone());
        req.reason = args.value("reason").unwrap_or("").to_string();
        req.location = args.value("location").unwrap_or("").to_string();
        let (out, kept) =
            kantor_esign::signer::sign(p.as_ref(), bytes, &req, &who, allow_break).map_err(fail)?;
        Ok((
            out,
            format!(
                "signed by {} ({}){}",
                who.id,
                p.info().name,
                kept_note(&kept)
            ),
        ))
    })
}

pub fn providers_cmd(rest: &[String], usage: &str) -> Outcome {
    let args = Args::parse(
        rest,
        &["credentials-file", "server"],
        &[
            "help",
            "allow-network",
            "no-network",
            "production",
            "sandbox",
        ],
        usage,
    )?;
    let what = args
        .positional
        .first()
        .map(String::as_str)
        .unwrap_or("list");
    let name = args.positional.get(1).map(String::as_str);
    let need_name =
        || name.ok_or_else(|| Failure::usage(format!("give the provider's name\n\n{usage}")));
    match what {
        "list" => {
            let saved = Settings::load().pdf.esign_providers;
            let mut out = String::new();
            for p in PROVIDERS {
                let kinds: Vec<&str> = p
                    .kinds
                    .iter()
                    .map(|k| match k {
                        Kind::EMeterai => "e-meterai",
                        Kind::Signing => "signing",
                    })
                    .collect();
                let state = match (p.support, saved.iter().find(|s| s.id == p.id)) {
                    (Support::Stub(_), _) => "API not public yet".to_string(),
                    (_, Some(s)) => format!(
                        "{}, {}",
                        if s.allow_network {
                            "allowed"
                        } else {
                            "not allowed online"
                        },
                        if s.sandbox { "sandbox" } else { "production" }
                    ),
                    _ => "not set up".into(),
                };
                let _ = writeln!(
                    out,
                    "{:<12} {:<20} {:<18} {state}",
                    p.id,
                    p.name,
                    kinds.join("+")
                );
                if p.implemented() {
                    let f: Vec<String> = p
                        .fields
                        .iter()
                        .map(|f| {
                            if f.optional {
                                format!("[{}]", f.key)
                            } else {
                                f.key.to_string()
                            }
                        })
                        .collect();
                    let _ = writeln!(out, "{:<12} fields: {}", "", f.join(", "));
                }
            }
            Ok(out.trim_end().to_string())
        }
        "set" => {
            let id = info(need_name()?)?;
            if let Support::Stub(why) = id.support {
                return Err(Failure::usage(format!("{}: {why}", id.name)));
            }
            let mut settings = Settings::load();
            let list = &mut settings.pdf.esign_providers;
            if !list.iter().any(|s| s.id == id.id) {
                list.push(EsignProviderSettings {
                    id: id.id.into(),
                    ..Default::default()
                });
            }
            let s = list.iter_mut().find(|s| s.id == id.id).expect("added");
            if args.flag("allow-network") {
                s.allow_network = true;
            }
            if args.flag("no-network") {
                s.allow_network = false;
            }
            if args.flag("production") {
                s.sandbox = false;
            }
            if args.flag("sandbox") {
                s.sandbox = true;
            }
            if let Some(u) = args.value("server") {
                s.base_url = u.to_string();
            }
            let mut note = String::new();
            if let Some(f) = args.value("credentials-file") {
                let text =
                    std::fs::read_to_string(f).map_err(|e| Failure::usage(format!("{f}: {e}")))?;
                let c =
                    Credentials::parse(&text).map_err(|e| Failure::usage(format!("{f}: {e}")))?;
                let missing: Vec<&str> = c.missing(id.fields).iter().map(|f| f.key).collect();
                if !missing.is_empty() {
                    return Err(Failure::usage(format!(
                        "{f}: missing {}",
                        missing.join(", ")
                    )));
                }
                let loc = creds::save(id.id, &c).map_err(|e| Failure {
                    message: e,
                    code: 1,
                })?;
                note = match loc {
                    kantor_ai::keys::KeyLocation::Keychain => {
                        "credentials saved in the system keychain".into()
                    }
                    kantor_ai::keys::KeyLocation::File => format!(
                        "no system keychain: credentials saved in {} (readable only by you)",
                        kantor_ai::keys::ESIGN
                            .fallback_path()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default()
                    ),
                };
            }
            let summary = format!(
                "{}: {}, {}",
                id.name,
                if s.allow_network {
                    "allowed online"
                } else {
                    "NOT allowed online (add --allow-network)"
                },
                if s.sandbox { "sandbox" } else { "production" }
            );
            settings.save().map_err(|e| Failure {
                message: e.to_string(),
                code: 1,
            })?;
            Ok(if note.is_empty() {
                summary
            } else {
                format!("{summary}; {note}")
            })
        }
        "remove" => {
            let id = info(need_name()?)?;
            creds::delete(id.id);
            let mut settings = Settings::load();
            settings.pdf.esign_providers.retain(|s| s.id != id.id);
            settings.save().map_err(|e| Failure {
                message: e.to_string(),
                code: 1,
            })?;
            Ok(format!("{} removed", id.name))
        }
        "test" => {
            let id = need_name()?;
            let a = account(&args, id)?;
            let name = a.info.name;
            let r = if a.info.offers(Kind::EMeterai) {
                providers::emeterai(a).and_then(|p| {
                    p.test()?;
                    p.balance()
                })
            } else {
                providers::signer(a).and_then(|p| p.test().map(|_| None))
            };
            match r.map_err(fail)? {
                Some(n) => Ok(format!("{name}: connected; {n} e-meterai left")),
                None => Ok(format!("{name}: connected")),
            }
        }
        "doc-types" => {
            let p = providers::emeterai(account(&args, need_name()?)?).map_err(fail)?;
            let mut out = String::new();
            for t in p.doc_types().map_err(fail)? {
                let _ = writeln!(out, "{:<10} {}", t.code, t.name);
            }
            Ok(out.trim_end().to_string())
        }
        other => Err(Failure::usage(format!(
            "unknown action \"{other}\"\n\n{usage}"
        ))),
    }
}

/// Plugins for a batch's e-meterai and provider-sign steps, with the
/// saved provider setups (`allow_network` from the command line adds
/// consent for this run).
pub fn add_plugins(b: &Batch, plugins: &mut Plugins, allow_network: bool) -> Result<(), Failure> {
    let mut stampers = Vec::new();
    let mut signers = Vec::new();
    let saved = Settings::load().pdf.esign_providers;
    for s in &b.steps {
        let Step::Plugin { kind, options } = s else {
            continue;
        };
        if kind != EMETERAI && kind != PROVIDER_SIGN {
            continue;
        }
        let id = options
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let info = info(&id)?;
        let mut st = saved
            .iter()
            .find(|x| x.id == info.id)
            .cloned()
            .ok_or_else(|| fail(Error::NotConfigured(info.name.into())))?;
        st.allow_network |= allow_network;
        let a = Account {
            info,
            settings: st,
            creds: creds::load(info.id),
            transport: Arc::new(kantor_esign::http::Ureq::new()),
        };
        if kind == EMETERAI {
            if !stampers
                .iter()
                .any(|p: &Arc<dyn kantor_esign::EMeteraiProvider>| p.info().id == info.id)
            {
                stampers.push(providers::emeterai(a).map_err(fail)?);
            }
        } else if !signers
            .iter()
            .any(|p: &Arc<dyn kantor_esign::RemoteSigner>| p.info().id == info.id)
        {
            signers.push(providers::signer(a).map_err(fail)?);
        }
    }
    if !stampers.is_empty() {
        plugins.add(Arc::new(EMeteraiPlugin::new(stampers)));
    }
    if !signers.is_empty() {
        plugins.add(Arc::new(ProviderSignPlugin::new(signers)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{run, verify};
    use kantor_esign::mock::{RsaKey, Server};
    use kantor_esign::providers::{vida, xignature};
    use std::sync::Arc;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn fixture(n: &str) -> String {
        format!(
            "{}/../kantor_esign/tests/fixtures/{n}",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    #[test]
    fn emeterai_and_provider_sign_against_mocks() {
        let p12 = fixture("meterai.p12");
        let id = Arc::new(
            pdf_engine::signing::Identity::from_p12(
                &std::fs::read(&p12).unwrap(),
                "kantor-test",
                std::path::Path::new(&p12),
            )
            .unwrap(),
        );
        let stamp = Arc::new(move |pdf: &[u8], page: u32, x: f32, y: f32| {
            kantor_esign::mock::stamp_with(&id, pdf, page, [x, y, x + 60.0, y + 60.0])
        });
        let vs = Server::start(vida::mock("cid", "sec", stamp, 1)).unwrap();
        let key = RsaKey::from_pkcs8(&std::fs::read(fixture("signer-key.der")).unwrap()).unwrap();
        let pem = |n: &str| std::fs::read_to_string(fixture(n)).unwrap();
        let xs = Server::start(xignature::mock(
            "xk",
            vec![(
                "me01".into(),
                vec![pem("signer.pem"), pem("ca.pem"), pem("root.pem")],
            )],
            Arc::new(move |d: &[u8]| key.sign_sha512(d)),
        ))
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("kontrak.pdf");
        std::fs::write(&input, kantor_pdf::testing::sample_pdf(2)).unwrap();
        let vc = tmp.path().join("vida.json");
        std::fs::write(&vc, r#"{"client_id":"cid","client_secret":"sec"}"#).unwrap();
        let xc = tmp.path().join("xig.txt");
        std::fs::write(&xc, "api_key = xk\n").unwrap();
        // Without consent nothing is sent.
        let refused = run(&s(&[
            "emeterai",
            input.to_str().unwrap(),
            "--provider",
            "vida",
            "--doc-type",
            "VD010",
            "--credentials-file",
            vc.to_str().unwrap(),
            "--server",
            &vs.url,
        ]));
        if let Err(f) = &refused {
            assert!(f.message.contains("isn't allowed"), "{}", f.message);
            assert!(vs.requests().is_empty());
        }
        let stamped = tmp.path().join("stamped.pdf");
        let msg = run(&s(&[
            "emeterai",
            input.to_str().unwrap(),
            "-o",
            stamped.to_str().unwrap(),
            "--provider",
            "vida",
            "--doc-type",
            "VD010",
            "--credentials-file",
            vc.to_str().unwrap(),
            "--server",
            &vs.url,
            "--allow-network",
        ]))
        .unwrap();
        assert!(
            msg.contains("e-meterai 08MO5DUHLD0GUV2N0000A9 by VIDA on page 2"),
            "{msg}"
        );
        let signed = tmp.path().join("signed.pdf");
        let xurl = format!("{}/v3", xs.url);
        let msg = run(&s(&[
            "sign",
            stamped.to_str().unwrap(),
            "-o",
            signed.to_str().unwrap(),
            "--provider",
            "xignature",
            "--signer",
            "me01",
            "--credentials-file",
            xc.to_str().unwrap(),
            "--server",
            &xurl,
            "--allow-network",
        ]))
        .unwrap();
        assert!(
            msg.contains("signed by me01 (Xignature), earlier signatures kept"),
            "{msg}"
        );
        let (text, _) = verify(&s(&[
            signed.to_str().unwrap(),
            "--trust",
            &fixture("root.pem"),
            "--no-system-roots",
        ]));
        assert!(text.contains("Mock Signer Xignature"), "{text}");
        assert!(text.contains("Mock e-Meterai Stamp"), "{text}");
        assert!(!text.contains("altered"), "{text}");
        // Stubs explain themselves.
        let e = run(&s(&[
            "emeterai",
            input.to_str().unwrap(),
            "--provider",
            "peruri",
            "--doc-type",
            "x",
        ]))
        .unwrap_err();
        assert!(e.message.contains("confidential"), "{}", e.message);
    }
}
