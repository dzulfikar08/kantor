//! Resumable uploads for big files: Nextcloud's chunked upload v2, or TUS
//! (OpenCloud/OCIS). A dropped upload continues from the last chunk the
//! server has instead of starting over. Other servers get one plain PUT.
//!
//! Nextcloud (`remote.php/dav/uploads/<user>/<id>`): MKCOL the upload
//! folder with the destination, PUT numbered chunks of at least 5 MiB,
//! then MOVE `.file` onto the destination. Chunks already on the server
//! (same name and size) are skipped on resume. The MOVE carries the
//! destination's ETag and lock token in a tagged `If` header.
//!
//! TUS (`Tus-Resumable: 1.0.0`): POST the parent folder with the length and
//! file name, PATCH from the offset the server reports (HEAD) on resume.

use base64::Engine as _;
use ureq::http::{self, Method};

use crate::{Client, Error, LockToken, Reply, STALL_TIMEOUT, percent_encode, reply_wait};

/// Files at least this big are uploaded resumably when the server can.
pub const RESUMABLE_MIN: usize = 8 * 1024 * 1024;
/// Nextcloud v2 chunk size (its minimum is 5 MiB, except the last chunk).
pub const NC_CHUNK: usize = 5 * 1024 * 1024;
/// TUS PATCH size: how much is redone at worst after a drop.
pub const TUS_CHUNK: usize = 2 * 1024 * 1024;

/// How a server takes resumable uploads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resumable {
    /// Nextcloud chunked upload v2 for this account id.
    Nextcloud {
        user: String,
    },
    Tus,
    None,
}

impl Client {
    /// Which resumable protocol the server offers for files in `folder`.
    pub fn resumable(&self, folder: &str) -> Resumable {
        if let Some(user) = self.nextcloud_user() {
            let url = format!(
                "{}/remote.php/dav/uploads/{}/",
                self.origin(),
                percent_encode(&user)
            );
            let req = self.request_url("PROPFIND", &url).header("depth", "0");
            if let Ok(r) = self.send(req, &[])
                && r.status == 207
            {
                return Resumable::Nextcloud { user };
            }
        }
        let mut dir = folder.trim_end_matches('/').to_owned();
        dir.push('/');
        if let Ok(r) = self.send(self.request("OPTIONS", &dir), &[])
            && r.tus
        {
            return Resumable::Tus;
        }
        Resumable::None
    }

    /// The account id in a Nextcloud-style WebDAV root
    /// (`…/remote.php/dav/files/<user>/`).
    fn nextcloud_user(&self) -> Option<String> {
        let i = self.base.find("/remote.php/dav/files/")?;
        let rest = &self.base[i + "/remote.php/dav/files/".len()..];
        let user = rest.trim_end_matches('/');
        (!user.is_empty() && !user.contains('/')).then(|| crate::percent_decode(user))
    }

    fn request_url(&self, method: &str, url: &str) -> http::request::Builder {
        let mut b = http::Request::builder()
            .method(Method::from_bytes(method.as_bytes()).expect("valid method"))
            .uri(url);
        if let Some(a) = &self.auth {
            b = b.header("authorization", a);
        }
        b
    }

