//! Parsing WebDAV XML replies (RFC 4918): `207 Multi-Status` listings and
//! `LOCK` responses. Only the `DAV:` namespace is read; vendor properties
//! (Nextcloud's `oc:`, OCIS's `oc:`) are ignored so the server stays swappable.

use crate::{Entry, Error, percent_decode};

const DAV: &str = "DAV:";

/// The entries of a `PROPFIND` reply, in server order. Propstats whose status
/// isn't 200 (properties the server doesn't have) are skipped.
pub fn parse_multistatus(body: &str) -> Result<Vec<Entry>, Error> {
    let doc = roxmltree::Document::parse(body)
        .map_err(|e| Error::Protocol(format!("bad PROPFIND XML: {e}")))?;
    let root = doc.root_element();
    if !is(root, "multistatus") {
        return Err(Error::Protocol(format!(
            "expected multistatus, got <{}>",
            root.tag_name().name()
        )));
    }
    let mut out = Vec::new();
    for resp in root.children().filter(|n| is(*n, "response")) {
        let Some(href) = child(resp, "href").and_then(|n| n.text()) else {
            continue;
        };
        let href = href.trim().to_string();
        let mut entry = Entry {
            name: name_of(&href),
            href,
            is_dir: false,
            size: None,
            etag: None,
            last_modified: None,
            content_type: None,
        };
        for ps in resp.children().filter(|n| is(*n, "propstat")) {
            let ok = child(ps, "status")
                .and_then(|n| n.text())
                .is_none_or(|s| s.split_whitespace().nth(1) == Some("200"));
            let Some(prop) = child(ps, "prop") else {
                continue;
            };
            if !ok {
                continue;
            }
            for p in prop.children().filter(|n| n.is_element()) {
                if p.tag_name().namespace() != Some(DAV) {
                    continue;
                }
                let text = || p.text().map(|t| t.trim().to_string());
                match p.tag_name().name() {
                    "resourcetype" => entry.is_dir = child(p, "collection").is_some(),
                    "getcontentlength" => entry.size = text().and_then(|t| t.parse().ok()),
                    "getetag" => entry.etag = text().filter(|t| !t.is_empty()),
                    "getlastmodified" => entry.last_modified = text(),
                    "getcontenttype" => entry.content_type = text(),
                    _ => {}
                }
            }
        }
        out.push(entry);
    }
    Ok(out)
}

/// The lock token in a `LOCK` reply body (`lockdiscovery/activelock/locktoken/href`).
pub fn parse_lock_token(body: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(body).ok()?;
    doc.descendants()
        .filter(|n| is(*n, "locktoken"))
        .find_map(|n| child(n, "href").and_then(|h| h.text()))
        .map(|t| t.trim().to_string())
}

/// Who holds a lock, from a PROPFIND of `lockdiscovery` (plus Nextcloud's
/// `nc:lock-owner-displayname` / `nc:lock-owner` when present). `None` when
/// the reply shows no active lock.
pub fn parse_lock_owner(body: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(body).ok()?;
    let text_of = |n: roxmltree::Node| -> Option<String> {
        let t: String = n
            .descendants()
            .filter(|d| d.is_text())
            .filter_map(|d| d.text())
            .collect::<Vec<_>>()
            .join(" ");
        let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
        (!t.is_empty()).then_some(t)
    };
    for name in ["lock-owner-displayname", "lock-owner"] {
        if let Some(t) = doc
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == name)
            .and_then(text_of)
        {
            return Some(t);
        }
    }
    let active = doc.descendants().find(|n| is(*n, "activelock"))?;
    Some(
        child(active, "owner")
            .and_then(text_of)
            .unwrap_or_else(|| "?".into()),
    )
}

/// The lock owner's account id when the server says it (Nextcloud's
/// `nc:lock-owner`).
pub fn parse_lock_user(body: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(body).ok()?;
    doc.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "lock-owner")
        .and_then(|n| n.text())
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
}

