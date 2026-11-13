//! The blocking workflows behind the app's cloud commands: open (download,
//! working copy, lock), upload with the conflict rules, reload, save a
//! conflict copy, lock refresh and release, crash cleanup. Run them off the
//! UI thread. The app turns their results into [`crate::sync`] events.

use std::time::Duration;

use crate::cache::{Cache, CacheEntry, HeldLock, Stamp, write_atomic};
use crate::sync::{LockState, UploadOutcome, numbered};
use crate::{Client, Error, LockToken, Precondition};

/// Locks are taken for this long and refreshed every [`LOCK_REFRESH`], so a
/// crashed Kantor's lock expires on the server within half an hour even if
/// the cleanup on the next start never runs.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(30 * 60);
pub const LOCK_REFRESH: Duration = Duration::from_secs(10 * 60);

/// One signed-in account.
pub struct Remote {
    pub client: Client,
    /// The account's id in the cache and settings.
    pub account: String,
    /// What a lock's `owner` says ("Kantor (budi)").
    pub owner: String,
}

/// A file opened from the server.
pub struct Opened {
    pub entry: CacheEntry,
    pub lock: LockState,
}

/// Errors that mean "the server can't be reached" (keep working offline),
/// as opposed to the server answering no.
pub fn is_offline(e: &Error) -> bool {
    matches!(e, Error::Network(_))
}

/// Lock `remote`, then download it into its working copy.
///
/// The lock comes first: Nextcloud changes a file's ETag when it is locked,
/// and locking first also means nobody can change it between the download
/// and the lock. If the working copy has local saves that never reached the
/// server (edited offline, then closed), those are kept: the copy is opened
/// as is and syncs under the usual rules.
pub fn open(r: &Remote, cache: &Cache, remote: &str) -> Result<Opened, Error> {
    if let Some(entry) = cache.find_remote(&r.account, remote)
        && entry.local.is_file()
        && entry.has_local_changes()
    {
        let lock = take_lock(r, cache, remote);
        return Ok(Opened { entry, lock });
    }
    let lock = take_lock(r, cache, remote);
    let mut entry = CacheEntry {
        account: r.account.clone(),
        remote: remote.to_owned(),
        local: cache.local_path(&r.account, remote),
        ..Default::default()
    };
    let downloaded = download(r, cache, &mut entry);
    if let Err(e) = downloaded {
        if let LockState::Held(t) = &lock {
            let _ = release(r, cache, remote, t);
        }
        return Err(e);
    }
    Ok(Opened { entry, lock })
}

/// GET into the working copy and record the version (ETag, time, size).
fn download(r: &Remote, cache: &Cache, entry: &mut CacheEntry) -> Result<(), Error> {
    let fetched = r.client.get(&entry.remote)?;
    write_atomic(&entry.local, &fetched.bytes).map_err(io)?;
    match r.client.stat(&entry.remote) {
        Ok(server) => entry.rebase(&server),
        Err(_) => {
            entry.etag = fetched.etag;
            entry.modified = None;
            entry.size = Some(fetched.bytes.len() as u64);
        }
    }
    entry.synced = Stamp::of(&entry.local);
    cache.save_entry(entry).map_err(io)
}

/// The working copy of `remote`, for opening while the server is unreachable.
pub fn open_offline(cache: &Cache, account: &str, remote: &str) -> Option<CacheEntry> {
    cache
        .find_remote(account, remote)
        .filter(|e| e.local.is_file())
}

/// Take the lock: held, held by someone else (with their name), or not
/// available on this server.
pub fn take_lock(r: &Remote, cache: &Cache, remote: &str) -> LockState {
    match r.client.lock(remote, LOCK_TIMEOUT, &r.owner) {
        Ok(token) => {
            let _ = cache.record_lock(HeldLock {
                account: r.account.clone(),
                remote: remote.to_owned(),
                token: token.0.clone(),
            });
            LockState::Held(token)
        }
        Err(Error::Locked) => LockState::LockedBy(
            r.client
                .lock_owner(remote)
                .ok()
                .flatten()
                .unwrap_or_else(|| "?".into()),
        ),
        Err(e) if is_offline(&e) => LockState::None,
        Err(e) => LockState::Unsupported(e.to_string()),
    }
}

/// Upload the working copy.
///
/// Unless `force`, the server's ETag is checked first: a stale `If-Match`
/// was accepted once by Nextcloud 35 in testing (ADR 0002), so a changed
/// ETag is a conflict even before the PUT. The PUT itself carries
/// `If-Match` (or `If-None-Match: *` for a file not on the server yet) and
/// the lock token.
pub fn upload(
    r: &Remote,
    entry: &CacheEntry,
    lock: Option<&LockToken>,
    force: bool,
    progress: &mut dyn FnMut(u64, u64),
) -> UploadOutcome {
    let stamp = Stamp::of(&entry.local);
    let bytes = match std::fs::read(&entry.local) {
        Ok(b) => b,
        Err(e) => return UploadOutcome::Failed(format!("can't read the working copy: {e}")),
    };
    // The app may have saved again while reading: upload what was read, and
    // let the stamp mismatch trigger another upload.
    if Stamp::of(&entry.local) != stamp {
        return UploadOutcome::Busy;
    }
    // The ETag to send: the base, or the server's current one when only
    // its metadata changed (a lock or unlock on Nextcloud).
    let mut if_match = entry.etag.clone();
    if !force {
        match r.client.stat(&entry.remote) {
            Ok(server) if entry.same_version(&server) => if_match = server.etag,
            Ok(server) => {
                return UploadOutcome::Conflict {
                    server_etag: server.etag,
                };
            }
            Err(Error::NotFound) if entry.etag.is_some() => {
                return UploadOutcome::Conflict { server_etag: None };
            }
            Err(Error::NotFound) => {}
            Err(e) => return failure(r, entry, lock, e),
        }
    }
    let cond = match (&if_match, force) {
        (_, true) => Precondition::Overwrite,
        (Some(e), false) => Precondition::IfMatch(e),
        (None, false) => Precondition::IfNoneMatch,
    };
    match r
        .client
        .put_with_progress(&entry.remote, &bytes, cond, lock, progress)
    {
        Ok(etag) => {
            let server = r.client.stat(&entry.remote).ok().or_else(|| {
                // No PROPFIND answer: keep what the PUT said.
                Some(crate::Entry {
                    href: String::new(),
                    name: entry.name().to_owned(),
                    is_dir: false,
                    size: Some(bytes.len() as u64),
                    etag,
                    last_modified: None,
                    content_type: None,
                })
            });
            UploadOutcome::Synced { server, stamp }
        }
        Err(e) => failure(r, entry, lock, e),
    }
}

