//! KANTOR PATCH (functions): IMAGE, WEBSERVICE and the functions that need
//! Microsoft's online services.
//!
//! The engine never touches the network. IMAGE and WEBSERVICE look their URL
//! up in [`KantorWeb`]: a result the host has fetched is used; otherwise the
//! URL is put on the list of requests ([`Model::kantor_web_requests`]) and the
//! cell shows `#BUSY!` until the host answers with
//! [`Model::kantor_set_web_result`] and recalculates (both functions are
//! evaluated on every recalculation). While the host doesn't allow fetching
//! ([`KantorWeb::enabled`], off by default) they give `#BLOCKED!`, Excel's
//! error for content it isn't allowed to download.
//!
//! An IMAGE cell's value is its alt text (empty when it has none); what the
//! grid draws in it is [`Model::kantor_cell_image`].
//!
//! The CUBE functions, RTD, STOCKHISTORY, TRANSLATE, DETECTLANGUAGE, COPILOT
//! and PY need servers Kantor can't reach. A cell whose formula is one of
//! them keeps the value saved in the file (a spilled array too), so a
//! workbook opens and saves with Excel's results unchanged; entered or
//! changed in Kantor, the formula gives the error Excel gives without a
//! connection.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::{
    calc_result::CalcResult,
    expressions::{
        parser::{ArrayNode, Node},
        token::Error,
        types::CellReferenceIndex,
    },
    functions::Function,
    model::Model,
    types::{ArrayKind, Cell, FormulaValue, SpillValue},
};

/// Longest text a cell holds; a longer WEBSERVICE answer is #VALUE!.
const MAX_TEXT: usize = 32767;

/// What a formula asks the host to fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WebKind {
    /// WEBSERVICE: the body as text.
    Text,
    /// IMAGE: a picture (BMP, JPG, GIF, TIFF, PNG, ICO or WEBP).
    Image,
}

/// The host's answer for a URL.
#[derive(Clone, Debug)]
pub enum WebResult {
    Text(String),
    /// The file's bytes and its size in pixels.
    Image {
        data: Arc<Vec<u8>>,
        width: u32,
        height: u32,
    },
    /// `#CONNECT!` (unreachable), `#VALUE!` (not a picture, too large…),
    /// `#BLOCKED!`.
    Error(Error),
}

/// What an IMAGE cell shows.
#[derive(Clone, Debug, PartialEq)]
pub struct CellImage {
    pub source: String,
    pub alt_text: String,
    /// 0 fit in the cell keeping the aspect ratio, 1 fill the cell, 2 the
    /// picture's own size, 3 `height` × `width` (points).
    pub sizing: u8,
    pub height: Option<f64>,
    pub width: Option<f64>,
    pub data: Arc<Vec<u8>>,
    /// The picture's size in pixels.
    pub pixel_width: u32,
    pub pixel_height: u32,
}

/// Fetched results, outstanding requests and the pictures of IMAGE cells.
#[derive(Default, Clone, Debug)]
pub struct KantorWeb {
    /// Whether the host fetches what formulas ask for. Off: IMAGE and
    /// WEBSERVICE give #BLOCKED!.
    pub enabled: bool,
    results: HashMap<String, WebResult>,
    requests: BTreeMap<(String, WebKind), ()>,
    /// Per cell: its picture and the evaluation that drew it.
    images: HashMap<(u32, i32, i32), (CellImage, u64)>,
    /// Pictures the file saved for IMAGE cells (Excel's rich values), per
    /// cell: shown until the formula's own download is there.
    saved: HashMap<(u32, i32, i32), SavedPicture>,
    /// The evaluation count when the saved pictures were set: until the
    /// next evaluation they are what the cells show.
    saved_at: Option<u64>,
}

/// A picture saved in the file for an IMAGE cell.
#[derive(Clone, Debug)]
struct SavedPicture {
    /// As the cell shows it before any evaluation (sizing from the
    /// formula's literal arguments).
    image: CellImage,
    /// The source of the cell's first evaluation: the picture belongs to it
    /// (a formula changed to another URL doesn't show it).
    source: Option<String>,
    /// The cell's formula when the file was read (another formula typed
    /// in doesn't show the picture).
    formula: Option<String>,
}

fn formula_value(v: &FormulaValue) -> Option<ArrayNode> {
    Some(match v {
        FormulaValue::Unevaluated => return None,
        FormulaValue::Boolean(b) => ArrayNode::Boolean(*b),
        FormulaValue::Number(f) => ArrayNode::Number(*f),
        FormulaValue::Text(s) => ArrayNode::String(s.clone()),
        FormulaValue::Error { ei, .. } => ArrayNode::Error(ei.clone()),
    })
}

