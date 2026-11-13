//! Server benchmark through `kantor_cloud` (used for docs/decisions/0002).
//!
//! Same env as the `dav` example (KANTOR_DAV_URL / _USER / _PASS).
//!
//! ```text
//! bench setup <dir> <n>                     # n small files in <dir> (skips if present)
//! bench latency <label> <reps> <dir500> <file>...   # PROPFIND dir500, then GET+PUT each file
//! bench conflict [path]                     # two writers, ETag and LOCK
//! bench load <clients> <secs> <dir500> <big> <small>
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use kantor_cloud::{Auth, Client, Error, Precondition};

fn client() -> Result<Client> {
    let url = std::env::var("KANTOR_DAV_URL").context("set KANTOR_DAV_URL")?;
    Ok(Client::new(
        &url,
        Auth::Basic {
            user: std::env::var("KANTOR_DAV_USER").context("set KANTOR_DAV_USER")?,
            password: std::env::var("KANTOR_DAV_PASS").unwrap_or_default(),
        },
    ))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("setup") => setup(&args[1], args[2].parse()?),
        Some("latency") => latency(&args[1], args[2].parse()?, &args[3], &args[4..]),
        Some("conflict") => conflict(args.get(1).map_or("bench/conflict.txt", String::as_str)),
        Some("ready") => ready(&args[1], args[2].parse()?, &args[3..]),
        Some("load") => load(
            args[1].parse()?,
            args[2].parse()?,
            &args[3],
            &args[4],
            &args[5],
        ),
        _ => bail!("see the source header for usage"),
    }
}

