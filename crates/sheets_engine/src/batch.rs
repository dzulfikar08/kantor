//! Several edits as one undo step (the MCP server's tool calls).

use anyhow::Result;

use crate::Workbook;

impl Workbook {
    /// Run `f`, then fold every undo entry it recorded into one, so the
    /// whole batch undoes and redoes as a single step. Entries recorded
    /// before an error stay (as one step), like [`Workbook::set_inputs`].
    pub fn batch<R>(&mut self, f: impl FnOnce(&mut Self) -> Result<R>) -> Result<R> {
        let depth = self.history.undo.len();
        let result = f(self);
        let h = &mut self.history;
        if h.undo.len() > depth + 1 {
            let steps = h.undo[depth..].iter().map(|e| e.1).sum();
            let first = h.undo[depth].0;
            h.undo.truncate(depth);
            h.undo.push((first, steps));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::{CellRange, StyleChange, Workbook};

    #[test]
    fn a_batch_undoes_as_one_step() {
        let mut book = Workbook::new_empty().unwrap();
        book.set_input(0, 0, 0, "1").unwrap();
        let range = CellRange::from_corners((0, 0), (1, 1));
        book.batch(|b| {
            b.set_input(0, 1, 1, "x")?;
            b.set_range_style(0, range, StyleChange::Bold(true))?;
            b.set_range_style(0, range, StyleChange::Italic(true))
        })
        .unwrap();
        assert!(book.cell_style(0, 0, 0).bold);
        assert!(book.undo().unwrap());
        assert_eq!(book.input(0, 1, 1), "");
        assert!(!book.cell_style(0, 0, 0).bold);
        assert_eq!(book.input(0, 0, 0), "1");
        assert!(book.redo().unwrap());
        assert_eq!(book.input(0, 1, 1), "x");
        assert!(book.cell_style(0, 0, 0).italic);
    }
}
