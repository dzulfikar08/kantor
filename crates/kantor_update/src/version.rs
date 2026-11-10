//! Versions and their order.
//!
//! A build is identified by its semver version plus a build stamp: releases
//! have stamp 0, nightlies the UTC commit time as `YYYYMMDDHHMMSS`. Nightlies
//! of the same version are ordered by stamp, so a nightly built after a
//! release of the same version is newer than it, and a release is never
//! offered to someone running a nightly of that same version.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

/// A semver version (`1.2.3`, `1.2.3-beta.2`); `+build` metadata is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pre: Vec<Ident>,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Ident {
    // Numeric identifiers sort before alphanumeric ones (semver §11.4.3).
    Num(u64),
    Alpha(String),
}

impl FromStr for Version {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || format!("not a version: {s:?}");
        let text = s.trim();
        let core_pre = text.split('+').next().unwrap_or_default();
        let (core, pre) = match core_pre.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (core_pre, None),
        };
        let mut nums = core.split('.').map(|n| {
            if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad());
            }
            n.parse::<u64>().map_err(|_| bad())
        });
        let (Some(major), Some(minor), Some(patch), None) =
            (nums.next(), nums.next(), nums.next(), nums.next())
        else {
            return Err(bad());
        };
        let pre = match pre {
            None => Vec::new(),
            Some(p) => p
                .split('.')
                .map(|id| {
                    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    {
                        Err(bad())
                    } else if id.bytes().all(|b| b.is_ascii_digit()) {
                        id.parse().map(Ident::Num).map_err(|_| bad())
                    } else {
                        Ok(Ident::Alpha(id.to_owned()))
                    }
                })
                .collect::<Result<_, _>>()?,
        };
        Ok(Self {
            major: major?,
            minor: minor?,
            patch: patch?,
            pre,
            text: core_pre.to_owned(),
        })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                // A pre-release sorts before its release.
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// A version plus build stamp (0 for releases); see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BuildId {
    pub version: Version,
    pub build: u64,
}

impl BuildId {
    pub fn new(version: &str, build: u64) -> Result<Self, String> {
        Ok(Self {
            version: version.parse()?,
            build,
        })
    }

    /// `0.2.1`, or `0.2.1 (nightly 2026-11-10 14:30)`.
    pub fn display(&self) -> String {
        let b = self.build.to_string();
        if self.build == 0 || b.len() < 12 {
            return self.version.to_string();
        }
        format!(
            "{} (nightly {}-{}-{} {}:{})",
            self.version,
            &b[0..4],
            &b[4..6],
            &b[6..8],
            &b[8..10],
            &b[10..12]
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    #[test]
    fn parses() {
        assert_eq!(v("0.2.0").to_string(), "0.2.0");
        assert_eq!(v("1.0.0-rc.1+abc").to_string(), "1.0.0-rc.1");
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "a.b.c",
            "1.2.x",
            "1.2.3-",
            "1.2.3-a..b",
            "-1.2.3",
        ] {
            assert!(bad.parse::<Version>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn semver_order() {
        // The example chain from semver.org §11.
        let chain = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "2.0.0",
        ];
        for w in chain.windows(2) {
            assert!(v(w[0]) < v(w[1]), "{} < {}", w[0], w[1]);
        }
        assert!(v("0.10.0") > v("0.9.9"), "numeric, not lexical");
        assert_eq!(v("1.0.0+a").cmp(&v("1.0.0+b")), Ordering::Equal);
    }

    #[test]
    fn nightly_builds() {
        let release = BuildId::new("0.2.0", 0).unwrap();
        let night1 = BuildId::new("0.2.0", 20261110143000).unwrap();
        let night2 = BuildId::new("0.2.0", 20261111090000).unwrap();
        let next = BuildId::new("0.2.1", 0).unwrap();
        let next_pre = BuildId::new("0.2.1-beta.1", 20261101000000).unwrap();
        assert!(
            night1 > release,
            "a nightly after 0.2.0 is newer than 0.2.0"
        );
        assert!(night2 > night1);
        assert!(next > night2, "the next release beats every older nightly");
        assert!(next_pre < next);
        assert!(next_pre > night2);
        assert_eq!(night1.display(), "0.2.0 (nightly 2026-11-10 14:30)");
        assert_eq!(release.display(), "0.2.0");
    }
}
