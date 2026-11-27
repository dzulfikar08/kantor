//! Windows code pages (PROJECTCODEPAGE) to encodings.

use encoding_rs::Encoding;

/// The encoding of Windows code page `cp` (windows-1252 when unknown).
pub fn encoding(cp: u16) -> &'static Encoding {
    let label = match cp {
        874 => "windows-874".to_owned(),
        1250..=1258 => format!("windows-{cp}"),
        932 => "shift_jis".to_owned(),
        936 => "gbk".to_owned(),
        949 => "euc-kr".to_owned(),
        950 => "big5".to_owned(),
        866 => "ibm866".to_owned(),
        10000 => "macintosh".to_owned(),
        10007 => "x-mac-cyrillic".to_owned(),
        20866 => "koi8-r".to_owned(),
        21866 => "koi8-u".to_owned(),
        28591 => "iso-8859-1".to_owned(),
        28592..=28599 => format!("iso-8859-{}", cp - 28590),
        28603 => "iso-8859-13".to_owned(),
        28605 => "iso-8859-15".to_owned(),
        50220..=50222 => "iso-2022-jp".to_owned(),
        51932 => "euc-jp".to_owned(),
        54936 => "gb18030".to_owned(),
        65001 => "utf-8".to_owned(),
        _ => "windows-1252".to_owned(),
    };
    Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::WINDOWS_1252)
}

/// `bytes` in code page `cp` as text.
pub fn decode(cp: u16, bytes: &[u8]) -> String {
    encoding(cp)
        .decode_without_bom_handling(bytes)
        .0
        .into_owned()
}

/// `text` in code page `cp`; `None` when it has characters the code page
/// can't hold.
pub fn encode(cp: u16, text: &str) -> Option<Vec<u8>> {
    let enc = encoding(cp);
    // encoding_rs writes UTF-8 for UTF-16 labels; VBA never uses those.
    let (bytes, _, errors) = enc.encode(text);
    (!errors).then(|| bytes.into_owned())
}