/// The display name of account `user` in a user-search reply: Nextcloud's
/// autocomplete (`ocs.data[].id` / `label`) or OpenCloud's Graph search
/// (`value[].onPremisesSamAccountName` / `displayName`).
pub fn parse_user_search(json: &str, user: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let found = |list: &serde_json::Value, id_keys: &[&str], name_key: &str| {
        list.as_array()?.iter().find_map(|u| {
            let matches = id_keys
                .iter()
                .any(|k| u.get(*k).and_then(|x| x.as_str()) == Some(user));
            matches
                .then(|| u.get(name_key)?.as_str().map(str::to_owned))
                .flatten()
        })
    };
    let name = v
        .pointer("/ocs/data")
        .and_then(|d| found(d, &["id"], "label"))
        .or_else(|| {
            v.get("value")
                .and_then(|d| found(d, &["onPremisesSamAccountName", "id"], "displayName"))
        })?;
    let name = name.trim().to_owned();
    (!name.is_empty()).then_some(name)
}

/// The display name in a Nextcloud OCS user reply or an OpenCloud Graph
/// user reply (JSON).
pub fn parse_display_name(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let name = v
        .pointer("/ocs/data/displayname")
        .or_else(|| v.pointer("/ocs/data/display-name"))
        .or_else(|| v.get("displayName"))?
        .as_str()?
        .trim()
        .to_owned();
    (!name.is_empty()).then_some(name)
}

fn is(n: roxmltree::Node, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(DAV)
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| is(*c, name))
}

