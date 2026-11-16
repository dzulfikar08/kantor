//! Bearer tokens on WebDAV with a local mock provider and drive: the token
//! is refreshed when it expires, a refused token is refreshed once and the
//! request sent again, a rotated refresh token is stored, and a refresh
//! the provider refuses ends the sign-in.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use kantor_cloud::oidc::{BearerSource, Endpoints, TokenSource, Tokens};
use kantor_cloud::{Auth, Client, Error};

#[derive(Default)]
struct Mock {
    /// Access tokens the drive accepts.
    valid: Mutex<Vec<String>>,
    /// Refresh tokens the provider accepts.
    refresh_ok: Mutex<Vec<String>>,
    refreshes: AtomicUsize,
    dav_calls: AtomicUsize,
}

fn reply(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn serve(mock: Arc<Mock>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in listener.incoming() {
            let Ok(mut s) = s else { continue };
            let mock = mock.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                let (mut auth, mut len) = (String::new(), 0usize);
                loop {
                    let mut l = String::new();
                    r.read_line(&mut l).unwrap();
                    let lower = l.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("authorization:") {
                        auth = l[l.len() - v.len()..].trim().to_owned();
                    }
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                    if l == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                let body = String::from_utf8(body).unwrap();
                let out = if first.starts_with("POST /token") {
                    mock.refreshes.fetch_add(1, Ordering::SeqCst);
                    let rt = body
                        .split('&')
                        .find_map(|p| p.strip_prefix("refresh_token="))
                        .unwrap_or("")
                        .to_owned();
                    if mock.refresh_ok.lock().unwrap().contains(&rt) {
                        let n = mock.refreshes.load(Ordering::SeqCst);
                        let at = format!("at{n}");
                        let next = format!("rt{n}");
                        mock.valid.lock().unwrap().push(at.clone());
                        *mock.refresh_ok.lock().unwrap() = vec![next.clone()];
                        let j = format!(
                            r#"{{"access_token":"{at}","refresh_token":"{next}","expires_in":300}}"#
                        );
                        reply("200 OK", &j)
                    } else {
                        reply("400 Bad Request", r#"{"error":"invalid_grant"}"#)
                    }
                } else {
                    mock.dav_calls.fetch_add(1, Ordering::SeqCst);
                    let token = auth.strip_prefix("Bearer ").unwrap_or("");
                    if mock.valid.lock().unwrap().iter().any(|t| t == token) {
                        reply(
                            "207 Multi-Status",
                            r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response><d:response><d:href>/dav/a.docx</d:href><d:propstat><d:prop><d:resourcetype/></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#,
                        )
                    } else {
                        reply("401 Unauthorized", "")
                    }
                };
                let _ = s.write_all(out.as_bytes());
            });
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
fn tokens_refresh_and_retry_then_sign_in_ends() {
    let mock = Arc::new(Mock::default());
    let origin = serve(mock.clone());
    *mock.refresh_ok.lock().unwrap() = vec!["rt0".into()];
    let ep = Endpoints {
        issuer: origin.clone(),
        authorization_endpoint: format!("{origin}/auth"),
        token_endpoint: format!("{origin}/token"),
        device_authorization_endpoint: None,
        revocation_endpoint: None,
    };
    let stored = Arc::new(Mutex::new(Vec::<String>::new()));
    let keep = stored.clone();
    // A fresh sign-in's access token the drive no longer accepts (revoked
    // session on the server): the first request is refused, the source
    // refreshes and the request goes again.
    let source = Arc::new(
        TokenSource::new(ep, "kantor".into(), "rt0".into(), move |r| {
            keep.lock().unwrap().push(r.to_owned())
        })
        .with_access(&Tokens {
            access_token: "stale".into(),
            expires_in: Some(300),
            ..Tokens::default()
        }),
    );
    let client = Client::new(&format!("{origin}/dav/"), Auth::Source(source.clone()));
    let list = client.list("").unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(mock.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(mock.dav_calls.load(Ordering::SeqCst), 2);
    // The rotated refresh token was stored.
    assert_eq!(*stored.lock().unwrap(), vec!["rt1".to_string()]);
    // The new token is used without refreshing again.
    client.list("").unwrap();
    assert_eq!(mock.refreshes.load(Ordering::SeqCst), 1);

    // The provider revokes the session: the token is refused, the refresh
    // too; the sign-in has ended and requests fail as unauthorised.
    mock.valid.lock().unwrap().clear();
    mock.refresh_ok.lock().unwrap().clear();
    let err = client.list("").unwrap_err();
    assert!(matches!(err, Error::Auth(401)), "{err:?}");
    assert!(source.needs_sign_in());
    // No more refresh attempts once it ended.
    let n = mock.refreshes.load(Ordering::SeqCst);
    assert!(matches!(source.token(), Err(Error::Auth(_))));
    assert_eq!(mock.refreshes.load(Ordering::SeqCst), n);
}
