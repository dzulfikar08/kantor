//! Changes to a pivot table's cache (shared by every pivot table on it, as
//! in Excel): grouping fields (Group, Group Selection, Ungroup) and
//! calculated fields (Insert Calculated Field). Each is one undo step and
//! lays the pivot tables out again with the same engine as Refresh.

use anyhow::{Result, anyhow, bail};

use crate::Workbook;
use crate::content_text::{tr, trf};
use crate::pivot::{Cache, CacheField, Def, Rebuild, Val};
use crate::pivot_calc::Formula;
use crate::pivot_edit::PivotGrouping;
use crate::pivot_group::{By, DateLevel, FieldGroup, GroupKind, RangeGroup};

/// A name not taken by any field of `fields` (`Years`, `Years2`...).
fn free_name(fields: &[CacheField], wanted: &str) -> String {
    let taken = |n: &str| fields.iter().any(|f| f.name.eq_ignore_ascii_case(n));
    if !taken(wanted) {
        return wanted.to_owned();
    }
    (2..)
        .map(|k| format!("{wanted}{k}"))
        .find(|n| !taken(n))
        .expect("a free name")
}

/// Remove fields `gone` from `cache`, moving every `base` / `par` link.
fn remove_fields(cache: &mut Cache, gone: &[usize]) {
    let map: Vec<Option<usize>> = (0..cache.fields.len())
        .scan(0usize, |next, i| {
            Some(if gone.contains(&i) {
                None
            } else {
                *next += 1;
                Some(*next - 1)
            })
        })
        .collect();
    let mut k = 0;
    cache.fields.retain(|_| {
        k += 1;
        !gone.contains(&(k - 1))
    });
    for f in cache.fields.iter_mut() {
        if let Some(g) = f.group.as_mut() {
            g.base = g.base.and_then(|b| map.get(b).copied().flatten());
            g.par = g.par.and_then(|p| map.get(p).copied().flatten());
        }
        if f.group
            .as_ref()
            .is_some_and(|g| g.kind.is_none() && g.par.is_none())
        {
            f.group = None;
        }
    }
}

/// The range-grouping fields above `base` (its other date levels).
fn level_fields(cache: &Cache, base: usize) -> Vec<usize> {
    (0..cache.fields.len())
        .filter(|&i| {
            i != base
                && !cache.fields[i].database
                && matches!(&cache.fields[i].group, Some(FieldGroup {
                    base: Some(b),
                    kind: Some(GroupKind::Range(_)),
                    ..
                }) if *b == base)
        })
        .collect()
}

/// Put fields `names` (found by name in `d`'s cache `c`) on the axis of
/// field `of`, just outside it, in order.
fn place_beside(d: &mut Def, c: &Cache, of: &str, names: &[String]) {
    let Some(of) = c
        .fields
        .iter()
        .position(|f| f.name.eq_ignore_ascii_case(of))
    else {
        return;
    };
    let idx: Vec<i32> = names
        .iter()
        .filter_map(|n| c.fields.iter().position(|f| f.name.eq_ignore_ascii_case(n)))
        .map(|i| i as i32)
        .collect();
    for list in [&mut d.row_fields, &mut d.col_fields] {
        list.retain(|x| !idx.contains(x));
        if let Some(at) = list.iter().position(|x| *x == of as i32) {
            for (k, x) in idx.iter().enumerate() {
                list.insert(at + k, *x);
            }
        }
    }
    for (j, f) in d.fields.iter_mut().enumerate() {
        let x = j as i32;
        if idx.contains(&x) {
            f.axis = if d.row_fields.contains(&x) {
                Some("axisRow".into())
            } else if d.col_fields.contains(&x) {
                Some("axisCol".into())
            } else {
                None
            };
        }
    }
}

