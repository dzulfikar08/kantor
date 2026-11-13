//! A read-only Compound File Binary ([MS-CFB]) reader: just enough to pull
//! the streams out of a `.doc`. Every offset and chain is checked; a damaged
//! file is an error, never a panic or an endless loop.

use anyhow::{Result, anyhow, bail};

use super::rd::{u16_at, u32_at};

const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const END_OF_CHAIN: u32 = 0xFFFF_FFFE;
const NO_STREAM: u32 = 0xFFFF_FFFF;

/// Whether `bytes` start like a compound file (`.doc`, `.xls`, encrypted
/// Office files…).
pub fn is_compound(bytes: &[u8]) -> bool {
    bytes.starts_with(&SIGNATURE)
}

pub struct Compound<'a> {
    bytes: &'a [u8],
    sector: usize,
    mini_sector: usize,
    mini_cutoff: u64,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    mini_stream: Vec<u8>,
    /// Names and (start sector, size) of the root storage's streams.
    streams: Vec<(String, u32, u64)>,
}

fn damaged() -> anyhow::Error {
    anyhow!("the file is damaged (its compound file structure is broken)")
}

impl<'a> Compound<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Self> {
        if !is_compound(bytes) || bytes.len() < 512 {
            bail!("not a Word 97-2003 document");
        }
        let shift = u16_at(bytes, 0x1E).ok_or_else(damaged)?;
        let mini_shift = u16_at(bytes, 0x20).ok_or_else(damaged)?;
        if !matches!(shift, 9 | 12) || mini_shift != 6 {
            return Err(damaged());
        }
        let sector = 1usize << shift;
        let mut c = Self {
            bytes,
            sector,
            mini_sector: 1 << mini_shift,
            mini_cutoff: u32_at(bytes, 0x38).ok_or_else(damaged)? as u64,
            fat: Vec::new(),
            mini_fat: Vec::new(),
            mini_stream: Vec::new(),
            streams: Vec::new(),
        };
        // DIFAT: 109 entries in the header, then a chain of DIFAT sectors.
        let n_fat = u32_at(bytes, 0x2C).ok_or_else(damaged)? as usize;
        let mut fat_sectors: Vec<u32> = (0..109)
            .filter_map(|i| u32_at(bytes, 0x4C + i * 4))
            .filter(|&s| s < END_OF_CHAIN)
            .collect();
        let mut difat = u32_at(bytes, 0x44).ok_or_else(damaged)?;
        let mut guard = 0;
        while difat < END_OF_CHAIN && fat_sectors.len() < n_fat {
            guard += 1;
            if guard > bytes.len() / sector {
                return Err(damaged());
            }
            let s = c.sector_bytes(difat)?;
            let per = sector / 4 - 1;
            for i in 0..per {
                let v = u32_at(s, i * 4).ok_or_else(damaged)?;
                if v < END_OF_CHAIN {
                    fat_sectors.push(v);
                }
            }
            difat = u32_at(s, per * 4).ok_or_else(damaged)?;
        }
        fat_sectors.truncate(n_fat.max(1));
        for s in fat_sectors {
            let data = c.sector_bytes(s)?;
            c.fat.extend(
                data.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| u32::from_le_bytes(*b)),
            );
        }
        // Directory.
        let dir_start = u32_at(bytes, 0x30).ok_or_else(damaged)?;
        let dir = c.chain(dir_start, None)?;
        let entry = |i: u32| -> Option<&[u8]> {
            let off = (i as usize).checked_mul(128)?;
            dir.get(off..off + 128)
        };
        let root = entry(0).ok_or_else(damaged)?;
        if root[66] != 5 {
            return Err(damaged());
        }
        let mini_start = u32_at(root, 116).ok_or_else(damaged)?;
        let mini_size = u32_at(root, 120).ok_or_else(damaged)? as u64;
        // Walk the root's children (a red-black tree by left/right siblings).
        let mut stack = vec![u32_at(root, 76).ok_or_else(damaged)?];
        let mut seen = std::collections::HashSet::new();
        while let Some(i) = stack.pop() {
            if i == NO_STREAM || !seen.insert(i) {
                continue;
            }
            let Some(e) = entry(i) else { continue };
            stack.push(u32_at(e, 68).unwrap_or(NO_STREAM));
            stack.push(u32_at(e, 72).unwrap_or(NO_STREAM));
            if e[66] != 2 {
                continue;
            }
            let len = (u16_at(e, 64).unwrap_or(0) as usize).min(64) / 2;
            let name: Vec<u16> = (0..len.saturating_sub(1))
                .filter_map(|k| u16_at(e, k * 2))
                .collect();
            let size = u32_at(e, 120).unwrap_or(0) as u64;
            c.streams.push((
                String::from_utf16_lossy(&name),
                u32_at(e, 116).unwrap_or(END_OF_CHAIN),
                size,
            ));
        }
        // Mini FAT and mini stream (small streams live there).
        let mini_fat_start = u32_at(bytes, 0x3C).ok_or_else(damaged)?;
        if mini_fat_start < END_OF_CHAIN {
            c.mini_fat = c
                .chain(mini_fat_start, None)?
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| u32::from_le_bytes(*b))
                .collect();
        }
        if mini_start < END_OF_CHAIN && mini_size > 0 {
            c.mini_stream = c.chain(mini_start, Some(mini_size))?;
        }
        Ok(c)
    }

    fn sector_bytes(&self, s: u32) -> Result<&'a [u8]> {
        let off = (s as usize + 1)
            .checked_mul(self.sector)
            .ok_or_else(damaged)?;
        // The last sector of a file may be cut short.
        let end = (off + self.sector).min(self.bytes.len());
        if off >= end {
            return Err(damaged());
        }
        Ok(&self.bytes[off..end])
    }

    /// The bytes of a FAT chain, cut to `size` when given.
    fn chain(&self, start: u32, size: Option<u64>) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut s = start;
        let mut n = 0usize;
        while s < END_OF_CHAIN {
            n += 1;
            if n > self.fat.len() + 1 {
                return Err(damaged());
            }
            out.extend_from_slice(self.sector_bytes(s)?);
            if size.is_some_and(|z| out.len() as u64 >= z) {
                break;
            }
            s = *self.fat.get(s as usize).ok_or_else(damaged)?;
        }
        if let Some(z) = size {
            if (out.len() as u64) < z {
                return Err(damaged());
            }
            out.truncate(z as usize);
        }
        Ok(out)
    }

    fn mini_chain(&self, start: u32, size: u64) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut s = start;
        let mut n = 0usize;
        while s < END_OF_CHAIN && (out.len() as u64) < size {
            n += 1;
            if n > self.mini_fat.len() + 1 {
                return Err(damaged());
            }
            let off = (s as usize)
                .checked_mul(self.mini_sector)
                .ok_or_else(damaged)?;
            let chunk = self
                .mini_stream
                .get(off..off + self.mini_sector)
                .ok_or_else(damaged)?;
            out.extend_from_slice(chunk);
            s = *self.mini_fat.get(s as usize).ok_or_else(damaged)?;
        }
        if (out.len() as u64) < size {
            return Err(damaged());
        }
        out.truncate(size as usize);
        Ok(out)
    }

    /// A stream of the root storage, `None` when there's no such stream.
    pub fn stream(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let Some(&(_, start, size)) = self.streams.iter().find(|(n, ..)| n == name) else {
            return Ok(None);
        };
        if size > self.bytes.len() as u64 * 2 {
            return Err(damaged());
        }
        if size == 0 {
            return Ok(Some(Vec::new()));
        }
        if size < self.mini_cutoff {
            self.mini_chain(start, size).map(Some)
        } else {
            self.chain(start, Some(size)).map(Some)
        }
    }

    pub fn has_stream(&self, name: &str) -> bool {
        self.streams.iter().any(|(n, ..)| n == name)
    }
}
