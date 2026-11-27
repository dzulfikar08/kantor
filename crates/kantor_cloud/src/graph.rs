//! Microsoft OneDrive (personal, and work or school accounts) through
//! Microsoft Graph, behind the same [`Client`] methods as WebDAV.
//!
//! Items are addressed by path (`/me/drive/root:/Keuangan/Q3.xlsx:`), so
//! the paths the rest of Kantor keeps work unchanged. A file's version is
//! its `eTag`: a save sends it in `If-Match` and OneDrive answers `412`
//! when the file changed since (the same rule as WebDAV). New files are
//! created with `conflictBehavior=fail`, so Save As never replaces a file.
//! Big files go through an upload session (parts of 5 MiB to a
//! pre-authorised address, resumable after a dropped link). OneDrive offers
//! no locks to apps: [`Client::lock`] says so and saves rely on versions.

use serde_json::Value;

use crate::{Client, Entry, Error, Precondition, Progress, Reply, percent_encode, reply_wait};

/// The signed-in user's drive.
pub const API: &str = "https://graph.microsoft.com/v1.0/me/drive";
/// The signed-in user (name, e-mail).
pub const ME: &str = "https://graph.microsoft.com/v1.0/me";

/// Files from this size go through an upload session.
pub const SESSION_MIN: usize = 4 * 1024 * 1024;
/// Upload session parts: Graph wants multiples of 320 KiB.
pub const PART: usize = 320 * 1024 * 16;

const SELECT: &str = "id,name,size,eTag,cTag,lastModifiedDateTime,file,folder,package,deleted";

/// A Graph item (JSON) as a listing entry.
pub fn entry_of(v: &Value) -> Entry {
    let sha1 = v["file"]["hashes"]["sha1Hash"]
        .as_str()
        .map(|h| format!("SHA1:{}", h.to_ascii_lowercase()));
    Entry {
        href: v["id"].as_str().unwrap_or_default().to_owned(),
        name: v["name"].as_str().unwrap_or_default().to_owned(),
        is_dir: v.get("folder").is_some_and(|f| !f.is_null()),
        size: v["size"].as_u64(),
        etag: v["eTag"].as_str().map(str::to_owned),
        last_modified: v["lastModifiedDateTime"].as_str().map(str::to_owned),
        content_type: v["file"]["mimeType"].as_str().map(str::to_owned),
        checksums: sha1,
    }
}

/// The error in a Graph error reply (`{"error":{"code":…}}`).
fn code_of(r: &Reply) -> String {
    serde_json::from_slice::<Value>(&r.bytes)
        .ok()
        .and_then(|v| v["error"]["code"].as_str().map(str::to_owned))
        .unwrap_or_default()
}

impl Client {
    /// The Graph address of the item at `path` (`…/root` or
    /// `…/root:/a/b.xlsx:`); append `/children`, `/content`… to it.
    pub(crate) fn graph_item(&self, path: &str) -> String {
        let segs: Vec<String> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(percent_encode)
            .collect();
        if segs.is_empty() {
            format!("{}root", self.base)
        } else {
            format!("{}root:/{}:", self.base, segs.join("/"))
        }
    }

    fn graph_json(&self, method: &str, url: &str, body: Option<&Value>) -> Result<Reply, Error> {
        let bytes = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        self.api_send(
            &|| {
                let mut b = self
                    .request_url(method, url)
                    .header("accept", "application/json");
                if body.is_some() {
                    b = b.header("content-type", "application/json");
                }
                b
            },
            &bytes,
        )
    }

    pub(crate) fn graph_list(&self, dir: &str) -> Result<Vec<Entry>, Error> {
        let mut url = Some(format!(
            "{}/children?$top=200&$select={SELECT}",
            self.graph_item(dir)
        ));
        let mut out = Vec::new();
        while let Some(u) = url.take() {
            let r = self.graph_json("GET", &u, None)?;
            if r.status != 200 {
                return Err(r.error());
            }
            let v = r.json()?;
            out.extend(
                v["value"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|i| i.get("deleted").is_none_or(Value::is_null))
                    .map(entry_of),
            );
            url = v["@odata.nextLink"].as_str().map(str::to_owned);
        }
        Ok(out)
    }

    pub(crate) fn graph_stat(&self, path: &str) -> Result<Entry, Error> {
        let url = format!("{}?$select={SELECT}", self.graph_item(path));
        let r = self.graph_json("GET", &url, None)?;
        if r.status != 200 {
            return Err(r.error());
        }
        Ok(entry_of(&r.json()?))
    }

