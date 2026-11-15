//! KANTOR PATCH (sheet tabs): inserting, deleting and copying sheets as
//! Excel does. See KANTOR_PATCHES.md §23.
//!
//! Upstream's `delete_sheet` left formulas and defined names pointing at
//! the deleted sheet's name (a sheet added later under that name picked
//! them up again), kept the sheet's local names (the writer then panicked
//! looking up their `localSheetId`) and its tables, and its undo put back
//! only part of the sheet (no notes, conditional formats or links). Here a
//! deletion turns references to the sheet into `#REF!`, drops its local
//! names and tables, and undo puts everything back as it was.

use crate::{
    expressions::{
        lexer::LexerMode,
        parser::{stringify::to_english_string, stringify::to_rc_format, Node},
        token::Error,
        types::CellReferenceRC,
    },
    model::Model,
    types::{DefinedName, Table, Worksheet},
    user_model::{history::Diff, UserModel},
};

/// What a deletion changed, to put back on undo.
#[derive(Clone, bitcode::Encode, bitcode::Decode)]
pub(crate) struct DeletedSheet {
    pub(crate) index: u32,
    pub(crate) worksheet: Box<Worksheet>,
    /// Other sheets' shared formulas before, by `sheet_id`.
    pub(crate) formulas: Vec<(u32, Vec<String>)>,
    pub(crate) defined_names: Vec<DefinedName>,
    pub(crate) tables: Vec<(String, Table)>,
}

/// Replace references to sheet `index` (written with a sheet name) by
/// `#REF!`. Returns whether anything changed.
fn cut_sheet_refs(node: &mut Node, index: u32) -> bool {
    match node {
        Node::ReferenceKind {
            sheet_name: Some(_),
            sheet_index,
            ..
        }
        | Node::RangeKind {
            sheet_name: Some(_),
            sheet_index,
            ..
        } if *sheet_index == index => {
            *node = Node::ErrorKind(Error::REF);
            true
        }
        Node::OpRangeKind { left, right }
        | Node::OpConcatenateKind { left, right }
        | Node::OpSumKind { left, right, .. }
        | Node::OpProductKind { left, right, .. }
        | Node::OpPowerKind { left, right }
        | Node::CompareKind { left, right, .. } => {
            let a = cut_sheet_refs(left, index);
            cut_sheet_refs(right, index) || a
        }
        Node::FunctionKind { args, .. } | Node::NamedFunctionKind { args, .. } => args
            .iter_mut()
            .fold(false, |c, a| cut_sheet_refs(a, index) || c),
        Node::LambdaDefKind { body, .. } => cut_sheet_refs(body, index),
        Node::LambdaCallKind { lambda, args } => {
            let a = cut_sheet_refs(lambda, index);
            args.iter_mut()
                .fold(a, |c, arg| cut_sheet_refs(arg, index) || c)
        }
        Node::UnaryKind { right, .. } => cut_sheet_refs(right, index),
        Node::ImplicitIntersection { child, .. } | Node::SpillRangeOperator { child } => {
            cut_sheet_refs(child, index)
        }
        _ => false,
    }
}

