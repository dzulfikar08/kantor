//! Clicking slicers drawn over the grid (see `sheets_scene::slicer`), as in
//! Excel: a button selects that item alone (Ctrl+click or Multi-Select adds
//! or removes it), Multi-Select toggles, Clear Filter selects them all. The
//! frame and header still select and move the slicer. Timelines are
//! read-only: a click tells why.

use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{t, t_dyn, tf};
use sheets_engine::drawing::{DrawingKey, DrawingKind};
use sheets_engine::slicer::slicer_layout;

use crate::SheetsView;

fn inside([x, y, w, h]: [f32; 4], px: f32, py: f32) -> bool {
    px >= x && px <= x + w && py >= y && py <= y + h
}

impl SheetsView {
    /// A click at `(x, y)` px inside slicer or timeline `kind` drawn into
    /// `w` × `h` px. Returns whether it hit a button (else the click selects
    /// the object, as on any drawing).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn slicer_click(
        &mut self,
        key: DrawingKey,
        kind: &DrawingKind,
        (x, y): (f32, f32),
        (w, h): (f32, f32),
        ctrl: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let name = match kind {
            DrawingKind::Slicer { name } => name.clone(),
            DrawingKind::Timeline { .. } => {
                self.status = t("Timelines are shown as the file saved them: Kantor can't change their dates yet").into();
                cx.notify();
                return false;
            }
            _ => return false,
        };
        let Some(view) = self
            .table
            .read(cx)
            .delegate()
            .book
            .slicer_view(key.sheet_id, &name)
        else {
            return false;
        };
        let l = slicer_layout(&view, w, h);
        if l.multi.is_some_and(|r| inside(r, x, y)) {
            let mut layer = self.drawings.borrow_mut();
            if !layer.slicer_multi.remove(&key) {
                layer.slicer_multi.insert(key);
            }
            drop(layer);
            cx.notify();
            return true;
        }
        let current: Vec<bool> = view.items.iter().map(|i| i.selected).collect();
        let wanted: Vec<bool> = if l.clear.is_some_and(|r| inside(r, x, y)) {
            if !view.filtered {
                return true;
            }
            vec![true; current.len()]
        } else if let Some(k) = l.buttons.iter().position(|r| inside(*r, x, y)) {
            let multi = ctrl || self.drawings.borrow().slicer_multi.contains(&key);
            let mut next = current.clone();
            if multi {
                next[k] = !next[k];
                if !next.iter().any(|s| *s) {
                    return true;
                }
            } else {
                next = (0..current.len()).map(|i| i == k).collect();
            }
            next
        } else {
            return false;
        };
        if wanted == current {
            return true;
        }
        if let Some(why) = &view.problem {
            self.status = tf(
                "The slicer {} can't filter: {}",
                &[&view.caption, &t_dyn(why)],
            )
            .into();
            cx.notify();
            return true;
        }
        let res = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let r = grid.book.slicer_select(key.sheet_id, &name, &wanted);
            grid.resize();
            table.refresh(cx);
            r
        });
        self.status = match res {
            Ok(()) if wanted.iter().all(|s| *s) => {
                tf("{}: filter cleared", &[&view.caption]).into()
            }
            Ok(()) => {
                let n = wanted.iter().filter(|s| **s).count();
                tf("{}: {} selected", &[&view.caption, &n]).into()
            }
            Err(e) => t_dyn(&format!("{e:#}")).to_owned().into(),
        };
        self.after_pivot_edit(window, cx);
        true
    }
}
