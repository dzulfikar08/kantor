//! End-to-end tests against a real WebDAV server (Nextcloud, OpenCloud),
//! through the same `session` + `sync` code the app runs. Ignored by
//! default; `crates/kantor_cloud/bench/e2e.sh` starts the servers and runs
//! them with:
//!
//! ```text
//! KANTOR_E2E_ALICE_URL / _ALICE_PASS   alice's WebDAV root and app password
//! KANTOR_E2E_BOB_URL   / _BOB_PASS     a second account
//! KANTOR_E2E_SHARED                    folder alice shared with bob
//! KANTOR_E2E_BOB_SHARED                where bob sees it (default: the same path;
//!                                      OpenCloud mounts shares under `Shares/`)
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
    bob_shared: String,
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
        bob_shared: var("KANTOR_E2E_BOB_SHARED")
            .or_else(|| var("KANTOR_E2E_SHARED"))
            .unwrap_or_else(|| "Bersama".into()),
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

/// `path` (under alice's shared folder) as bob sees it.
fn bob_path(e: &Env, path: &str) -> String {
    path.replacen(&e.shared, &e.bob_shared, 1)
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
    let cache = Cache::new(d.entry.local.parent().unwrap().join(".e2e-index"));
    let out = session::upload(r, &cache, &d.entry, lock.as_ref(), force, &mut |s, _| {
        last = s
    });
    if matches!(out, UploadOutcome::Synced { .. }) {
        assert_eq!(last, total, "progress reached the end");
    }
    d.on_upload(out)
}

