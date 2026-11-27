//! Data Encryption (MS-OVBA 2.4.3): the obfuscation of the `PROJECT`
//! stream's `CMG=` (protection state), `DPB=` (password) and `GC=`
//! (visibility) values. It isn't security, only a format: a project
//! written from scratch needs the three lines, as the grammar of the
//! `PROJECT` stream requires them (2.3.1.1).

/// `data` encrypted for project `project_id` (its CLSID with the braces,
/// `{…}`), as the hex digits the `PROJECT` stream holds. `seed` picks the
/// key stream and how many filler bytes go first (0–3).
pub fn encrypt(project_id: &str, seed: u8, data: &[u8]) -> String {
    let key = project_key(project_id);
    let version: u8 = 2;
    let version_enc = seed ^ version;
    let key_enc = seed ^ key;
    let mut out = vec![seed, version_enc, key_enc];
    let mut unencrypted1 = key;
    let mut encrypted1 = key_enc;
    let mut encrypted2 = version_enc;
    let mut push = |byte: u8, out: &mut Vec<u8>| {
        let enc = byte ^ encrypted2.wrapping_add(unencrypted1);
        out.push(enc);
        encrypted2 = encrypted1;
        encrypted1 = enc;
        unencrypted1 = byte;
    };
    let ignored = (seed & 6) / 2;
    for i in 0..ignored {
        push(i.wrapping_mul(37).wrapping_add(11), &mut out);
    }
    for b in (data.len() as u32).to_le_bytes() {
        push(b, &mut out);
    }
    for &b in data {
        push(b, &mut out);
    }
    out.iter().map(|b| format!("{b:02X}")).collect()
}

/// The data of an encrypted value: `(version, project key, data)`.
/// `None` when the hex digits don't make one.
pub fn decrypt(hex: &str) -> Option<(u8, u8, Vec<u8>)> {
    let hex = hex.trim().trim_matches('"');
    if hex.len() % 2 != 0 || hex.len() < 6 {
        return None;
    }
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<_>>()?;
    let seed = bytes[0];
    let version = seed ^ bytes[1];
    let key = seed ^ bytes[2];
    let mut unencrypted1 = key;
    let mut encrypted1 = bytes[2];
    let mut encrypted2 = bytes[1];
    let mut at = 3;
    let mut next = |at: &mut usize| -> Option<u8> {
        let enc = *bytes.get(*at)?;
        *at += 1;
        let byte = enc ^ encrypted2.wrapping_add(unencrypted1);
        encrypted2 = encrypted1;
        encrypted1 = enc;
        unencrypted1 = byte;
        Some(byte)
    };
    for _ in 0..(seed & 6) / 2 {
        next(&mut at)?;
    }
    let mut len = [0u8; 4];
    for b in &mut len {
        *b = next(&mut at)?;
    }
    let len = u32::from_le_bytes(len) as usize;
    let mut data = Vec::with_capacity(len.min(64));
    for _ in 0..len {
        data.push(next(&mut at)?);
    }
    Some((version, key, data))
}

/// ProjKey: the byte sum of the project's CLSID text.
pub fn project_key(project_id: &str) -> u8 {
    project_id.bytes().fold(0u8, |k, b| k.wrapping_add(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let id = "{5F3C1A2B-7D4E-4C8B-9A6F-0E1D2C3B4A59}";
        for seed in [0u8, 0x17, 0x6E, 0xFF] {
            for data in [&[0u8, 0, 0, 0][..], &[0][..], &[0xFF][..]] {
                let hex = encrypt(id, seed, data);
                let (version, key, back) = decrypt(&hex).unwrap();
                assert_eq!(version, 2);
                assert_eq!(key, project_key(id));
                assert_eq!(back, data);
            }
        }
        // Lengths the PROJECT grammar allows: CMG 22–28 digits, DPB and GC 16–22.
        let cmg = encrypt(id, 0x06, &[0; 4]);
        assert!((22..=28).contains(&cmg.len()), "{cmg}");
        let gc = encrypt(id, 0x06, &[0xFF]);
        assert!((16..=22).contains(&gc.len()), "{gc}");
    }
}
