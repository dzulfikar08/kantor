//! GETs over the same ureq + rustls stack as `kantor_ai`. Redirects are
//! followed (the site redirects `/update/*.json` to GitHub release assets,
//! which redirect again to their storage).

use std::io::{Read, Write};
use std::time::Duration;

use crate::Error;

pub struct Http {
    agent: ureq::Agent,
}

impl Default for Http {
    fn default() -> Self {
        Self::new()
    }
}

/// `Kantor/0.2.0 (linux; x86_64)`: the version and OS are all a check sends.
pub fn user_agent() -> String {
    format!(
        "Kantor/{} ({}; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

impl Http {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(10)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_per_read(Some(Duration::from_secs(60)))
            .user_agent(user_agent())
            .build();
        Self {
            agent: config.into(),
        }
    }

    fn open(&self, url: &str) -> Result<ureq::Body, Error> {
        let resp = self
            .agent
            .get(url)
            .call()
            .map_err(|e| Error::Network(format!("{}: {e}", host(url))))?;
        let status = resp.status().as_u16();
        if status != 200 {
            return Err(Error::Network(format!("{}: HTTP {status}", host(url))));
        }
        Ok(resp.into_body())
    }

    /// The whole body, refusing more than `max` bytes.
    pub fn get(&self, url: &str, max: u64) -> Result<Vec<u8>, Error> {
        let mut body = Vec::new();
        self.open(url)?
            .into_reader()
            .take(max + 1)
            .read_to_end(&mut body)
            .map_err(|e| Error::Network(format!("{}: {e}", host(url))))?;
        if body.len() as u64 > max {
            return Err(Error::Network(format!("{}: response too large", host(url))));
        }
        Ok(body)
    }

    /// Stream `url` into `out`, stopping past `max` bytes; `progress` gets
    /// the running byte count. Returns the bytes written.
    pub fn download(
        &self,
        url: &str,
        out: &mut dyn Write,
        max: u64,
        progress: &mut dyn FnMut(u64),
    ) -> Result<u64, Error> {
        let mut reader = self.open(url)?.into_reader();
        let mut buf = vec![0u8; 64 * 1024];
        let mut total = 0u64;
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| Error::Network(format!("{}: {e}", host(url))))?;
            if n == 0 {
                return Ok(total);
            }
            total += n as u64;
            if total > max {
                return Err(Error::Checksum(format!(
                    "{} is larger than the manifest says",
                    host(url)
                )));
            }
            out.write_all(&buf[..n])?;
            progress(total);
        }
    }
}

/// `https://host/…` → `host`, for messages.
fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?']).next().unwrap_or(rest)
}

#[cfg(test)]
mod tests {
    #[test]
    fn host_and_agent() {
        assert_eq!(super::host("https://a.example:8443/x?y"), "a.example:8443");
        let ua = super::user_agent();
        assert!(
            ua.starts_with("Kantor/") && ua.contains(std::env::consts::OS),
            "{ua}"
        );
    }
}