fn setup(dir: &str, n: usize) -> Result<()> {
    let c = Arc::new(client()?);
    c.mkcol("bench")?;
    c.mkcol(dir)?;
    let have = c.list(dir)?.len();
    if have >= n {
        eprintln!("{dir}: {have} files already");
        return Ok(());
    }
    let t = Instant::now();
    let next = Arc::new(Mutex::new(0usize));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let (c, next, dir) = (c.clone(), next.clone(), dir.to_string());
            std::thread::spawn(move || -> Result<()> {
                loop {
                    let i = {
                        let mut g = next.lock().unwrap();
                        *g += 1;
                        *g
                    };
                    if i > n {
                        return Ok(());
                    }
                    let body = format!("Nota dinas nomor {i}\n").repeat(40);
                    c.put(
                        &format!("{dir}/nota-{i:04}.txt"),
                        body.as_bytes(),
                        Precondition::Overwrite,
                        None,
                    )?;
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap()?;
    }
    eprintln!("{dir}: uploaded {n} files in {:?}", t.elapsed());
    Ok(())
}

fn stats(mut ms: Vec<f64>) -> (f64, f64, f64, f64) {
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| {
        let rank = (p * ms.len() as f64).ceil() as usize;
        ms[rank.clamp(1, ms.len()) - 1]
    };
    (pct(0.5), pct(0.95), ms[0], ms[ms.len() - 1])
}

fn row(label: &str, op: &str, ms: Vec<f64>) {
    let n = ms.len();
    let (med, p95, min, max) = stats(ms);
    println!("| {label} | {op} | {n} | {med:.1} | {p95:.1} | {min:.1} | {max:.1} |");
}

fn latency(label: &str, reps: usize, dir500: &str, files: &[String]) -> Result<()> {
    let c = client()?;
    println!("| setup | operation | n | median ms | p95 ms | min ms | max ms |");
    println!("|---|---|---|---|---|---|---|");
    // Warm-up (connection, server caches), not counted.
    let n = c.list(dir500)?.len();
    let mut ms = Vec::new();
    for _ in 0..reps {
        let t = Instant::now();
        let got = c.list(dir500)?.len();
        ms.push(t.elapsed().as_secs_f64() * 1e3);
        assert_eq!(got, n);
    }
    row(label, &format!("PROPFIND depth 1, {n} entries"), ms);
    for f in files {
        // `path[:reps]`: big files get fewer repetitions on slow links.
        let (f, reps) = match f.rsplit_once(':') {
            Some((p, r)) => (p, r.parse()?),
            None => (f.as_str(), reps),
        };
        let bytes = std::fs::read(f)?;
        let name = std::path::Path::new(f)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let remote = format!("bench/{name}");
        // Reuse what an earlier run uploaded (spares a slow link the upload).
        let mut etag = match c.stat(&remote) {
            Ok(e) if e.size == Some(bytes.len() as u64) => e.etag,
            _ => None,
        };
        if etag.is_none() {
            etag = c.put(&remote, &bytes, Precondition::Overwrite, None)?;
        }
        if etag.is_none() {
            etag = c.stat(&remote)?.etag;
        }
        // A save: PUT If-Match the ETag we have, keep the new one.
        // Failures (a lossy link can drop a transfer) are counted, not fatal.
        let (mut put_ms, mut put_err) = (Vec::new(), 0);
        for _ in 0..reps {
            let t = Instant::now();
            let e = etag.clone().context("server sent no ETag")?;
            match c.put(&remote, &bytes, Precondition::IfMatch(&e), None) {
                Ok(new) => {
                    put_ms.push(t.elapsed().as_secs_f64() * 1e3);
                    etag = new;
                }
                Err(err) => {
                    eprintln!("PUT {remote} failed after {:?}: {err}", t.elapsed());
                    put_err += 1;
                    etag = None;
                }
            }
            if etag.is_none() {
                etag = c.stat(&remote)?.etag;
            }
        }
        let (mut get_ms, mut get_err) = (Vec::new(), 0);
        for _ in 0..reps {
            let t = Instant::now();
            match c.get(&remote) {
                Ok(got) => {
                    get_ms.push(t.elapsed().as_secs_f64() * 1e3);
                    assert_eq!(got.bytes.len(), bytes.len());
                }
                Err(err) => {
                    eprintln!("GET {remote} failed after {:?}: {err}", t.elapsed());
                    get_err += 1;
                }
            }
        }
        let kb = bytes.len() as f64 / 1024.0;
        let size = if kb > 1024.0 {
            format!("{:.1} MB", kb / 1024.0)
        } else {
            format!("{kb:.0} KB")
        };
        let failed = |n: usize| {
            if n > 0 {
                format!(" ({n} failed)")
            } else {
                String::new()
            }
        };
        if !get_ms.is_empty() {
            row(label, &format!("GET {size}{}", failed(get_err)), get_ms);
        }
        if !put_ms.is_empty() {
            row(
                label,
                &format!("PUT If-Match {size}{}", failed(put_err)),
                put_ms,
            );
        }
    }
    Ok(())
}

/// Save then reopen at once: how long after a PUT returns until a GET is
/// served (OpenCloud answers 425 while post-processing an upload).
fn ready(label: &str, reps: usize, files: &[String]) -> Result<()> {
    let c = client()?;
    for f in files {
        let bytes = std::fs::read(f)?;
        let name = std::path::Path::new(f)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let remote = format!("bench/ready-{name}");
        let (mut waits, mut early) = (Vec::new(), 0usize);
        for _ in 0..reps {
            c.put(&remote, &bytes, Precondition::Overwrite, None)?;
            let t = Instant::now();
            loop {
                match c.try_get(&remote) {
                    Err(Error::TooEarly) => {
                        early += 1;
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    r => {
                        r?;
                        break;
                    }
                }
            }
            waits.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let mb = bytes.len() as f64 / 1048576.0;
        row(
            label,
            &format!("PUT -> first successful GET ({mb:.2} MB), {early} x 425"),
            waits,
        );
    }
    Ok(())
}

fn conflict(path: &str) -> Result<()> {
    let (alice, bob) = (client()?, client()?);
    alice.mkcol("bench")?;
    alice.put(path, b"v0", Precondition::Overwrite, None)?;
    let a = alice.get(path)?;
    let b = bob.get(path)?;
    println!("both opened, etags {:?} / {:?}", a.etag, b.etag);
    let a_new = alice.put(
        path,
        b"alice v1",
        Precondition::IfMatch(a.etag.as_deref().unwrap()),
        None,
    );
    println!(
        "alice saves with If-Match: {:?}",
        a_new.as_ref().map_err(|e| e.to_string())
    );
    let b_res = bob.put(
        path,
        b"bob v1",
        Precondition::IfMatch(b.etag.as_deref().unwrap()),
        None,
    );
    println!(
        "bob saves with stale If-Match: {:?}",
        b_res.as_ref().map_err(|e| e.to_string())
    );
    println!(
        "server now has: {:?}",
        String::from_utf8_lossy(&alice.get(path)?.bytes)
    );
    let new = bob.put(path, b"x", Precondition::IfNoneMatch, None);
    println!(
        "bob If-None-Match: * on an existing file: {:?}",
        new.map_err(|e| e.to_string())
    );

    // Locks.
    match alice.lock(path, Duration::from_secs(600)) {
        Ok(tok) => {
            println!("alice LOCK: ok ({})", tok.0);
            let e = bob.stat(path)?.etag.unwrap_or_default();
            let r = bob.put(path, b"bob while locked", Precondition::IfMatch(&e), None);
            println!(
                "bob PUT while alice holds the lock: {:?}",
                r.map_err(|e| e.to_string())
            );
            let r2 = bob.lock(path, Duration::from_secs(600));
            println!(
                "bob LOCK while alice holds it: {:?}",
                r2.map(|t| t.0).map_err(|e| e.to_string())
            );
            let e = alice.stat(path)?.etag.unwrap_or_default();
            let r = alice.put(
                path,
                b"alice v2 under lock",
                Precondition::IfMatch(&e),
                Some(&tok),
            );
            println!(
                "alice PUT with her lock token: {:?}",
                r.map_err(|e| e.to_string())
            );
            println!(
                "alice UNLOCK: {:?}",
                alice.unlock(path, &tok).map_err(|e| e.to_string())
            );
            println!(
                "server now has: {:?}",
                String::from_utf8_lossy(&alice.get(path)?.bytes)
            );
        }
        Err(e) => println!("alice LOCK: {e}"),
    }
    Ok(())
}

fn load(clients: usize, secs: u64, dir500: &str, big: &str, small: &str) -> Result<()> {
    let small_bytes = Arc::new(std::fs::read(small)?);
    let big_name = std::path::Path::new(big)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let big_remote = format!("bench/{big_name}");
    client()?.put(
        &big_remote,
        &std::fs::read(big)?,
        Precondition::Overwrite,
        None,
    )?;
    let stop = Arc::new(AtomicBool::new(false));
    let results: Arc<Mutex<[Vec<f64>; 3]>> = Arc::default();
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let start = Instant::now();
    let threads: Vec<_> = (0..clients)
        .map(|i| {
            let (stop, results, errors, small_bytes) = (
                stop.clone(),
                results.clone(),
                errors.clone(),
                small_bytes.clone(),
            );
            let (dir500, big_remote) = (dir500.to_string(), big_remote.clone());
            std::thread::spawn(move || {
                let c = client().unwrap();
                let mine = format!("bench/load-{i}.xlsx");
                let mut etag = c
                    .put(&mine, &small_bytes, Precondition::Overwrite, None)
                    .ok()
                    .flatten();
                while !stop.load(Ordering::Relaxed) {
                    let timed = |k: usize, f: &mut dyn FnMut() -> Result<(), Error>| {
                        let t = Instant::now();
                        match f() {
                            Ok(()) => {
                                results.lock().unwrap()[k].push(t.elapsed().as_secs_f64() * 1e3)
                            }
                            Err(e) => errors.lock().unwrap().push(e.to_string()),
                        }
                    };
                    timed(0, &mut || c.list(&dir500).map(|_| ()));
                    timed(1, &mut || c.get(&big_remote).map(|_| ()));
                    timed(2, &mut || {
                        let cond = match &etag {
                            Some(e) => Precondition::IfMatch(e),
                            None => Precondition::Overwrite,
                        };
                        let e = c.put(&mine, &small_bytes, cond, None)?;
                        etag = match e {
                            Some(e) => Some(e),
                            None => c.stat(&mine)?.etag,
                        };
                        Ok(())
                    });
                }
            })
        })
        .collect();
    std::thread::sleep(Duration::from_secs(secs));
    stop.store(true, Ordering::Relaxed);
    for t in threads {
        t.join().unwrap();
    }
    let wall = start.elapsed().as_secs_f64();
    let r = results.lock().unwrap();
    let errs = errors.lock().unwrap();
    let total: usize = r.iter().map(Vec::len).sum();
    println!(
        "{clients} clients, {wall:.1} s, {total} requests ({:.1}/s), {} errors",
        total as f64 / wall,
        errs.len()
    );
    if let Some(e) = errs.first() {
        println!("first error: {e}");
    }
    println!("| operation | n | req/s | median ms | p95 ms | max ms |");
    println!("|---|---|---|---|---|---|");
    for (k, op) in ["PROPFIND 500", "GET 2 MB", "PUT If-Match 50 KB"]
        .iter()
        .enumerate()
    {
        if r[k].is_empty() {
            continue;
        }
        let n = r[k].len();
        let (med, p95, _, max) = stats(r[k].clone());
        println!(
            "| {op} | {n} | {:.1} | {med:.1} | {p95:.1} | {max:.1} |",
            n as f64 / wall
        );
    }
    Ok(())
}
