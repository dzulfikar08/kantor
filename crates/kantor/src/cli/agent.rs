//! `kantor agent run`: Paparan AI's agent loop without a window. A model
//! makes decks, documents and workbooks in one folder with Kantor's tools,
//! following a skill, until it calls `finish`.
//!
//! ```text
//! kantor agent run --skill ~/.claude/skills/paparan-deck --dir job \
//!     --model @cf/deepseek-ai/deepseek-v4.1-flash-1017 \
//!     --base-url https://api.cloudflare.com/client/v4/accounts/ID/ai/v1 \
//!     --api-key-env CF_TOKEN --json "A 6-slide deck on …"
//! ```

use std::ffi::OsString;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use kantor_ai::agent::{self, Ending, Event, Limits, LocalSkill, Prices, PromptSource};
use kantor_ai::{Cancel, Client, ProviderConfig, ProviderId, UreqTransport};
use serde_json::json;

use crate::harness::{App, DocTools};

pub const USAGE: &str = "usage: kantor agent run [options] PROMPT\n\
    \n\
    Run Paparan AI's agent on the files in one folder: the model builds,\n\
    checks and looks at decks, documents and workbooks with Kantor's tools\n\
    (no shell), following a skill, until it calls finish. PROMPT may be\n\
    @FILE. Files already in the folder are kept: results go to new files.\n\
    \n\
    Model (any OpenAI-compatible API with tool calls):\n\
    \x20     --model ID          the model\n\
    \x20     --base-url URL      the API's base URL (…/v1)\n\
    \x20     --api-key-env VAR   the environment variable holding the API key\n\
    \x20     --provider paparan  Paparan AI with the sign-in saved by Kantor\n\
    \x20                         (credits; --model paparan-auto by default)\n\
    Job:\n\
    \x20     --skill DIR         a skill folder (SKILL.md, references/, examples/)\n\
    \x20     --dir DIR           the work folder (default: the current one)\n\
    \x20     --allow-overwrite   let the agent change files already in the folder\n\
    Limits:\n\
    \x20     --max-steps N       model turns (default 40)\n\
    \x20     --max-tokens N      input + output tokens for the job\n\
    \x20     --max-cost USD      cost for the job (needs the prices)\n\
    \x20     --price-input USD   per million input tokens (and --price-cached,\n\
    \x20     --price-output)     --price-output): the cost is shown with them\n\
    \x20     --max-images N      pictures kept in the conversation (default 4)\n\
    Output:\n\
    \x20     --json              events as JSON lines\n\
    \x20 -h, --help             show this help\n\
    \n\
    The last line is the usage: input, cached input and output tokens.\n\
    Exit status: 0 when the agent finished, 1 when it stopped or failed,\n\
    2 for usage errors.";

#[derive(Default)]
struct Opts {
    model: Option<String>,
    base_url: Option<String>,
    key_env: Option<String>,
    paparan: bool,
    skill: Option<PathBuf>,
    dir: Option<PathBuf>,
    allow_overwrite: bool,
    json: bool,
    limits: Limits,
    price: [Option<f64>; 3],
    prompt: Vec<String>,
}

fn parse(args: &[OsString]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let a = a.to_str().ok_or("arguments must be valid UTF-8")?;
        let mut value = |what: &str| -> Result<String, String> {
            it.next()
                .and_then(|v| v.to_str())
                .map(str::to_owned)
                .ok_or_else(|| format!("{a} needs {what}"))
        };
        let num = |s: String| -> Result<f64, String> {
            s.trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite() && *n >= 0.0)
                .ok_or_else(|| format!("{a} needs a number, not {s}"))
        };
        match a {
            "--model" => o.model = Some(value("a model id")?),
            "--base-url" => o.base_url = Some(value("a URL")?),
            "--api-key-env" => o.key_env = Some(value("a variable name")?),
            "--provider" => match value("a provider")?.as_str() {
                "paparan" => o.paparan = true,
                "openai" | "custom" => o.paparan = false,
                p => {
                    return Err(format!(
                        "unknown provider {p} (paparan, or give --base-url)"
                    ));
                }
            },
            "--skill" => o.skill = Some(PathBuf::from(value("a folder")?)),
            "--dir" => o.dir = Some(PathBuf::from(value("a folder")?)),
            "--allow-overwrite" => o.allow_overwrite = true,
            "--json" => o.json = true,
            "--max-steps" => o.limits.max_steps = num(value("a number")?)? as u32,
            "--max-tokens" => o.limits.max_tokens = Some(num(value("a number")?)? as u64),
            "--max-cost" => o.limits.max_cost = Some(num(value("an amount")?)?),
            "--max-images" => o.limits.max_images = num(value("a number")?)? as usize,
            "--price-input" => o.price[0] = Some(num(value("a price")?)?),
            "--price-cached" => o.price[1] = Some(num(value("a price")?)?),
            "--price-output" => o.price[2] = Some(num(value("a price")?)?),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s => o.prompt.push(s.to_owned()),
        }
    }
    if let [Some(input), cached, Some(output)] = o.price {
        o.limits.prices = Some(Prices {
            input,
            cached: cached.unwrap_or(input),
            output,
        });
    } else if o.price.iter().any(Option::is_some) {
        return Err("give --price-input and --price-output (and --price-cached)".into());
    }
    if o.limits.max_cost.is_some() && o.limits.prices.is_none() {
        return Err("--max-cost needs the prices (--price-input, --price-output)".into());
    }
    if o.limits.max_steps == 0 {
        return Err("--max-steps must be at least 1".into());
    }
    Ok(o)
}

