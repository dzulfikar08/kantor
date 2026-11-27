//! Google Drive (Drive API v3) behind the same [`Client`] methods as
//! WebDAV.
//!
//! Kantor asks only for the `drive.file` scope (no security assessment
//! needed, and Kantor never sees the rest of the user's Drive): it sees the
//! files and folders it created or that were opened with it. So the top of
//! the browser shows those items whose folder Kantor can't see (often "My
//! Drive" itself), and folders Kantor made (e.g. "Kantor") show their
//! contents as usual.
//!
//! Drive addresses files by id, and names aren't unique. Kantor keeps
//! paths (`Kantor/Laporan.xlsx`) everywhere, so [`Ids`] remembers the id
//! behind each path, filled by listings; a path not known yet is found by
//! listing its folder (the most recently changed file wins when two share
//! a name). A file's version is Drive's `version` number. Drive v3 has no
//! conditional writes, so a save checks the version just before it sends
//! (a change in the moment between the two isn't caught). Big files go
//! through a resumable upload session.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::{Client, Entry, Error, Precondition, Progress, Reply, percent_encode, reply_wait};

pub const API: &str = "https://www.googleapis.com/drive/v3/";
pub const UPLOAD: &str = "https://www.googleapis.com/upload/drive/v3/";
pub const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
/// The folder Kantor saves into by default (and creates when missing).
pub const KANTOR_FOLDER: &str = "Kantor";
/// Resumable upload parts (multiples of 256 KiB).
pub const PART: usize = 256 * 1024 * 20;

const FIELDS: &str = "id,name,mimeType,size,version,modifiedTime,sha1Checksum,parents,trashed";

/// Path → file id, for one account.
#[derive(Debug, Default)]
pub struct Ids {
    paths: Mutex<HashMap<String, String>>,
}

impl Ids {
    pub fn get(&self, path: &str) -> Option<String> {
        self.paths
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path.trim_matches('/'))
            .cloned()
    }

    pub fn set(&self, path: &str, id: &str) {
        self.paths
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.trim_matches('/').to_owned(), id.to_owned());
    }

    /// Forget `path` and everything under it.
    pub fn forget(&self, path: &str) {
        let path = path.trim_matches('/');
        let under = format!("{path}/");
        self.paths
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|p, _| p != path && !p.starts_with(&under));
    }
}

/// The ids of `account`'s Google Drive, shared by all its clients.
pub fn shared_ids(account: &str) -> Arc<Ids> {
    static ALL: Mutex<Vec<(String, Arc<Ids>)>> = Mutex::new(Vec::new());
    let mut all = ALL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, ids)) = all.iter().find(|(a, _)| a == account) {
        return ids.clone();
    }
    let ids = Arc::new(Ids::default());
    all.push((account.to_owned(), ids.clone()));
    ids
}

#[derive(Clone)]
pub(crate) struct Drive {
    /// Ends with `/` (`…/upload/drive/v3/`).
    upload: String,
    ids: Arc<Ids>,
}