fn assert_unlocked(e: &Env, path: &str) {
    // Nobody holds a lock on the file any more (asked as both accounts).
    assert_eq!(
        e.alice.client.lock_holder(path).unwrap(),
        None,
        "{path} still locked"
    );
    assert_eq!(e.bob.client.lock_holder(&bob_path(e, path)).unwrap(), None);
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
    let bp = bob_path(&e, &path);
    // Bob (a different account) has it open. OpenCloud doesn't let a share
    // recipient lock (Unimplemented): then alice holds it and bob is the
    // one who finds it locked.
    let bob_open = session::open(&e.bob, &e.cache, &bp).unwrap();
    let bob_locks = matches!(bob_open.lock, LockState::Held(_));
    let (holder, holder_path, other, other_path, name_var) = if bob_locks {
        (
            &e.bob,
            bp.clone(),
            &e.alice,
            path.clone(),
            "KANTOR_E2E_BOB_NAME",
        )
    } else {
        eprintln!("bob can't lock here ({:?}); swapping roles", bob_open.lock);
        (
            &e.alice,
            path.clone(),
            &e.bob,
            bp.clone(),
            "KANTOR_E2E_ALICE_NAME",
        )
    };
    let held = if bob_locks {
        bob_open
    } else {
        session::open(holder, &e.cache, &holder_path).unwrap()
    };
    let LockState::Held(token) = held.lock else {
        panic!("the holder should hold the lock: {:?}", held.lock);
    };
    // The other account opens it: read-only, with the holder's name.
    let opened = session::open(other, &e.cache, &other_path).unwrap();
    let LockState::LockedBy(who) = &opened.lock else {
        panic!("expected read-only, got {:?}", opened.lock);
    };
    if let Some(name) = var(name_var) {
        assert!(
            who.to_lowercase().contains(&name.to_lowercase()),
            "owner shown as {who:?}, expected {name:?}"
        );
    }
    let mut d = DocSync::new(opened.entry, opened.lock.clone());
    assert!(matches!(d.status(), Status::ReadOnly(_)));
    edit(&d.entry.local, "other tries");
    assert_eq!(d.on_local_save(), Action::NotifyReadOnly);
    assert_eq!(d.on_reconnect(), Action::None);
    assert_eq!(server_a1(holder, &holder_path), "server v1");
    // Even a direct upload attempt is refused by the server's lock.
    let out = session::upload(other, &e.cache, &d.entry, None, false, &mut |_, _| {});
    assert!(
        matches!(out, UploadOutcome::LockedBy(_) | UploadOutcome::Failed(_)),
        "{out:?}"
    );
    assert_eq!(server_a1(holder, &holder_path), "server v1");

    // The holder closes; "Edit now" takes the lock (where the server lets
    // this account lock) and the save goes through.
    session::release(holder, &e.cache, &holder_path, &token).unwrap();
    d.lock = session::take_lock(other, &e.cache, &other_path);
    assert!(
        matches!(d.lock, LockState::Held(_) | LockState::Unsupported(_)),
        "{:?}",
        d.lock
    );
    assert_eq!(d.on_reconnect(), Action::Upload);
    assert_eq!(sync_upload(other, &mut d, false), Action::None);
    assert_eq!(server_a1(holder, &holder_path), "other tries");
    if let Some(t) = d.lock_token().cloned() {
        session::release(other, &e.cache, &other_path, &t).unwrap();
    }
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

/// 12 MiB that compress badly (a big file's working copy).
fn big_bytes(seed: u8) -> Vec<u8> {
    let mut x: u32 = 0x9e37_79b9 ^ u32::from(seed);
    (0..12 * 1024 * 1024)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

#[test]
#[ignore]
fn big_upload_resumes_after_a_drop() {
    let e = env_or_skip!();
    let (Some(ap), Some(_)) = (&e.alice_proxy, &e.toxi) else {
        eprintln!("skipped: no Toxiproxy");
        return;
    };
    let path = new_file(&e, "big");
    let opened = session::open(ap, &e.cache, &path).unwrap();
    let mut d = DocSync::new(opened.entry, opened.lock);
    let big = big_bytes(1);
    std::fs::write(&d.entry.local, &big).unwrap();
    let kind = ap.client.resumable(d.entry.folder());
    eprintln!("resumable: {kind:?}");
    assert_ne!(
        kind,
        kantor_cloud::chunked::Resumable::None,
        "server offers resumable uploads"
    );
    assert_eq!(d.on_local_save(), Action::Upload);
    let total = big.len() as u64;
    d.begin_upload(total);
    let lock = d.lock_token().cloned();
    // The link goes down once the first chunk (Nextcloud: 5 MiB; TUS:
    // 2 MiB parts) is on the server.
    let mut cut = false;
    let out = session::upload(ap, &e.cache, &d.entry, lock.as_ref(), false, &mut |s, _| {
        if !cut && s >= 6_500_000 {
            cut = true;
            toxi(&e, false);
        }
    });
    toxi(&e, true);
    assert!(cut, "the upload got past the first chunk");
    assert!(matches!(out, UploadOutcome::Offline(_)), "{out:?}");
    d.on_upload(out);
    assert!(
        e.cache.pending_upload(&d.entry.local).is_some(),
        "resume point kept"
    );
    // Sync now: continues where it stopped.
    assert_eq!(d.on_reconnect(), Action::Upload);
    d.begin_upload(total);
    let mut first = None;
    let lock = d.lock_token().cloned();
    let out = session::upload(ap, &e.cache, &d.entry, lock.as_ref(), false, &mut |s, _| {
        first.get_or_insert(s);
    });
    let first = first.unwrap_or(0);
    eprintln!("resumed at {first} of {total}");
    assert!(
        first >= 2 * 1024 * 1024,
        "resumed, not restarted (first progress {first})"
    );
    assert!(matches!(out, UploadOutcome::Synced { .. }), "{out:?}");
    d.on_upload(out);
    assert!(e.cache.pending_upload(&d.entry.local).is_none());
    assert!(
        e.alice.client.get(&path).unwrap().bytes == big,
        "server has the whole file"
    );
    // A later save of different bytes, through the version check.
    let big2 = big_bytes(2);
    std::fs::write(&d.entry.local, &big2).unwrap();
    assert_eq!(d.on_local_save(), Action::Upload);
    assert_eq!(sync_upload(ap, &mut d, false), Action::None);
    assert_eq!(d.status(), Status::Synced, "{:?}", d.state);
    let got = e.alice.client.get(&path).unwrap().bytes;
    let sha = kantor_cloud::cache::sha256_hex;
    assert!(
        got == big2,
        "server has {} bytes {} (big {}, big2 {})",
        got.len(),
        &sha(&got)[..12],
        &sha(&big)[..12],
        &sha(&big2)[..12]
    );
    // (TUS releases the lock for the upload; the app takes it again.)
    if let Some(t) = d.lock_token().cloned() {
        session::release(ap, &e.cache, &path, &t).unwrap();
    }
}

/// Add (`true`) or remove a toxic that swallows all data both ways
/// without closing the connections: a link that died silently (Wi-Fi
/// gone), unlike a disabled proxy, which refuses connections at once.
fn blackhole(e: &Env, on: bool) {
    let (api, proxy) = e.toxi.as_ref().unwrap();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    for stream in ["upstream", "downstream"] {
        let name = format!("blackhole_{stream}");
        if on {
            let body = format!(
                r#"{{"name":"{name}","type":"timeout","stream":"{stream}","attributes":{{"timeout":0}}}}"#
            );
            let r = agent
                .post(&format!("{api}/proxies/{proxy}/toxics"))
                .send(body)
                .unwrap();
            assert!(r.status().is_success(), "{}", r.status());
        } else {
            let _ = agent
                .delete(&format!("{api}/proxies/{proxy}/toxics/{name}"))
                .call();
        }
    }
}

#[test]
#[ignore]
fn dead_link_fails_fast() {
    let e = env_or_skip!();
    let (Some(ap), Some(_)) = (&e.alice_proxy, &e.toxi) else {
        eprintln!("skipped: no Toxiproxy");
        return;
    };
    let path = new_file(&e, "dead");
    let opened = session::open(ap, &e.cache, &path).unwrap();
    let mut d = DocSync::new(opened.entry, opened.lock);
    // 3 MB: one plain PUT, small enough to sit in socket buffers, so the
    // reply wait (not the sending) has to notice the dead link.
    let bytes: Vec<u8> = big_bytes(3)[..3 * 1024 * 1024].to_vec();
    std::fs::write(&d.entry.local, &bytes).unwrap();
    assert_eq!(d.on_local_save(), Action::Upload);
    blackhole(&e, true);
    let start = Instant::now();
    let a = sync_upload(ap, &mut d, false);
    let took = start.elapsed();
    blackhole(&e, false);
    eprintln!("dead link noticed after {took:?}");
    assert_eq!(a, Action::None);
    assert_eq!(d.status(), Status::NotSynced, "{:?}", d.state);
    assert!(took < Duration::from_secs(60), "took {took:?}");
    // The link is back: the next try goes through.
    assert_eq!(d.on_reconnect(), Action::Upload);
    let a = sync_upload(ap, &mut d, false);
    assert_eq!(a, Action::None);
    assert_eq!(d.status(), Status::Synced, "{:?}", d.state);
    assert!(e.alice.client.get(&path).unwrap().bytes == bytes);
    if let Some(t) = d.lock_token().cloned() {
        session::release(ap, &e.cache, &path, &t).unwrap();
    }
}

#[test]
#[ignore]
fn restart_check_needs_no_download() {
    let e = env_or_skip!();
    let path = new_file(&e, "restart");
    let opened = session::open(&e.alice, &e.cache, &path).unwrap();
    let mut d = DocSync::new(opened.entry, opened.lock);
    edit(&d.entry.local, "before restart");
    assert_eq!(d.on_local_save(), Action::Upload);
    assert_eq!(sync_upload(&e.alice, &mut d, false), Action::None);
    // What the app does after an upload: keep the entry.
    e.cache.save_entry(&d.entry).unwrap();
    let token = d.lock_token().cloned().unwrap();
    session::release(&e.alice, &e.cache, &path, &token).unwrap();

    // Next start. A wrong SHA-256 makes any download-and-hash check fail,
    // so passing proves no download was needed.
    let mut entry = e.cache.find_remote("alice", &path).unwrap();
    entry.sha256 = Some("0".repeat(64));
    let server = e.alice.client.stat(&path).unwrap();
    assert_eq!(
        entry.etag, server.etag,
        "the ETag after our UNLOCK was recorded"
    );
    let (lock, changed) = session::take_lock_settled(&e.alice, &e.cache, &mut entry);
    assert!(matches!(lock, LockState::Held(_)), "{lock:?}");
    assert_eq!(changed, Some(false));

    // Without the recorded ETag (an old index, a crash), the server's SHA1
    // settles it, still without a download.
    let server = e.alice.client.stat(&path).unwrap();
    assert!(server.sha1().is_some(), "server keeps a SHA1: {server:?}");
    entry.etag = Some("\"stale\"".into());
    assert!(session::is_base_version(&e.alice, &entry, &server).unwrap());
    if let LockState::Held(t) = lock {
        session::release(&e.alice, &e.cache, &path, &t).unwrap();
    }
}

#[test]
#[ignore]
fn big_upload_conflict_is_refused() {
    let e = env_or_skip!();
    let path = new_file(&e, "bigconflict");
    let opened = session::open(&e.alice, &e.cache, &path).unwrap();
    let mut d = DocSync::new(opened.entry, opened.lock);
    let t = d.lock_token().cloned().unwrap();
    session::release(&e.alice, &e.cache, &path, &t).unwrap();
    d.lock = LockState::None;
    e.alice2
        .client
        .put(&path, &xlsx_with("theirs"), Precondition::Overwrite, None)
        .unwrap();
    std::fs::write(&d.entry.local, big_bytes(3)).unwrap();
    assert_eq!(d.on_local_save(), Action::Upload);
    assert_eq!(sync_upload(&e.alice, &mut d, false), Action::ShowConflict);
    assert_eq!(
        server_a1(&e.alice, &path),
        "theirs",
        "nothing assembled over theirs"
    );
    // The assemble step itself checks the ETag too (Nextcloud).
    if let kantor_cloud::chunked::Resumable::Nextcloud { user } =
        e.alice.client.resumable(&e.shared)
    {
        let r = e.alice.client.upload_nextcloud(
            &user,
            "kantor-e2e-stale",
            &path,
            &big_bytes(4),
            Some("\"stale-etag\""),
            None,
            &mut |_, _| {},
        );
        assert!(matches!(r, Err(Error::Conflict)), "{r:?}");
        assert_eq!(server_a1(&e.alice, &path), "theirs");
    }
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