    /// Send one part of an upload (`req`: method, URL and headers) with
    /// progress: `base` bytes were sent before, of `total`.
    fn put_part(
        &self,
        req: http::request::Builder,
        bytes: &[u8],
        base: u64,
        total: u64,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<Reply, Error> {
        let req = req.header("content-length", bytes.len().to_string());
        self.exec(
            req,
            bytes,
            Some(reply_wait(bytes.len() as u64)),
            Some((progress, base, total)),
        )
    }

    /// `location` (absolute or a path) on this client's server: OpenCloud
    /// builds upload URLs from its configured address, which may differ
    /// from the one this client uses (a proxy, another name).
    fn same_origin(&self, location: &str) -> String {
        let path = match location.find("://") {
            Some(i) => location[i + 3..]
                .find('/')
                .map_or("/", |j| &location[i + 3 + j..]),
            None => location,
        };
        format!("{}{path}", self.origin())
    }

    /// Nextcloud chunked upload v2 of `bytes` to `path` under upload id
    /// `id` (reuse the id to resume). `if_match`: the ETag the destination
    /// must still have.
    #[allow(clippy::too_many_arguments)]
    pub fn upload_nextcloud(
        &self,
        user: &str,
        id: &str,
        path: &str,
        bytes: &[u8],
        if_match: Option<&str>,
        lock: Option<&LockToken>,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<Option<String>, Error> {
        let dest = self.url(path);
        let up = format!(
            "{}/remote.php/dav/uploads/{}/{}",
            self.origin(),
            percent_encode(user),
            percent_encode(id)
        );
        let total = bytes.len() as u64;
        let r = self.send(
            self.request_url("MKCOL", &up).header("destination", &dest),
            &[],
        )?;
        if !matches!(r.status, 201 | 405) {
            return Err(r.error());
        }
        // Chunks the server already has (resume).
        let have: Vec<(String, Option<u64>)> = {
            let req = self
                .request_url("PROPFIND", &format!("{up}/"))
                .header("depth", "1")
                .header("content-type", "application/xml; charset=utf-8");
            let body = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:getcontentlength/></d:prop></d:propfind>"#;
            let r = self.send(req, body.as_bytes())?;
            if r.status != 207 {
                return Err(r.error());
            }
            crate::parse_multistatus(&String::from_utf8_lossy(&r.decoded()?))?
                .into_iter()
                .map(|e| (e.name, e.size))
                .collect()
        };
        let mut sent = 0u64;
        for (i, chunk) in bytes.chunks(NC_CHUNK).enumerate() {
            let name = format!("{:05}", i + 1);
            let len = chunk.len() as u64;
            if have.iter().any(|(n, s)| *n == name && *s == Some(len)) {
                sent += len;
                progress(sent, total);
                continue;
            }
            let req = self
                .request_url("PUT", &format!("{up}/{name}"))
                .header("destination", &dest)
                .header("oc-total-length", total.to_string());
            let r = self.put_part(req, chunk, sent, total, progress)?;
            if !matches!(r.status, 200 | 201 | 204) {
                return Err(r.error());
            }
            sent += len;
        }
        let mut req = self
            .request_url("MOVE", &format!("{up}/.file"))
            .header("destination", &dest)
            .header("oc-total-length", total.to_string())
            .header("oc-checksum", format!("SHA1:{}", crate::sha1_hex(bytes)))
            .header("overwrite", "T");
        // The destination's ETag and our lock go in a tagged `If` list:
        // Nextcloud checks `If-Match` on a MOVE against the source (the
        // assembled `.file`), so it would always fail.
        if let Some(cond) = dest_condition(&dest, if_match, lock) {
            req = req.header("if", cond);
        }
        // Assembling a big file can take a while on the server.
        let r = self.exec(req, &[], Some(reply_wait(total) + STALL_TIMEOUT), None)?;
        match r.status {
            200 | 201 | 204 => Ok(r.etag),
            _ => Err(r.error()),
        }
    }

    /// TUS upload of `bytes` to `path`. `url`: the upload's URL from an
    /// earlier attempt (resume), set when a new upload is created.
    pub fn upload_tus(
        &self,
        path: &str,
        bytes: &[u8],
        if_match: Option<&str>,
        url: &mut Option<String>,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<(), Error> {
        let total = bytes.len() as u64;
        let tus = ("tus-resumable", "1.0.0".to_owned());
        // Where to continue: the server's offset for a known upload.
        let mut offset = None;
        if let Some(u) = url.as_deref() {
            let req = self.request_url("HEAD", u).header(tus.0, &tus.1);
            match self.send(req, &[]) {
                Ok(r) if r.status == 200 || r.status == 204 => offset = r.upload_offset,
                Ok(_) => {}
                Err(e) => return Err(e),
            }
        }
        let mut offset = match offset {
            Some(o) => o,
            None => {
                let (folder, name) = match path.trim_matches('/').rsplit_once('/') {
                    Some((f, n)) => (format!("{f}/"), n.to_owned()),
                    None => (String::new(), path.trim_matches('/').to_owned()),
                };
                let b64 = base64::engine::general_purpose::STANDARD;
                let mut req = self
                    .request("POST", &folder)
                    .header(tus.0, &tus.1)
                    .header("upload-length", total.to_string())
                    .header("upload-metadata", format!("filename {}", b64.encode(&name)));
                // OpenCloud checks If-Match when the upload is created.
                if let Some(e) = if_match {
                    req = req.header("if-match", e);
                }
                let r = self.send(req, &[])?;
                if r.status != 201 {
                    return Err(r.error());
                }
                let loc = r
                    .location
                    .ok_or_else(|| Error::Protocol("TUS: no Location".into()))?;
                *url = Some(self.same_origin(&loc));
                0
            }
        };
        let u = url.clone().unwrap_or_default();
        progress(offset, total);
        while offset < total {
            let end = (offset as usize + TUS_CHUNK).min(bytes.len());
            let part = &bytes[offset as usize..end];
            let req = self
                .request_url("PATCH", &u)
                .header(tus.0, &tus.1)
                .header("upload-offset", offset.to_string())
                .header("content-type", "application/offset+octet-stream");
            let r = self.put_part(req, part, offset, total, progress)?;
            if !matches!(r.status, 200 | 204) {
                return Err(r.error());
            }
            offset = r.upload_offset.unwrap_or(end as u64);
        }
        Ok(())
    }
}

/// `<dest> (<lock-token> ["etag"])`: RFC 4918 §10.4 tagged-list
/// conditions on the destination.
fn dest_condition(dest: &str, etag: Option<&str>, lock: Option<&LockToken>) -> Option<String> {
    let mut list = Vec::new();
    if let Some(LockToken(t)) = lock {
        list.push(format!("<{t}>"));
    }
    if let Some(e) = etag {
        list.push(format!("[{e}]"));
    }
    (!list.is_empty()).then(|| format!("<{dest}> ({})", list.join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Auth;

    #[test]
    fn destination_conditions() {
        let t = LockToken("opaquelocktoken:1".into());
        assert_eq!(
            dest_condition("http://h/f", Some("\"e1\""), Some(&t)).as_deref(),
            Some("<http://h/f> (<opaquelocktoken:1> [\"e1\"])")
        );
        assert_eq!(
            dest_condition("http://h/f", Some("\"e1\""), None).as_deref(),
            Some("<http://h/f> ([\"e1\"])")
        );
        assert_eq!(dest_condition("http://h/f", None, None), None);
    }

    #[test]
    fn upload_urls_use_our_origin() {
        let c = Client::new(
            "http://127.0.0.1:29200/remote.php/dav/files/alan/",
            Auth::None,
        );
        assert_eq!(
            c.same_origin("http://localhost:9200/data/abc.def?x=1"),
            "http://127.0.0.1:29200/data/abc.def?x=1"
        );
        assert_eq!(
            c.same_origin("/data/xyz"),
            "http://127.0.0.1:29200/data/xyz"
        );
    }

    #[test]
    fn nextcloud_user_from_root() {
        let c = Client::new(
            "https://d.example/remote.php/dav/files/budi%20s/",
            Auth::None,
        );
        assert_eq!(c.nextcloud_user().as_deref(), Some("budi s"));
        let c = Client::new("https://d.example/dav/files/x/", Auth::None);
        assert_eq!(c.nextcloud_user(), None);
    }
}
