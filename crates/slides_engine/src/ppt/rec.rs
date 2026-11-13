//! Records of the binary format ([MS-PPT] 2.3.1 RecordHeader, shared with
//! OfficeArt): every read is bounds-checked, so a malformed file ends a
//! list early instead of panicking.

/// A record: header fields and its body.
#[derive(Clone, Copy, Debug)]
pub(super) struct Rec<'a> {
    pub ver: u8,
    pub inst: u16,
    pub ty: u16,
    pub data: &'a [u8],
}

impl<'a> Rec<'a> {
    /// The record starting at `pos` of `buf`, if its header and body fit.
    pub fn at(buf: &'a [u8], pos: usize) -> Option<Self> {
        let mut c = Cur::new(buf.get(pos..)?);
        let vi = c.u16()?;
        let ty = c.u16()?;
        let len = c.u32()? as usize;
        let start = pos.checked_add(8)?;
        let data = buf.get(start..start.checked_add(len)?)?;
        Some(Self {
            ver: (vi & 0xF) as u8,
            inst: vi >> 4,
            ty,
            data,
        })
    }

    pub fn is_container(&self) -> bool {
        self.ver == 0xF
    }

    /// The records inside a container (none for an atom).
    pub fn children(&self) -> Recs<'a> {
        Recs {
            buf: if self.is_container() { self.data } else { &[] },
            pos: 0,
        }
    }

    pub fn child(&self, ty: u16) -> Option<Rec<'a>> {
        self.children().find(|r| r.ty == ty)
    }

    pub fn kids(&self, ty: u16) -> impl Iterator<Item = Rec<'a>> {
        self.children().filter(move |r| r.ty == ty)
    }

    /// The first record of type `ty` at any depth (depth-first).
    pub fn find(&self, ty: u16) -> Option<Rec<'a>> {
        fn go<'a>(r: &Rec<'a>, ty: u16, depth: u32) -> Option<Rec<'a>> {
            if depth > 16 {
                return None;
            }
            for c in r.children() {
                if c.ty == ty {
                    return Some(c);
                }
                if let Some(f) = go(&c, ty, depth + 1) {
                    return Some(f);
                }
            }
            None
        }
        go(self, ty, 0)
    }

    pub fn cur(&self) -> Cur<'a> {
        Cur::new(self.data)
    }
}

/// Records one after another; stops at the first that doesn't fit.
pub(super) struct Recs<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Iterator for Recs<'a> {
    type Item = Rec<'a>;
    fn next(&mut self) -> Option<Rec<'a>> {
        let r = Rec::at(self.buf, self.pos)?;
        self.pos += 8 + r.data.len();
        Some(r)
    }
}

/// A little-endian reader over a byte slice.
#[derive(Clone, Copy, Debug)]
pub(super) struct Cur<'a> {
    pub b: &'a [u8],
    pub p: usize,
}

impl<'a> Cur<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }

    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(s)
    }

    pub fn skip(&mut self, n: usize) -> Option<()> {
        self.bytes(n).map(|_| ())
    }

    pub fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }

    pub fn u16(&mut self) -> Option<u16> {
        self.bytes(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }

    pub fn u32(&mut self) -> Option<u32> {
        self.bytes(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }

    pub fn rest(&self) -> &'a [u8] {
        self.b.get(self.p..).unwrap_or(&[])
    }
}