impl Workbook {
    /// The pivot table at `(row, col)`: its index and definition, with its
    /// cache; refused when it can't be changed.
    fn pivot_for_change(
        &self,
        sheet: u32,
        row: u32,
        col: u32,
    ) -> Result<(usize, std::sync::Arc<Def>, std::sync::Arc<Cache>)> {
        let i = self
            .placed_pivots(sheet)
            .iter()
            .find(|p| p.contains(row, col))
            .map(|p| p.index)
            .ok_or_else(|| anyhow!("there is no PivotTable at {}", crate::cell_name(row, col)))?;
        let state = self.pivot_state();
        let def = self.pivot_def(&state, i);
        let cache = self.pivot_cache(&state, def.cache);
        if let Some(why) = cache.unsupported.as_ref().or(def.unsupported.as_ref()) {
            bail!("can't change the PivotTable {}: {why}", def.name);
        }
        Ok((i, def, cache))
    }

    /// Rebuild the pivot tables on `def`'s cache with `cache_change` applied
    /// and `adjust` for `def` itself (one undo step).
    fn change_pivot_cache(
        &mut self,
        def: &Def,
        cache_change: impl Fn(&mut Cache) -> Result<()> + 'static,
        adjust: impl Fn(&mut Def, &Cache) + 'static,
    ) -> Result<()> {
        // Pivot names are unique per sheet (as in Excel): a copied sheet's
        // pivots keep theirs.
        let (name, sheet_id) = (def.name.clone(), def.sheet_id);
        let change = Rebuild {
            adjust: Some(Box::new(move |d: &mut Def, c: &Cache| {
                if d.name == name && d.sheet_id == sheet_id {
                    adjust(d, c);
                }
            })),
            cache_change: Some(Box::new(cache_change)),
            ..Default::default()
        };
        self.rebuild_pivots(&[def.cache], change)?;
        Ok(())
    }

