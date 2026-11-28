//! The path diagram as data (no GPUI, tested without a window): the model
//! (`kantor_pls::Model`, with its `layout`), the editing operations with
//! undo and redo, the auto-layout, and the geometry the canvas draws and
//! hit-tests (ellipses for constructs, boxes for indicators, arrows).
//!
//! Coordinates are diagram points, y down; the canvas adds its scroll
//! offset.

use std::collections::{BTreeMap, HashSet};

use kantor_pls::{Bootstrap, Construct, Layout, Measurement, Model, Options, Path};

/// Half the width and height of a construct's ellipse.
pub const RX: f64 = 60.0;
pub const RY: f64 = 30.0;
/// An indicator's box.
pub const IND_W: f64 = 72.0;
pub const IND_H: f64 = 22.0;
/// Space between two indicator boxes.
pub const IND_GAP: f64 = 8.0;
/// From the ellipse's edge to the near side of its indicators.
pub const IND_DIST: f64 = 80.0;
/// Auto-layout's column and row steps.
const COL_STEP: f64 = 320.0;
const ROW_STEP: f64 = 240.0;
const FIRST_X: f64 = 280.0;
const FIRST_Y: f64 = 190.0;
/// Undo steps kept.
const MAX_UNDO: usize = 200;

/// Where a construct's indicators are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
}

/// A rectangle (top-left corner, size).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, p: (f64, f64)) -> bool {
        p.0 >= self.x && p.0 <= self.x + self.w && p.1 >= self.y && p.1 <= self.y + self.h
    }

    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// What is under a point of the canvas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hit {
    Construct(String),
    Indicator(String),
    Path(String, String),
}

/// Why an edit was refused (the UI words them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    EmptyName,
    NameTaken(String),
    NoConstruct(String),
    NoIndicators,
    SelfPath,
    PathExists,
    /// The path would close a cycle (PLS models are recursive).
    Cycle,
}

/// The model being edited, with its undo history.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagram {
    pub model: Model,
    undo: Vec<Model>,
    redo: Vec<Model>,
}