/// Run `kantor agent …`; returns the exit status.
pub fn main(args: &[OsString]) -> i32 {
    let cmd = args.first().and_then(|a| a.to_str());
    if matches!(cmd, None | Some("-h" | "--help" | "help"))
        || args.iter().any(|a| a == "-h" || a == "--help")
    {
        println!("{USAGE}");
        return i32::from(cmd.is_none()) * 2;
    }
    if cmd != Some("run") {
        eprintln!(
            "kantor agent: unknown command {}\n\n{USAGE}",
            cmd.unwrap_or_default()
        );
        return 2;
    }
    let o = match parse(&args[1..]) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("kantor agent run: {e}\n\n{USAGE}");
            return 2;
        }
    };
    match run(o) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("kantor agent run: {e}");
            2
        }
    }
}

fn run(o: Opts) -> Result<i32, String> {
    let prompt = o.prompt.join(" ");
    let prompt = match prompt.strip_prefix('@') {
        Some(file) => {
            std::fs::read_to_string(file).map_err(|e| format!("can't read {file}: {e}"))?
        }
        None => prompt,
    };
    if prompt.trim().is_empty() {
        return Err("give the job as PROMPT (or @FILE)".into());
    }
    let config = if o.paparan {
        let token = kantor_ai::paparan::load_token()
            .ok_or("not signed in to Paparan AI: sign in from Kantor's AI settings first")?;
        ProviderConfig {
            provider: ProviderId::Paparan,
            model: o
                .model
                .clone()
                .unwrap_or_else(|| kantor_ai::paparan::DEFAULT_MODEL.to_owned()),
            base_url: o
                .base_url
                .clone()
                .unwrap_or_else(kantor_ai::paparan::chat_base),
            api_key: Some(token),
        }
    } else {
        let base_url = o
            .base_url
            .clone()
            .ok_or("give --base-url (or --provider paparan)")?;
        let model = o.model.clone().ok_or("give --model")?;
        let api_key = match &o.key_env {
            Some(var) => Some(
                std::env::var(var)
                    .ok()
                    .filter(|k| !k.trim().is_empty())
                    .ok_or_else(|| format!("{var} is not set"))?,
            ),
            None => None,
        };
        ProviderConfig {
            provider: ProviderId::Custom,
            model,
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key,
        }
    };
    // TODO: `--skill paparan:NAME` for prompts served by Paparan AI
    // (agent::ServedSkill) once the backend serves them.
    let skill = match &o.skill {
        Some(dir) if dir.to_string_lossy().starts_with("paparan:") => {
            return Err(
                "skills served by Paparan AI aren't available yet; give a skill folder".into(),
            );
        }
        Some(dir) => Some(LocalSkill::open(dir)?),
        None => None,
    };
    let dir = match &o.dir {
        Some(d) => {
            std::fs::create_dir_all(d).map_err(|e| format!("can't make {}: {e}", d.display()))?;
            d.clone()
        }
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let apps = skill
        .as_ref()
        .map_or_else(|| App::ALL.to_vec(), |s| App::for_skill(&s.name()));
    let mut tools = DocTools::new(&dir, apps, !o.allow_overwrite).map_err(|e| format!("{e:#}"))?;
    if !o.json {
        println!("work folder: {}", tools.root().display());
    }

    let transport = Arc::new(UreqTransport::with_timeouts(
        Duration::from_secs(300),
        Duration::from_secs(1200),
    ));
    let client = Client::new(config, transport);
    let cancel = Cancel::default();
    watch_interrupt(&cancel);

    let json = o.json;
    let mut out = std::io::stdout().lock();
    let mut streaming = false;
    let outcome = agent::run(
        &client,
        &mut tools,
        skill.as_ref().map(|s| s as &dyn PromptSource),
        &prompt,
        &o.limits,
        &cancel,
        &mut |e| {
            if json {
                if !matches!(e, Event::TextDelta { .. })
                    && let Ok(line) = serde_json::to_string(e)
                {
                    let _ = writeln!(out, "{line}");
                    let _ = out.flush();
                }
            } else {
                print_event(&mut out, e, &mut streaming);
            }
        },
    );
    let u = outcome.usage;
    let ending = match &outcome.ending {
        Ending::Finished { .. } => "finished",
        Ending::Stopped { .. } => "stopped",
        Ending::Failed(_) => "failed",
    };
    if json {
        let line = json!({
            "type": "usage_total",
            "ending": ending,
            "steps": outcome.steps,
            "input": u.input,
            "cached_input": u.cached,
            "output": u.output,
            "cost": outcome.cost,
            "reported": outcome.usage_reported,
        });
        let _ = writeln!(out, "{line}");
    } else {
        let cost = outcome
            .cost
            .map(|c| format!(", cost ${c:.4}"))
            .unwrap_or_default();
        let est = if outcome.usage_reported {
            ""
        } else {
            " (partly estimated)"
        };
        let _ = writeln!(
            out,
            "usage: input {} tokens (cached {}), output {} tokens{cost}; {} steps, {ending}{est}",
            u.input, u.cached, u.output, outcome.steps
        );
    }
    Ok(i32::from(!matches!(
        outcome.ending,
        Ending::Finished { .. }
    )))
}

/// First Ctrl+C: stop after the current step. Second: quit.
fn watch_interrupt(cancel: &Cancel) {
    #[cfg(unix)]
    {
        use signal_hook::consts::SIGINT;
        let cancel = cancel.clone();
        if let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGINT]) {
            std::thread::spawn(move || {
                for _ in signals.forever() {
                    if cancel.is_cancelled() {
                        std::process::exit(130);
                    }
                    eprintln!("kantor agent: stopping (Ctrl+C again to quit now)");
                    cancel.cancel();
                }
            });
        }
    }
    #[cfg(not(unix))]
    let _ = cancel;
}

