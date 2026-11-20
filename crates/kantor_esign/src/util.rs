//! Small things the providers share.

use base64::Engine as _;

pub fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Decode base64, tolerating line breaks, URL-safe letters and missing
/// padding.
pub fn unb64(s: &str) -> Option<Vec<u8>> {
    let clean: String = s
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            c => c,
        })
        .collect();
    let trimmed = clean.trim_end_matches('=');
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(trimmed)
        .ok()
}

/// A random UUID (version 4), as transaction references.
pub fn uuid4() -> String {
    let mut b = [0u8; 16];
    let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

pub fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

/// An HTTP date (RFC 7231 IMF-fixdate), e.g. `Thu, 20 Nov 2026 07:30:12 GMT`.
pub fn http_date(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (y, m, d) = crate::emeterai::civil_pub(days);
    // 1970-01-01 was a Thursday.
    let wd = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][days.rem_euclid(7) as usize];
    let mon = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(m - 1) as usize];
    format!(
        "{wd}, {d:02} {mon} {y:04} {:02}:{:02}:{:02} GMT",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// Certificates from PEM text that may have lost its line breaks
/// (`-----BEGIN CERTIFICATE-----MIIH…-----END CERTIFICATE-----`).
pub fn pem_certs(text: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("-----BEGIN CERTIFICATE-----") {
        rest = &rest[i + "-----BEGIN CERTIFICATE-----".len()..];
        let Some(j) = rest.find("-----END CERTIFICATE-----") else {
            break;
        };
        let body = rest[..j].replace("\\n", "");
        if let Some(der) = unb64(&body) {
            out.push(der);
        }
        rest = &rest[j..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_dates_and_uuids() {
        assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(http_date(1_795_163_412), "Fri, 20 Nov 2026 08:30:12 GMT");
        let u = uuid4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert_eq!(unb64("aGk"), Some(b"hi".to_vec()));
        assert_eq!(unb64(&b64(b"hello")), Some(b"hello".to_vec()));
    }
}
