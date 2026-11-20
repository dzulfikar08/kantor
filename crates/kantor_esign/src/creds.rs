//! Provider credentials: in the OS keychain ([`kantor_ai::keys::ESIGN`]),
//! never in settings.toml, never logged. One keychain entry per provider
//! holds its fields as a JSON object.

use std::collections::BTreeMap;
use std::fmt;

use kantor_ai::keys::{ESIGN, KeyLocation};

/// The values of a provider's [`crate::Field`]s, by key.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Credentials(pub BTreeMap<String, String>);

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Keys only: values are secrets.
        f.debug_list().entries(self.0.keys()).finish()
    }
}

impl Credentials {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.0.insert(key.to_string(), value.to_string());
    }

    pub fn is_empty(&self) -> bool {
        self.0.values().all(|v| v.trim().is_empty())
    }

    /// From a JSON object (`{"client_id": "…"}`) or `key=value` lines.
    pub fn parse(text: &str) -> Result<Credentials, String> {
        let t = text.trim();
        if t.starts_with('{') {
            let v: serde_json::Value =
                serde_json::from_str(t).map_err(|e| format!("not valid JSON: {e}"))?;
            let obj = v.as_object().ok_or("expected a JSON object")?;
            return Ok(Credentials(
                obj.iter()
                    .filter_map(|(k, v)| {
                        let s = match v {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Number(n) => n.to_string(),
                            serde_json::Value::Bool(b) => b.to_string(),
                            _ => return None,
                        };
                        Some((k.clone(), s))
                    })
                    .collect(),
            ));
        }
        let mut m = BTreeMap::new();
        for line in t.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| format!("expected key=value, got \"{line}\""))?;
            m.insert(k.trim().to_string(), v.trim().to_string());
        }
        Ok(Credentials(m))
    }

    fn to_json(&self) -> String {
        serde_json::to_string(&self.0).unwrap_or_default()
    }

    /// Check the required fields of `fields` are there.
    pub fn missing<'a>(&self, fields: &'a [crate::Field]) -> Vec<&'a crate::Field> {
        fields
            .iter()
            .filter(|f| !f.optional && self.get(f.key).is_none())
            .collect()
    }
}

/// The saved credentials of provider `id` (empty when none). Can block on
/// the keychain: call off the UI thread.
pub fn load(id: &str) -> Credentials {
    ESIGN
        .load(id)
        .and_then(|s| Credentials::parse(&s).ok())
        .unwrap_or_default()
}

/// Save (or, when empty, delete) provider `id`'s credentials.
pub fn save(id: &str, creds: &Credentials) -> Result<KeyLocation, String> {
    if creds.is_empty() {
        ESIGN.delete(id);
        return Ok(KeyLocation::Keychain);
    }
    ESIGN.save(id, &creds.to_json())
}

pub fn delete(id: &str) {
    ESIGN.delete(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_and_lines_and_hides_values() {
        let a =
            Credentials::parse(r#"{"client_id":"abc","client_secret":"s3cret","n":5}"#).unwrap();
        assert_eq!(a.get("client_id"), Some("abc"));
        assert_eq!(a.get("n"), Some("5"));
        let b = Credentials::parse("# comment\nclient_id = abc\nclient_secret=s3cret\n").unwrap();
        assert_eq!(b.get("client_secret"), Some("s3cret"));
        assert!(!format!("{b:?}").contains("s3cret"));
        assert!(Credentials::parse("nonsense").is_err());
        let back = Credentials::parse(&a.to_json()).unwrap();
        assert_eq!(back, a);
    }
}