/// One line per event for people.
fn print_event(out: &mut impl std::io::Write, e: &Event, streaming: &mut bool) {
    let short = |s: &str, n: usize| {
        let s = s.replace('\n', " ");
        if s.chars().count() > n {
            format!("{}…", s.chars().take(n).collect::<String>())
        } else {
            s
        }
    };
    if *streaming && !matches!(e, Event::TextDelta { .. }) {
        let _ = writeln!(out);
        *streaming = false;
    }
    let _ = match e {
        Event::StepStart { step } => writeln!(out, "── step {step}"),
        Event::TextDelta { text } => {
            *streaming = true;
            write!(out, "{text}")
        }
        Event::Text { .. } | Event::Reasoning { .. } => Ok(()),
        Event::ToolCall {
            name, arguments, ..
        } => {
            writeln!(out, "→ {name} {}", short(&arguments.to_string(), 160))
        }
        Event::ToolResult {
            name,
            ok,
            text,
            images,
            ..
        } => {
            let pics = if images.is_empty() {
                String::new()
            } else {
                format!(" [{}]", images.join(", "))
            };
            writeln!(
                out,
                "← {name} {}: {}{pics}",
                if *ok { "ok" } else { "error" },
                short(text, 300)
            )
        }
        Event::ImagesDropped { labels, .. } => {
            writeln!(out, "  (pictures dropped: {})", labels.join(", "))
        }
        Event::Nudge { count, reason, .. } => writeln!(out, "! nudge {count}: {reason}"),
        Event::Usage { usage, cost, .. } => {
            let c = cost
                .map(|c| format!(", ${c:.4} so far"))
                .unwrap_or_default();
            writeln!(
                out,
                "  tokens: in {} (cached {}), out {}{c}",
                usage.input, usage.cached, usage.output
            )
        }
        Event::Done { summary, outputs } => {
            writeln!(
                out,
                "\n== finished\n{summary}\nfiles: {}",
                outputs.join(", ")
            )
        }
        Event::Stopped { message, .. } => writeln!(out, "\n== stopped: {message}"),
        Event::Error { message } => writeln!(out, "\n== error: {message}"),
    };
    let _ = out.flush();
}
