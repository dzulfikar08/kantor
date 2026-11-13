//! End-to-end tests against a real WebDAV server (Nextcloud, OpenCloud),
//! through the same `session` + `sync` code the app runs. Ignored by
//! default; `crates/kantor_cloud/bench/e2e.sh` starts the servers and runs
//! them with:
//!
//! ```text
//! KANTOR_E2E_ALICE_URL / _ALICE_PASS   alice's WebDAV root and app password
//! KANTOR_E2E_BOB_URL   / _BOB_PASS     a second account
//! KANTOR_E2E_SHARED                    folder alice shared with bob (same name in both roots)
//! KANTOR_E2E_ALICE_PROXY_URL           alice's root through Toxiproxy (optional)
//! KANTOR_E2E_TOXI_API / _TOXI_PROXY    Toxiproxy API and proxy name (optional)
//! ```

use std::path::Path;
use std::time::{Duration, Instant};

use kantor_cloud::cache::Cache;
use kantor_cloud::session::{self, Remote};
use kantor_cloud::sync::{Action, DocSync, LockState, Status, UploadOutcome};
use kantor_cloud::{Auth, Client, Error, Precondition};

struct Env {
    alice: Remote,
    /// A second client of alice's account (another computer).
    alice2: Remote,
    bob: Remote,
    alice_proxy: Option<Remote>,
    toxi: Option<(String, String)>,
    shared: String,
    cache: Cache,
    _dir: tempfile::TempDir,
}

fn var(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

fn remote(url: &str, pass: &str, account: &str) -> Remote {
    let user = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    Remote {
        client: Client::new(
            url,
            Auth::Basic {
                user: user.clone(),
                password: pass.to_owned(),
            },
        ),
        account: account.to_owned(),
        owner: format!("{user} (Kantor)"),
    }
}

fn env() -> Option<Env> {
    let (au, ap) = (var("KANTOR_E2E_ALICE_URL")?, var("KANTOR_E2E_ALICE_PASS")?);
    let (bu, bp) = (var("KANTOR_E2E_BOB_URL")?, var("KANTOR_E2E_BOB_PASS")?);
    let dir = tempfile::tempdir().unwrap();
    Some(Env {
        alice: remote(&au, &ap, "alice"),
        alice2: remote(&au, &ap, "alice2"),
        bob: remote(&bu, &bp, "bob"),
        alice_proxy: var("KANTOR_E2E_ALICE_PROXY_URL").map(|u| remote(&u, &ap, "alice-proxy")),
        toxi: var("KANTOR_E2E_TOXI_API").zip(var("KANTOR_E2E_TOXI_PROXY")),
        shared: var("KANTOR_E2E_SHARED").unwrap_or_else(|| "Bersama".into()),
        cache: Cache::new(dir.path().join("cloud")),
        _dir: dir,
    })
}

macro_rules! env_or_skip {
    () => {
        match env() {
            Some(e) => e,
            None => {
                eprintln!("skipped: KANTOR_E2E_* not set");
                return;
            }
        }
    };
}

/// A fresh .xlsx in `<shared>/e2e` (kept out of the way of real files);
/// returns its path.
fn new_file(e: &Env, tag: &str) -> String {
    e.alice.client.mkcol(&format!("{}/e2e", e.shared)).unwrap();
    let path = format!(
        "{}/e2e/{tag}-{}.xlsx",
        e.shared,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let bytes = xlsx_with("server v1");
    e.alice
        .client
        .put(&path, &bytes, Precondition::IfNoneMatch, None)
        .unwrap();
    path
}

/// An .xlsx whose A1 holds `text`, made with the real Sheets engine.
fn xlsx_with(text: &str) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.xlsx");
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/corpus/xlsx/01_formulas.xlsx"
    );
    std::fs::copy(fixture, &p).unwrap();
    edit(&p, text);
    std::fs::read(&p).unwrap()
}

/// What the app does on Ctrl+S: edit A1 in the working copy and save it.
fn edit(path: &Path, text: &str) {
    let mut wb = sheets_engine::Workbook::open(path).unwrap();
    wb.set_input(0, 1, 1, text).unwrap();
    wb.save_as(path).unwrap();
}

fn a1(bytes: &[u8]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("b.xlsx");
    std::fs::write(&p, bytes).unwrap();
    sheets_engine::Workbook::open(&p)
        .unwrap()
        .display_value(0, 1, 1)
}

fn server_a1(r: &Remote, path: &str) -> String {
    a1(&r.client.get(path).unwrap().bytes)
}