/// A Drive file (JSON) as a listing entry; its version is the "ETag".
pub fn entry_of(v: &Value) -> Entry {
    let mime = v["mimeType"].as_str().unwrap_or_default();
    Entry {
        href: v["id"].as_str().unwrap_or_default().to_owned(),
        name: v["name"].as_str().unwrap_or_default().to_owned(),
        is_dir: mime == FOLDER_MIME,
        // Drive sends sizes as strings.
        size: v["size"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .or_else(|| v["size"].as_u64()),
        etag: version_of(v),
        last_modified: v["modifiedTime"].as_str().map(str::to_owned),
        content_type: Some(mime.to_owned()).filter(|m| !m.is_empty()),
        checksums: v["sha1Checksum"]
            .as_str()
            .map(|h| format!("SHA1:{}", h.to_ascii_lowercase())),
    }
}

fn version_of(v: &Value) -> Option<String> {
    v["version"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| v["version"].as_u64().map(|n| n.to_string()))
}

/// `folder/name` (no leading slash at the top).
fn join(folder: &str, name: &str) -> String {
    let folder = folder.trim_matches('/');
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

fn split(path: &str) -> (&str, &str) {
    let path = path.trim_matches('/');
    path.rsplit_once('/').unwrap_or(("", path))
}

fn same_version(a: &str, b: &str) -> bool {
    a.trim_matches('"') == b.trim_matches('"')
}

impl Drive {
    pub(crate) fn new(upload: &str, ids: Arc<Ids>) -> Self {
        let mut upload = upload.to_owned();
        if !upload.ends_with('/') {
            upload.push('/');
        }
        Self { upload, ids }
    }

    fn call(c: &Client, method: &str, url: &str, body: Option<&Value>) -> Result<Reply, Error> {
        let bytes = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        c.api_send(
            &|| {
                let mut b = c
                    .request_url(method, url)
                    .header("accept", "application/json");
                if body.is_some() {
                    b = b.header("content-type", "application/json; charset=UTF-8");
                }
                b
            },
            &bytes,
        )
    }

    /// All files matching the Drive query `q` (every page).
    fn query(c: &Client, q: &str) -> Result<Vec<Value>, Error> {
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut url = format!(
                "{}files?q={}&fields=nextPageToken,files({FIELDS})&pageSize=1000&spaces=drive&orderBy=modifiedTime%20desc",
                c.base,
                percent_encode(q)
            );
            if let Some(p) = &page {
                url.push_str(&format!("&pageToken={}", percent_encode(p)));
            }
            let r = Self::call(c, "GET", &url, None)?;
            if r.status != 200 {
                return Err(r.error());
            }
            let v = r.json()?;
            out.extend(v["files"].as_array().into_iter().flatten().cloned());
            page = v["nextPageToken"].as_str().map(str::to_owned);
            if page.is_none() {
                return Ok(out);
            }
        }
    }

    pub(crate) fn list(&self, c: &Client, dir: &str) -> Result<Vec<Entry>, Error> {
        let dir = dir.trim_matches('/');
        let items = if dir.is_empty() {
            // Everything Kantor may see; the top shows what isn't inside
            // a folder Kantor sees.
            let all = Self::query(c, "trashed = false")?;
            let folders: std::collections::HashSet<&str> = all
                .iter()
                .filter(|v| v["mimeType"] == FOLDER_MIME)
                .filter_map(|v| v["id"].as_str())
                .collect();
            all.iter()
                .filter(|v| {
                    !v["parents"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .any(|p| folders.contains(p))
                })
                .cloned()
                .collect::<Vec<_>>()
        } else {
            let id = self.resolve(c, dir)?;
            Self::query(c, &format!("'{id}' in parents and trashed = false"))?
        };
        // Newest first from the query: the newest of two same-named files
        // gets the path.
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for v in &items {
            let e = entry_of(v);
            if seen.insert(e.name.clone()) {
                self.ids.set(&join(dir, &e.name), &e.href);
            }
            out.push(e);
        }
        Ok(out)
    }

    /// The file id of `path` (`root` for the top).
    pub(crate) fn resolve(&self, c: &Client, path: &str) -> Result<String, Error> {
        let path = path.trim_matches('/');
        if path.is_empty() {
            return Ok("root".into());
        }
        if let Some(id) = self.ids.get(path) {
            return Ok(id);
        }
        let (folder, _) = split(path);
        self.list(c, folder)?;
        self.ids.get(path).ok_or(Error::NotFound)
    }

    fn get_meta(c: &Client, id: &str) -> Result<Value, Error> {
        let url = format!("{}files/{id}?fields={FIELDS}", c.base);
        let r = Self::call(c, "GET", &url, None)?;
        if r.status != 200 {
            return Err(r.error());
        }
        let v = r.json()?;
        if v["trashed"].as_bool() == Some(true) {
            return Err(Error::NotFound);
        }
        Ok(v)
    }

    pub(crate) fn stat(&self, c: &Client, path: &str) -> Result<Entry, Error> {
        if path.trim_matches('/').is_empty() {
            return Ok(Entry {
                is_dir: true,
                href: "root".into(),
                ..Default::default()
            });
        }
        let id = self.resolve(c, path)?;
        match Self::get_meta(c, &id) {
            Err(Error::NotFound) => {
                // Gone or moved: look again from the folder.
                self.ids.forget(path);
                let again = self.resolve(c, path)?;
                if again == id {
                    return Err(Error::NotFound);
                }
                Ok(entry_of(&Self::get_meta(c, &again)?))
            }
            r => Ok(entry_of(&r?)),
        }
    }

    pub(crate) fn get(&self, c: &Client, path: &str) -> Result<crate::Fetched, Error> {
        let id = self.resolve(c, path)?;
        let url = format!("{}files/{id}?alt=media", c.base);
        let r = c.api_send(&|| c.request_url("GET", &url), &[])?;
        if r.status == 404 {
            self.ids.forget(path);
        }
        if r.status != 200 {
            return Err(r.error());
        }
        Ok(crate::Fetched {
            bytes: r.bytes,
            etag: None,
        })
    }

    /// The id of the file at `path` if there is one, after checking `cond`
    /// against it (Drive can't check it in the write itself).
    fn existing(
        &self,
        c: &Client,
        path: &str,
        cond: Precondition,
    ) -> Result<Option<String>, Error> {
        let existing = match self.resolve(c, path) {
            Ok(id) => match Self::get_meta(c, &id) {
                Ok(meta) => Some((id, meta)),
                Err(Error::NotFound) => {
                    self.ids.forget(path);
                    None
                }
                Err(e) => return Err(e),
            },
            Err(Error::NotFound) => None,
            Err(e) => return Err(e),
        };
        match (existing, cond) {
            (Some(_), Precondition::IfNoneMatch) => Err(Error::Conflict),
            (None, Precondition::IfMatch(_)) => Err(Error::Conflict),
            (Some((_, meta)), Precondition::IfMatch(v))
                if !version_of(&meta).is_some_and(|now| same_version(&now, v)) =>
            {
                Err(Error::Conflict)
            }
            (Some((id, _)), _) => Ok(Some(id)),
            (None, _) => Ok(None),
        }
    }

    /// Metadata for a new file at `path` (its folder must exist).
    fn new_file_meta(&self, c: &Client, path: &str) -> Result<Value, Error> {
        let (folder, name) = split(path);
        let parent = self.resolve(c, folder)?;
        Ok(serde_json::json!({ "name": name, "parents": [parent] }))
    }

    /// Remember the id of what a write returned; its version.
    fn written(&self, path: &str, r: Reply) -> Result<Option<String>, Error> {
        if !matches!(r.status, 200 | 201) {
            return Err(r.error());
        }
        let v = r.json()?;
        if let Some(id) = v["id"].as_str() {
            self.ids.set(path, id);
        }
        Ok(version_of(&v))
    }

    pub(crate) fn put(
        &self,
        c: &Client,
        path: &str,
        bytes: &[u8],
        cond: Precondition,
        progress: Progress,
    ) -> Result<Option<String>, Error> {
        let (url, ctype, body) = match self.existing(c, path, cond)? {
            Some(id) => (
                format!("{}files/{id}?uploadType=media&fields={FIELDS}", self.upload),
                mime_for(path).to_owned(),
                bytes.to_vec(),
            ),
            None => {
                let meta = self.new_file_meta(c, path)?;
                let boundary = format!("kantor-{}", crate::oidc::random_token(12));
                let mut body = format!(
                    "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: {}\r\n\r\n",
                    mime_for(path)
                )
                .into_bytes();
                body.extend_from_slice(bytes);
                body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
                (
                    format!("{}files?uploadType=multipart&fields={FIELDS}", self.upload),
                    format!("multipart/related; boundary={boundary}"),
                    body,
                )
            }
        };
        let method = if url.contains("uploadType=media") {
            "PATCH"
        } else {
            "POST"
        };
        let req = c
            .request_url(method, &url)
            .header("content-type", ctype)
            .header("content-length", body.len().to_string());
        let total = body.len() as u64;
        let r = c.exec(
            req,
            &body,
            Some(reply_wait(total)),
            Some((progress, 0, total)),
        )?;
        self.written(path, r)
    }

    pub(crate) fn mkdir(&self, c: &Client, path: &str) -> Result<(), Error> {
        let path = path.trim_matches('/');
        if path.is_empty() {
            return Ok(());
        }
        match self.resolve(c, path) {
            Ok(_) => return Ok(()),
            Err(Error::NotFound) => {}
            Err(e) => return Err(e),
        }
        let (folder, name) = split(path);
        let parent = self.resolve(c, folder)?;
        let body =
            serde_json::json!({ "name": name, "mimeType": FOLDER_MIME, "parents": [parent] });
        let url = format!("{}files?fields={FIELDS}", c.base);
        let r = Self::call(c, "POST", &url, Some(&body))?;
        self.written(path, r).map(|_| ())
    }

    /// Move to the bin (Drive keeps it for 30 days).
    pub(crate) fn delete(&self, c: &Client, path: &str) -> Result<(), Error> {
        let id = match self.resolve(c, path) {
            Ok(id) => id,
            Err(Error::NotFound) => return Ok(()),
            Err(e) => return Err(e),
        };
        let url = format!("{}files/{id}", c.base);
        let r = Self::call(
            c,
            "PATCH",
            &url,
            Some(&serde_json::json!({ "trashed": true })),
        )?;
        self.ids.forget(path);
        match r.status {
            200 | 204 | 404 => Ok(()),
            _ => Err(r.error()),
        }
    }

    /// A resumable upload: start a session (after the version check), or
    /// ask `url` how much it has, then send the rest in [`PART`]s.
    pub(crate) fn upload_session(
        &self,
        c: &Client,
        path: &str,
        bytes: &[u8],
        cond: Precondition,
        url: &mut Option<String>,
        progress: Progress,
    ) -> Result<Option<String>, Error> {
        let total = bytes.len() as u64;
        let mut offset = 0u64;
        if let Some(u) = url.clone() {
            let req = c
                .request_url("PUT", &u)
                .header("content-length", "0")
                .header("content-range", format!("bytes */{total}"));
            let r = c.send(req, &[])?;
            match r.status {
                200 | 201 => {
                    *url = None;
                    return self.written(path, r);
                }
                308 => offset = range_end(r.range.as_deref()),
                // Expired or unknown: start a new one.
                _ => *url = None,
            }
        }
        if url.is_none() {
            let (method, start, meta) = match self.existing(c, path, cond)? {
                Some(id) => (
                    "PATCH",
                    format!(
                        "{}files/{id}?uploadType=resumable&fields={FIELDS}",
                        self.upload
                    ),
                    serde_json::json!({}),
                ),
                None => (
                    "POST",
                    format!("{}files?uploadType=resumable&fields={FIELDS}", self.upload),
                    self.new_file_meta(c, path)?,
                ),
            };
            let text = meta.to_string();
            let r = c.api_send(
                &|| {
                    c.request_url(method, &start)
                        .header("content-type", "application/json; charset=UTF-8")
                        .header("x-upload-content-type", mime_for(path))
                        .header("x-upload-content-length", total.to_string())
                },
                text.as_bytes(),
            )?;
            if r.status != 200 {
                return Err(r.error());
            }
            let session = r
                .location
                .clone()
                .ok_or_else(|| Error::Protocol("upload session without Location".into()))?;
            *url = Some(session);
            offset = 0;
        }
        let session = url.clone().unwrap_or_default();
        loop {
            let start = offset as usize;
            let end = (start + PART).min(bytes.len());
            let part = &bytes[start..end];
            let req = c
                .request_url("PUT", &session)
                .header("content-length", part.len().to_string())
                .header(
                    "content-range",
                    format!("bytes {start}-{}/{total}", end.max(1) - 1),
                );
            let r = c.exec(
                req,
                part,
                Some(reply_wait(part.len() as u64)),
                Some((&mut *progress, start as u64, total)),
            )?;
            match r.status {
                200 | 201 => {
                    *url = None;
                    return self.written(path, r);
                }
                308 => {
                    offset = range_end(r.range.as_deref());
                    if offset as usize >= bytes.len() {
                        return Err(Error::Protocol(
                            "upload has every byte but isn't finished".into(),
                        ));
                    }
                }
                404 | 410 => {
                    *url = None;
                    return Err(Error::Network("the upload session expired".into()));
                }
                _ => return Err(r.error()),
            }
        }
    }
}

/// The content type to send for `path` (Drive keeps it as the file's
/// type, which decides how its web app shows and opens the file).
pub fn mime_for(path: &str) -> &'static str {
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsm" => "application/vnd.ms-excel.sheet.macroEnabled.12",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "pdf" => "application/pdf",
        "csv" => "text/csv",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odt" => "application/vnd.oasis.opendocument.text",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "xls" => "application/vnd.ms-excel",
        "doc" => "application/msword",
        "ppt" => "application/vnd.ms-powerpoint",
        _ => "application/octet-stream",
    }
}