/// Record types used here ([MS-PPT] 2.13.24 RecordType, [MS-ODRAW] 2.2).
pub(super) mod rt {
    pub const DOCUMENT: u16 = 0x03E8;
    pub const DOCUMENT_ATOM: u16 = 0x03E9;
    pub const SLIDE: u16 = 0x03EE;
    pub const SLIDE_ATOM: u16 = 0x03EF;
    pub const NOTES: u16 = 0x03F0;
    pub const NOTES_ATOM: u16 = 0x03F1;
    pub const ENVIRONMENT: u16 = 0x03F2;
    pub const SLIDE_PERSIST_ATOM: u16 = 0x03F3;
    pub const MAIN_MASTER: u16 = 0x03F8;
    pub const SLIDE_SHOW_SLIDE_INFO: u16 = 0x03F9;
    pub const DRAWING_GROUP: u16 = 0x040B;
    pub const DRAWING: u16 = 0x040C;
    pub const COLOR_SCHEME: u16 = 0x07F0;
    pub const FONT_COLLECTION: u16 = 0x07D5;
    pub const OUTLINE_TEXT_REF: u16 = 0x0F9E;
    pub const TEXT_HEADER: u16 = 0x0F9F;
    pub const TEXT_CHARS: u16 = 0x0FA0;
    pub const STYLE_TEXT_PROP: u16 = 0x0FA1;
    pub const MASTER_TEXT_PROP: u16 = 0x0FA2;
    pub const TEXT_MASTER_STYLE: u16 = 0x0FA3;
    pub const TEXT_RULER: u16 = 0x0FA6;
    pub const TEXT_BYTES: u16 = 0x0FA8;
    pub const FONT_ENTITY: u16 = 0x0FB7;
    pub const SLIDE_NUMBER_MC: u16 = 0x0FD8;
    pub const SLIDE_LIST_WITH_TEXT: u16 = 0x0FF0;
    pub const USER_EDIT: u16 = 0x0FF5;
    pub const CURRENT_USER: u16 = 0x0FF6;
    pub const DATE_TIME_MC: u16 = 0x0FF7;
    pub const GENERIC_DATE_MC: u16 = 0x0FF8;
    pub const HEADER_MC: u16 = 0x0FF9;
    pub const FOOTER_MC: u16 = 0x0FFA;
    pub const RTF_DATE_TIME_MC: u16 = 0x1015;
    pub const PLACEHOLDER: u16 = 0x0BC3;
    pub const PERSIST_DIRECTORY: u16 = 0x1772;
    pub const CRYPT_SESSION: u16 = 0x2F14;

    pub const DGG_CONTAINER: u16 = 0xF000;
    pub const BSTORE: u16 = 0xF001;
    pub const DG_CONTAINER: u16 = 0xF002;
    pub const SPGR_CONTAINER: u16 = 0xF003;
    pub const SP_CONTAINER: u16 = 0xF004;
    pub const FBSE: u16 = 0xF007;
    pub const FSPGR: u16 = 0xF009;
    pub const FSP: u16 = 0xF00A;
    pub const FOPT: u16 = 0xF00B;
    pub const CLIENT_TEXTBOX: u16 = 0xF00D;
    pub const CHILD_ANCHOR: u16 = 0xF00F;
    pub const CLIENT_ANCHOR: u16 = 0xF010;
    pub const CLIENT_DATA: u16 = 0xF011;
    pub const SECONDARY_FOPT: u16 = 0xF121;
    pub const TERTIARY_FOPT: u16 = 0xF122;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_never_read_past_the_end() {
        // A container claiming 100 bytes with only 4 present.
        let mut b = vec![0x0F, 0x00, 0xE8, 0x03, 100, 0, 0, 0];
        b.extend_from_slice(&[1, 2, 3, 4]);
        assert!(Rec::at(&b, 0).is_none());
        assert!(Rec::at(&b, 6).is_none());
        assert!(Rec::at(&b, usize::MAX).is_none());
        // A child that overruns its container ends the list.
        let mut b = vec![0x0F, 0x00, 0xE8, 0x03, 12, 0, 0, 0];
        b.extend_from_slice(&[0, 0, 0xE9, 0x03, 0, 0, 0, 0]);
        b.extend_from_slice(&[0, 0, 0xE9, 0x03]);
        let r = Rec::at(&b, 0).unwrap();
        assert_eq!(r.children().count(), 1);
    }
}
