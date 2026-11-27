//! The status bar's own messages (over the document's status): what was
//! just done, what is running, what went wrong.
//!
//! The newest message always wins: a timer or a progress report from an
//! earlier message never hides a later one. An error stays until the
//! user's next key press or click (or a newer message), so a failure that
//! arrives while the user looks away is still there when they look back.

use gpui_kit::SharedString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NoticeKind {
    /// Shown for a few seconds.
    Info,
    /// Work under way ("Saving to Google Drive… 40%"): stays until a newer
    /// message replaces it.
    Progress,
    /// Stays until the user's next action or a newer message.
    Error,
}

#[derive(Debug)]
struct Notice {
    text: SharedString,
    kind: NoticeKind,
    id: u64,
}

#[derive(Debug, Default)]
pub(super) struct Notices {
    current: Option<Notice>,
    last_id: u64,
}

impl Notices {
    /// Show `text`, over whatever was there. Returns its id (for
    /// [`Notices::expire`] and [`Notices::progress`]).
    pub fn post(&mut self, text: SharedString, kind: NoticeKind) -> u64 {
        self.last_id += 1;
        self.current = Some(Notice {
            text,
            kind,
            id: self.last_id,
        });
        self.last_id
    }

    /// The text to show, if any.
    pub fn text(&self) -> Option<SharedString> {
        self.current.as_ref().map(|n| n.text.clone())
    }

    /// An info message's time is up: hide it unless a newer message (or an
    /// error) took its place. True when something changed.
    pub fn expire(&mut self, id: u64) -> bool {
        if self
            .current
            .as_ref()
            .is_some_and(|n| n.id == id && n.kind == NoticeKind::Info)
        {
            self.current = None;
            return true;
        }
        false
    }

    /// New progress `text` for the work that posted `id`: shown while that
    /// message is still the newest one, or when nothing is shown (a newer
    /// message came and went, an error was seen), never over a newer
    /// message. True when something changed.
    pub fn progress(&mut self, id: u64, text: SharedString) -> bool {
        match &mut self.current {
            Some(n) if n.id == id => {
                let changed = n.text != text;
                n.text = text;
                changed
            }
            None => {
                self.current = Some(Notice {
                    text,
                    kind: NoticeKind::Progress,
                    id,
                });
                true
            }
            _ => false,
        }
    }

    /// The user pressed a key or clicked: an error has been seen. True
    /// when something changed.
    pub fn user_action(&mut self) -> bool {
        if self
            .current
            .as_ref()
            .is_some_and(|n| n.kind == NoticeKind::Error)
        {
            self.current = None;
            return true;
        }
        false
    }
}

/// "4.2 of 10.0 MB" style progress for an upload of `total` bytes.
pub(super) fn upload_progress(sent: u64, total: u64) -> String {
    if total == 0 {
        return String::new();
    }
    let percent = (sent.min(total) * 100 / total).min(100);
    let mb = |b: u64| format!("{:.1}", b as f64 / 1_000_000.0);
    format!("{percent}% ({} of {} MB)", mb(sent.min(total)), mb(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(n: &Notices) -> Option<String> {
        n.text().map(|s| s.to_string())
    }

    #[std::prelude::v1::test]
    fn an_older_timer_never_hides_a_newer_message() {
        let mut n = Notices::default();
        let saving = n.post("Saving to Google Drive…".into(), NoticeKind::Info);
        let failed = n.post("Couldn't save to the cloud".into(), NoticeKind::Error);
        // The first message's timer fires after the error came in.
        assert!(!n.expire(saving));
        assert_eq!(shown(&n).as_deref(), Some("Couldn't save to the cloud"));
        // Errors don't time out at all.
        assert!(!n.expire(failed));
        assert_eq!(shown(&n).as_deref(), Some("Couldn't save to the cloud"));
        // The next key press or click clears it.
        assert!(n.user_action());
        assert_eq!(shown(&n), None);
    }

    #[std::prelude::v1::test]
    fn info_messages_time_out_and_survive_clicks() {
        let mut n = Notices::default();
        let id = n.post("Copied".into(), NoticeKind::Info);
        assert!(!n.user_action());
        assert_eq!(shown(&n).as_deref(), Some("Copied"));
        assert!(n.expire(id));
        assert_eq!(shown(&n), None);
    }

    #[std::prelude::v1::test]
    fn a_newer_message_replaces_an_error() {
        let mut n = Notices::default();
        n.post("Couldn't save".into(), NoticeKind::Error);
        let id = n.post("Saved".into(), NoticeKind::Info);
        assert_eq!(shown(&n).as_deref(), Some("Saved"));
        assert!(n.expire(id));
        assert_eq!(shown(&n), None);
    }

    #[std::prelude::v1::test]
    fn late_progress_never_covers_a_newer_message() {
        let mut n = Notices::default();
        let up = n.post("Saving… 0%".into(), NoticeKind::Progress);
        assert!(n.progress(up, "Saving… 50%".into()));
        assert_eq!(shown(&n).as_deref(), Some("Saving… 50%"));
        // Progress doesn't time out and isn't cleared by a click.
        assert!(!n.expire(up));
        assert!(!n.user_action());
        // A newer message: the upload's progress no longer shows.
        let copied = n.post("Copied".into(), NoticeKind::Info);
        assert!(!n.progress(up, "Saving… 60%".into()));
        assert_eq!(shown(&n).as_deref(), Some("Copied"));
        // Once that one has gone, the upload shows its progress again.
        assert!(n.expire(copied));
        assert!(n.progress(up, "Saving… 70%".into()));
        assert_eq!(shown(&n).as_deref(), Some("Saving… 70%"));
        // The failure that ends the upload is newest.
        n.post("Couldn't save to the cloud".into(), NoticeKind::Error);
        assert!(!n.progress(up, "Saving… 80%".into()));
        assert_eq!(shown(&n).as_deref(), Some("Couldn't save to the cloud"));
    }

    #[std::prelude::v1::test]
    fn upload_progress_reads_as_percent_and_megabytes() {
        assert_eq!(upload_progress(0, 0), "");
        assert_eq!(
            upload_progress(5_242_880, 10_485_760),
            "50% (5.2 of 10.5 MB)"
        );
        assert_eq!(upload_progress(11, 10), "100% (0.0 of 0.0 MB)");
    }
}
