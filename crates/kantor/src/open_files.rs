//! Files the system asks a running Kantor to open: macOS sends them as
//! `file://` URLs (Finder's Open With, a double-click, a drop on the Dock
//! icon), through GPUI's `on_open_urls`.
//!
//! On a cold start macOS can send the URLs before GPUI has finished
//! launching (before the first window exists) or right after it. The
//! callback only queues them ([`OpenRequests`]): `main` takes what is queued
//! when it builds the window, so those files open in it instead of a Home
//! screen, and everything that comes later opens in the running window.
//!
//! Linux and Windows pass files on the command line (`kantor %F`,
//! `kantor.exe "%1"`); those go through the same opening code.

use std::path::{Path, PathBuf};

/// The path of a `file://` URL (`file:///Users/a/My%20Budget.xlsx`), or of
/// a plain path. `None` for other URLs (`https://…`).
pub fn path_from_url(url: &str) -> Option<PathBuf> {
    let Some(rest) = strip_prefix_ignore_case(url, "file://") else {
        // A plain path (a command-line argument).
        return (!url.is_empty() && !url.contains("://")).then(|| PathBuf::from(url));
    };
    // `file:///p` and `file://localhost/p`; another host is a network
    // share Kantor can't reach this way.
    let path = if rest.starts_with('/') {
        rest
    } else {
        strip_prefix_ignore_case(rest, "localhost").filter(|p| p.starts_with('/'))?
    };
    // A query or fragment isn't part of the path.
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let bytes = percent_decode(path)?;
    #[cfg(windows)]
    {
        // file:///C:/Users/a/b.xlsx
        let s = String::from_utf8(bytes).ok()?;
        let s = s
            .strip_prefix('/')
            .filter(|s| s.get(1..2) == Some(":"))
            .unwrap_or(&s);
        Some(PathBuf::from(s.replace('/', "\\")))
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(any(unix, windows)))]
    {
        String::from_utf8(bytes).ok().map(PathBuf::from)
    }
}

fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

/// `%XX` escapes to bytes; `None` for a broken escape.
fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(out)
}

/// The paths among `urls`, in order (other URLs are skipped).
pub fn paths_from_urls(urls: &[String]) -> Vec<PathBuf> {
    urls.iter()
        .filter_map(|u| {
            let path = path_from_url(u);
            if path.is_none() {
                log::warn!("asked to open {u}, which isn't a file");
            }
            path
        })
        .collect()
}

/// Pictures a dropped file can be inserted as (Docs and Slides read PNG and
/// JPEG).
pub fn is_picture(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg"))
}

/// Files the system asked to open, queued until the window can take them.
pub struct OpenRequests {
    tx: async_channel::Sender<Vec<PathBuf>>,
    rx: async_channel::Receiver<Vec<PathBuf>>,
}

impl Default for OpenRequests {
    fn default() -> Self {
        let (tx, rx) = async_channel::unbounded();
        Self { tx, rx }
    }
}

impl OpenRequests {
    /// The handler for GPUI's `on_open_urls`: queues the files.
    pub fn handler(&self) -> impl FnMut(Vec<String>) + 'static {
        let tx = self.tx.clone();
        move |urls| {
            let paths = paths_from_urls(&urls);
            if !paths.is_empty() {
                let _ = tx.try_send(paths);
            }
        }
    }

    /// Everything queued so far (the files a cold start was launched with).
    pub fn take_pending(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        while let Ok(batch) = self.rx.try_recv() {
            paths.extend(batch);
        }
        paths
    }

    /// The queue's receiving end, for the files that come later.
    pub fn receiver(&self) -> async_channel::Receiver<Vec<PathBuf>> {
        self.rx.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn file_urls_become_paths() {
        let p = |u: &str| path_from_url(u).map(|p| p.to_string_lossy().into_owned());
        assert_eq!(
            p("file:///Users/a/My%20Budget.xlsx").as_deref(),
            Some("/Users/a/My Budget.xlsx")
        );
        assert_eq!(
            p("file://localhost/tmp/a.docx").as_deref(),
            Some("/tmp/a.docx")
        );
        assert_eq!(p("FILE:///tmp/a.pptx").as_deref(), Some("/tmp/a.pptx"));
        // UTF-8 escapes (Finder sends decomposed or composed forms as given).
        assert_eq!(
            p("file:///tmp/Laporan%20Keuangan%E2%80%942026.xlsx").as_deref(),
            Some("/tmp/Laporan Keuangan—2026.xlsx")
        );
        assert_eq!(p("file:///tmp/a%23b.csv").as_deref(), Some("/tmp/a#b.csv"));
        assert_eq!(p("file:///tmp/a.csv#frag").as_deref(), Some("/tmp/a.csv"));
        // Plain paths (command line) pass through.
        assert_eq!(p("/home/a/b.xlsx").as_deref(), Some("/home/a/b.xlsx"));
        assert_eq!(p("rel/b.xlsx").as_deref(), Some("rel/b.xlsx"));
        // Not files.
        assert_eq!(p("https://example.com/a.xlsx"), None);
        assert_eq!(p("file://server/share/a.xlsx"), None);
        assert_eq!(p("file:///tmp/bad%2"), None);
        assert_eq!(p("file:///tmp/bad%zz"), None);
        assert_eq!(p(""), None);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_names_survive() {
        use std::os::unix::ffi::OsStrExt as _;
        let path = path_from_url("file:///tmp/caf%E9.xlsx").unwrap();
        assert_eq!(path.as_os_str().as_bytes(), b"/tmp/caf\xe9.xlsx");
    }

    #[test]
    fn other_urls_are_skipped() {
        let urls = vec![
            "file:///tmp/a.xlsx".to_string(),
            "kantor://open".to_string(),
            "file:///tmp/b.docx".to_string(),
        ];
        let paths = paths_from_urls(&urls);
        assert_eq!(paths.len(), 2);
        assert!(paths[0].ends_with("a.xlsx") && paths[1].ends_with("b.docx"));
    }

    /// A cold start: URLs that come before the window are taken for it;
    /// later ones wait in the queue for the running window.
    #[test]
    fn cold_start_queue() {
        let requests = OpenRequests::default();
        let mut on_open = requests.handler();
        assert!(requests.take_pending().is_empty());
        on_open(vec!["file:///tmp/a.xlsx".into()]);
        on_open(vec![
            "file:///tmp/b.docx".into(),
            "file:///tmp/c.pptx".into(),
        ]);
        on_open(vec!["https://example.com".into()]);
        let first = requests.take_pending();
        assert_eq!(
            first,
            ["/tmp/a.xlsx", "/tmp/b.docx", "/tmp/c.pptx"]
                .map(PathBuf::from)
                .to_vec()
        );
        // Taken once: nothing is opened twice.
        assert!(requests.take_pending().is_empty());

        let later = requests.receiver();
        on_open(vec!["file:///tmp/d.csv".into()]);
        let got = futures::executor::block_on(later.recv()).unwrap();
        assert_eq!(got, vec![PathBuf::from("/tmp/d.csv")]);
    }

    #[test]
    fn pictures() {
        assert!(is_picture(Path::new("a/Photo.JPG")));
        assert!(is_picture(Path::new("a.png")));
        assert!(!is_picture(Path::new("a.xlsx")));
        assert!(!is_picture(Path::new("png")));
    }
}
