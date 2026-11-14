//! The blocking workflows behind the app's cloud commands: open (download,
//! working copy, lock), upload with the conflict rules, reload, save a
//! conflict copy, lock refresh and release, crash cleanup. Run them off the
//! UI thread. The app turns their results into [`crate::sync`] events.

use std::time::Duration;

use crate::cache::{
    Cache, CacheEntry, Compare, HeldLock, PendingUpload, Stamp, sha256_hex, write_atomic,
};
use crate::chunked::{RESUMABLE_MIN, Resumable};
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
    /// What a lock's `owner` says ("budi (Kantor)"): shown to others.
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
        let mut entry = entry;
        let (lock, _) = take_lock_settled(r, cache, &mut entry);
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
    entry.sha256 = Some(sha256_hex(&fetched.bytes));
    entry.sha1 = Some(crate::sha1_hex(&fetched.bytes));
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
            lock_holder(r, remote)
                .ok()
                .flatten()
                .unwrap_or_else(|| "?".into()),
        ),
        Err(e) if is_offline(&e) => LockState::None,
        // Locks may not be possible here (OpenCloud doesn't lock files in a
        // share's mount for the recipient) while someone else's lock is:
        // still show the file read-only then.
        Err(e) => match lock_holder(r, remote) {
            Ok(Some(owner)) => LockState::LockedBy(owner),
            _ => LockState::Unsupported(e.to_string()),
        },
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
    cache: &Cache,
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
            Ok(server) => match is_base_version(r, entry, &server) {
                Ok(true) => if_match = server.etag,
                // An earlier upload of these very bytes went through after
                // Kantor had given up on it (the link came back): nothing
                // to send, and no conflict.
                Ok(false) if has_bytes(&server, &bytes) => {
                    return UploadOutcome::Synced {
                        server: Some(Box::new(server)),
                        stamp,
                        lock_lost: false,
                        sha256: Some(sha256_hex(&bytes)),
                        sha1: Some(crate::sha1_hex(&bytes)),
                    };
                }
                Ok(false) => {
                    return UploadOutcome::Conflict {
                        server_etag: server.etag,
                    };
                }
                Err(e) => return failure(r, entry, lock, e),
            },
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
    let mut lock_lost = false;
    if bytes.len() >= RESUMABLE_MIN {
        match resumable_upload(r, cache, entry, &bytes, &if_match, force, lock, progress) {
            Some(Ok((etag, lock_lost))) => {
                return synced(r, entry, &bytes, etag, stamp, lock_lost);
            }
            Some(Err(e)) => return failure(r, entry, lock, e),
            None => {}
        }
    }
    let mut put = r
        .client
        .put_with_progress(&entry.remote, &bytes, cond, lock, progress);
    // A refusal of a PUT carrying our lock token may mean the lock is gone
    // (expired while the computer slept, or removed by an admin), not that
    // the file changed: the version check above passed (or the user chose
    // to overwrite). Nextcloud answers 412, OpenCloud 500. Retry without
    // the token.
    if let (Err(e), Some(token)) = (&put, lock)
        && !is_offline(e)
        && !matches!(e, Error::Locked)
        && r.client
            .refresh_lock(&entry.remote, token, LOCK_TIMEOUT)
            .is_err()
    {
        lock_lost = true;
        put = r
            .client
            .put_with_progress(&entry.remote, &bytes, cond, None, progress);
    }
    let lock = if lock_lost { None } else { lock };
    match put {
        Ok(etag) => synced(r, entry, &bytes, etag, stamp, lock_lost),
        Err(e) => failure(r, entry, lock, e),
    }
}

/// The outcome of an upload that went through.
fn synced(
    r: &Remote,
    entry: &CacheEntry,
    bytes: &[u8],
    etag: Option<String>,
    stamp: Option<Stamp>,
    lock_lost: bool,
) -> UploadOutcome {
    let server = r.client.stat(&entry.remote).ok().or_else(|| {
        // No PROPFIND answer: keep what the upload said.
        Some(crate::Entry {
            name: entry.name().to_owned(),
            size: Some(bytes.len() as u64),
            etag,
            ..Default::default()
        })
    });
    UploadOutcome::Synced {
        server: server.map(Box::new),
        stamp,
        lock_lost,
        sha256: Some(sha256_hex(bytes)),
        sha1: Some(crate::sha1_hex(bytes)),
    }
}

/// Whether the server's file is exactly `bytes` by its checksum (no
/// download): same size and the same SHA1.
fn has_bytes(server: &crate::Entry, bytes: &[u8]) -> bool {
    server.size == Some(bytes.len() as u64)
        && server.sha1().is_some_and(|s| s == crate::sha1_hex(bytes))
}

