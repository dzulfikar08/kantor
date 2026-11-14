//! Notes (Excel's cell comments) of imported workbooks.
//!
//! IronCalc keeps a sheet's notes in its model (`Worksheet::comments`, read
//! from an `.xlsx` or brought in from an `.xls`) but doesn't write them. An
//! `.xlsx`'s own notes are carried into saves with their parts (`preserve`);
//! an import has no package to carry them from, so a save writes them from
//! the model: a comments part and the VML boxes Excel needs to show them,
//! at the cells they moved to with inserted and deleted rows and columns.

use crate::Workbook;
use crate::shift::parse_range;

/// A note on a cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellNote {
    /// 0-based, where the note was in the opened file.
    pub row: u32,
    pub col: u32,
    pub author: String,
    pub text: String,
}

/// A note to write, at its current cell.
pub(crate) struct NoteSave {
    pub row: u32,
    pub col: u32,
    pub author: String,
    pub text: String,
}

impl Workbook {
    /// The notes of `sheet` (an index), as the opened file had them.
    pub fn notes(&self, sheet: u32) -> Vec<CellNote> {
        let Ok(ws) = self.model.get_model().workbook.worksheet(sheet) else {
            return Vec::new();
        };
        ws.comments
            .iter()
            .filter_map(|c| {
                let r = parse_range(&c.cell_ref)?.area();
                Some(CellNote {
                    row: r.top,
                    col: r.left,
                    author: c.author_name.clone(),
                    text: c.text.clone(),
                })
            })
            .collect()
    }

    /// Notes a save writes from the model: an import's (an `.xlsx`'s are
    /// carried with its parts), moved to where their cells are now.
    pub(crate) fn note_patches(&self, out: &mut crate::preserve::Patches) {
        if self.origin.xlsx.is_some() {
            return;
        }
        let moves = self.origin.xlsx_layout.moves_to(&self.history.layout());
        for sheet in 0..self.sheet_names().len() as u32 {
            let id = self.sheet_id(sheet);
            let mut list: Vec<NoteSave> = self
                .notes(sheet)
                .into_iter()
                .filter_map(|n| {
                    let (row, col) = moves.cell(id, n.row, n.col)?;
                    Some(NoteSave {
                        row,
                        col,
                        author: n.author,
                        text: n.text,
                    })
                })
                .collect();
            list.sort_by_key(|n| (n.row, n.col));
            list.dedup_by_key(|n| (n.row, n.col));
            if !list.is_empty() {
                out.notes.push((id, list));
            }
        }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The `comments` part of `notes`.
pub(crate) fn comments_xml(notes: &[NoteSave]) -> String {
    let mut authors: Vec<&str> = Vec::new();
    for n in notes {
        if !authors.contains(&n.author.as_str()) {
            authors.push(&n.author);
        }
    }
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<comments xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><authors>",
    );
    for a in &authors {
        out.push_str(&format!("<author>{}</author>", esc(a)));
    }
    out.push_str("</authors><commentList>");
    for n in notes {
        let author = authors.iter().position(|a| *a == n.author).unwrap_or(0);
        out.push_str(&format!(
            "<comment ref=\"{}\" authorId=\"{author}\"><text><t xml:space=\"preserve\">{}</t></text></comment>",
            crate::cell_name(n.row, n.col),
            esc(&n.text)
        ));
    }
    out.push_str("</commentList></comments>");
    out
}

/// The VML drawing with a hidden note box per note (Excel shows a note from
/// its box; without one it drops the note).
/// `block` numbers the drawing's shape ids (1024 per block), unique in the
/// workbook.
pub(crate) fn vml_xml(notes: &[NoteSave], block: usize) -> String {
    let mut out = format!(
        "<xml xmlns:v=\"urn:schemas-microsoft-com:vml\" xmlns:o=\"urn:schemas-microsoft-com:office:office\" xmlns:x=\"urn:schemas-microsoft-com:office:excel\">\
<o:shapelayout v:ext=\"edit\"><o:idmap v:ext=\"edit\" data=\"{block}\"/></o:shapelayout>\
<v:shapetype id=\"_x0000_t202\" coordsize=\"21600,21600\" o:spt=\"202\" path=\"m,l,21600r21600,l21600,xe\">\
<v:stroke joinstyle=\"miter\"/><v:path gradientshapeok=\"t\" o:connecttype=\"rect\"/></v:shapetype>",
    );
    for (i, n) in notes.iter().enumerate() {
        // The box: right of the cell, 2 columns wide and 4 rows high.
        let anchor = format!(
            "{}, 15, {}, 10, {}, 15, {}, 4",
            n.col + 1,
            n.row.saturating_sub(1),
            n.col + 3,
            n.row + 3
        );
        out.push_str(&format!(
            "<v:shape id=\"_x0000_s{}\" type=\"#_x0000_t202\" style=\"position:absolute;margin-left:59.25pt;margin-top:1.5pt;width:108pt;height:59.25pt;z-index:{};visibility:hidden\" fillcolor=\"#ffffe1\" o:insetmode=\"auto\">\
<v:fill color2=\"#ffffe1\"/><v:shadow on=\"t\" color=\"black\" obscured=\"t\"/><v:path o:connecttype=\"none\"/>\
<v:textbox style=\"mso-direction-alt:auto\"><div style=\"text-align:left\"></div></v:textbox>\
<x:ClientData ObjectType=\"Note\"><x:MoveWithCells/><x:SizeWithCells/><x:Anchor>{anchor}</x:Anchor><x:AutoFill>False</x:AutoFill><x:Row>{}</x:Row><x:Column>{}</x:Column></x:ClientData></v:shape>",
            1024 * block + 1 + i,
            i + 1,
            n.row,
            n.col
        ));
    }
    out.push_str("</xml>");
    out
}