    /// Download: Graph answers `/content` with a redirect to a
    /// pre-authorised address, fetched without the token.
    pub(crate) fn graph_get(&self, path: &str) -> Result<crate::Fetched, Error> {
        let url = format!("{}/content", self.graph_item(path));
        let r = self.api_send(&|| self.request_url("GET", &url), &[])?;
        let r = match r.status {
            301 | 302 | 303 | 307 | 308 => {
                let to = r
                    .location
                    .clone()
                    .ok_or_else(|| Error::Protocol("download redirect without Location".into()))?;
                self.anonymous().send(self.request_url("GET", &to), &[])?
            }
            _ => r,
        };
        if r.status != 200 {
            return Err(r.error());
        }
        Ok(crate::Fetched {
            etag: None,
            bytes: r.bytes,
        })
    }

    pub(crate) fn graph_put(
        &self,
        path: &str,
        bytes: &[u8],
        cond: Precondition,
        progress: Progress,
    ) -> Result<Option<String>, Error> {
        let behaviour = match cond {
            Precondition::IfNoneMatch => "fail",
            _ => "replace",
        };
        let url = format!(
            "{}/content?@microsoft.graph.conflictBehavior={behaviour}",
            self.graph_item(path)
        );
        let mut req = self
            .request_url("PUT", &url)
            .header("content-type", "application/octet-stream")
            .header("content-length", bytes.len().to_string());
        if let Precondition::IfMatch(etag) = cond {
            req = req.header("if-match", etag);
        }
        let total = bytes.len() as u64;
        let r = self.exec(
            req,
            bytes,
            Some(reply_wait(total)),
            Some((progress, 0, total)),
        )?;
        put_reply(r)
    }

    pub(crate) fn graph_mkdir(&self, path: &str) -> Result<(), Error> {
        let path = path.trim_matches('/');
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        if name.is_empty() {
            return Ok(());
        }
        let url = format!("{}/children", self.graph_item(parent));
        let body = serde_json::json!({
            "name": name,
            "folder": {},
            "@microsoft.graph.conflictBehavior": "fail",
        });
        let r = self.graph_json("POST", &url, Some(&body))?;
        match r.status {
            200 | 201 => Ok(()),
            // Already there.
            409 => Ok(()),
            _ => Err(r.error()),
        }
    }

    pub(crate) fn graph_delete(&self, path: &str) -> Result<(), Error> {
        let url = self.graph_item(path);
        let r = self.graph_json("DELETE", &url, None)?;
        match r.status {
            200 | 204 | 404 => Ok(()),
            _ => Err(r.error()),
        }
    }

    /// An upload session: create it (with `If-Match`, or failing when the
    /// file exists for a new one), or resume `url` from the part the
    /// server expects next, then send the rest in [`PART`]s.
    pub(crate) fn graph_upload_session(
        &self,
        path: &str,
        bytes: &[u8],
        cond: Precondition,
        url: &mut Option<String>,
        progress: Progress,
    ) -> Result<Option<String>, Error> {
        let total = bytes.len() as u64;
        let anon = self.anonymous();
        let mut offset = 0u64;
        if let Some(u) = url.clone() {
            // An unfinished session: where does it stand?
            match anon.send(anon.request_url("GET", &u), &[]) {
                Ok(r) if r.status == 200 => {
                    offset = next_expected(&r.json()?).unwrap_or(0);
                }
                Ok(_) => *url = None,
                Err(e) => return Err(e),
            }
        }
        if url.is_none() {
            let behaviour = match cond {
                Precondition::IfNoneMatch => "fail",
                _ => "replace",
            };
            let body = serde_json::json!({
                "item": { "@microsoft.graph.conflictBehavior": behaviour }
            });
            let create = format!("{}/createUploadSession", self.graph_item(path));
            let text = body.to_string();
            let r = self.api_send(
                &|| {
                    let b = self
                        .request_url("POST", &create)
                        .header("content-type", "application/json")
                        .header("accept", "application/json");
                    match cond {
                        Precondition::IfMatch(etag) => b.header("if-match", etag),
                        _ => b,
                    }
                },
                text.as_bytes(),
            )?;
            if r.status != 200 {
                return Err(session_error(r));
            }
            let v = r.json()?;
            let u = v["uploadUrl"]
                .as_str()
                .ok_or_else(|| Error::Protocol("upload session without uploadUrl".into()))?;
            *url = Some(u.to_owned());
            offset = 0;
        }
        let session = url.clone().unwrap_or_default();
        loop {
            let start = offset as usize;
            let end = (start + PART).min(bytes.len());
            let part = &bytes[start..end];
            let req = anon
                .request_url("PUT", &session)
                .header("content-length", part.len().to_string())
                .header(
                    "content-range",
                    format!("bytes {start}-{}/{total}", end.max(1) - 1),
                );
            let r = anon.exec(
                req,
                part,
                Some(reply_wait(part.len() as u64)),
                Some((&mut *progress, start as u64, total)),
            )?;
            match r.status {
                200 | 201 => {
                    *url = None;
                    return put_reply(r);
                }
                202 => {
                    offset = next_expected(&r.json()?).unwrap_or(end as u64);
                    if offset as usize >= bytes.len() {
                        return Err(Error::Protocol(
                            "upload session wants nothing more but isn't done".into(),
                        ));
                    }
                }
                // The session expired or is gone: start again next time.
                404 | 410 => {
                    *url = None;
                    return Err(Error::Network("the upload session expired".into()));
                }
                _ => return Err(session_error(r)),
            }
        }
    }
}