/// A big file through Nextcloud chunking or TUS, resuming an unfinished
/// upload of the same bytes. `None`: the server offers neither (use PUT).
/// Ok: the new ETag if the server said it, and whether our lock had to go.
#[allow(clippy::too_many_arguments)]
fn resumable_upload(
    r: &Remote,
    cache: &Cache,
    entry: &CacheEntry,
    bytes: &[u8],
    if_match: &Option<String>,
    force: bool,
    lock: Option<&LockToken>,
    progress: &mut dyn FnMut(u64, u64),
) -> Option<Result<(Option<String>, bool), Error>> {
    let kind = r.client.resumable(entry.folder());
    if kind == Resumable::None {
        return None;
    }
    let sha = sha256_hex(bytes);
    let pending = cache
        .pending_upload(&entry.local)
        .filter(|p| p.sha256 == sha);
    let id = match (&pending, &kind) {
        (Some(p), _) => p.id.clone(),
        // New for every upload: an id reused for another file would pick up
        // that upload's chunks and destination.
        (None, Resumable::Nextcloud { .. }) => {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let who = sha256_hex(format!("{}\n{}\n{nanos}", entry.remote, sha).as_bytes());
            format!("kantor-{}", &who[..24])
        }
        _ => String::new(),
    };
    let record = |id: &str| {
        let _ = cache.set_pending_upload(
            &entry.local,
            Some(PendingUpload {
                local: entry.local.clone(),
                sha256: sha.clone(),
                id: id.to_owned(),
            }),
        );
    };
    let cond = if force { None } else { if_match.as_deref() };
    let result = match kind {
        Resumable::Nextcloud { user } => {
            record(&id);
            r.client
                .upload_nextcloud(&user, &id, &entry.remote, bytes, cond, lock, progress)
                .map(|etag| (etag, false))
        }
        Resumable::Tus => {
            // OpenCloud refuses to start a TUS upload onto a file locked
            // even by us (423, whatever lock header is sent), so the lock
            // is released for the upload and taken again afterwards by the
            // app (`lock_lost`); If-Match still guards the start.
            let lock_lost = match lock {
                Some(t) => {
                    let _ = release(r, cache, &entry.remote, t);
                    true
                }
                None => false,
            };
            let mut url = (!id.is_empty()).then_some(id);
            let res = r
                .client
                .upload_tus(&entry.remote, bytes, cond, &mut url, progress);
            if res.is_err()
                && let Some(u) = &url
            {
                record(u);
            }
            res.map(|()| (None, lock_lost))
        }
        Resumable::None => return None,
    };
    if result.is_ok() || !result.as_ref().is_err_and(is_offline) {
        // Done, or refused for good (conflict, no space): start fresh next time.
        let _ = cache.set_pending_upload(&entry.local, None);
    }
    Some(result)
}