/// The next byte after a `Range: bytes=0-N` reply (0 without one).
fn range_end(range: Option<&str>) -> u64 {
    range
        .and_then(|r| {
            r.trim()
                .strip_prefix("bytes=")?
                .split('-')
                .nth(1)?
                .parse::<u64>()
                .ok()
        })
        .map_or(0, |n| n + 1)
}

/// Who signed in: `(permission id, e-mail, display name)`.
pub fn whoami(c: &Client) -> Result<(String, String, String), Error> {
    let url = format!(
        "{}about?fields=user(displayName,emailAddress,permissionId)",
        c.base
    );
    let r = Drive::call(c, "GET", &url, None)?;
    if r.status != 200 {
        return Err(r.error());
    }
    let v = r.json()?;
    let u = &v["user"];
    let email = u["emailAddress"].as_str().unwrap_or_default().to_owned();
    let id = u["permissionId"].as_str().unwrap_or(&email).to_owned();
    let name = u["displayName"].as_str().unwrap_or_default().to_owned();
    Ok((id, email, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_and_ranges() {
        let v: Value = serde_json::from_str(
            r#"{"id":"F1","name":"a.xlsx","mimeType":"application/x","size":"1234","version":"17","modifiedTime":"2026-11-27T01:02:03.000Z","sha1Checksum":"AA"}"#,
        )
        .unwrap();
        let e = entry_of(&v);
        assert_eq!((e.size, e.etag.as_deref()), (Some(1234), Some("17")));
        assert_eq!(e.sha1(), None, "not 40 hex digits");
        assert!(entry_of(&serde_json::json!({"name":"K","mimeType":FOLDER_MIME})).is_dir);
        assert_eq!(range_end(Some("bytes=0-5242879")), 5_242_880);
        assert_eq!(range_end(None), 0);
        assert!(same_version("\"17\"", "17"));
        assert_eq!(mime_for("Kantor/A.PDF"), "application/pdf");
        assert_eq!(mime_for("x"), "application/octet-stream");
    }

    #[test]
    fn ids_forget_subtrees() {
        let ids = Ids::default();
        ids.set("Kantor", "k");
        ids.set("Kantor/a.docx", "a");
        ids.set("Kantorx", "x");
        ids.forget("Kantor");
        assert_eq!(ids.get("Kantor/a.docx"), None);
        assert_eq!(ids.get("Kantorx").as_deref(), Some("x"));
    }
}
