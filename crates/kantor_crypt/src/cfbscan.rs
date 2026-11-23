//! A cheap look into a compound file's directory: whether it holds the
//! `EncryptionInfo` and `EncryptedPackage` streams of an encrypted OOXML
//! package. Reads the header, the directory sectors and the few FAT (and
//! DIFAT) sectors that chain them, never the streams, so it is quick on a
//! 100 MB `.xls` too.

use std::io::{Read, Seek, SeekFrom};

pub const MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

const END: u32 = 0xFFFF_FFFE;
const MAX_SECT: u32 = 0xFFFF_FFFA;
/// Directory sectors looked at (Office's encrypted files have one or two;
/// a directory this long isn't an encrypted package).
const MAX_DIR_SECTORS: usize = 256;

struct Cfb<'r, R> {
    r: &'r mut R,
    shift: u32,
    difat: Vec<u32>,
    first_difat: u32,
    num_fat: u32,
}

impl<R: Read + Seek> Cfb<'_, R> {
    fn size(&self) -> u64 {
        1 << self.shift
    }

    fn read_at(&mut self, pos: u64, buf: &mut [u8]) -> Option<()> {
        self.r.seek(SeekFrom::Start(pos)).ok()?;
        self.r.read_exact(buf).ok()
    }

    fn sector(&mut self, n: u32, buf: &mut [u8]) -> Option<()> {
        if n > MAX_SECT {
            return None;
        }
        let pos = (u64::from(n) + 1).checked_mul(self.size())?;
        self.read_at(pos, buf)
    }

    /// Where FAT sector `k` is (the header's DIFAT, then the DIFAT chain).
    fn fat_sector(&mut self, k: u32) -> Option<u32> {
        if k >= self.num_fat {
            return None;
        }
        if let Some(&s) = self.difat.get(k as usize) {
            return Some(s);
        }
        let per = (self.size() / 4 - 1) as u32;
        let mut idx = k - 109;
        let mut sect = self.first_difat;
        let mut buf = vec![0u8; self.size() as usize];
        for _ in 0..1024 {
            self.sector(sect, &mut buf)?;
            if idx < per {
                let o = idx as usize * 4;
                return Some(u32::from_le_bytes(buf[o..o + 4].try_into().ok()?));
            }
            idx -= per;
            let o = per as usize * 4;
            sect = u32::from_le_bytes(buf[o..o + 4].try_into().ok()?);
        }
        None
    }

    fn next(&mut self, sect: u32) -> Option<u32> {
        let per = (self.size() / 4) as u32;
        let fs = self.fat_sector(sect / per)?;
        let mut b = [0u8; 4];
        let pos = (u64::from(fs) + 1).checked_mul(self.size())? + u64::from(sect % per) * 4;
        self.read_at(pos, &mut b)?;
        Some(u32::from_le_bytes(b))
    }
}

/// Whether the compound file in `r` (positioned anywhere) has both streams.
pub fn has_encrypted_package<R: Read + Seek>(r: &mut R) -> bool {
    scan(r).unwrap_or(false)
}

fn scan<R: Read + Seek>(r: &mut R) -> Option<bool> {
    let mut h = [0u8; 512];
    r.seek(SeekFrom::Start(0)).ok()?;
    r.read_exact(&mut h).ok()?;
    if h[..8] != MAGIC {
        return None;
    }
    let u32_at = |o: usize| u32::from_le_bytes(h[o..o + 4].try_into().unwrap());
    let shift = u32::from(u16::from_le_bytes([h[0x1E], h[0x1F]]));
    if shift != 9 && shift != 12 {
        return None;
    }
    let difat = (0..109).map(|i| u32_at(0x4C + i * 4)).collect();
    let mut cfb = Cfb {
        r,
        shift,
        difat,
        first_difat: u32_at(0x44),
        num_fat: u32_at(0x2C),
    };
    let mut sect = u32_at(0x30);
    let mut buf = vec![0u8; cfb.size() as usize];
    let (mut info, mut package) = (false, false);
    for _ in 0..MAX_DIR_SECTORS {
        if sect == END {
            break;
        }
        cfb.sector(sect, &mut buf)?;
        for e in buf.as_chunks::<128>().0 {
            let len = usize::from(u16::from_le_bytes([e[64], e[65]]));
            // Streams only (type 2); names are UTF-16LE with a terminator.
            if e[66] != 2 || !(2..=64).contains(&len) {
                continue;
            }
            let name: Vec<u16> = e[..len - 2]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            let name = String::from_utf16_lossy(&name);
            info |= name == "EncryptionInfo";
            package |= name == "EncryptedPackage";
        }
        if info && package {
            return Some(true);
        }
        sect = cfb.next(sect)?;
    }
    Some(info && package)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write as _};

    fn compound(streams: &[&str], filler: usize) -> Vec<u8> {
        let mut c = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        // Enough streams that the directory needs several sectors.
        for i in 0..filler {
            c.create_stream(format!("/filler{i}"))
                .unwrap()
                .write_all(&[1u8; 5000])
                .unwrap();
        }
        for s in streams {
            c.create_stream(format!("/{s}"))
                .unwrap()
                .write_all(b"x")
                .unwrap();
        }
        c.flush().unwrap();
        c.into_inner().into_inner()
    }

    #[std::prelude::v1::test]
    fn finds_both_streams() {
        let yes = compound(&["EncryptionInfo", "EncryptedPackage"], 0);
        assert!(has_encrypted_package(&mut Cursor::new(&yes)));
        let late = compound(&["EncryptionInfo", "EncryptedPackage"], 40);
        assert!(has_encrypted_package(&mut Cursor::new(&late)));
        let half = compound(&["EncryptionInfo"], 3);
        assert!(!has_encrypted_package(&mut Cursor::new(&half)));
        let word = compound(&["WordDocument", "1Table"], 3);
        assert!(!has_encrypted_package(&mut Cursor::new(&word)));
    }

    #[std::prelude::v1::test]
    fn damaged_headers_are_not_packages() {
        let mut yes = compound(&["EncryptionInfo", "EncryptedPackage"], 0);
        for cut in [0, 8, 100, 511, 600, 1000] {
            let cut = cut.min(yes.len());
            let _ = has_encrypted_package(&mut Cursor::new(&yes[..cut]));
        }
        yes[0x30..0x34].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        assert!(!has_encrypted_package(&mut Cursor::new(&yes)));
    }
}