/// Upload as the app does: begin, progress, outcome.
fn sync_upload(r: &Remote, d: &mut DocSync, force: bool) -> Action {
    let total = std::fs::metadata(&d.entry.local).unwrap().len();
    d.begin_upload(total);
    let lock = d.lock_token().cloned();
    let mut last = 0;
    let out = session::upload(r, &d.entry, lock.as_ref(), force, &mut |s, _| last = s);
    if matches!(out, UploadOutcome::Synced { .. }) {
        assert_eq!(last, total, "progress reached the end");
    }
    d.on_upload(out)
}

fn assert_unlocked(e: &Env, bob_path: &str) {
    // Bob can take (and drop) the lock: nobody holds it any more.
    let t = e
        .bob
        .client
        .lock(bob_path, Duration::from_secs(60), "bob check")
        .expect("file should be unlocked");
    e.bob.client.unlock(bob_path, &t).unwrap();
}

#[test]
#[ignore]
fn open_edit_save_release() {
    let e = env_or_skip!();
    let path = new_file(&e, "save");
    let opened = session::open(&e.alice, &e.cache, &path).unwrap();
    assert!(
        matches!(opened.lock, LockState::Held(_)),
        "{:?}",
        opened.lock
    );
    let mut d = DocSync::new(opened.entry, opened.lock);
    assert_eq!(d.status(), Status::Synced);
    assert_eq!(a1(&std::fs::read(&d.entry.local).unwrap()), "server v1");

    edit(&d.entry.local, "alice edit 1");
    assert_eq!(d.on_local_save(), Action::Upload);
    assert_eq!(sync_upload(&e.alice, &mut d, false), Action::None);
    assert_eq!(d.status(), Status::Synced);
    assert_eq!(server_a1(&e.alice, &path), "alice edit 1");

    // Save twice more (the saved file is the next save's original).
    edit(&d.entry.local, "alice edit 2");
    assert_eq!(d.on_local_save(), Action::Upload);
    assert_eq!(sync_upload(&e.alice, &mut d, false), Action::None);
    assert_eq!(server_a1(&e.alice, &path), "alice edit 2");

    // While alice holds the lock, another client can't write.
    let other = e
        .alice2
        .client
        .put(&path, b"x", Precondition::Overwrite, None);
    assert!(matches!(other, Err(Error::Locked)), "{other:?}");

    let token = d.lock_token().cloned().unwrap();
    session::release(&e.alice, &e.cache, &path, &token).unwrap();
    assert!(e.cache.held_locks().is_empty());
    assert_unlocked(&e, &path);
}

#[test]
#[ignore]
fn conflict_from_second_client() {
    let e = env_or_skip!();
    let path = new_file(&e, "conflict");
    let opened = session::open(&e.alice, &e.cache, &path).unwrap();
    let mut d = DocSync::new(opened.entry, opened.lock);
    // The lock expired / was lost (e.g. the laptop slept): it no longer
    // protects the file, and another client saves in the meantime.
    let token = d.lock_token().cloned().unwrap();
    session::release(&e.alice, &e.cache, &path, &token).unwrap();
    d.lock = LockState::None;
    e.alice2
        .client
        .put(
            &path,
            &xlsx_with("other client"),
            Precondition::Overwrite,
            None,
        )
        .unwrap();

    edit(&d.entry.local, "mine");
    assert_eq!(d.on_local_save(), Action::Upload);
    assert_eq!(sync_upload(&e.alice, &mut d, false), Action::ShowConflict);
    assert_eq!(d.status(), Status::Conflict);
    assert_eq!(
        server_a1(&e.alice, &path),
        "other client",
        "nothing overwritten"
    );

    // "Save my version as a copy": the copy holds mine, the window theirs.
    let copy =
        kantor_cloud::sync::conflict_copy_name(d.entry.name(), "konflik", "alice", "2026-11-14");
    let copy_path = session::save_copy(&e.alice, &d.entry, &copy).unwrap();
    assert!(
        copy_path.ends_with("(konflik alice 2026-11-14).xlsx"),
        "{copy_path}"
    );
    assert_eq!(server_a1(&e.alice, &copy_path), "mine");
    // A second copy with the same name is numbered, never overwritten.
    let copy2 = session::save_copy(&e.alice, &d.entry, &copy).unwrap();
    assert!(copy2.ends_with(" 2.xlsx"), "{copy2}");
    let fresh = session::reload(&e.alice, &e.cache, &d.entry).unwrap();
    d.on_reloaded(fresh);
    assert_eq!(d.status(), Status::Synced);
    assert_eq!(a1(&std::fs::read(&d.entry.local).unwrap()), "other client");

    // Another conflict, resolved by overwriting theirs.
    e.alice2
        .client
        .put(
            &path,
            &xlsx_with("other client 2"),
            Precondition::Overwrite,
            None,
        )
        .unwrap();
    edit(&d.entry.local, "mine wins");
    assert_eq!(d.on_local_save(), Action::Upload);
    assert_eq!(sync_upload(&e.alice, &mut d, false), Action::ShowConflict);
    assert_eq!(sync_upload(&e.alice, &mut d, true), Action::None);
    assert_eq!(server_a1(&e.alice, &path), "mine wins");

    // A stale If-Match alone is refused too (412), not just the re-check.
    let stale = e.alice2.client.put(
        &path,
        b"stale",
        Precondition::IfMatch("\"definitely-not-the-etag\""),
        None,
    );
    assert!(matches!(stale, Err(Error::Conflict)), "{stale:?}");
}