    /// Group field `field` of the pivot table at `(row, col)` by ranges
    /// (Excel's Grouping dialog): numbers in steps, or dates by the levels
    /// chosen. The finest level groups the field; each coarser one adds a
    /// field (`Years`, `Quarters`...) put just outside it. Grouping a level
    /// field regroups its base field. One undo step.
    pub fn group_pivot_field(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
        field: usize,
        grouping: &PivotGrouping,
    ) -> Result<()> {
        let (_, def, cache) = self.pivot_for_change(sheet, row, col)?;
        let cf = cache
            .fields
            .get(field)
            .ok_or_else(|| anyhow!("the PivotTable has no field {field}"))?;
        // A level field (`Years`) stands for its base field.
        let base = match &cf.group {
            Some(FieldGroup {
                base: Some(b),
                kind: Some(GroupKind::Range(_)),
                ..
            }) => *b,
            _ => field,
        };
        let bf = &cache.fields[base];
        if !bf.calc_items.is_empty() {
            bail!(
                "Cannot group that selection: {} has calculated items",
                bf.name
            );
        }
        if !bf.database || bf.formula.is_some() {
            bail!(
                "Cannot group that selection: {} isn't a field of the source",
                bf.name
            );
        }
        if bf
            .group
            .as_ref()
            .is_some_and(|g| g.par.is_some() && g.kind.is_none())
        {
            bail!(
                "Cannot group that selection: {} has groups of its items; ungroup them first",
                bf.name
            );
        }
        let base_name = bf.name.clone();
        let (_, _, data) = self.read_source(&cache)?;
        let column = data
            .names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(&base_name))
            .ok_or_else(|| anyhow!("{base_name} is not in the source any more"))?;
        let values: Vec<&Val> = data
            .rows
            .iter()
            .map(|r| &r[column])
            .filter(|v| **v != Val::Missing)
            .collect();
        let all_numbers = values.iter().all(|v| v.number().is_some());
        let dates = values.iter().all(|v| matches!(v, Val::Date(_)));
        if values.is_empty() || !all_numbers {
            bail!("Cannot group that selection: {base_name} must hold only numbers or dates");
        }
        let (levels, range) = match grouping {
            PivotGrouping::Numbers {
                start,
                end,
                by,
                auto_start,
                auto_end,
            } => {
                if by.is_nan() || *by <= 0.0 {
                    bail!("By must be a number greater than 0");
                }
                if start > end {
                    bail!("Ending at must be greater than Starting at");
                }
                (
                    vec![],
                    RangeGroup {
                        by: By::Numbers,
                        auto_start: *auto_start,
                        auto_end: *auto_end,
                        start: *start,
                        end: *end,
                        interval: *by,
                    },
                )
            }
            PivotGrouping::Dates {
                start,
                end,
                auto_start,
                auto_end,
                levels,
                days,
            } => {
                if !dates {
                    bail!("Cannot group that selection: {base_name} doesn't hold dates");
                }
                let mut levels = levels.clone();
                levels.sort();
                levels.dedup();
                let Some(finest) = levels.first().copied() else {
                    bail!("choose at least one way to group by (Months, Years...)");
                };
                if start > end {
                    bail!("Ending at must be later than Starting at");
                }
                let interval = if levels == [DateLevel::Days] {
                    f64::from((*days).max(1))
                } else {
                    1.0
                };
                (
                    levels[1..].to_vec(),
                    RangeGroup {
                        by: By::Date(finest),
                        auto_start: *auto_start,
                        auto_end: *auto_end,
                        start: start.floor(),
                        end: end.floor(),
                        interval,
                    },
                )
            }
        };
        // The fields this adds, coarsest first (outermost on the axis).
        let mut names: Vec<String> = Vec::new();
        let mut probe = cache.fields.clone();
        let old_levels = level_fields(&cache, base);
        let mut k = 0;
        probe.retain(|_| {
            k += 1;
            !old_levels.contains(&(k - 1))
        });
        for l in &levels {
            let n = free_name(&probe, tr(l.label()));
            probe.push(CacheField::source(n.clone()));
            names.push(n);
        }
        let names_change = names.clone();
        let base_change = base_name.clone();
        let cache_change = move |c: &mut Cache| -> Result<()> {
            let b = c
                .fields
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(&base_change))
                .ok_or_else(|| anyhow!("{base_change} is gone"))?;
            let gone = level_fields(c, b);
            remove_fields(c, &gone);
            let b = c
                .fields
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(&base_change))
                .ok_or_else(|| anyhow!("{base_change} is gone"))?;
            let first = c.fields.len();
            for (k, (level, name)) in levels.iter().zip(&names_change).enumerate() {
                c.fields.push(CacheField {
                    database: false,
                    group: Some(FieldGroup {
                        base: Some(b),
                        par: (k + 1 < levels.len()).then_some(first + k + 1),
                        kind: Some(GroupKind::Range(RangeGroup {
                            by: By::Date(*level),
                            interval: 1.0,
                            ..range.clone()
                        })),
                        items: Vec::new(),
                    }),
                    ..CacheField::source(name.clone())
                });
            }
            c.fields[b].items.clear();
            c.fields[b].group = Some(FieldGroup {
                base: Some(b),
                par: (!levels.is_empty()).then_some(first),
                kind: Some(GroupKind::Range(range.clone())),
                items: Vec::new(),
            });
            Ok(())
        };
        let mut outer = names.clone();
        outer.reverse();
        self.change_pivot_cache(&def, cache_change, move |d, c| {
            place_beside(d, c, &base_name, &outer);
        })
    }

    /// Group Selection: items `items` (by label) of field `field` become
    /// one group (`Group1`...) of a grouping field over it (`Region2`, put
    /// just outside it; made if needed). Returns the group's name. One
    /// undo step.
    pub fn group_pivot_items(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
        field: usize,
        items: &[String],
    ) -> Result<String> {
        let (_, def, cache) = self.pivot_for_change(sheet, row, col)?;
        let cf = cache
            .fields
            .get(field)
            .ok_or_else(|| anyhow!("the PivotTable has no field {field}"))?;
        if items.is_empty() {
            bail!("choose the items to group");
        }
        if !cf.calc_items.is_empty() {
            bail!(
                "Cannot group that selection: {} has calculated items",
                cf.name
            );
        }
        if !cf.database || cf.range_grouped() {
            bail!(
                "Cannot group that selection: {} is grouped by ranges or isn't a field of the source",
                cf.name
            );
        }
        let base_name = cf.name.clone();
        // The grouping field: there already, or a new one.
        let existing = cf
            .group
            .as_ref()
            .and_then(|g| g.par)
            .and_then(|p| cache.fields.get(p))
            .map(|f| f.name.clone());
        let group_field = existing
            .clone()
            .unwrap_or_else(|| free_name(&cache.fields, &format!("{base_name}2")));
        let taken: Vec<String> = existing
            .as_ref()
            .and_then(|n| cache.fields.iter().find(|f| f.name == *n))
            .and_then(|f| f.group.as_ref())
            .map(|g| g.items.iter().map(Val::label).collect())
            .unwrap_or_default();
        let group_name = (1..)
            .map(|k| trf("Group{}", &[&k]))
            .find(|n| !taken.iter().any(|t| t.eq_ignore_ascii_case(n)))
            .expect("a free name");
        let wanted: Vec<String> = items.to_vec();
        let gf = group_field.clone();
        let gn = group_name.clone();
        let bn = base_name.clone();
        let cache_change = move |c: &mut Cache| -> Result<()> {
            let b = c
                .fields
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(&bn))
                .ok_or_else(|| anyhow!("{bn} is gone"))?;
            let base_items = c.fields[b].items.clone();
            let g = match c.fields.iter().position(|f| f.name == gf) {
                Some(g) => g,
                None => {
                    // Every item its own group to start with.
                    c.fields.push(CacheField {
                        database: false,
                        group: Some(FieldGroup {
                            base: Some(b),
                            par: None,
                            kind: Some(GroupKind::Discrete((0..base_items.len()).collect())),
                            items: base_items.iter().map(|v| Val::Str(v.label())).collect(),
                        }),
                        ..CacheField::source(gf.clone())
                    });
                    let g = c.fields.len() - 1;
                    c.fields[b].group = Some(FieldGroup {
                        base: Some(b),
                        par: Some(g),
                        kind: None,
                        items: Vec::new(),
                    });
                    g
                }
            };
            let Some(FieldGroup {
                kind: Some(GroupKind::Discrete(map)),
                items: names,
                ..
            }) = c.fields[g].group.clone()
            else {
                bail!("{gf} is not a group field");
            };
            // The base items picked: by their label, or by the label of the
            // group they are in (grouping groups merges them).
            let picked: Vec<bool> = base_items
                .iter()
                .enumerate()
                .map(|(x, v)| {
                    let own = v.label();
                    let group = map.get(x).and_then(|k| names.get(*k)).map(Val::label);
                    wanted
                        .iter()
                        .any(|w| *w == own || Some(w) == group.as_ref())
                })
                .collect();
            if !picked.iter().any(|p| *p) {
                bail!("none of the items chosen is in {bn}");
            }
            // New item names: each base item's group, the picked ones in
            // the new group, placed where its first member is.
            let mut out_names: Vec<String> = Vec::new();
            let mut out_map = Vec::with_capacity(base_items.len());
            for (x, _) in base_items.iter().enumerate() {
                let name = if picked[x] {
                    gn.clone()
                } else {
                    map.get(x)
                        .and_then(|k| names.get(*k))
                        .map(Val::label)
                        .unwrap_or_default()
                };
                let k = match out_names.iter().position(|n| *n == name) {
                    Some(k) => k,
                    None => {
                        out_names.push(name);
                        out_names.len() - 1
                    }
                };
                out_map.push(k);
            }
            if let Some(grp) = c.fields[g].group.as_mut() {
                grp.kind = Some(GroupKind::Discrete(out_map));
                grp.items = out_names.into_iter().map(Val::Str).collect();
            }
            Ok(())
        };
        let outer = vec![group_field];
        self.change_pivot_cache(&def, cache_change, move |d, c| {
            if existing.is_none() {
                place_beside(d, c, &base_name, &outer);
            }
        })?;
        Ok(group_name)
    }

    /// Ungroup field `field` of the pivot table at `(row, col)`: a field
    /// grouped by ranges (or one of its level fields) loses its grouping
    /// and level fields; a grouping field of manual groups goes (with
    /// `item`, only that group is ungrouped). One undo step.
    pub fn ungroup_pivot_field(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
        field: usize,
        item: Option<&str>,
    ) -> Result<()> {
        let (_, def, cache) = self.pivot_for_change(sheet, row, col)?;
        let cf = cache
            .fields
            .get(field)
            .ok_or_else(|| anyhow!("the PivotTable has no field {field}"))?;
        let Some(g) = &cf.group else {
            bail!("{} isn't grouped", cf.name);
        };
        let target = cf.name.clone();
        // The field the grouping hangs on, and whether it is by ranges.
        let (base, ranged) = match &g.kind {
            Some(GroupKind::Range(_)) => (g.base.unwrap_or(field), true),
            Some(GroupKind::Discrete(_)) => (g.base.unwrap_or(field), false),
            None => (field, false),
        };
        let base_name = cache.fields[base].name.clone();
        let one = item.map(str::to_owned);
        let cache_change = move |c: &mut Cache| -> Result<()> {
            let b = c
                .fields
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(&base_name))
                .ok_or_else(|| anyhow!("{base_name} is gone"))?;
            if ranged {
                let gone = level_fields(c, b);
                c.fields[b].group = None;
                remove_fields(c, &gone);
                return Ok(());
            }
            let Some(g) = c.fields[b].group.as_ref().and_then(|g| g.par).or_else(|| {
                c.fields
                    .iter()
                    .position(|f| f.name.eq_ignore_ascii_case(&target))
                    .filter(|_| target != base_name)
            }) else {
                bail!("{base_name} has no groups");
            };
            if let Some(one) = &one
                && let Some(FieldGroup {
                    kind: Some(GroupKind::Discrete(map)),
                    items,
                    ..
                }) = c.fields[g].group.clone()
            {
                let base_items = c.fields[b].items.clone();
                let mut names: Vec<String> = Vec::new();
                let mut new_map = Vec::new();
                for (x, v) in base_items.iter().enumerate() {
                    let current = map.get(x).and_then(|k| items.get(*k)).map(Val::label);
                    let name = if current.as_deref() == Some(one.as_str()) {
                        v.label()
                    } else {
                        current.unwrap_or_else(|| v.label())
                    };
                    let k = match names.iter().position(|n| *n == name) {
                        Some(k) => k,
                        None => {
                            names.push(name);
                            names.len() - 1
                        }
                    };
                    new_map.push(k);
                }
                // Groups left?
                let left = names
                    .iter()
                    .any(|n| !base_items.iter().any(|v| v.label() == *n));
                if left {
                    if let Some(grp) = c.fields[g].group.as_mut() {
                        grp.kind = Some(GroupKind::Discrete(new_map));
                        grp.items = names.into_iter().map(Val::Str).collect();
                    }
                    return Ok(());
                }
            }
            c.fields[b].group = None;
            remove_fields(c, &[g]);
            Ok(())
        };
        self.change_pivot_cache(&def, cache_change, |_, _| {})
    }

    /// Insert Calculated Item: add item `name` with `formula` (over the
    /// items of the same field, e.g. `=East+West` or `='North East'*2`) to
    /// row or column field `field` of the pivot table at `(row, col)`, or
    /// change the formula of the calculated item of that name. The item is
    /// worked out as Excel does: in each cell, the formula over the values
    /// of the items it names; totals across the field include it. One undo
    /// step.
    pub fn set_pivot_calculated_item(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
        field: usize,
        name: &str,
        formula: &str,
    ) -> Result<()> {
        let (_, def, cache) = self.pivot_for_change(sheet, row, col)?;
        let cf = cache
            .fields
            .get(field)
            .ok_or_else(|| anyhow!("the PivotTable has no field {field}"))?;
        let on_axis =
            def.row_fields.contains(&(field as i32)) || def.col_fields.contains(&(field as i32));
        if !on_axis {
            bail!(
                "Calculated items go in a row or column field: put {} in Rows or Columns first",
                cf.name
            );
        }
        if !cf.database || cf.formula.is_some() || cf.group.is_some() {
            bail!(
                "You cannot add a calculated item to a grouped field ({})",
                cf.name
            );
        }
        if def
            .fields
            .get(field)
            .is_some_and(|f| !f.subtotals.is_empty())
        {
            bail!("Calculated items do not work with custom subtotals");
        }
        let name = name.trim().to_owned();
        if name.is_empty() {
            bail!("type a name for the calculated item");
        }
        let existing = cf
            .calc_items
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(&name));
        let taken = cf
            .items
            .iter()
            .any(|v| v.label().eq_ignore_ascii_case(&name))
            || cache
                .fields
                .iter()
                .any(|f| f.name.eq_ignore_ascii_case(&name));
        if taken && !existing {
            bail!("The item name {name:?} is already in use");
        }
        let names: Vec<String> = cf.items.iter().map(Val::label).collect();
        let at = names.iter().position(|n| n.eq_ignore_ascii_case(&name));
        let compiled =
            Formula::compile_items(formula, &cf.name, &names, at).map_err(|e| anyhow!("{e}"))?;
        if compiled
            .fields
            .iter()
            .any(|&k| names.get(k).is_some_and(|n| n.eq_ignore_ascii_case(&name)))
        {
            bail!("the formula of {name} can't use {name} itself");
        }
        let text = formula.trim().trim_start_matches('=').trim().to_owned();
        let field_name = cf.name.clone();
        self.change_pivot_cache(
            &def,
            move |c: &mut Cache| {
                let f = c
                    .fields
                    .iter_mut()
                    .find(|f| f.database && f.name.eq_ignore_ascii_case(&field_name))
                    .ok_or_else(|| anyhow!("{field_name} is gone"))?;
                match f
                    .calc_items
                    .iter_mut()
                    .find(|c| c.name.eq_ignore_ascii_case(&name))
                {
                    Some(ci) => ci.formula = text.clone(),
                    None => f.calc_items.push(crate::pivot::CalcItem {
                        name: name.clone(),
                        formula: text.clone(),
                    }),
                }
                Ok(())
            },
            |_, _| {},
        )
    }

    /// Delete calculated item `name` of field `field` (from every pivot
    /// table on the cache). One undo step.
    pub fn delete_pivot_calculated_item(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
        field: usize,
        name: &str,
    ) -> Result<()> {
        let (_, def, cache) = self.pivot_for_change(sheet, row, col)?;
        let cf = cache
            .fields
            .get(field)
            .ok_or_else(|| anyhow!("the PivotTable has no field {field}"))?;
        if !cf
            .calc_items
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(name))
        {
            bail!("there is no calculated item {name:?} in {}", cf.name);
        }
        let field_name = cf.name.clone();
        let n = name.to_owned();
        self.change_pivot_cache(
            &def,
            move |c: &mut Cache| {
                if let Some(f) = c
                    .fields
                    .iter_mut()
                    .find(|f| f.database && f.name.eq_ignore_ascii_case(&field_name))
                {
                    f.calc_items.retain(|c| !c.name.eq_ignore_ascii_case(&n));
                    let key = Val::Str(n.clone()).key();
                    f.items.retain(|v| v.key() != key);
                }
                Ok(())
            },
            |_, _| {},
        )
    }

    /// Check a calculated field's formula against the pivot table's fields.
    pub fn check_pivot_formula(&self, sheet: u32, row: u32, col: u32, formula: &str) -> Result<()> {
        let (_, _, cache) = self.pivot_for_change(sheet, row, col)?;
        let names: Vec<String> = cache
            .fields
            .iter()
            .filter(|f| f.database)
            .map(|f| f.name.clone())
            .collect();
        Formula::compile(formula, &names).map_err(|e| anyhow!("{e}"))?;
        Ok(())
    }

    /// Insert Calculated Field: add field `name` with `formula` (over the
    /// source's field names, e.g. `=Revenue/Units`) to the pivot table's
    /// cache, or change the formula of the calculated field of that name.
    /// A new one is added to Values, as in Excel. One undo step.
    pub fn set_pivot_calculated_field(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
        name: &str,
        formula: &str,
    ) -> Result<()> {
        let (_, def, cache) = self.pivot_for_change(sheet, row, col)?;
        let name = name.trim().to_owned();
        if name.is_empty() {
            bail!("type a name for the calculated field");
        }
        let existing = cache
            .fields
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(&name));
        if existing.is_some_and(|f| f.formula.is_none()) {
            bail!("The PivotTable field name {name:?} is already in use");
        }
        let names: Vec<String> = cache
            .fields
            .iter()
            .filter(|f| f.database)
            .map(|f| f.name.clone())
            .collect();
        Formula::compile(formula, &names).map_err(|e| anyhow!("{e}"))?;
        let text = formula.trim().trim_start_matches('=').trim().to_owned();
        let is_new = existing.is_none();
        let n = name.clone();
        let cache_change = move |c: &mut Cache| -> Result<()> {
            match c
                .fields
                .iter_mut()
                .find(|f| f.name.eq_ignore_ascii_case(&n))
            {
                Some(f) => f.formula = Some(text.clone()),
                None => c.fields.push(CacheField {
                    database: false,
                    formula: Some(text.clone()),
                    ..CacheField::source(n.clone())
                }),
            }
            Ok(())
        };
        self.change_pivot_cache(&def, cache_change, move |d, c| {
            if !is_new {
                return;
            }
            let Some(f) = c
                .fields
                .iter()
                .position(|x| x.name.eq_ignore_ascii_case(&name))
            else {
                return;
            };
            if d.data_fields.iter().any(|x| x.fld == f) {
                return;
            }
            d.data_fields.push(crate::pivot::DataField {
                name: Some(crate::pivot::Func::Sum.default_name(&name)),
                fld: f,
                func: crate::pivot::Func::Sum,
                num_fmt_id: None,
                show_as: crate::pivot_calc::ShowAs::Normal,
                base_field: 0,
                base_item: 0,
                xml: None,
            });
            d.fields[f].data = true;
            let has_values = d.row_fields.contains(&-2) || d.col_fields.contains(&-2);
            if d.data_fields.len() >= 2 && !has_values {
                d.col_fields.push(-2);
            }
        })
    }

    /// Delete calculated field `name` from the pivot table's cache (and
    /// so from Values). One undo step.
    pub fn delete_pivot_calculated_field(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
        name: &str,
    ) -> Result<()> {
        let (_, def, cache) = self.pivot_for_change(sheet, row, col)?;
        if !cache
            .fields
            .iter()
            .any(|f| f.formula.is_some() && f.name.eq_ignore_ascii_case(name))
        {
            bail!("there is no calculated field {name:?}");
        }
        let n = name.to_owned();
        self.change_pivot_cache(
            &def,
            move |c: &mut Cache| {
                let gone: Vec<usize> = (0..c.fields.len())
                    .filter(|&i| {
                        c.fields[i].formula.is_some() && c.fields[i].name.eq_ignore_ascii_case(&n)
                    })
                    .collect();
                remove_fields(c, &gone);
                Ok(())
            },
            |_, _| {},
        )
    }
}

/// The built-in PivotTable styles, as the gallery lists them.
pub fn pivot_style_names() -> Vec<String> {
    let mut v = Vec::new();
    for family in ["Light", "Medium", "Dark"] {
        for i in 1..=28 {
            v.push(format!("PivotStyle{family}{i}"));
        }
    }
    v
}