fn failure(r: &Remote, entry: &CacheEntry, lock: Option<&LockToken>, e: Error) -> UploadOutcome {
    match e {
        Error::Conflict => UploadOutcome::Conflict {
            server_etag: r.client.stat(&entry.remote).ok().and_then(|e| e.etag),
        },
        Error::Locked if lock.is_some() => UploadOutcome::Busy,
        Error::Locked => UploadOutcome::LockedBy(
            r.client
                .lock_owner(&entry.remote)
                .ok()
                .flatten()
                .unwrap_or_else(|| "?".into()),
        ),
        Error::TooEarly => UploadOutcome::Busy,
        e if is_offline(&e) => UploadOutcome::Offline(e.to_string()),
        e => UploadOutcome::Failed(e.to_string()),
    }
}

/// Replace the working copy with the server's version.
pub fn reload(r: &Remote, cache: &Cache, entry: &CacheEntry) -> Result<CacheEntry, Error> {
    let mut fresh = entry.clone();
    download(r, cache, &mut fresh)?;
    Ok(fresh)
}

/// Whether the server's copy is a different version than the working
/// copy's base (not just a lock's new ETag). `None`: can't tell (offline).
pub fn server_changed(r: &Remote, entry: &CacheEntry) -> Option<bool> {
    match r.client.stat(&entry.remote) {
        Ok(server) => Some(!entry.same_version(&server)),
        Err(Error::NotFound) => Some(true),
        Err(_) => None,
    }
}

/// Upload the working copy as `copy_name` in the same folder, never over an
/// existing file (numbered ` 2`, ` 3`… if taken). Returns its remote path.
///
/// OpenCloud 7.2 ignores `If-None-Match: *` (ADR 0002), so each name is
/// checked with a PROPFIND first as well.
pub fn save_copy(r: &Remote, entry: &CacheEntry, copy_name: &str) -> Result<String, Error> {
    let bytes = std::fs::read(&entry.local).map_err(io)?;
    let folder = entry.folder();
    for n in 1..100 {
        let name = if n == 1 {
            copy_name.to_owned()
        } else {
            numbered(copy_name, n)
        };
        let path = if folder.is_empty() {
            name
        } else {
            format!("{folder}/{name}")
        };
        match r.client.stat(&path) {
            Ok(_) => continue,
            Err(Error::NotFound) => {}
            Err(e) => return Err(e),
        }
        match r.client.put(&path, &bytes, Precondition::IfNoneMatch, None) {
            Ok(_) => return Ok(path),
            Err(Error::Conflict) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(Error::Protocol("no free name for the copy".into()))
}

/// Overwrite the server's copy with the working copy (the user chose to).
pub fn overwrite(r: &Remote, entry: &CacheEntry, lock: Option<&LockToken>) -> UploadOutcome {
    upload(r, entry, lock, true, &mut |_, _| {})
}

pub fn refresh_lock(r: &Remote, remote: &str, token: &LockToken) -> Result<(), Error> {
    r.client.refresh_lock(remote, token, LOCK_TIMEOUT)
}

/// Release a lock and drop it from the ledger. An unreachable server keeps
/// it in the ledger for [`cleanup_locks`]; the lock expires anyway.
pub fn release(r: &Remote, cache: &Cache, remote: &str, token: &LockToken) -> Result<(), Error> {
    match r.client.unlock(remote, token) {
        Ok(()) | Err(Error::NotFound) | Err(Error::Conflict) => {
            let _ = cache.forget_lock(&r.account, remote);
            Ok(())
        }
        // 4xx other than the above: the lock is gone or not ours anymore.
        Err(Error::Http { status, .. }) if (400..500).contains(&status) => {
            let _ = cache.forget_lock(&r.account, remote);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Release the locks a previous run left behind (crash, kill, no network at
/// exit) for `r`'s account, except those in `keep` (files open now).
/// Returns how many were released.
pub fn cleanup_locks(r: &Remote, cache: &Cache, keep: &[String]) -> usize {
    let mut n = 0;
    for l in cache.held_locks() {
        if l.account != r.account || keep.contains(&l.remote) {
            continue;
        }
        if release(r, cache, &l.remote, &l.token()).is_ok() {
            n += 1;
        }
    }
    n
}

fn io(e: std::io::Error) -> Error {
    Error::Protocol(format!("working copy: {e}"))
}