#[test]
#[ignore]
fn lock_held_by_second_account() {
    let e = env_or_skip!();
    let path = new_file(&e, "lock");
    // Bob (a different account) has it open.
    let bob_open = session::open(&e.bob, &e.cache, &path).unwrap();
    let LockState::Held(bob_token) = bob_open.lock else {
        panic!("bob should hold the lock: {:?}", bob_open.lock);
    };
    // Alice opens it: read-only, with bob's name.
    let opened = session::open(&e.alice, &e.cache, &path).unwrap();
    let LockState::LockedBy(who) = &opened.lock else {
        panic!("expected read-only, got {:?}", opened.lock);
    };
    assert!(who.to_lowercase().contains("bob"), "owner shown as {who:?}");
    let mut d = DocSync::new(opened.entry, opened.lock.clone());
    assert!(matches!(d.status(), Status::ReadOnly(_)));
    edit(&d.entry.local, "alice tries");
    assert_eq!(d.on_local_save(), Action::NotifyReadOnly);
    assert_eq!(d.on_reconnect(), Action::None);
    assert_eq!(server_a1(&e.bob, &path), "server v1");
    // Even a direct upload attempt is refused by the server's lock.
    let out = session::upload(&e.alice, &d.entry, None, false, &mut |_, _| {});
    assert!(matches!(out, UploadOutcome::LockedBy(_)), "{out:?}");

    // Bob closes; alice's "Sync now" takes the lock and can save.
    session::release(&e.bob, &e.cache, &path, &bob_token).unwrap();
    d.lock = session::take_lock(&e.alice, &e.cache, &path);
    assert!(matches!(d.lock, LockState::Held(_)), "{:?}", d.lock);
    assert_eq!(d.on_reconnect(), Action::Upload);
    assert_eq!(sync_upload(&e.alice, &mut d, false), Action::None);
    assert_eq!(server_a1(&e.bob, &path), "alice tries");
    let t = d.lock_token().cloned().unwrap();
    session::release(&e.alice, &e.cache, &path, &t).unwrap();
}

fn toxi(e: &Env, enabled: bool) {
    let (api, proxy) = e.toxi.as_ref().unwrap();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let r = agent
        .post(&format!("{api}/proxies/{proxy}"))
        .send(format!("{{\"enabled\": {enabled}}}"))
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
}

#[test]
#[ignore]
fn offline_then_sync_now() {
    let e = env_or_skip!();
    let Some(ap) = &e.alice_proxy else {
        eprintln!("skipped: no Toxiproxy");
        return;
    };
    if e.toxi.is_none() {
        return;
    }
    let path = new_file(&e, "offline");
    let opened = session::open(ap, &e.cache, &path).unwrap();
    let mut d = DocSync::new(opened.entry, opened.lock);
    toxi(&e, false);
    edit(&d.entry.local, "offline 1");
    assert_eq!(d.on_local_save(), Action::Upload);
    let a = sync_upload(ap, &mut d, false);
    assert_eq!(a, Action::None);
    assert_eq!(d.status(), Status::NotSynced);
    // Keep editing offline: no upload attempts.
    edit(&d.entry.local, "offline 2");
    assert_eq!(d.on_local_save(), Action::None);
    // Reopening while offline uses the working copy.
    let again = session::open(ap, &e.cache, &path);
    assert!(again.is_ok(), "working copy with local changes reopens");
    toxi(&e, true);
    assert_eq!(d.on_reconnect(), Action::Upload);
    assert_eq!(sync_upload(ap, &mut d, false), Action::None);
    assert_eq!(d.status(), Status::Synced);
    assert_eq!(server_a1(&e.alice, &path), "offline 2");
    let t = d.lock_token().cloned().unwrap();
    session::release(ap, &e.cache, &path, &t).unwrap();
    assert_unlocked(&e, &path);
}

