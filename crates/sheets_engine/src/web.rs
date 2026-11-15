//! What IMAGE and WEBSERVICE formulas get from the web.
//!
//! The engine never fetches by itself: formulas leave their URLs on a list
//! ([`Workbook::web_requests`]) and show `#BUSY!`; the app fetches them with
//! [`fetch`] off the UI thread, hands the answers back with
//! [`Workbook::set_web_result`] and recalculates. Fetching is off until the
//! app turns it on ([`Workbook::set_web_enabled`]); meanwhile both functions
//! give `#BLOCKED!`, as Excel does when it isn't allowed to download.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use ironcalc_base::expressions::token::Error;
pub use ironcalc_base::kantor_web::{CellImage, WebKind, WebResult};

use crate::Workbook;

/// Largest picture IMAGE downloads.
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
/// Largest WEBSERVICE answer read (a cell holds 32,767 characters).
const MAX_TEXT_BYTES: u64 = 256 * 1024;

impl Workbook {
    /// Lets IMAGE and WEBSERVICE ask for downloads (off: they give
    /// `#BLOCKED!`). Recalculate afterwards.
    pub fn set_web_enabled(&mut self, enabled: bool) {
        self.model.kantor_set_web_enabled(enabled);
    }

    /// The URLs formulas wait for, after the last recalculation.
    pub fn web_requests(&self) -> Vec<(String, WebKind)> {
        self.model.kantor_web_requests()
    }

    /// The answer for `url` (from [`fetch`]). Recalculate afterwards.
    pub fn set_web_result(&mut self, url: &str, kind: WebKind, result: WebResult) {
        self.model.kantor_set_web_result(url, kind, result);
    }

    /// The picture an IMAGE formula shows in a cell (0-based row and
    /// column), drawn over the cell's value (its alt text).
    pub fn cell_image(&self, sheet: u32, row: u32, col: u32) -> Option<&CellImage> {
        self.model
            .kantor_cell_image(sheet, row as i32 + 1, col as i32 + 1)
    }
}

/// `Kantor/0.7.0 (linux; x86_64)`.
fn user_agent() -> String {
    format!(
        "Kantor/{} ({}; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Fetches `url` for a formula. Blocking (at most about 30 s): call it off
/// the UI thread. Unreachable is `#CONNECT!`; a picture Kantor can't read,
/// an answer too large or not text is `#VALUE!`, as in Excel.
pub fn fetch(url: &str, kind: WebKind) -> WebResult {
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(5)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent(user_agent())
        .build();
    let agent: ureq::Agent = config.into();
    let Ok(response) = agent.get(url).call() else {
        return WebResult::Error(Error::CONNECT);
    };
    if response.status().as_u16() != 200 {
        return WebResult::Error(match kind {
            WebKind::Image => Error::CONNECT,
            WebKind::Text => Error::VALUE,
        });
    }
    let max = match kind {
        WebKind::Image => MAX_IMAGE_BYTES,
        WebKind::Text => MAX_TEXT_BYTES,
    };
    let mut body = Vec::new();
    if response
        .into_body()
        .into_reader()
        .take(max + 1)
        .read_to_end(&mut body)
        .is_err()
    {
        return WebResult::Error(Error::CONNECT);
    }
    if body.len() as u64 > max {
        return WebResult::Error(Error::VALUE);
    }
    answer(body, kind)
}

/// What a downloaded body gives a formula.
pub fn answer(body: Vec<u8>, kind: WebKind) -> WebResult {
    match kind {
        WebKind::Text => match String::from_utf8(body) {
            Ok(text) => WebResult::Text(text),
            Err(_) => WebResult::Error(Error::VALUE),
        },
        WebKind::Image => match picture_size(&body) {
            Some((width, height)) => WebResult::Image {
                data: Arc::new(body),
                width,
                height,
            },
            None => WebResult::Error(Error::VALUE),
        },
    }
}

/// The size in pixels of a picture IMAGE supports (BMP, JPG, GIF, TIFF,
/// PNG, ICO, WEBP), from its header.
pub fn picture_size(data: &[u8]) -> Option<(u32, u32)> {
    use imagesize::ImageType;
    let supported = matches!(
        imagesize::image_type(data).ok()?,
        ImageType::Bmp
            | ImageType::Jpeg
            | ImageType::Gif
            | ImageType::Tiff
            | ImageType::Png
            | ImageType::Ico
            | ImageType::Webp
    );
    if !supported {
        return None;
    }
    let size = imagesize::blob_size(data).ok()?;
    let (w, h) = (
        u32::try_from(size.width).ok()?,
        u32::try_from(size.height).ok()?,
    );
    (w > 0 && h > 0).then_some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2 × 3 PNG (header and IHDR are all the size reader needs).
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&width.to_be_bytes());
        v.extend_from_slice(&height.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        v
    }

    #[test]
    fn answers_for_formulas() {
        match answer(png(2, 3), WebKind::Image) {
            WebResult::Image { width, height, .. } => assert_eq!((width, height), (2, 3)),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            answer(b"<html>".to_vec(), WebKind::Image),
            WebResult::Error(Error::VALUE)
        ));
        assert!(matches!(
            answer(b"1.08".to_vec(), WebKind::Text),
            WebResult::Text(t) if t == "1.08"
        ));
        assert!(matches!(
            answer(vec![0xFF, 0xFE, 0x00], WebKind::Text),
            WebResult::Error(Error::VALUE)
        ));
    }

    #[test]
    fn unreachable_is_connect() {
        // `.invalid` never resolves (RFC 2606).
        assert!(matches!(
            fetch("https://kantor-test.invalid/a.png", WebKind::Image),
            WebResult::Error(Error::CONNECT)
        ));
    }
}