impl Diagram {
    /// An empty model named `name` on the data at `sheet`!`range`, with
    /// SmartPLS's settings (5000 subsamples kept for Bootstrap).
    pub fn new(name: &str, sheet: &str, range: &str) -> Self {
        let model = Model {
            options: Some(Options {
                bootstrap: Some(Bootstrap::default()),
                ..Default::default()
            }),
            layout: Some(Layout {
                name: Some(name.to_owned()),
                sheet: Some(sheet.to_owned()),
                range: Some(range.to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        };
        Self::from_model(model)
    }

    pub fn from_model(mut model: Model) -> Self {
        model.layout.get_or_insert_with(Layout::default);
        model.options.get_or_insert_with(Options::default);
        let mut d = Self {
            model,
            undo: Vec::new(),
            redo: Vec::new(),
        };
        // Constructs without a place (a model written by hand) are laid
        // out; ones placed are kept.
        let missing = d
            .model
            .constructs
            .iter()
            .any(|c| !d.layout().positions.contains_key(&c.name));
        if missing {
            if d.layout().positions.is_empty() {
                d.layout_now();
            } else {
                let names: Vec<String> = d
                    .model
                    .constructs
                    .iter()
                    .map(|c| c.name.clone())
                    .filter(|n| !d.layout().positions.contains_key(n))
                    .collect();
                for n in names {
                    let at = d.free_spot();
                    d.layout_mut().positions.insert(n, [at.0, at.1]);
                }
            }
        }
        d
    }

    /// A model from pls-json text (a saved `.json`, or the workbook's part).
    pub fn from_json(text: &str) -> Result<Self, String> {
        let model: Model = serde_json::from_str(text).map_err(|e| e.to_string())?;
        Ok(Self::from_model(model))
    }

    /// The model as pls-json, with the layout (runs with `kantor pls`).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.model).unwrap_or_default()
    }

    pub fn layout(&self) -> &Layout {
        static EMPTY: std::sync::OnceLock<Layout> = std::sync::OnceLock::new();
        self.model
            .layout
            .as_ref()
            .unwrap_or_else(|| EMPTY.get_or_init(Layout::default))
    }

    fn layout_mut(&mut self) -> &mut Layout {
        self.model.layout.get_or_insert_with(Layout::default)
    }

    pub fn name(&self) -> &str {
        self.layout().name.as_deref().unwrap_or("PLS-SEM")
    }

    pub fn set_name(&mut self, name: &str) {
        self.layout_mut().name = Some(name.trim().to_owned());
    }

    /// The data's sheet and range, as saved.
    pub fn data_ref(&self) -> (Option<&str>, Option<&str>) {
        (
            self.layout().sheet.as_deref(),
            self.layout().range.as_deref(),
        )
    }

    pub fn set_data_ref(&mut self, sheet: &str, range: &str) {
        let l = self.layout_mut();
        l.sheet = Some(sheet.to_owned());
        l.range = Some(range.to_owned());
    }

    pub fn options(&self) -> Options {
        self.model.options.clone().unwrap_or_default()
    }

    /// Change the settings (not an undo step, as in SmartPLS's dialog).
    pub fn set_options(&mut self, o: Options) {
        self.model.options = Some(o);
    }

    pub fn one_tailed(&self) -> bool {
        self.layout().one_tailed
    }

    pub fn set_one_tailed(&mut self, on: bool) {
        self.layout_mut().one_tailed = on;
    }

    // ------------------------------------------------------------ undo

    /// Record the model before an edit (one undo step).
    pub fn checkpoint(&mut self) {
        self.undo.push(self.model.clone());
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Drop the last checkpoint (the edit it was for changed nothing).
    fn forget_checkpoint(&mut self) {
        self.undo.pop();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        let now = std::mem::replace(&mut self.model, prev);
        self.redo.push(now);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        let now = std::mem::replace(&mut self.model, next);
        self.undo.push(now);
        true
    }

    // ------------------------------------------------------- queries

    pub fn construct(&self, name: &str) -> Option<&Construct> {
        self.model.constructs.iter().find(|c| c.name == name)
    }

    fn construct_mut(&mut self, name: &str) -> Option<&mut Construct> {
        self.model.constructs.iter_mut().find(|c| c.name == name)
    }

    /// The construct an indicator belongs to.
    pub fn construct_of(&self, indicator: &str) -> Option<&str> {
        self.model
            .constructs
            .iter()
            .find(|c| c.indicators.iter().any(|i| i == indicator))
            .map(|c| c.name.as_str())
    }

    pub fn has_path(&self, from: &str, to: &str) -> bool {
        self.model
            .paths
            .iter()
            .any(|p| p.from == from && p.to == to)
    }

    /// Whether `to` can be reached from `from` along paths.
    pub fn reaches(&self, from: &str, to: &str) -> bool {
        let mut seen = HashSet::new();
        let mut stack = vec![from.to_owned()];
        while let Some(n) = stack.pop() {
            if n == to {
                return true;
            }
            if !seen.insert(n.clone()) {
                continue;
            }
            for p in &self.model.paths {
                if p.from == n {
                    stack.push(p.to.clone());
                }
            }
        }
        false
    }

    /// The centre of a construct's ellipse.
    pub fn position(&self, name: &str) -> (f64, f64) {
        match self.layout().positions.get(name) {
            Some(p) => (p[0], p[1]),
            None => (FIRST_X, FIRST_Y),
        }
    }

    /// Where a construct's indicators go: left of a construct nothing
    /// points to, right of one that points nowhere, above one in between.
    pub fn side(&self, name: &str) -> Side {
        let into = self.model.paths.iter().any(|p| p.to == name);
        let out = self.model.paths.iter().any(|p| p.from == name);
        match (into, out) {
            (false, _) => Side::Left,
            (true, false) => Side::Right,
            (true, true) => Side::Top,
        }
    }

    /// A construct's indicators and their boxes, in order.
    pub fn indicator_boxes(&self, c: &Construct) -> Vec<(String, Rect)> {
        let (cx, cy) = self.position(&c.name);
        let n = c.indicators.len() as f64;
        let side = self.side(&c.name);
        c.indicators
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let i = i as f64;
                let rect = match side {
                    Side::Left | Side::Right => {
                        let total = n * IND_H + (n - 1.0).max(0.0) * IND_GAP;
                        let y = cy - total / 2.0 + i * (IND_H + IND_GAP);
                        let x = if side == Side::Left {
                            cx - RX - IND_DIST - IND_W
                        } else {
                            cx + RX + IND_DIST
                        };
                        Rect {
                            x,
                            y,
                            w: IND_W,
                            h: IND_H,
                        }
                    }
                    Side::Top => {
                        let total = n * IND_W + (n - 1.0).max(0.0) * IND_GAP;
                        Rect {
                            x: cx - total / 2.0 + i * (IND_W + IND_GAP),
                            y: cy - RY - IND_DIST * 0.75 - IND_H,
                            w: IND_W,
                            h: IND_H,
                        }
                    }
                };
                (name.clone(), rect)
            })
            .collect()
    }

    /// The line between an indicator's box and its construct's ellipse:
    /// `(box end, ellipse end)`.
    pub fn indicator_link(&self, c: &Construct, rect: &Rect) -> ((f64, f64), (f64, f64)) {
        let center = self.position(&c.name);
        let anchor = match self.side(&c.name) {
            Side::Left => (rect.x + rect.w, rect.y + rect.h / 2.0),
            Side::Right => (rect.x, rect.y + rect.h / 2.0),
            Side::Top => (rect.x + rect.w / 2.0, rect.y + rect.h),
        };
        (anchor, ellipse_point(center, anchor))
    }

    /// A path's arrow, from edge to edge of the two ellipses.
    pub fn path_line(&self, from: &str, to: &str) -> ((f64, f64), (f64, f64)) {
        let a = self.position(from);
        let b = self.position(to);
        (ellipse_point(a, b), ellipse_point(b, a))
    }

    /// What is at `p` (constructs over indicators over paths).
    pub fn hit(&self, p: (f64, f64)) -> Option<Hit> {
        for c in self.model.constructs.iter().rev() {
            let (cx, cy) = self.position(&c.name);
            let (dx, dy) = ((p.0 - cx) / RX, (p.1 - cy) / RY);
            if dx * dx + dy * dy <= 1.0 {
                return Some(Hit::Construct(c.name.clone()));
            }
        }
        for c in &self.model.constructs {
            for (name, rect) in self.indicator_boxes(c) {
                if rect.contains(p) {
                    return Some(Hit::Indicator(name));
                }
            }
        }
        let mut best: Option<(f64, &Path)> = None;
        for path in &self.model.paths {
            let (a, b) = self.path_line(&path.from, &path.to);
            let d = segment_distance(p, a, b);
            if d <= 6.0 && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, path));
            }
        }
        best.map(|(_, path)| Hit::Path(path.from.clone(), path.to.clone()))
    }

    /// The diagram's extent (for the canvas's scroll area).
    pub fn extent(&self) -> (f64, f64) {
        let mut w: f64 = 0.0;
        let mut h: f64 = 0.0;
        for c in &self.model.constructs {
            let (x, y) = self.position(&c.name);
            w = w.max(x + RX);
            h = h.max(y + RY);
            for (_, r) in self.indicator_boxes(c) {
                w = w.max(r.x + r.w);
                h = h.max(r.y + r.h);
            }
        }
        (w + 80.0, h + 80.0)
    }

    /// A place for a new construct, away from the others.
    pub fn free_spot(&self) -> (f64, f64) {
        let taken: Vec<(f64, f64)> = self
            .model
            .constructs
            .iter()
            .filter(|c| self.layout().positions.contains_key(&c.name))
            .map(|c| self.position(&c.name))
            .collect();
        for row in 0..50 {
            for col in 0..4 {
                let at = (
                    FIRST_X + col as f64 * COL_STEP,
                    FIRST_Y + row as f64 * ROW_STEP,
                );
                if taken.iter().all(|t| {
                    (t.0 - at.0).abs() > COL_STEP / 2.0 || (t.1 - at.1).abs() > ROW_STEP / 2.0
                }) {
                    return at;
                }
            }
        }
        (FIRST_X, FIRST_Y)
    }

    /// A name for a construct of `indicators`: their common prefix without
    /// trailing digits and separators (`CUSA1`, `CUSA2` → `CUSA`), else
    /// "Construct N"; unique.
    pub fn suggest_name(&self, indicators: &[String]) -> String {
        let mut prefix = indicators.first().cloned().unwrap_or_default();
        for i in indicators.iter().skip(1) {
            let n = prefix
                .chars()
                .zip(i.chars())
                .take_while(|(a, b)| a == b)
                .count();
            prefix = prefix.chars().take(n).collect();
        }
        if indicators.len() == 1 {
            prefix = indicators[0].clone();
        }
        let base = prefix
            .trim_end_matches(|c: char| {
                c.is_ascii_digit() || c == '_' || c == '.' || c == ' ' || c == '-'
            })
            .to_owned();
        let taken = |n: &str| {
            self.model
                .constructs
                .iter()
                .any(|c| c.name.eq_ignore_ascii_case(n))
        };
        if !base.is_empty() && !taken(&base) {
            return base;
        }
        let stem = if base.is_empty() {
            "Construct".to_owned()
        } else {
            base
        };
        (1..)
            .map(|k| format!("{stem} {k}"))
            .find(|n| !taken(n))
            .unwrap_or(stem)
    }

    // --------------------------------------------------------- edits

    /// A new reflective construct of `indicators` (taken from the
    /// constructs they were in) at `at` (or a free spot). Returns its name.
    pub fn create_construct(
        &mut self,
        indicators: &[String],
        at: Option<(f64, f64)>,
    ) -> Result<String, EditError> {
        if indicators.is_empty() {
            return Err(EditError::NoIndicators);
        }
        self.checkpoint();
        let name = self.suggest_name(indicators);
        let at = at.unwrap_or_else(|| self.free_spot());
        self.take_indicators(indicators);
        self.model.constructs.push(Construct {
            name: name.clone(),
            indicators: indicators.to_vec(),
            ..Default::default()
        });
        self.layout_mut()
            .positions
            .insert(name.clone(), [at.0, at.1]);
        Ok(name)
    }

    fn take_indicators(&mut self, indicators: &[String]) {
        for c in &mut self.model.constructs {
            c.indicators.retain(|i| !indicators.contains(i));
        }
    }

    /// Move `indicators` to construct `name` (from wherever they were).
    pub fn add_indicators(&mut self, name: &str, indicators: &[String]) -> Result<(), EditError> {
        if self.construct(name).is_none() {
            return Err(EditError::NoConstruct(name.to_owned()));
        }
        if indicators.is_empty() {
            return Err(EditError::NoIndicators);
        }
        self.checkpoint();
        self.take_indicators(indicators);
        if let Some(c) = self.construct_mut(name) {
            c.indicators.extend(indicators.iter().cloned());
        }
        Ok(())
    }

    /// Take an indicator out of its construct.
    pub fn remove_indicator(&mut self, indicator: &str) -> bool {
        if self.construct_of(indicator).is_none() {
            return false;
        }
        self.checkpoint();
        self.take_indicators(&[indicator.to_owned()]);
        true
    }

    /// Rename a construct (its paths and place follow).
    pub fn rename(&mut self, old: &str, new: &str) -> Result<(), EditError> {
        let new = new.trim();
        if new.is_empty() {
            return Err(EditError::EmptyName);
        }
        if self.construct(old).is_none() {
            return Err(EditError::NoConstruct(old.to_owned()));
        }
        if new == old {
            return Ok(());
        }
        if self
            .model
            .constructs
            .iter()
            .any(|c| c.name != old && c.name.eq_ignore_ascii_case(new))
        {
            return Err(EditError::NameTaken(new.to_owned()));
        }
        self.checkpoint();
        if let Some(c) = self.construct_mut(old) {
            c.name = new.to_owned();
        }
        for p in &mut self.model.paths {
            if p.from == old {
                p.from = new.to_owned();
            }
            if p.to == old {
                p.to = new.to_owned();
            }
        }
        let l = self.layout_mut();
        if let Some(at) = l.positions.remove(old) {
            l.positions.insert(new.to_owned(), at);
        }
        Ok(())
    }

    /// Reflective (Mode A) or formative (a composite, Mode B).
    pub fn set_measurement(&mut self, name: &str, m: Measurement) -> bool {
        match self.construct(name) {
            Some(c) if c.measurement != m || c.weights.is_some() => {}
            _ => return false,
        }
        self.checkpoint();
        if let Some(c) = self.construct_mut(name) {
            c.measurement = m;
            c.weights = None;
        }
        true
    }

    /// Delete a construct and its paths (its indicators go back to the
    /// list).
    pub fn delete_construct(&mut self, name: &str) -> bool {
        if self.construct(name).is_none() {
            return false;
        }
        self.checkpoint();
        self.model.constructs.retain(|c| c.name != name);
        self.model.paths.retain(|p| p.from != name && p.to != name);
        self.layout_mut().positions.remove(name);
        true
    }

    pub fn add_path(&mut self, from: &str, to: &str) -> Result<(), EditError> {
        for n in [from, to] {
            if self.construct(n).is_none() {
                return Err(EditError::NoConstruct(n.to_owned()));
            }
        }
        if from == to {
            return Err(EditError::SelfPath);
        }
        if self.has_path(from, to) {
            return Err(EditError::PathExists);
        }
        if self.reaches(to, from) {
            return Err(EditError::Cycle);
        }
        self.checkpoint();
        self.model.paths.push(Path::new(from, to));
        Ok(())
    }

    pub fn remove_path(&mut self, from: &str, to: &str) -> bool {
        if !self.has_path(from, to) {
            return false;
        }
        self.checkpoint();
        self.model.paths.retain(|p| !(p.from == from && p.to == to));
        true
    }

    /// Put a construct's centre at `at` (no undo step: a drag records one
    /// when it starts).
    pub fn move_construct(&mut self, name: &str, at: (f64, f64)) {
        if self.construct(name).is_some() {
            self.layout_mut()
                .positions
                .insert(name.to_owned(), [at.0.max(0.0), at.1.max(0.0)]);
        }
    }

    /// Lay the constructs out left to right by path depth (one undo
    /// step). Returns whether anything moved.
    pub fn auto_layout(&mut self) -> bool {
        self.checkpoint();
        let before = self.layout().positions.clone();
        self.layout_now();
        if self.layout().positions == before {
            self.forget_checkpoint();
            return false;
        }
        true
    }

    fn layout_now(&mut self) {
        let depth = self.depths();
        let max_depth = depth.values().copied().max().unwrap_or(0);
        let mut columns: Vec<Vec<String>> = vec![Vec::new(); max_depth + 1];
        for c in &self.model.constructs {
            columns[depth[&c.name]].push(c.name.clone());
        }
        let rows = columns.iter().map(Vec::len).max().unwrap_or(0);
        let mut positions = BTreeMap::new();
        for (d, col) in columns.iter().enumerate() {
            let offset = (rows - col.len()) as f64 * ROW_STEP / 2.0;
            for (i, name) in col.iter().enumerate() {
                positions.insert(
                    name.clone(),
                    [
                        FIRST_X + d as f64 * COL_STEP,
                        FIRST_Y + offset + i as f64 * ROW_STEP,
                    ],
                );
            }
        }
        self.layout_mut().positions = positions;
    }

    /// Each construct's depth: 0 when nothing points to it, else one more
    /// than its deepest predecessor's (the model is recursive; a cycle
    /// stops at the number of constructs).
    pub fn depths(&self) -> BTreeMap<String, usize> {
        let mut depth: BTreeMap<String, usize> = self
            .model
            .constructs
            .iter()
            .map(|c| (c.name.clone(), 0))
            .collect();
        let k = self.model.constructs.len();
        for _ in 0..k {
            let mut changed = false;
            for p in &self.model.paths {
                let (Some(&f), Some(&t)) = (depth.get(&p.from), depth.get(&p.to)) else {
                    continue;
                };
                if t <= f && f < k {
                    depth.insert(p.to.clone(), f + 1);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        depth
    }
}

/// The point where the line from `center` toward `toward` leaves the
/// ellipse (radii [`RX`], [`RY`]).
pub fn ellipse_point(center: (f64, f64), toward: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = (toward.0 - center.0, toward.1 - center.1);
    let len = ((dx / RX).powi(2) + (dy / RY).powi(2)).sqrt();
    if len <= 1e-9 {
        return (center.0 + RX, center.1);
    }
    (center.0 + dx / len, center.1 + dy / len)
}

/// The distance from `p` to the segment `a`–`b`.
pub fn segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (vx, vy) = (b.0 - a.0, b.1 - a.1);
    let len2 = vx * vx + vy * vy;
    let t = if len2 <= 1e-12 {
        0.0
    } else {
        (((p.0 - a.0) * vx + (p.1 - a.1) * vy) / len2).clamp(0.0, 1.0)
    };
    let (qx, qy) = (a.0 + t * vx, a.1 + t * vy);
    ((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt()
}

/// The two back corners of an arrowhead at `tip` pointing away from
/// `from`, `len` long.
pub fn arrow_head(from: (f64, f64), tip: (f64, f64), len: f64) -> [(f64, f64); 2] {
    let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
    let d = (dx * dx + dy * dy).sqrt().max(1e-9);
    let (ux, uy) = (dx / d, dy / d);
    let (bx, by) = (tip.0 - ux * len, tip.1 - uy * len);
    let w = len * 0.45;
    [(bx - uy * w, by + ux * w), (bx + uy * w, by - ux * w)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    fn three() -> Diagram {
        let mut d = Diagram::new("Model 1", "Data", "A1:H101");
        d.create_construct(&s(&["IMAG1", "IMAG2"]), None).unwrap();
        d.create_construct(&s(&["CUSA1", "CUSA2", "CUSA3"]), None)
            .unwrap();
        d.create_construct(&s(&["CUSL1", "CUSL2"]), None).unwrap();
        d
    }

    #[std::prelude::v1::test]
    fn constructs_are_named_from_their_indicators() {
        let d = three();
        let names: Vec<&str> = d.model.constructs.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["IMAG", "CUSA", "CUSL"]);
        assert_eq!(d.suggest_name(&s(&["IMAG3"])), "IMAG 1");
        assert_eq!(d.suggest_name(&s(&["a", "b"])), "Construct 1");
        assert_eq!(d.suggest_name(&s(&["q_1", "q_2"])), "q");
        // Placed apart.
        let a = d.position("IMAG");
        let b = d.position("CUSA");
        assert!((a.0 - b.0).abs() > 100.0 || (a.1 - b.1).abs() > 100.0);
    }

    #[std::prelude::v1::test]
    fn indicators_move_between_constructs() {
        let mut d = three();
        d.add_indicators("IMAG", &s(&["CUSA3"])).unwrap();
        assert_eq!(
            d.construct("CUSA").unwrap().indicators,
            s(&["CUSA1", "CUSA2"])
        );
        assert_eq!(d.construct_of("CUSA3"), Some("IMAG"));
        assert!(d.remove_indicator("CUSA3"));
        assert_eq!(d.construct_of("CUSA3"), None);
        assert!(!d.remove_indicator("CUSA3"));
        assert_eq!(
            d.add_indicators("NOPE", &s(&["x"])),
            Err(EditError::NoConstruct("NOPE".into()))
        );
        assert_eq!(d.create_construct(&[], None), Err(EditError::NoIndicators));
    }

    #[std::prelude::v1::test]
    fn paths_refuse_loops_and_follow_renames() {
        let mut d = three();
        d.add_path("IMAG", "CUSA").unwrap();
        d.add_path("CUSA", "CUSL").unwrap();
        assert_eq!(d.add_path("CUSL", "IMAG"), Err(EditError::Cycle));
        assert_eq!(d.add_path("IMAG", "IMAG"), Err(EditError::SelfPath));
        assert_eq!(d.add_path("IMAG", "CUSA"), Err(EditError::PathExists));
        d.add_path("IMAG", "CUSL").unwrap();
        assert_eq!(
            d.rename("CUSA", "imag"),
            Err(EditError::NameTaken("imag".into()))
        );
        assert_eq!(d.rename("CUSA", "  "), Err(EditError::EmptyName));
        let at = d.position("CUSA");
        d.rename("CUSA", "Satisfaction").unwrap();
        assert!(d.has_path("IMAG", "Satisfaction"));
        assert!(d.has_path("Satisfaction", "CUSL"));
        assert_eq!(d.position("Satisfaction"), at);
        assert!(d.remove_path("IMAG", "CUSL"));
        assert!(!d.remove_path("IMAG", "CUSL"));
        assert!(d.delete_construct("Satisfaction"));
        assert!(d.model.paths.is_empty());
        assert!(!d.layout().positions.contains_key("Satisfaction"));
    }

    #[std::prelude::v1::test]
    fn undo_and_redo_every_edit() {
        let mut d = three();
        let start = d.model.clone();
        d.add_path("IMAG", "CUSA").unwrap();
        assert!(d.set_measurement("IMAG", Measurement::Composite));
        assert!(!d.set_measurement("IMAG", Measurement::Composite));
        d.rename("CUSL", "LOY").unwrap();
        assert!(d.undo());
        assert!(d.construct("CUSL").is_some());
        assert!(d.undo());
        assert_eq!(
            d.construct("IMAG").unwrap().measurement,
            Measurement::Reflective
        );
        assert!(d.undo());
        assert_eq!(d.model, start);
        assert!(d.redo() && d.redo() && d.redo());
        assert!(d.construct("LOY").is_some());
        assert!(!d.redo());
        // A new edit drops the redo steps.
        d.undo();
        d.delete_construct("IMAG");
        assert!(!d.can_redo());
    }

    #[std::prelude::v1::test]
    fn auto_layout_goes_left_to_right_by_depth() {
        let mut d = three();
        d.create_construct(&s(&["QUAL1", "QUAL2"]), None).unwrap();
        d.add_path("IMAG", "CUSA").unwrap();
        d.add_path("QUAL", "CUSA").unwrap();
        d.add_path("CUSA", "CUSL").unwrap();
        d.add_path("IMAG", "CUSL").unwrap();
        // Scatter, then lay out.
        d.move_construct("CUSL", (5.0, 5.0));
        assert!(d.auto_layout());
        let x = |n: &str| d.position(n).0;
        let y = |n: &str| d.position(n).1;
        assert_eq!(x("IMAG"), x("QUAL"));
        assert!(x("IMAG") < x("CUSA") && x("CUSA") < x("CUSL"));
        assert_ne!(y("IMAG"), y("QUAL"));
        // CUSA is centred against its column's neighbours.
        assert_eq!(y("CUSA"), (y("IMAG") + y("QUAL")) / 2.0);
        // Laying out again changes nothing (and isn't an undo step).
        let steps = d.undo.len();
        assert!(!d.auto_layout());
        assert_eq!(d.undo.len(), steps);
        assert!(d.undo());
        assert_eq!(d.position("CUSL"), (5.0, 5.0));
        // Indicators: left of sources, above middles, right of ends.
        assert_eq!(d.side("IMAG"), Side::Left);
        assert_eq!(d.side("CUSA"), Side::Top);
        assert_eq!(d.side("CUSL"), Side::Right);
    }

    #[std::prelude::v1::test]
    fn geometry_and_hits() {
        let mut d = three();
        d.add_path("IMAG", "CUSA").unwrap();
        d.move_construct("IMAG", (300.0, 200.0));
        d.move_construct("CUSA", (700.0, 200.0));
        let ((x1, y1), (x2, y2)) = d.path_line("IMAG", "CUSA");
        assert_eq!((x1, y1), (300.0 + RX, 200.0));
        assert_eq!((x2, y2), (700.0 - RX, 200.0));
        assert_eq!(d.hit((300.0, 200.0)), Some(Hit::Construct("IMAG".into())));
        assert_eq!(
            d.hit((500.0, 203.0)),
            Some(Hit::Path("IMAG".into(), "CUSA".into()))
        );
        assert_eq!(d.hit((500.0, 260.0)), None);
        let c = d.construct("IMAG").unwrap().clone();
        let boxes = d.indicator_boxes(&c);
        assert_eq!(boxes.len(), 2);
        // Left of IMAG, stacked and centred on it.
        assert!(boxes[0].1.x + IND_W < 300.0 - RX);
        assert_eq!((boxes[0].1.y + boxes[1].1.y + IND_H) / 2.0, 200.0);
        assert_eq!(
            d.hit(boxes[1].1.center()),
            Some(Hit::Indicator("IMAG2".into()))
        );
        let (a, b) = d.indicator_link(&c, &boxes[0].1);
        assert_eq!(a.0, boxes[0].1.x + IND_W);
        assert!(((b.0 - 300.0) / RX).powi(2) + ((b.1 - 200.0) / RY).powi(2) - 1.0 < 1e-9);
        let [l, r] = arrow_head((0.0, 0.0), (10.0, 0.0), 10.0);
        assert_eq!(l.0, 0.0);
        assert_eq!(r.0, 0.0);
        assert!(l.1 > 0.0 && r.1 < 0.0);
        let (w, h) = d.extent();
        assert!(w > 700.0 + RX && h > 200.0);
    }

    #[std::prelude::v1::test]
    fn json_round_trip_keeps_the_layout() {
        let mut d = three();
        d.add_path("IMAG", "CUSA").unwrap();
        d.set_measurement("CUSL", Measurement::Composite);
        d.set_one_tailed(true);
        d.move_construct("IMAG", (111.0, 222.0));
        let text = d.to_json();
        let back = Diagram::from_json(&text).unwrap();
        assert_eq!(back.model, d.model);
        assert_eq!(back.position("IMAG"), (111.0, 222.0));
        assert!(back.one_tailed());
        assert_eq!(back.data_ref(), (Some("Data"), Some("A1:H101")));
        assert_eq!(back.name(), "Model 1");
        assert_eq!(back.options().bootstrap.unwrap().samples, 5000);
        // A model written by hand (no layout) is laid out on opening.
        let bare = r#"{"constructs":[{"name":"X","indicators":["x1"]},{"name":"Y","indicators":["y1"]}],"paths":[["X","Y"]]}"#;
        let d = Diagram::from_json(bare).unwrap();
        assert!(d.position("X").0 < d.position("Y").0);
        assert!(Diagram::from_json("{\"constructs\": 3}").is_err());
    }
}