/// A save's reply: the item with its new eTag.
fn put_reply(r: Reply) -> Result<Option<String>, Error> {
    match r.status {
        200 | 201 => Ok(entry_of(&r.json()?).etag),
        // `conflictBehavior=fail` found a file there.
        409 if code_of(&r) == "nameAlreadyExists" => Err(Error::Conflict),
        _ => Err(session_error(r)),
    }
}

fn session_error(r: Reply) -> Error {
    match (r.status, code_of(&r).as_str()) {
        (409, "nameAlreadyExists") => Error::Conflict,
        // OneDrive: someone has the file open for editing in Office.
        (423, _) => Error::Http {
            status: 423,
            body: "the file is in use (open in an Office app)".into(),
        },
        _ => r.error(),
    }
}

/// The first byte an upload session still expects
/// (`"nextExpectedRanges": ["26-"]`).
fn next_expected(v: &Value) -> Option<u64> {
    v["nextExpectedRanges"]
        .as_array()?
        .first()?
        .as_str()?
        .split('-')
        .next()?
        .parse()
        .ok()
}

/// Who signed in: `(id, e-mail or user principal name, display name)`.
pub fn whoami_with(client: &Client, me_url: &str) -> Result<(String, String, String), Error> {
    let url = format!("{me_url}?$select=id,displayName,userPrincipalName,mail");
    let r = client.graph_json("GET", &url, None)?;
    if r.status != 200 {
        return Err(r.error());
    }
    let v = r.json()?;
    let id = v["id"].as_str().unwrap_or_default().to_owned();
    let login = v["mail"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| v["userPrincipalName"].as_str())
        .unwrap_or(&id)
        .to_owned();
    let name = v["displayName"].as_str().unwrap_or_default().to_owned();
    Ok((id, login, name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Auth;

    #[test]
    fn item_addresses() {
        let c = Client::graph_at("https://g.example/v1.0/me/drive", Auth::None);
        assert_eq!(c.graph_item(""), "https://g.example/v1.0/me/drive/root");
        assert_eq!(
            c.graph_item("Keuangan/Q3 #1.xlsx"),
            "https://g.example/v1.0/me/drive/root:/Keuangan/Q3%20%231.xlsx:"
        );
    }

    #[test]
    fn items_and_ranges() {
        let v: Value = serde_json::from_str(
            r#"{"id":"X1","name":"a.docx","size":12,"eTag":"\"{A},3\"","lastModifiedDateTime":"2026-11-27T01:02:03Z","file":{"mimeType":"application/x","hashes":{"sha1Hash":"AB"}}}"#,
        )
        .unwrap();
        let e = entry_of(&v);
        assert_eq!(e.name, "a.docx");
        assert!(!e.is_dir);
        assert_eq!(e.etag.as_deref(), Some("\"{A},3\""));
        assert_eq!(e.checksums.as_deref(), Some("SHA1:ab"));
        assert_eq!(
            next_expected(&serde_json::json!({"nextExpectedRanges": ["327680-"]})),
            Some(327680)
        );
        assert!(entry_of(&serde_json::json!({"name": "d", "folder": {"childCount": 1}})).is_dir);
    }
}