fn failure(r: &Remote, entry: &CacheEntry, lock: Option<&LockToken>, e: Error) -> UploadOutcome {
    match e {
        Error::Conflict => UploadOutcome::Conflict {
            server_etag: r.client.stat(&entry.remote).ok().and_then(|e| e.etag),
        },
        Error::Locked if lock.is_some() => UploadOutcome::Busy,
        Error::Locked => UploadOutcome::LockedBy(
            lock_holder(r, &entry.remote)
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
        Ok(server) => is_base_version(r, entry, &server).ok().map(|same| !same),
        Err(Error::NotFound) => Some(true),
        Err(_) => None,
    }
}

/// [`server_changed`], and when the server still has the base version but
/// under a new ETag (Nextcloud changes it on LOCK and UNLOCK, not on a
/// lock refresh), take that ETag as the base: otherwise the next upload
/// would verify the same thing again by downloading the whole file.
pub fn settle_base(r: &Remote, cache: &Cache, entry: &mut CacheEntry) -> Option<bool> {
    match r.client.stat(&entry.remote) {
        Ok(server) => {
            let same = is_base_version(r, entry, &server).ok()?;
            if same && server.etag != entry.etag {
                entry.rebase(&server);
                let _ = cache.save_entry(entry);
            }
            Some(!same)
        }
        Err(Error::NotFound) => Some(true),
        Err(_) => None,
    }
}

/// Take the lock and keep `entry`'s base in step, without downloading in
/// the usual case. Nextcloud gives the file a new ETag on LOCK, so the
/// version is checked just before the lock (same ETag as the base: the
/// server has our base) and again after it: when only the ETag moved in
/// between (same time and size, and the same SHA1 if the server keeps
/// one), the new ETag becomes the base. Otherwise the full check
/// ([`settle_base`]) decides. Returns the lock and whether the server's
/// copy changed (`None`: can't tell).
pub fn take_lock_settled(
    r: &Remote,
    cache: &Cache,
    entry: &mut CacheEntry,
) -> (LockState, Option<bool>) {
    let before = r.client.stat(&entry.remote).ok();
    let lock = take_lock(r, cache, &entry.remote);
    let same_before = before
        .as_ref()
        .is_some_and(|b| entry.compare(b) == Compare::Same);
    if same_before
        && let Ok(after) = r.client.stat(&entry.remote)
        && entry.compare(&after) != Compare::Changed
        && checksum_agrees(entry, &after)
    {
        if after.etag != entry.etag {
            entry.rebase(&after);
            let _ = cache.save_entry(entry);
        }
        return (lock, Some(false));
    }
    let changed = settle_base(r, cache, entry);
    (lock, changed)
}

/// The server's SHA1 doesn't contradict the base (no SHA1 on either side
/// counts as agreeing).
fn checksum_agrees(entry: &CacheEntry, server: &crate::Entry) -> bool {
    match (&entry.sha1, server.sha1()) {
        (Some(ours), Some(theirs)) => *ours == theirs,
        _ => true,
    }
}

/// Whether `server` (a fresh PROPFIND of the file) is still the working
/// copy's base version. A new ETag with the same modification time and
/// size is settled by downloading and comparing SHA-256: Nextcloud changes
/// the ETag on every lock and unlock, and getlastmodified has one-second
/// resolution, so neither alone can tell a lock from a quick same-size edit.
pub fn is_base_version(
    r: &Remote,
    entry: &CacheEntry,
    server: &crate::Entry,
) -> Result<bool, Error> {
    match entry.compare(server) {
        Compare::Same => Ok(true),
        Compare::Changed => Ok(false),
        // The server's SHA1 settles it without a download when both sides
        // have one (OpenCloud always; Nextcloud for files Kantor uploaded).
        Compare::Unsure => match (&entry.sha1, server.sha1(), &entry.sha256) {
            (Some(ours), Some(theirs), _) => Ok(*ours == theirs),
            (_, _, Some(base)) => Ok(&sha256_hex(&r.client.get(&entry.remote)?.bytes) == base),
            _ => Ok(false),
        },
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
pub fn overwrite(
    r: &Remote,
    cache: &Cache,
    entry: &CacheEntry,
    lock: Option<&LockToken>,
) -> UploadOutcome {
    upload(r, cache, entry, lock, true, &mut |_, _| {})
}

pub fn refresh_lock(r: &Remote, remote: &str, token: &LockToken) -> Result<(), Error> {
    r.client.refresh_lock(remote, token, LOCK_TIMEOUT)
}

/// Release a lock and drop it from the ledger. An unreachable server keeps
/// it in the ledger for [`cleanup_locks`]; the lock expires anyway.
pub fn release(r: &Remote, cache: &Cache, remote: &str, token: &LockToken) -> Result<(), Error> {
    match r.client.unlock(remote, token) {
        // Unreachable: keep it in the ledger and try again next start.
        Err(e) if is_offline(&e) => Err(e),
        // Any answer means the lock is released or no longer exists (an
        // unknown token gets 412, 409, or 500 from Nextcloud 35).
        Err(_) => {
            let _ = cache.forget_lock(&r.account, remote);
            Ok(())
        }
        Ok(()) => {
            let _ = cache.forget_lock(&r.account, remote);
            // Nextcloud gives the file a new ETag on UNLOCK. We held the lock
            // until now, so a new ETag with the same time and size is still
            // our base: record it, and the next start needs no check.
            if let Some(mut entry) = cache.find_remote(&r.account, remote)
                && let Ok(server) = r.client.stat(remote)
                && entry.compare(&server) == Compare::Unsure
                && checksum_agrees(&entry, &server)
            {
                entry.rebase(&server);
                let _ = cache.save_entry(&entry);
            }
            Ok(())
        }
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

/// Who holds the lock on `remote`, by display name ("Bob Santoso") when the
/// server will say it, else the lock's owner text. `None`: not locked.
pub fn lock_holder(r: &Remote, remote: &str) -> Result<Option<String>, Error> {
    let Some(owner) = r.client.lock_holder(remote)? else {
        return Ok(None);
    };
    let user = owner
        .user
        .clone()
        .or_else(|| kantor_user(&owner.text))
        .or_else(|| id_at_issuer(&owner.text));
    let name = user.and_then(|u| r.client.display_name(&u));
    Ok(Some(name.unwrap_or(owner.text)))
}

/// The account id in an owner text like OpenCloud's
/// `<uuid>@https://drive.example` (it records the user, not our owner).
fn id_at_issuer(text: &str) -> Option<String> {
    let (id, issuer) = text.split_once('@')?;
    (issuer.starts_with("http") && !id.is_empty() && !id.contains(' ')).then(|| id.to_owned())
}

/// The account in an owner text Kantor wrote ("bob (Kantor)" → "bob").
fn kantor_user(text: &str) -> Option<String> {
    text.strip_suffix(" (Kantor)")
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_owned)
}

fn io(e: std::io::Error) -> Error {
    Error::Protocol(format!("working copy: {e}"))
}