impl Model<'_> {
    /// Delete sheet `index` as Excel does (see the module). Fails for the
    /// only sheet.
    pub(crate) fn kantor_delete_sheet(&mut self, index: u32) -> Result<DeletedSheet, String> {
        if self.workbook.worksheets.len() <= 1 {
            return Err("Cannot delete only sheet".to_string());
        }
        let worksheet = Box::new(self.workbook.worksheet(index)?.clone());
        let (name, sheet_id) = (worksheet.name.clone(), worksheet.sheet_id);
        let mut formulas = Vec::new();
        // Formulas are R1C1, parsed in their own sheet's context.
        self.parser.set_lexer_mode(LexerMode::R1C1);
        let mut rewritten: Vec<(usize, Vec<String>)> = Vec::new();
        for (i, ws) in self.workbook.worksheets.iter().enumerate() {
            if i as u32 == index {
                continue;
            }
            let context = CellReferenceRC {
                sheet: ws.get_name(),
                row: 1,
                column: 1,
            };
            let mut changed = false;
            let list: Vec<String> = ws
                .shared_formulas
                .iter()
                .map(|f| {
                    let mut node = self.parser.parse(f, &context);
                    if cut_sheet_refs(&mut node, index) {
                        changed = true;
                        to_rc_format(&node)
                    } else {
                        f.clone()
                    }
                })
                .collect();
            if changed {
                formulas.push((ws.sheet_id, ws.shared_formulas.clone()));
                rewritten.push((i, list));
            }
        }
        self.parser.set_lexer_mode(LexerMode::A1);
        for (i, list) in rewritten {
            self.workbook.worksheets[i].shared_formulas = list;
        }

        // Defined names: the sheet's own go, the others' references to it
        // become #REF!.
        let defined_names = self.workbook.defined_names.clone();
        let context = self.defined_name_context();
        let context = if context.sheet == name {
            // Parsed in the context of a sheet that stays.
            CellReferenceRC {
                sheet: self
                    .workbook
                    .worksheets
                    .iter()
                    .find(|w| w.sheet_id != sheet_id)
                    .map(|w| w.get_name())
                    .unwrap_or_default(),
                ..context
            }
        } else {
            context
        };
        let mut names = Vec::with_capacity(defined_names.len());
        for dn in &defined_names {
            if dn.sheet_id == Some(sheet_id) {
                continue;
            }
            let had_equals = dn.formula.trim_start().starts_with('=');
            let body = dn.formula.strip_prefix('=').unwrap_or(&dn.formula);
            let mut node = self.parse_internal_formula(body, &context);
            let formula = if cut_sheet_refs(&mut node, index) {
                let text = to_english_string(&node, &context);
                if had_equals {
                    format!("={text}")
                } else {
                    text
                }
            } else {
                dn.formula.clone()
            };
            names.push(DefinedName {
                formula,
                ..dn.clone()
            });
        }
        self.workbook.defined_names = names;

        let tables: Vec<(String, Table)> = self
            .workbook
            .tables
            .iter()
            .filter(|(_, t)| t.sheet_name == name)
            .map(|(k, t)| (k.clone(), t.clone()))
            .collect();
        for (k, _) in &tables {
            self.workbook.tables.remove(k);
        }
        self.workbook.worksheets.remove(index as usize);
        self.reset_parsed_structures();
        Ok(DeletedSheet {
            index,
            worksheet,
            formulas,
            defined_names,
            tables,
        })
    }

    /// Undo [`Model::kantor_delete_sheet`].
    pub(crate) fn kantor_restore_sheet(&mut self, d: &DeletedSheet) -> Result<(), String> {
        if d.index as usize > self.workbook.worksheets.len() {
            return Err("Sheet index out of range".to_string());
        }
        self.workbook
            .worksheets
            .insert(d.index as usize, (*d.worksheet).clone());
        for (id, list) in &d.formulas {
            if let Some(ws) = self
                .workbook
                .worksheets
                .iter_mut()
                .find(|w| w.sheet_id == *id)
            {
                ws.shared_formulas = list.clone();
            }
        }
        self.workbook.defined_names = d.defined_names.clone();
        for (k, t) in &d.tables {
            self.workbook.tables.insert(k.clone(), t.clone());
        }
        self.reset_parsed_structures();
        Ok(())
    }

    /// A copy of sheet `source` named `name`, at `index` (counted once the
    /// copy is in). Returns the copy's index.
    pub(crate) fn kantor_copy_sheet(
        &mut self,
        source: u32,
        name: &str,
        index: u32,
    ) -> Result<u32, String> {
        let (_, at) = self.duplicate_sheet_as(source, Some(name))?;
        if index != at {
            self.move_sheet(at, index)?;
        }
        Ok(index)
    }
}

impl UserModel<'_> {
    /// Delete sheet `sheet` as Excel does: references to it become `#REF!`,
    /// its local names and tables go (see the module). One history entry.
    pub fn kantor_delete_sheet(&mut self, sheet: u32) -> Result<(), String> {
        let count = self.model.workbook.worksheets.len() as u32;
        let deleted = self.model.kantor_delete_sheet(sheet)?;
        let sheet_id = deleted.worksheet.sheet_id;
        if self.view.sheet >= count - 1 || self.view.sheet > sheet {
            self.view.sheet = self.view.sheet.saturating_sub(1);
        }
        self.view.sheets.remove(&sheet_id);
        self.push_diff_list(vec![Diff::KantorDeleteSheet(Box::new(deleted))]);
        Ok(())
    }

    /// A new empty sheet named `name` at `index`, selected. One history
    /// entry.
    pub fn kantor_insert_sheet(&mut self, name: &str, index: u32) -> Result<(), String> {
        self.model.insert_sheet(name, index, None)?;
        self.set_selected_sheet(index)?;
        self.push_diff_list(vec![Diff::NewSheet {
            index,
            name: name.to_string(),
        }]);
        Ok(())
    }

    /// A copy of sheet `source` named `name` at `index` (counted once the
    /// copy is in), selected: cells, styles, merged cells, columns, rows,
    /// notes, conditional formats, links and the sheet's local names (see
    /// [`Model::duplicate_sheet`]). One history entry.
    pub fn kantor_copy_sheet(&mut self, source: u32, name: &str, index: u32) -> Result<(), String> {
        let count = self.model.workbook.worksheets.len() as u32;
        if index > count {
            return Err("Sheet index out of range".to_string());
        }
        self.model.kantor_copy_sheet(source, name, index)?;
        self.set_selected_sheet(index)?;
        self.push_diff_list(vec![Diff::KantorCopySheet {
            source,
            index,
            name: name.to_string(),
        }]);
        Ok(())
    }
}