fn spill_value(v: &SpillValue) -> ArrayNode {
    match v {
        SpillValue::Boolean(b) => ArrayNode::Boolean(*b),
        SpillValue::Number(f) => ArrayNode::Number(*f),
        SpillValue::Text(s) => ArrayNode::String(s.clone()),
        SpillValue::Error(e) => ArrayNode::Error(e.clone()),
    }
}

fn node_result(node: ArrayNode, cell: CellReferenceIndex) -> CalcResult {
    match node {
        ArrayNode::Boolean(b) => CalcResult::Boolean(b),
        ArrayNode::Number(f) => CalcResult::Number(f),
        ArrayNode::String(s) => CalcResult::String(s),
        ArrayNode::Error(e) => CalcResult::new_error(e, cell, String::new()),
        ArrayNode::Empty => CalcResult::EmptyCell,
    }
}

impl Model<'_> {
    /// Whether formulas may fetch from the web (the host does the fetching).
    pub fn kantor_set_web_enabled(&mut self, enabled: bool) {
        self.kantor_web.enabled = enabled;
    }

    /// The URLs formulas are waiting for, with what they want of each.
    pub fn kantor_web_requests(&self) -> Vec<(String, WebKind)> {
        self.kantor_web.requests.keys().cloned().collect()
    }

    /// The host's answer for `url`. Recalculate afterwards.
    pub fn kantor_set_web_result(&mut self, url: &str, kind: WebKind, result: WebResult) {
        self.kantor_web.requests.remove(&(url.to_string(), kind));
        self.kantor_web.results.insert(url.to_string(), result);
    }

    /// Forgets fetched results (they are fetched again when needed).
    pub fn kantor_clear_web_results(&mut self) {
        self.kantor_web.results.clear();
    }

    /// Forgets what WEBSERVICE fetched and every failed download, keeping
    /// the pictures: a recalculation the user asked for (F9) fetches them
    /// again.
    pub fn kantor_refresh_web(&mut self) {
        self.kantor_web
            .results
            .retain(|_, r| matches!(r, WebResult::Image { .. }));
    }

    /// A picture the file saved for the IMAGE formula in a cell (Excel
    /// keeps the last download as a rich value), with its alt text. Shown
    /// from now on, and by the formula whenever its own download isn't
    /// there (blocked, offline, waiting).
    #[allow(clippy::too_many_arguments)]
    pub fn kantor_set_saved_picture(
        &mut self,
        sheet: u32,
        row: i32,
        column: i32,
        data: Arc<Vec<u8>>,
        pixel_width: u32,
        pixel_height: u32,
        alt_text: Option<String>,
    ) {
        let cell = CellReferenceIndex { sheet, row, column };
        // A picture placed in a cell is `#VALUE!` (the importer may have
        // read another error from the cell's value metadata).
        if let Some(Cell::ErrorCell { ei, s }) = self.fetch_cell(cell).map(|c| c.into_owned()) {
            if ei != Error::VALUE {
                if let Ok(ws) = self.workbook.worksheet_mut(sheet) {
                    let _ = ws.update_cell(
                        row,
                        column,
                        Cell::ErrorCell {
                            ei: Error::VALUE,
                            s,
                        },
                    );
                }
            }
        }
        let (sizing, height, width, literal_alt) = self.image_literal_args(cell);
        let image = CellImage {
            source: String::new(),
            alt_text: alt_text.or(literal_alt).unwrap_or_default(),
            sizing,
            height,
            width,
            data,
            pixel_width,
            pixel_height,
        };
        let formula = self.kantor_formula_text(cell);
        self.kantor_web.saved.insert(
            (sheet, row, column),
            SavedPicture {
                image,
                source: None,
                formula,
            },
        );
        self.kantor_web.saved_at = Some(self.kantor_evaluations);
    }

    /// A cell's formula as stored (its sheet's shared formula).
    fn kantor_formula_text(&self, cell: CellReferenceIndex) -> Option<String> {
        let f = match self.fetch_cell(cell)?.as_ref() {
            Cell::CellFormula { f, .. } | Cell::ArrayFormula { f, .. } => *f,
            _ => return None,
        };
        self.workbook
            .worksheets
            .get(cell.sheet as usize)?
            .shared_formulas
            .get(f as usize)
            .cloned()
    }

    /// Whether any IMAGE cell has a picture saved in the file.
    pub fn kantor_has_saved_pictures(&self) -> bool {
        !self.kantor_web.saved.is_empty()
    }

    /// The sizing, height, width and alt text an IMAGE formula gives as
    /// literal arguments (fit, no alt text, for anything else).
    fn image_literal_args(
        &self,
        cell: CellReferenceIndex,
    ) -> (u8, Option<f64>, Option<f64>, Option<String>) {
        let none = (0, None, None, None);
        let Some(f) = self.fetch_cell(cell).and_then(|c| match &*c {
            Cell::CellFormula { f, .. } | Cell::ArrayFormula { f, .. } => Some(*f),
            _ => None,
        }) else {
            return none;
        };
        let Some(mut node) = self
            .parsed_formulas
            .get(cell.sheet as usize)
            .and_then(|s| s.get(f as usize))
            .map(|p| p.0.as_ref())
        else {
            return none;
        };
        while let Node::ImplicitIntersection { child, .. } = node {
            node = child;
        }
        let Node::FunctionKind {
            kind: Function::Image,
            args,
        } = node
        else {
            return none;
        };
        let number = |i: usize| match args.get(i) {
            Some(Node::NumberKind(n)) => Some(*n),
            _ => None,
        };
        let alt = match args.get(1) {
            Some(Node::StringKind(s)) => Some(s.clone()),
            _ => None,
        };
        let sizing = number(2).filter(|s| (0.0..=3.0).contains(s)).unwrap_or(0.0) as u8;
        if sizing == 3 {
            (3, number(3), number(4), alt)
        } else {
            (sizing, None, None, alt)
        }
    }

    /// The picture an IMAGE formula shows in a cell, after the last
    /// recalculation (before any, the picture the file saved).
    pub fn kantor_cell_image(&self, sheet: u32, row: i32, column: i32) -> Option<&CellImage> {
        match self.kantor_web.images.get(&(sheet, row, column)) {
            Some((image, at)) if *at == self.kantor_evaluations => Some(image),
            _ => {
                let saved = &self.kantor_web.saved.get(&(sheet, row, column))?.image;
                let unevaluated = self.kantor_web.saved_at == Some(self.kantor_evaluations);
                (unevaluated || self.kantor_is_placed_picture(sheet, row, column)).then_some(saved)
            }
        }
    }

    /// Whether a cell is a picture placed in it (Excel's Place in Cell: an
    /// error value with a saved picture, no formula), as the file had it.
    fn kantor_is_placed_picture(&self, sheet: u32, row: i32, column: i32) -> bool {
        let cell = CellReferenceIndex { sheet, row, column };
        matches!(
            self.fetch_cell(cell).as_deref(),
            Some(Cell::ErrorCell { .. })
        )
    }

    /// Whether the picture a cell shows is the one the file saved (not a
    /// new download).
    pub fn kantor_shows_saved_picture(&self, sheet: u32, row: i32, column: i32) -> bool {
        match (
            self.kantor_cell_image(sheet, row, column),
            self.kantor_web.saved.get(&(sheet, row, column)),
        ) {
            (Some(shown), Some(saved)) => Arc::ptr_eq(&shown.data, &saved.image.data),
            _ => false,
        }
    }

    /// The cells showing the picture the file saved for them: (sheet, row,
    /// column).
    pub fn kantor_saved_picture_cells(&self) -> Vec<(u32, i32, i32)> {
        let mut cells: Vec<_> = self
            .kantor_web
            .saved
            .keys()
            .copied()
            .filter(|&(s, r, c)| self.kantor_shows_saved_picture(s, r, c))
            .collect();
        cells.sort_unstable();
        cells
    }

    /// Every cell showing a picture: (sheet, row, column).
    pub fn kantor_image_cells(&self) -> Vec<(u32, i32, i32)> {
        let mut cells: Vec<_> = self
            .kantor_web
            .images
            .keys()
            .chain(self.kantor_web.saved.keys())
            .copied()
            .filter(|&(s, r, c)| self.kantor_cell_image(s, r, c).is_some())
            .collect();
        cells.sort_unstable();
        cells.dedup();
        cells
    }

    /// The fetched result for `url`; `Err` is what the formula gives
    /// meanwhile (#BUSY! while it is being fetched, #BLOCKED! when fetching
    /// is off).
    fn web_result(&mut self, url: &str, kind: WebKind) -> Result<WebResult, Error> {
        if let Some(result) = self.kantor_web.results.get(url) {
            return Ok(result.clone());
        }
        if !self.kantor_web.enabled {
            return Err(Error::BLOCKED);
        }
        self.kantor_web.requests.insert((url.to_string(), kind), ());
        Err(Error::BUSY)
    }

    /// `WEBSERVICE(url)`: the text the URL returns.
    pub(crate) fn fn_webservice(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        if args.len() != 1 {
            return CalcResult::new_args_number_error(cell);
        }
        let url = match self.get_string(&args[0], cell) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let error = |e: Error| CalcResult::new_error(e, cell, String::new());
        if url.chars().count() > 2048 || !is_web_url(&url) {
            return error(Error::VALUE);
        }
        match self.web_result(&url, WebKind::Text) {
            Ok(WebResult::Text(s)) if s.chars().count() <= MAX_TEXT => CalcResult::String(s),
            Ok(WebResult::Error(e)) => error(e),
            Ok(_) => error(Error::VALUE),
            // Not allowed to download: the value the cell had (the file's,
            // or the last download), as Excel keeps it until it may refresh.
            Err(Error::BLOCKED) => self
                .kantor_saved_result(&Function::Webservice, cell)
                .filter(|r| {
                    !matches!(
                        r,
                        CalcResult::Error {
                            error: Error::BLOCKED | Error::BUSY,
                            ..
                        }
                    )
                })
                .unwrap_or_else(|| error(Error::BLOCKED)),
            Err(e) => error(e),
        }
    }

    /// `IMAGE(source, [alt_text], [sizing], [height], [width])`.
    pub(crate) fn fn_image(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        if args.is_empty() || args.len() > 5 {
            return CalcResult::new_args_number_error(cell);
        }
        let error = |e: Error| CalcResult::new_error(e, cell, String::new());
        let source = match self.get_string(&args[0], cell) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let alt_text = match args.get(1) {
            None | Some(Node::EmptyArgKind) => String::new(),
            Some(node) => match self.get_string(node, cell) {
                Ok(s) => s,
                Err(e) => return e,
            },
        };
        let mut number = |i: usize| -> Result<Option<f64>, CalcResult> {
            match args.get(i) {
                None | Some(Node::EmptyArgKind) => Ok(None),
                Some(node) => match self.evaluate_node_in_context(node, cell) {
                    CalcResult::EmptyCell | CalcResult::EmptyArg => Ok(None),
                    result => self.cast_to_number(result, cell).map(Some),
                },
            }
        };
        let (sizing, height, width) = match (number(2), number(3), number(4)) {
            (Ok(s), Ok(h), Ok(w)) => (s.unwrap_or(0.0).trunc(), h, w),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return e,
        };
        // Sizing 0–3; height and width only for 3, at least one of them
        // there, positive.
        let custom = sizing == 3.0;
        if !(0.0..=3.0).contains(&sizing)
            || (!custom && (height.is_some() || width.is_some()))
            || (custom && height.is_none() && width.is_none())
            || height.is_some_and(|h| h < 1.0)
            || width.is_some_and(|w| w < 1.0)
        {
            return error(Error::VALUE);
        }
        if !source.to_ascii_lowercase().starts_with("https://") || !is_web_url(&source) {
            return error(Error::VALUE);
        }
        match self.web_result(&source, WebKind::Image) {
            Ok(WebResult::Image {
                data,
                width: pixel_width,
                height: pixel_height,
            }) => {
                let image = CellImage {
                    source,
                    alt_text: alt_text.clone(),
                    sizing: sizing as u8,
                    height,
                    width,
                    data,
                    pixel_width,
                    pixel_height,
                };
                let at = self.kantor_evaluations;
                self.kantor_web
                    .images
                    .insert((cell.sheet, cell.row, cell.column), (image, at));
                CalcResult::String(alt_text)
            }
            other => {
                // The picture the file saved, while the formula's own
                // download isn't there (blocked, offline, waiting).
                let key = (cell.sheet, cell.row, cell.column);
                let formula = self.kantor_formula_text(cell);
                if let Some(saved) = self
                    .kantor_web
                    .saved
                    .get_mut(&key)
                    .filter(|s| s.formula == formula)
                {
                    let bound = saved.source.get_or_insert_with(|| source.clone());
                    if *bound == source {
                        let image = CellImage {
                            source,
                            alt_text: alt_text.clone(),
                            sizing: sizing as u8,
                            height,
                            width,
                            data: saved.image.data.clone(),
                            pixel_width: saved.image.pixel_width,
                            pixel_height: saved.image.pixel_height,
                        };
                        let at = self.kantor_evaluations;
                        self.kantor_web.images.insert(key, (image, at));
                        return CalcResult::String(alt_text);
                    }
                }
                match other {
                    Ok(WebResult::Error(e)) | Err(e) => error(e),
                    _ => error(Error::VALUE),
                }
            }
        }
    }

    /// The value saved for `cell` if its formula is a call of `kind` (a
    /// spilled array whole), `None` for a formula entered since.
    fn kantor_saved_result(&self, kind: &Function, cell: CellReferenceIndex) -> Option<CalcResult> {
        let stored = self.fetch_cell(cell)?.into_owned();
        let (f, value, range) = match &stored {
            Cell::CellFormula { f, v, .. } => (*f, v, (1, 1)),
            Cell::ArrayFormula {
                f,
                v,
                r,
                kind: ArrayKind::Dynamic,
                ..
            } => (*f, v, *r),
            _ => return None,
        };
        let mut node: &Node = &self
            .parsed_formulas
            .get(cell.sheet as usize)?
            .get(f as usize)?
            .0;
        // An implicit intersection the parser added (or `@`) is not a
        // formula of its own.
        while let Node::ImplicitIntersection { child, .. } = node {
            node = child;
        }
        if !matches!(node, Node::FunctionKind { kind: k, .. } if k == kind) {
            return None;
        }
        let first = formula_value(value)?;
        let (width, height) = range;
        if width <= 1 && height <= 1 {
            return Some(node_result(first, cell));
        }
        let mut rows = Vec::new();
        for dr in 0..height {
            let mut row = Vec::new();
            for dc in 0..width {
                if dr == 0 && dc == 0 {
                    row.push(first.clone());
                    continue;
                }
                let at = CellReferenceIndex {
                    sheet: cell.sheet,
                    row: cell.row + dr,
                    column: cell.column + dc,
                };
                let node = match self.fetch_cell(at).as_deref() {
                    Some(Cell::SpillCell { a, v, .. }) if *a == (cell.row, cell.column) => {
                        spill_value(v)
                    }
                    _ => ArrayNode::Empty,
                };
                row.push(node);
            }
            rows.push(row);
        }
        Some(CalcResult::Array(rows))
    }

    /// The CUBE functions, RTD, STOCKHISTORY, TRANSLATE, DETECTLANGUAGE,
    /// COPILOT and PY: the value saved in the file, or Excel's error without
    /// a connection.
    pub(crate) fn fn_online(
        &mut self,
        kind: &Function,
        _args: &[Node],
        cell: CellReferenceIndex,
    ) -> CalcResult {
        if let Some(saved) = self.kantor_saved_result(kind, cell) {
            return saved;
        }
        let error = match kind {
            // No connection to the OLAP server: the connection is unknown.
            Function::Cubekpimember
            | Function::Cubemember
            | Function::Cubememberproperty
            | Function::Cuberankedmember
            | Function::Cubeset
            | Function::Cubesetcount
            | Function::Cubevalue => Error::NAME,
            // No RTD server.
            Function::Rtd => Error::NA,
            _ => Error::CONNECT,
        };
        CalcResult::new_error(
            error,
            cell,
            "This function needs a Microsoft online service".to_string(),
        )
    }
}

