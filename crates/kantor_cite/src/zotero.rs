//! A running Zotero, over its local HTTP server (`127.0.0.1:23119`; the
//! address can be changed with `KANTOR_ZOTERO_URL`, for tests).
//!
//! - `GET /connector/ping`: is Zotero running?
//! - Better BibTeX (a Zotero plug-in): `GET /better-bibtex/cayw?probe=true`
//!   answers `ready`; `GET /better-bibtex/cayw?format=json` opens Zotero's
//!   own citation picker and answers with what was picked (an empty answer:
//!   cancelled).
//! - Zotero 7's local API (on when "Allow other applications on this
//!   computer to communicate with Zotero" is ticked in Settings ▸
//!   Advanced): `GET /api/users/0/items/top?q=…&format=json&include=csljson`
//!   searches the library; `/api/users/0/items/KEY?format=csljson` reads one
//!   item.
//!
//! Every call is blocking: run them off the UI thread.

use std::time::Duration;

use anyhow::{Context as _, bail};
use serde_json::Value;

/// Zotero's default local server.
pub const DEFAULT_URL: &str = "http://127.0.0.1:23119";

/// What's there to talk to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub running: bool,
    /// Better BibTeX's picker.
    pub better_bibtex: bool,
    /// Zotero 7's local API.
    pub local_api: bool,
}

/// An item from the library.
#[derive(Clone, Debug, PartialEq)]
pub struct ZoteroItem {
    /// The item key (`ABCD2345`).
    pub key: String,
    /// `http://zotero.org/users/<id>/items/<key>` when the library id is
    /// known (what Zotero's Word plug-in links citations with).
    pub uri: Option<String>,
    /// CSL-JSON.
    pub data: Value,
}

/// An item picked in Better BibTeX's picker, with what was typed for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Picked {
    pub item: ZoteroItem,
    pub locator: Option<String>,
    pub label: Option<String>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub suppress_author: bool,
}

pub struct Zotero {
    base: String,
    agent: ureq::Agent,
    /// For the picker, which waits on the user.
    slow: ureq::Agent,
}

impl Default for Zotero {
    fn default() -> Self {
        Self::new()
    }
}