/// The last path segment of an href, percent-decoded (`/a/b%20c/` -> `b c`).
fn name_of(href: &str) -> String {
    let path = href
        .split_once("://")
        .map_or(href, |(_, rest)| rest.find('/').map_or("/", |i| &rest[i..]));
    let seg = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    percent_decode(seg)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from a real Nextcloud 35 reply.
    const NEXTCLOUD: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:s="http://sabredav.org/ns" xmlns:oc="http://owncloud.org/ns">
 <d:response><d:href>/remote.php/dav/files/admin/Docs/</d:href>
  <d:propstat><d:prop><d:getlastmodified>Thu, 13 Nov 2026 06:00:00 GMT</d:getlastmodified>
   <d:resourcetype><d:collection/></d:resourcetype><d:getetag>"6915a1"</d:getetag></d:prop>
   <d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  <d:propstat><d:prop><d:getcontentlength/><d:getcontenttype/></d:prop>
   <d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response>
 <d:response><d:href>/remote.php/dav/files/admin/Docs/Laporan%20Q3.xlsx</d:href>
  <d:propstat><d:prop><d:getlastmodified>Thu, 13 Nov 2026 06:01:00 GMT</d:getlastmodified>
   <d:getcontentlength>68437</d:getcontentlength><d:resourcetype/>
   <d:getetag>&quot;0b5c2e1f&quot;</d:getetag>
   <d:getcontenttype>application/vnd.openxmlformats-officedocument.spreadsheetml.sheet</d:getcontenttype>
   <oc:fileid>42</oc:fileid></d:prop>
   <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
</d:multistatus>"#;

    // OCIS/OpenCloud use the default namespace prefix differently.
    const OCIS: &str = r#"<multistatus xmlns="DAV:" xmlns:oc="http://owncloud.org/ns">
 <response><href>/dav/spaces/abc%24def/%C3%A9t%C3%A9.docx</href>
  <propstat><prop><resourcetype></resourcetype><getcontentlength>38291</getcontentlength>
   <getetag>"e1"</getetag></prop><status>HTTP/1.1 200 OK</status></propstat></response>
</multistatus>"#;

    #[test]
    fn nextcloud_listing() {
        let e = parse_multistatus(NEXTCLOUD).unwrap();
        assert_eq!(e.len(), 2);
        assert!(e[0].is_dir);
        assert_eq!(e[0].name, "Docs");
        assert_eq!(e[0].size, None, "404 propstat must be ignored");
        assert!(!e[1].is_dir);
        assert_eq!(e[1].name, "Laporan Q3.xlsx");
        assert_eq!(e[1].size, Some(68437));
        assert_eq!(e[1].etag.as_deref(), Some("\"0b5c2e1f\""));
        assert!(
            e[1].content_type
                .as_deref()
                .unwrap()
                .contains("spreadsheetml")
        );
    }

    #[test]
    fn ocis_listing_default_namespace() {
        let e = parse_multistatus(OCIS).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].name, "été.docx");
        assert_eq!(e[0].size, Some(38291));
        assert!(!e[0].is_dir);
    }

    #[test]
    fn rejects_non_multistatus() {
        assert!(parse_multistatus("<html/>").is_err());
        assert!(parse_multistatus("not xml").is_err());
    }

    #[test]
    fn lock_token() {
        let body = r#"<?xml version="1.0"?><d:prop xmlns:d="DAV:"><d:lockdiscovery><d:activelock>
          <d:locktype><d:write/></d:locktype><d:lockscope><d:exclusive/></d:lockscope>
          <d:locktoken><d:href>opaquelocktoken:1234-abcd</d:href></d:locktoken>
          </d:activelock></d:lockdiscovery></d:prop>"#;
        assert_eq!(
            parse_lock_token(body).as_deref(),
            Some("opaquelocktoken:1234-abcd")
        );
        assert_eq!(parse_lock_token("<d:prop xmlns:d=\"DAV:\"/>"), None);
    }

    #[test]
    fn lock_owner_nextcloud_and_plain() {
        let nc = r#"<d:multistatus xmlns:d="DAV:" xmlns:nc="http://nextcloud.org/ns"><d:response>
          <d:href>/f.xlsx</d:href><d:propstat><d:prop>
          <d:lockdiscovery><d:activelock><d:owner>Kantor (budi)</d:owner></d:activelock></d:lockdiscovery>
          <nc:lock-owner-displayname>Budi Santoso</nc:lock-owner-displayname>
          </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
        assert_eq!(parse_lock_owner(nc).as_deref(), Some("Budi Santoso"));
        let plain = r#"<multistatus xmlns="DAV:"><response><href>/f</href><propstat><prop>
          <lockdiscovery><activelock><owner><href>mailto:sri@example.go.id</href></owner></activelock></lockdiscovery>
          </prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
        assert_eq!(
            parse_lock_owner(plain).as_deref(),
            Some("mailto:sri@example.go.id")
        );
        let unlocked = r#"<multistatus xmlns="DAV:"><response><href>/f</href><propstat><prop>
          <lockdiscovery/></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
        assert_eq!(parse_lock_owner(unlocked), None);
        let with_user = r#"<d:multistatus xmlns:d="DAV:" xmlns:nc="http://nextcloud.org/ns"><d:response><d:propstat><d:prop>
          <nc:lock-owner>bob</nc:lock-owner><nc:lock-owner-displayname>bob (Kantor)</nc:lock-owner-displayname>
          </d:prop></d:propstat></d:response></d:multistatus>"#;
        assert_eq!(parse_lock_user(with_user).as_deref(), Some("bob"));
        assert_eq!(parse_lock_user(plain), None);
    }

    #[test]
    fn display_names() {
        let nc =
            r#"{"ocs":{"meta":{"status":"ok"},"data":{"id":"bob","displayname":"Bob Santoso"}}}"#;
        assert_eq!(parse_display_name(nc).as_deref(), Some("Bob Santoso"));
        let oc = r#"{"id":"1","displayName":"Sri Wahyuni","onPremisesSamAccountName":"sri"}"#;
        assert_eq!(parse_display_name(oc).as_deref(), Some("Sri Wahyuni"));
        assert_eq!(parse_display_name(r#"{"ocs":{"data":[]}}"#), None);
        assert_eq!(parse_display_name("not json"), None);
        let ac = r#"{"ocs":{"data":[{"id":"bobby","label":"Bobby"},{"id":"bob","label":"Bob Santoso"}]}}"#;
        assert_eq!(parse_user_search(ac, "bob").as_deref(), Some("Bob Santoso"));
        assert_eq!(parse_user_search(ac, "sri"), None);
        let graph = r#"{"value":[{"id":"u1","displayName":"Marie Curie","onPremisesSamAccountName":"marie"}]}"#;
        assert_eq!(
            parse_user_search(graph, "marie").as_deref(),
            Some("Marie Curie")
        );
    }

    #[test]
    fn names_from_absolute_hrefs() {
        assert_eq!(name_of("https://h.example/dav/a%20b/"), "a b");
        assert_eq!(name_of("/"), "");
    }
}