impl crate::UserModel<'_> {
    /// See [`Model::kantor_set_web_enabled`].
    pub fn kantor_set_web_enabled(&mut self, enabled: bool) {
        self.model.kantor_set_web_enabled(enabled);
    }

    /// See [`Model::kantor_web_requests`].
    pub fn kantor_web_requests(&self) -> Vec<(String, WebKind)> {
        self.model.kantor_web_requests()
    }

    /// See [`Model::kantor_set_web_result`]. Recalculate afterwards.
    pub fn kantor_set_web_result(&mut self, url: &str, kind: WebKind, result: WebResult) {
        self.model.kantor_set_web_result(url, kind, result);
    }

    /// See [`Model::kantor_cell_image`].
    pub fn kantor_cell_image(&self, sheet: u32, row: i32, column: i32) -> Option<&CellImage> {
        self.model.kantor_cell_image(sheet, row, column)
    }

    /// See [`Model::kantor_refresh_web`].
    pub fn kantor_refresh_web(&mut self) {
        self.model.kantor_refresh_web();
    }

    /// See [`Model::kantor_set_saved_picture`].
    #[allow(clippy::too_many_arguments)]
    pub fn kantor_set_saved_picture(
        &mut self,
        sheet: u32,
        row: i32,
        column: i32,
        data: Arc<Vec<u8>>,
        pixel_width: u32,
        pixel_height: u32,
        alt_text: Option<String>,
    ) {
        self.model.kantor_set_saved_picture(
            sheet,
            row,
            column,
            data,
            pixel_width,
            pixel_height,
            alt_text,
        );
    }
}

/// An http(s) URL with a host.
fn is_web_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    match rest {
        Some(rest) => {
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            !host.is_empty() && !url.chars().any(char::is_whitespace)
        }
        None => false,
    }
}