fn agent(read: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(2)))
        .timeout_per_read(Some(read))
        .user_agent(concat!("Kantor/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn str_of(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The item key at the end of a Zotero URI or local API link.
pub fn key_of(uri: &str) -> Option<&str> {
    let last = uri.trim_end_matches('/').rsplit('/').next()?;
    (last.len() == 8 && last.chars().all(|c| c.is_ascii_alphanumeric())).then_some(last)
}

impl Zotero {
    /// The local server (`KANTOR_ZOTERO_URL`, else [`DEFAULT_URL`]).
    pub fn new() -> Self {
        let base = std::env::var("KANTOR_ZOTERO_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_URL.to_owned());
        Self::at(&base)
    }

    pub fn at(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            agent: agent(Duration::from_secs(10)),
            slow: agent(Duration::from_secs(600)),
        }
    }

    fn get(&self, agent: &ureq::Agent, path: &str) -> anyhow::Result<(u16, String)> {
        let url = format!("{}{path}", self.base);
        let resp = agent
            .get(&url)
            .header("Zotero-API-Version", "3")
            .call()
            .with_context(|| format!("Zotero isn't answering at {}", self.base))?;
        let status = resp.status().as_u16();
        let body = resp
            .into_body()
            .read_to_string()
            .context("reading Zotero's answer")?;
        Ok((status, body))
    }

    /// What's running (each probe a quick request).
    pub fn status(&self) -> Status {
        let ok = |path: &str| matches!(self.get(&self.agent, path), Ok((200, _)));
        if !ok("/connector/ping") {
            return Status::default();
        }
        let better_bibtex = matches!(
            self.get(&self.agent, "/better-bibtex/cayw?probe=true"),
            Ok((200, b)) if b.trim() == "ready"
        );
        let local_api = ok("/api/users/0/items/top?limit=1&format=keys");
        Status {
            running: true,
            better_bibtex,
            local_api,
        }
    }

    /// Search the library (title, creator, year), at most `limit` items.
    pub fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<ZoteroItem>> {
        let path = format!(
            "/api/users/0/items/top?q={}&qmode=titleCreatorYear&format=json&include=csljson&limit={limit}",
            encode(query)
        );
        let (status, body) = self.get(&self.agent, &path)?;
        match status {
            200 => {}
            403 | 404 => bail!(
                "Zotero's local API is off: tick \"Allow other applications on this computer to communicate with Zotero\" in Zotero's Settings ▸ Advanced"
            ),
            s => bail!("Zotero answered HTTP {s}"),
        }
        parse_search(&body)
    }

    /// One item's CSL-JSON by key.
    pub fn item(&self, key: &str) -> anyhow::Result<ZoteroItem> {
        let path = format!(
            "/api/users/0/items/{}?format=json&include=csljson",
            encode(key)
        );
        let (status, body) = self.get(&self.agent, &path)?;
        if status != 200 {
            bail!("Zotero has no item {key} (HTTP {status})");
        }
        let v: Value = serde_json::from_str(&body).context("Zotero's answer isn't JSON")?;
        let v = match v {
            Value::Array(mut a) if !a.is_empty() => a.remove(0),
            v => v,
        };
        item_of(&v).with_context(|| format!("Zotero's item {key} has no CSL data"))
    }

    /// Open Better BibTeX's picker and wait for the user. Empty: cancelled.
    pub fn pick(&self) -> anyhow::Result<Vec<Picked>> {
        let (status, body) = self.get(&self.slow, "/better-bibtex/cayw?format=json")?;
        if status != 200 {
            bail!("Better BibTeX's picker answered HTTP {status}");
        }
        let picks = parse_cayw(&body)?;
        let mut out = Vec::new();
        for (uri, mut p) in picks {
            let key = uri
                .as_deref()
                .and_then(key_of)
                .map(str::to_owned)
                .or_else(|| (!p.item.key.is_empty()).then(|| p.item.key.clone()));
            let Some(key) = key else {
                bail!("Better BibTeX didn't say which item was picked");
            };
            let mut item = self.item(&key)?;
            if uri.as_deref().is_some_and(|u| u.contains("zotero.org/")) {
                item.uri = uri.clone();
            }
            p.item = item;
            out.push(p);
        }
        Ok(out)
    }
}

/// Percent-encoding for a query parameter.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A local API item (`format=json&include=csljson`).
fn item_of(v: &Value) -> Option<ZoteroItem> {
    let key = str_of(v.get("key"))?;
    let item_type = v
        .pointer("/data/itemType")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if matches!(item_type, "attachment" | "note" | "annotation") {
        return None;
    }
    let mut data = v.get("csljson")?.clone();
    if !data.is_object() {
        return None;
    }
    let lib_type = v.pointer("/library/type").and_then(Value::as_str);
    let lib_id = v.pointer("/library/id").and_then(Value::as_i64);
    let uri = match (lib_type, lib_id) {
        (Some("user"), Some(id)) if id > 0 => {
            Some(format!("http://zotero.org/users/{id}/items/{key}"))
        }
        (Some("group"), Some(id)) if id > 0 => {
            Some(format!("http://zotero.org/groups/{id}/items/{key}"))
        }
        _ => None,
    };
    if let Some(o) = data.as_object_mut() {
        o.insert("id".into(), Value::String(key.clone()));
    }
    Some(ZoteroItem { key, uri, data })
}

fn parse_search(body: &str) -> anyhow::Result<Vec<ZoteroItem>> {
    let v: Value = serde_json::from_str(body).context("Zotero's answer isn't JSON")?;
    Ok(v.as_array()
        .map(|a| a.iter().filter_map(item_of).collect())
        .unwrap_or_default())
}

/// Better BibTeX's `format=json` answer: one object per pick (its fields
/// as Better BibTeX names them; read loosely).
fn parse_cayw(body: &str) -> anyhow::Result<Vec<(Option<String>, Picked)>> {
    let body = body.trim();
    if body.is_empty() {
        return Ok(Vec::new());
    }
    let v: Value = serde_json::from_str(body).context("Better BibTeX's answer isn't JSON")?;
    let list = match v {
        Value::Array(a) => a,
        Value::Object(o) => match o.get("items").or_else(|| o.get("citationItems")) {
            Some(Value::Array(a)) => a.clone(),
            _ => vec![Value::Object(o)],
        },
        _ => Vec::new(),
    };
    Ok(list
        .iter()
        .map(|p| {
            let uri = str_of(p.get("uri")).or_else(|| {
                p.get("uris")
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
            let key = str_of(p.get("itemKey"))
                .or_else(|| str_of(p.get("key")))
                .unwrap_or_default();
            let picked = Picked {
                item: ZoteroItem {
                    key,
                    uri: None,
                    data: Value::Null,
                },
                locator: str_of(p.get("locator")),
                label: str_of(p.get("label")),
                prefix: str_of(p.get("prefix")),
                suffix: str_of(p.get("suffix")),
                suppress_author: p
                    .get("suppressAuthor")
                    .or_else(|| p.get("suppress-author"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            };
            (uri, picked)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::net::TcpListener;

    /// A fake Zotero: answers each request path with a canned body.
    fn fake(routes: Vec<(&'static str, u16, String)>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in l.incoming() {
                let Ok(mut s) = stream else { break };
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap() == 0 || h.trim().is_empty() {
                        break;
                    }
                }
                let (status, body) = routes
                    .iter()
                    .find(|(p, _, _)| path.starts_with(p))
                    .map(|(_, s, b)| (*s, b.clone()))
                    .unwrap_or((404, "not found".into()));
                let _ = write!(
                    s,
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        format!("http://{addr}")
    }

    fn api_item(key: &str, title: &str) -> String {
        format!(
            r#"{{"key":"{key}","version":3,"library":{{"type":"user","id":1234567,"name":"x"}},
            "data":{{"itemType":"journalArticle"}},
            "csljson":{{"id":"1234567/{key}","type":"article-journal","title":"{title}",
            "author":[{{"family":"Nugroho","given":"Adi"}}],"issued":{{"date-parts":[["2021"]]}}}}}}"#
        )
    }

    #[test]
    fn status_and_search() {
        let base = fake(vec![
            ("/connector/ping", 200, "Zotero is running".into()),
            ("/better-bibtex/cayw?probe", 404, String::new()),
            ("/api/users/0/items/top?limit=1", 200, "[]".into()),
            (
                "/api/users/0/items/top?q=nugroho",
                200,
                format!(
                    r#"[{}, {{"key":"ATTACH01","data":{{"itemType":"attachment"}},"csljson":{{}}}}]"#,
                    api_item("NUGRH001", "Pengaruh media sosial")
                ),
            ),
        ]);
        let z = Zotero::at(&base);
        let st = z.status();
        assert!(st.running && st.local_api && !st.better_bibtex);
        let found = z.search("nugroho", 10).unwrap();
        assert_eq!(found.len(), 1, "attachments left out");
        assert_eq!(found[0].key, "NUGRH001");
        assert_eq!(
            found[0].uri.as_deref(),
            Some("http://zotero.org/users/1234567/items/NUGRH001")
        );
        assert_eq!(found[0].data["id"], "NUGRH001");
        assert_eq!(found[0].data["title"], "Pengaruh media sosial");
    }

    #[test]
    fn nothing_running() {
        // A closed port.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        drop(l);
        assert_eq!(Zotero::at(&base).status(), Status::default());
    }

    #[test]
    fn picker_then_item_data() {
        let base = fake(vec![
            (
                "/better-bibtex/cayw?format=json",
                200,
                r#"[{"citekey":"nugroho2021","uri":"http://zotero.org/users/1234567/items/NUGRH001","locator":"12","label":"page","prefix":"lihat","suppressAuthor":false}]"#.into(),
            ),
            ("/api/users/0/items/NUGRH001", 200, api_item("NUGRH001", "T")),
        ]);
        let picks = Zotero::at(&base).pick().unwrap();
        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].locator.as_deref(), Some("12"));
        assert_eq!(picks[0].prefix.as_deref(), Some("lihat"));
        assert_eq!(picks[0].item.data["title"], "T");
        assert_eq!(
            picks[0].item.uri.as_deref(),
            Some("http://zotero.org/users/1234567/items/NUGRH001")
        );
        // Cancelled: an empty answer.
        let base = fake(vec![("/better-bibtex/cayw", 200, String::new())]);
        assert!(Zotero::at(&base).pick().unwrap().is_empty());
    }

    #[test]
    fn keys_and_encoding() {
        assert_eq!(
            key_of("http://zotero.org/users/1/items/ABCD2345"),
            Some("ABCD2345")
        );
        assert_eq!(key_of("http://zotero.org/users/1/items/"), None);
        assert_eq!(encode("a b&c"), "a%20b%26c");
    }
}
