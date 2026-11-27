//! The `VBA/dir` stream (MS-OVBA 2.3.4.2), decompressed: a run of records,
//! each an id (u16), a size (u32) and that many bytes. One record breaks
//! the rule: PROJECTVERSION (0x0009) has a 4-byte reserved field where the
//! size would be, then 6 bytes.

use crate::{Error, Result};

/// One record: its id, and where its data is in the stream.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Record {
    pub id: u16,
    pub start: usize,
    pub len: usize,
}

pub const SYS_KIND: u16 = 0x0001;
pub const LCID: u16 = 0x0002;
pub const CODE_PAGE: u16 = 0x0003;
pub const NAME: u16 = 0x0004;
pub const DOC_STRING: u16 = 0x0005;
pub const HELP_FILE: u16 = 0x0006;
pub const HELP_CONTEXT: u16 = 0x0007;
pub const LIB_FLAGS: u16 = 0x0008;
pub const VERSION: u16 = 0x0009;
pub const CONSTANTS: u16 = 0x000C;
pub const REFERENCE_REGISTERED: u16 = 0x000D;
pub const REFERENCE_PROJECT: u16 = 0x000E;
pub const MODULES: u16 = 0x000F;
pub const TERMINATOR: u16 = 0x0010;
pub const COOKIE: u16 = 0x0013;
pub const LCID_INVOKE: u16 = 0x0014;
pub const REFERENCE_NAME: u16 = 0x0016;
pub const MODULE_NAME: u16 = 0x0019;
pub const MODULE_STREAM_NAME: u16 = 0x001A;
pub const MODULE_DOC_STRING: u16 = 0x001C;
pub const MODULE_HELP_CONTEXT: u16 = 0x001E;
pub const MODULE_TYPE_PROCEDURAL: u16 = 0x0021;
pub const MODULE_TYPE_OTHER: u16 = 0x0022;
pub const MODULE_READ_ONLY: u16 = 0x0025;
pub const MODULE_PRIVATE: u16 = 0x0028;
pub const MODULE_TERMINATOR: u16 = 0x002B;
pub const MODULE_COOKIE: u16 = 0x002C;
pub const REFERENCE_CONTROL: u16 = 0x002F;
pub const REFERENCE_CONTROL_EXTENDED: u16 = 0x0030;
pub const MODULE_OFFSET: u16 = 0x0031;
pub const MODULE_STREAM_NAME_UNICODE: u16 = 0x0032;
pub const REFERENCE_ORIGINAL: u16 = 0x0033;
pub const CONSTANTS_UNICODE: u16 = 0x003C;
pub const HELP_FILE_2: u16 = 0x003D;
pub const REFERENCE_NAME_UNICODE: u16 = 0x003E;
pub const DOC_STRING_UNICODE: u16 = 0x0040;
pub const MODULE_NAME_UNICODE: u16 = 0x0047;
pub const MODULE_DOC_STRING_UNICODE: u16 = 0x0048;

/// Split a decompressed `dir` stream into records. Stops at the
/// terminator; a stream cut off after the last module still reads.
pub(crate) fn records(dir: &[u8]) -> Result<Vec<Record>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 6 <= dir.len() {
        let id = u16::from_le_bytes([dir[at], dir[at + 1]]);
        let size =
            u32::from_le_bytes([dir[at + 2], dir[at + 3], dir[at + 4], dir[at + 5]]) as usize;
        let start = at + 6;
        let len = if id == VERSION { 6 } else { size };
        if start + len > dir.len() {
            return Err(Error::new(format!(
                "dir record {id:#06x} at {at} runs past the end of the stream"
            )));
        }
        out.push(Record { id, start, len });
        at = start + len;
        if id == TERMINATOR {
            break;
        }
    }
    Ok(out)
}

/// Write one record.
pub(crate) fn push(out: &mut Vec<u8>, id: u16, data: &[u8]) {
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
}

pub(crate) fn u16_at(data: &[u8]) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(..2)?.try_into().ok()?))
}

pub(crate) fn u32_at(data: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(..4)?.try_into().ok()?))
}

/// UTF-16LE bytes as text.
pub(crate) fn utf16(data: &[u8]) -> String {
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

pub(crate) fn utf16_bytes(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}
