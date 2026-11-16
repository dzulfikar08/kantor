//! What IMAGE and WEBSERVICE formulas get from the web.
//!
//! The engine never fetches by itself: formulas leave their URLs on a list
//! ([`Workbook::web_requests`]) and show `#BUSY!`; the app fetches them with
//! [`fetch_cached`] off the UI thread, hands the answers back with
//! [`Workbook::set_web_result`] and recalculates. Fetching is off until the
//! app turns it on ([`Workbook::set_web_enabled`]); meanwhile both functions
//! give `#BLOCKED!`, as Excel does when it isn't allowed to download (an
//! IMAGE cell keeps the picture the file saved, a WEBSERVICE cell its
//! value).
//!
//! Downloads are kept for the session ([`cached`]), for every workbook:
//! pictures until Kantor quits, WEBSERVICE answers until a recalculation the
//! user asks for (F9, [`Workbook::refresh_web`] and [`forget_text`]).
//! Headless conversions (`kantor convert`) never recalculate, so they never
//! download either.

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
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

    /// Whether IMAGE and WEBSERVICE may ask for downloads.
    pub fn web_enabled(&self) -> bool {
        self.model.get_model().kantor_web.enabled
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

    /// Before a recalculation the user asked for (F9): WEBSERVICE answers
    /// and failed downloads are fetched again (pictures are kept).
    pub fn refresh_web(&mut self) {
        self.model.kantor_refresh_web();
    }

    /// Answers the requests already in the session cache (another workbook
    /// or an earlier recalculation fetched them). Returns whether any was:
    /// recalculate then.
    pub fn answer_from_cache(&mut self) -> bool {
        let mut any = false;
        for (url, kind) in self.web_requests() {
            if let Some(result) = cached(&url, kind) {
                self.set_web_result(&url, kind, result);
                any = true;
            }
        }
        any
    }

    /// Whether a formula uses IMAGE or WEBSERVICE (they may download).
    pub fn has_web_formulas(&self) -> bool {
        self.model
            .get_model()
            .workbook
            .worksheets
            .iter()
            .flat_map(|ws| ws.shared_formulas.iter())
            .any(|f| {
                let f = f.to_ascii_uppercase();
                f.contains("IMAGE(") || f.contains("WEBSERVICE(")
            })
    }

    /// Pictures Excel saved for IMAGE cells (rich values), by `(sheetId,
    /// row, col)`: shown until the formulas download their own.
    pub(crate) fn set_saved_pictures(
        &mut self,
        pictures: std::collections::HashMap<(u32, u32, u32), crate::preserve::RichPicture>,
    ) {
        let sheets: Vec<u32> = self
            .model
            .get_model()
            .workbook
            .worksheets
            .iter()
            .map(|ws| ws.sheet_id)
            .collect();
        for ((sheet_id, row, col), p) in pictures {
            let Some(sheet) = sheets.iter().position(|id| *id == sheet_id) else {
                continue;
            };
            self.model.kantor_set_saved_picture(
                sheet as u32,
                row as i32 + 1,
                col as i32 + 1,
                p.data,
                p.width,
                p.height,
                p.alt,
            );
        }
    }

    /// Whether an IMAGE cell shows a picture the file saved.
    pub fn has_saved_pictures(&self) -> bool {
        self.model.get_model().kantor_has_saved_pictures()
    }
}

/// What this session downloaded, by URL and kind.
static SESSION: Mutex<Option<HashMap<(String, WebKind), WebResult>>> = Mutex::new(None);

/// What the session downloaded for `url`, if it did (errors aren't kept).
pub fn cached(url: &str, kind: WebKind) -> Option<WebResult> {
    let session = SESSION.lock().ok()?;
    session.as_ref()?.get(&(url.to_owned(), kind)).cloned()
}

/// Forgets the WEBSERVICE answers of the session (F9 fetches them again).
pub fn forget_text() {
    if let Ok(mut session) = SESSION.lock()
        && let Some(map) = session.as_mut()
    {
        map.retain(|(_, kind), _| *kind == WebKind::Image);
    }
}

/// [`fetch`], through the session cache: a URL is downloaded once per
/// session (WEBSERVICE answers until [`forget_text`]).
pub fn fetch_cached(url: &str, kind: WebKind) -> WebResult {
    if let Some(hit) = cached(url, kind) {
        return hit;
    }
    let result = fetch(url, kind);
    if !matches!(result, WebResult::Error(_))
        && let Ok(mut session) = SESSION.lock()
    {
        session
            .get_or_insert_with(HashMap::new)
            .insert((url.to_owned(), kind), result.clone());
    }
    result
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
/// the UI thread. As in Excel: for IMAGE, unreachable is `#CONNECT!`, a
/// redirect `#BLOCKED!` (Excel doesn't follow them, for security) and a
/// file that isn't a picture it reads `#VALUE!`; WEBSERVICE gives `#VALUE!`
/// for anything that fails (unreachable, an error status, an answer too
/// large or not text).
pub fn fetch(url: &str, kind: WebKind) -> WebResult {
    let failed = |image_error: Error| {
        WebResult::Error(match kind {
            WebKind::Image => image_error,
            WebKind::Text => Error::VALUE,
        })
    };
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(match kind {
            WebKind::Image => 0,
            WebKind::Text => 5,
        })
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent(user_agent())
        .build();
    let agent: ureq::Agent = config.into();
    let Ok(response) = agent.get(url).call() else {
        return failed(Error::CONNECT);
    };
    match response.status().as_u16() {
        200 => {}
        300..=399 => return failed(Error::BLOCKED),
        _ => return failed(Error::CONNECT),
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
        return failed(Error::CONNECT);
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

/// The format of a picture IMAGE supports: `png`, `jpeg`, `gif`, `bmp`,
/// `tiff`, `ico` or `webp`.
pub fn picture_format(data: &[u8]) -> Option<&'static str> {
    use imagesize::ImageType;
    Some(match imagesize::image_type(data).ok()? {
        ImageType::Bmp => "bmp",
        ImageType::Jpeg => "jpeg",
        ImageType::Gif => "gif",
        ImageType::Tiff => "tiff",
        ImageType::Png => "png",
        ImageType::Ico => "ico",
        ImageType::Webp => "webp",
        _ => return None,
    })
}

/// The size in pixels of a picture IMAGE supports (BMP, JPG, GIF, TIFF,
/// PNG, ICO, WEBP), from its header.
pub fn picture_size(data: &[u8]) -> Option<(u32, u32)> {
    picture_format(data)?;
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
    fn unreachable_is_connect_for_pictures_and_value_for_text() {
        // `.invalid` never resolves (RFC 2606).
        assert!(matches!(
            fetch("https://kantor-test.invalid/a.png", WebKind::Image),
            WebResult::Error(Error::CONNECT)
        ));
        assert!(matches!(
            fetch("https://kantor-test.invalid/rate", WebKind::Text),
            WebResult::Error(Error::VALUE)
        ));
    }
}
