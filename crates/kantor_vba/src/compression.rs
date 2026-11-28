//! MS-OVBA 2.4.1 compression: the `dir` stream and module source are stored
//! as a CompressedContainer (a signature byte, then chunks of at most 4096
//! decompressed bytes, each compressed with LZ77-style copy tokens or kept
//! raw).

use std::collections::HashMap;

use crate::{Error, Result};

const CHUNK: usize = 4096;

/// Decompress a CompressedContainer.
pub fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    let Some((&sig, mut rest)) = data.split_first() else {
        return Err(Error::new("empty compressed container"));
    };
    if sig != 0x01 {
        return Err(Error::new(format!(
            "compressed container signature is {sig:#04x}, not 0x01"
        )));
    }
    let mut out = Vec::with_capacity(data.len() * 2);
    while rest.len() >= 2 {
        let header = u16::from_le_bytes([rest[0], rest[1]]);
        let size = usize::from(header & 0x0FFF) + 3;
        let compressed = header & 0x8000 != 0;
        if (header >> 12) & 0b111 != 0b011 {
            return Err(Error::new(format!(
                "bad chunk signature in header {header:#06x}"
            )));
        }
        // A last chunk may be cut short by a careless writer: take what is there.
        let end = size.min(rest.len());
        let body = &rest[2..end];
        if compressed {
            decompress_chunk(body, &mut out)?;
        } else {
            // A raw chunk is always 4096 bytes (MS-OVBA 2.4.1.3.3).
            out.extend_from_slice(&body[..body.len().min(CHUNK)]);
        }
        rest = &rest[end..];
    }
    Ok(out)
}

fn decompress_chunk(body: &[u8], out: &mut Vec<u8>) -> Result<()> {
    let start = out.len();
    let mut i = 0;
    while i < body.len() {
        let flags = body[i];
        i += 1;
        for bit in 0..8 {
            if i >= body.len() {
                break;
            }
            if flags & (1 << bit) == 0 {
                out.push(body[i]);
                i += 1;
            } else {
                if i + 1 >= body.len() {
                    return Err(Error::new("copy token cut off"));
                }
                let token = u16::from_le_bytes([body[i], body[i + 1]]);
                i += 2;
                let (offset, length) = unpack(token, out.len() - start);
                if offset > out.len() - start {
                    return Err(Error::new("copy token reaches before its chunk"));
                }
                let from = out.len() - offset;
                // Byte by byte: a copy may overlap what it writes.
                for k in 0..length {
                    let b = out[from + k];
                    out.push(b);
                }
            }
        }
    }
    Ok(())
}

/// Bits of a copy token for the offset when `difference` bytes of the
/// chunk are decompressed (MS-OVBA 2.4.1.3.19.1, at least 4).
fn bit_count(difference: usize) -> u32 {
    let mut bits = 0;
    while (1usize << bits) < difference {
        bits += 1;
    }
    bits.max(4)
}

/// `(offset, length)` of a copy token.
fn unpack(token: u16, difference: usize) -> (usize, usize) {
    let bits = bit_count(difference);
    let length_mask = 0xFFFFu16 >> bits;
    let length = usize::from(token & length_mask) + 3;
    let offset = usize::from((token & !length_mask) >> (16 - bits)) + 1;
    (offset, length)
}

fn pack(offset: usize, length: usize, difference: usize) -> u16 {
    let bits = bit_count(difference);
    (((offset - 1) as u16) << (16 - bits)) | (length - 3) as u16
}

/// Compress `data` into a CompressedContainer. Any valid encoding reads the
/// same; this one looks for the longest earlier match of each position in
/// its chunk (through a short hash chain).
pub fn compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2 + 8);
    out.push(0x01);
    for chunk in data.chunks(CHUNK) {
        let body = compress_chunk(chunk);
        if body.len() > CHUNK {
            // Raw: exactly 4096 bytes, padded (MS-OVBA 2.4.1.3.10).
            let header: u16 = 0x3000 | (CHUNK as u16 + 2 - 3);
            out.extend_from_slice(&header.to_le_bytes());
            out.extend_from_slice(chunk);
            out.resize(out.len() + CHUNK - chunk.len(), 0);
        } else {
            let header: u16 = 0xB000 | (body.len() as u16 + 2 - 3);
            out.extend_from_slice(&header.to_le_bytes());
            out.extend_from_slice(&body);
        }
    }
    out
}

/// Candidates looked at per position.
const CHAIN: usize = 64;

fn compress_chunk(chunk: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(chunk.len() + chunk.len() / 8 + 1);
    // Positions by their first three bytes, oldest first.
    let mut heads: HashMap<[u8; 3], Vec<usize>> = HashMap::new();
    let key = |p: usize| [chunk[p], chunk[p + 1], chunk[p + 2]];
    let mut pos = 0;
    while pos < chunk.len() {
        let flag_at = body.len();
        body.push(0);
        let mut flags = 0u8;
        for bit in 0..8 {
            if pos >= chunk.len() {
                break;
            }
            let bits = bit_count(pos);
            let max_len = (0xFFFFusize >> bits) + 3;
            let mut best = (0usize, 0usize);
            if pos + 3 <= chunk.len()
                && let Some(list) = heads.get(&key(pos))
            {
                let limit = max_len.min(chunk.len() - pos);
                for &cand in list.iter().rev().take(CHAIN) {
                    let mut len = 0;
                    while len < limit && chunk[cand + len] == chunk[pos + len] {
                        len += 1;
                    }
                    if len > best.1 {
                        best = (pos - cand, len);
                        if len == limit {
                            break;
                        }
                    }
                }
            }
            let step = if best.1 >= 3 {
                body.extend_from_slice(&pack(best.0, best.1, pos).to_le_bytes());
                flags |= 1 << bit;
                best.1
            } else {
                body.push(chunk[pos]);
                1
            };
            for p in pos..pos + step {
                if p + 3 <= chunk.len() {
                    heads.entry(key(p)).or_default().push(p);
                }
            }
            pos += step;
        }
        body[flag_at] = flags;
    }
    body
}