#[test]
#[ignore]
fn aborted_save_leaves_no_lock() {
    let e = env_or_skip!();
    let (Some(ap), Some((api, proxy))) = (&e.alice_proxy, &e.toxi) else {
        eprintln!("skipped: no Toxiproxy");
        return;
    };
    let path = new_file(&e, "abort");
    let opened = session::open(ap, &e.cache, &path).unwrap();
    let mut d = DocSync::new(opened.entry, opened.lock);
    // A big save, cut off after 300 KB went up.
    let big = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/corpus/xlsx/10_large_10k.xlsx"
    ))
    .unwrap();
    let mut padded = big.clone();
    padded.resize(2 * 1024 * 1024, 0);
    std::fs::write(&d.entry.local, &padded).unwrap();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    // (A failed earlier run may have left it.)
    let _ = agent
        .delete(&format!("{api}/proxies/{proxy}/toxics/cut"))
        .call();
    let r = agent
        .post(&format!("{api}/proxies/{proxy}/toxics"))
        .send(r#"{"name":"cut","type":"limit_data","stream":"upstream","attributes":{"bytes":300000}}"#)
        .unwrap();
    assert_eq!(r.status().as_u16(), 200, "add toxic");
    assert_eq!(d.on_local_save(), Action::Upload);
    let a = sync_upload(ap, &mut d, false);
    assert_eq!(a, Action::None);
    assert_eq!(d.status(), Status::NotSynced, "aborted: kept for Sync now");
    agent
        .delete(&format!("{api}/proxies/{proxy}/toxics/cut"))
        .call()
        .unwrap();
    // The real file (A1 edited) replaces the padded bytes, then Sync now;
    // the server may still be finishing the aborted upload (423 with our
    // own lock): retried until it goes through.
    std::fs::write(&d.entry.local, &big).unwrap();
    edit(&d.entry.local, "after abort");
    let start = Instant::now();
    let mut action = d.on_reconnect();
    while action != Action::None {
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "stuck: {:?}",
            d.state
        );
        if action == Action::RetryLater {
            std::thread::sleep(Duration::from_secs(2));
        }
        action = sync_upload(ap, &mut d, false);
    }
    assert_eq!(d.status(), Status::Synced);
    assert_eq!(server_a1(&e.alice, &path), "after abort");
    let t = d.lock_token().cloned().unwrap();
    session::release(ap, &e.cache, &path, &t).unwrap();
    assert_unlocked(&e, &path);
}

#[test]
#[ignore]
fn lost_lock_still_saves() {
    let e = env_or_skip!();
    let path = new_file(&e, "lostlock");
    let opened = session::open(&e.alice, &e.cache, &path).unwrap();
    let mut d = DocSync::new(opened.entry, opened.lock);
    // The lock vanishes behind Kantor's back (expired while asleep).
    let token = d.lock_token().cloned().unwrap();
    e.alice.client.unlock(&path, &token).unwrap();
    edit(&d.entry.local, "after lost lock");
    assert_eq!(d.on_local_save(), Action::Upload);
    assert_eq!(sync_upload(&e.alice, &mut d, false), Action::None);
    assert_eq!(d.status(), Status::Synced);
    assert_eq!(d.lock, LockState::None, "lost lock noticed, to be retaken");
    assert_eq!(server_a1(&e.alice, &path), "after lost lock");
    // Same with the user choosing to overwrite after a conflict.
    let token = session::take_lock(&e.alice, &e.cache, &path);
    let LockState::Held(t) = &token else {
        panic!("{token:?}");
    };
    e.alice.client.unlock(&path, t).unwrap();
    d.lock = token;
    e.alice2
        .client
        .put(&path, &xlsx_with("theirs"), Precondition::Overwrite, None)
        .unwrap();
    edit(&d.entry.local, "mine, forced");
    assert_eq!(d.on_local_save(), Action::Upload);
    assert_eq!(sync_upload(&e.alice, &mut d, false), Action::ShowConflict);
    assert_eq!(sync_upload(&e.alice, &mut d, true), Action::None);
    assert_eq!(server_a1(&e.alice, &path), "mine, forced");
    let _ = e.cache.forget_lock("alice", &path);
}

#[test]
#[ignore]
fn crash_cleanup_releases_left_locks() {
    let e = env_or_skip!();
    let path = new_file(&e, "crash");
    // A run that took the lock and then died without releasing it.
    let opened = session::open(&e.alice, &e.cache, &path).unwrap();
    assert!(matches!(opened.lock, LockState::Held(_)));
    assert_eq!(e.cache.held_locks().len(), 1);
    drop(opened);
    // Next start: cleanup releases it.
    assert_eq!(session::cleanup_locks(&e.alice, &e.cache, &[]), 1);
    assert!(e.cache.held_locks().is_empty());
    assert_unlocked(&e, &path);
}
