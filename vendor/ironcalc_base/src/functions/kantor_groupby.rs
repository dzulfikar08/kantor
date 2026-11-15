//! KANTOR PATCH (functions): GROUPBY, PIVOTBY and functions passed by name.
//!
//! `GROUPBY(row_fields, values, function, [field_headers], [total_depth],
//! [sort_order], [filter_array], [field_relationship])`
//!
//! `PIVOTBY(row_fields, col_fields, values, function, [field_headers],
//! [row_total_depth], [row_sort_order], [col_total_depth], [col_sort_order],
//! [filter_array], [relative_to])`
//!
//! Both spill their result like any dynamic array. `function` is a LAMBDA or
//! a function passed by name (`SUM`, `AVERAGE`, `PERCENTOF`…, an "eta
//! reduced" LAMBDA, see [`Model::lambda_arg`]). It gets the values of a group
//! as a one-column array; a function of two arguments (PERCENTOF) also gets
//! the values it is compared with: for GROUPBY all values of the column, for
//! PIVOTBY what `relative_to` says.
//!
//! `function` may also be a vector of functions: `HSTACK(SUM,AVERAGE)` puts
//! each aggregation in its own columns, under a header row of function names
//! (for several value columns, a row of function names over a row of value
//! headers); `VSTACK(SUM,AVERAGE)` gives each group one row per function,
//! the function's name in a column after the keys. A second row (column) of
//! text, `VSTACK(HSTACK(SUM,AVERAGE),{"Total","Mean"})`, names the functions
//! instead (one name per function, otherwise #VALUE!). A LAMBDA is labelled
//! "LAMBDA".
//!
//! What differs from Excel: a whole-column reference is read down to the
//! last used row of its sheet, the empty rows below standing for one blank
//! row; generated headers are "Field n" and "Value n".

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::{
    calc_result::CalcResult,
    expressions::{
        parser::{ArrayNode, NamedVariable, Node},
        token::Error,
        types::CellReferenceIndex,
    },
    functions::{
        kantor_names::eta_function, math_and_trigonometry::array_size::check_array_size,
        util::compare_values, Function,
    },
    model::Model,
};

type Grid = Vec<Vec<ArrayNode>>;

/// A grouping key: text matches regardless of case, as in Excel.
fn key_text(node: &ArrayNode) -> String {
    match node {
        ArrayNode::Number(f) => format!("n{f}"),
        ArrayNode::String(s) => format!("s{}", s.to_uppercase()),
        ArrayNode::Boolean(b) => format!("b{b}"),
        ArrayNode::Error(e) => format!("e{e}"),
        ArrayNode::Empty => "_".to_string(),
    }
}

fn prefix_key(keys: &[ArrayNode]) -> String {
    keys.iter().map(key_text).collect::<Vec<_>>().join("\u{1}")
}

/// Sort order of keys and values: numbers, text, booleans, errors, blanks.
fn node_cmp(a: &ArrayNode, b: &ArrayNode) -> Ordering {
    let as_result = |n: &ArrayNode| match n {
        ArrayNode::Number(f) => CalcResult::Number(*f),
        ArrayNode::String(s) => CalcResult::String(s.clone()),
        ArrayNode::Boolean(b) => CalcResult::Boolean(*b),
        ArrayNode::Error(e) => CalcResult::Error {
            error: e.clone(),
            origin: CellReferenceIndex {
                sheet: 0,
                row: 1,
                column: 1,
            },
            message: String::new(),
        },
        ArrayNode::Empty => CalcResult::EmptyCell,
    };
    match (a, b) {
        (ArrayNode::Empty, ArrayNode::Empty) => Ordering::Equal,
        (ArrayNode::Empty, _) => Ordering::Greater,
        (_, ArrayNode::Empty) => Ordering::Less,
        _ => compare_values(&as_result(a), &as_result(b)).cmp(&0),
    }
}

fn result_to_node(result: CalcResult) -> ArrayNode {
    match result {
        CalcResult::Number(f) => ArrayNode::Number(f),
        CalcResult::String(s) => ArrayNode::String(s),
        CalcResult::Boolean(b) => ArrayNode::Boolean(b),
        CalcResult::Error { error, .. } => ArrayNode::Error(error),
        CalcResult::EmptyCell | CalcResult::EmptyArg => ArrayNode::Empty,
        CalcResult::Array(a) if a.len() == 1 && a[0].len() == 1 => a[0][0].clone(),
        _ => ArrayNode::Error(Error::VALUE),
    }
}

/// How one axis is sorted: by key columns (0-based, descending?), or by an
/// aggregated value column.
enum SortSpec {
    Keys(Vec<(usize, bool)>),
    Value(usize, bool),
}

/// A line of an axis: a group (`prefix_len` = all key columns), a subtotal
/// (fewer) or the grand total (0), with the data rows it covers.
struct Line {
    keys: Vec<ArrayNode>,
    prefix_len: usize,
    rows: Vec<usize>,
}

struct Axis<'k> {
    /// The key of each data row.
    keys: &'k [Vec<ArrayNode>],
    /// Number of key columns.
    width: usize,
}

impl Axis<'_> {
    fn rows_with_prefix(&self, prefix: &[ArrayNode], rows: &[usize]) -> Vec<usize> {
        let wanted: Vec<String> = prefix.iter().map(key_text).collect();
        rows.iter()
            .copied()
            .filter(|&r| {
                self.keys[r]
                    .iter()
                    .zip(&wanted)
                    .all(|(k, w)| key_text(k) == *w)
            })
            .collect()
    }

    /// The lines of this axis, sorted, with subtotals and the grand total.
    /// `value_of` aggregates rows for sorting by a value column.
    #[allow(clippy::too_many_arguments)]
    fn lines(
        &self,
        rows: &[usize],
        depth: i64,
        sort: &SortSpec,
        hierarchy: bool,
        value_of: &mut dyn FnMut(&[usize], usize) -> ArrayNode,
    ) -> Vec<Line> {
        let k = self.width;
        // Groups of every prefix length, in first-seen order.
        let mut groups: Vec<HashMap<String, Vec<usize>>> = vec![HashMap::new(); k + 1];
        let mut leaves: Vec<Vec<ArrayNode>> = Vec::new();
        for &r in rows {
            let key = &self.keys[r];
            for (len, map) in groups.iter_mut().enumerate().skip(1) {
                let entry = map.entry(prefix_key(&key[..len])).or_default();
                if len == k && entry.is_empty() {
                    leaves.push(key.clone());
                }
                entry.push(r);
            }
        }
        let group_rows = |keys: &[ArrayNode], len: usize| -> &[usize] {
            groups[len]
                .get(&prefix_key(&keys[..len]))
                .map_or(&[], Vec::as_slice)
        };

        // The sort key of a prefix of `len` columns at its last column.
        let mut values: HashMap<(usize, String), ArrayNode> = HashMap::new();
        if let SortSpec::Value(v, _) = sort {
            let levels: Vec<usize> = if hierarchy {
                (1..=k).collect()
            } else {
                vec![k]
            };
            for leaf in &leaves {
                for &len in &levels {
                    let id = (len, prefix_key(&leaf[..len]));
                    values
                        .entry(id)
                        .or_insert_with(|| value_of(group_rows(leaf, len), *v));
                }
            }
        }
        let direction = |column: usize| match sort {
            SortSpec::Keys(spec) => spec.iter().find(|(c, _)| *c == column).is_some_and(|s| s.1),
            SortSpec::Value(..) => false,
        };
        let by_keys = |a: &Vec<ArrayNode>, b: &Vec<ArrayNode>, order: &[(usize, bool)]| {
            for &(c, desc) in order {
                let o = node_cmp(&a[c], &b[c]);
                let o = if desc { o.reverse() } else { o };
                if o != Ordering::Equal {
                    return o;
                }
            }
            Ordering::Equal
        };
        let all_ascending: Vec<(usize, bool)> = (0..k).map(|c| (c, false)).collect();
        match (sort, hierarchy) {
            (SortSpec::Keys(_), true) => {
                let order: Vec<(usize, bool)> = (0..k).map(|c| (c, direction(c))).collect();
                leaves.sort_by(|a, b| by_keys(a, b, &order));
            }
            (SortSpec::Keys(spec), false) => {
                leaves.sort_by(|a, b| {
                    by_keys(a, b, spec).then_with(|| by_keys(a, b, &all_ascending))
                });
            }
            (SortSpec::Value(_, desc), _) => {
                let levels: Vec<usize> = if hierarchy {
                    (1..=k).collect()
                } else {
                    vec![k]
                };
                leaves.sort_by(|a, b| {
                    for &len in &levels {
                        let va = &values[&(len, prefix_key(&a[..len]))];
                        let vb = &values[&(len, prefix_key(&b[..len]))];
                        let o = node_cmp(va, vb);
                        let o = if *desc { o.reverse() } else { o };
                        let o = o.then_with(|| by_keys(a, b, &[(len - 1, false)]));
                        if o != Ordering::Equal {
                            return o;
                        }
                    }
                    by_keys(a, b, &all_ascending)
                });
            }
        }

        // Subtotals for prefixes of 1 ..= levels columns.
        let levels = (depth.unsigned_abs() as usize).saturating_sub(1).min(k - 1);
        let before = depth < 0;
        let subtotal = |keys: &[ArrayNode], len: usize| Line {
            keys: (0..k)
                .map(|c| {
                    if c < len {
                        keys[c].clone()
                    } else {
                        ArrayNode::Empty
                    }
                })
                .collect(),
            prefix_len: len,
            rows: group_rows(keys, len).to_vec(),
        };
        let mut out: Vec<Line> = Vec::new();
        let mut previous: Option<&Vec<ArrayNode>> = None;
        for leaf in &leaves {
            // The first level at which this leaf leaves the previous groups.
            let split = previous.map_or(0, |p| {
                (0..levels)
                    .take_while(|&l| key_text(&p[l]) == key_text(&leaf[l]))
                    .count()
            });
            if before {
                for len in split + 1..=levels {
                    out.push(subtotal(leaf, len));
                }
            } else if let Some(p) = previous {
                for len in (split + 1..=levels).rev() {
                    out.push(subtotal(p, len));
                }
            }
            out.push(Line {
                keys: leaf.clone(),
                prefix_len: k,
                rows: group_rows(leaf, k).to_vec(),
            });
            previous = Some(leaf);
        }
        if !before {
            if let Some(p) = previous {
                for len in (1..=levels).rev() {
                    out.push(subtotal(p, len));
                }
            }
        }
        if depth != 0 {
            let mut keys = vec![ArrayNode::Empty; k];
            keys[0] =
                ArrayNode::String(if levels > 0 { "Grand Total" } else { "Total" }.to_string());
            let total = Line {
                keys,
                prefix_len: 0,
                rows: rows.to_vec(),
            };
            if before {
                out.insert(0, total);
            } else {
                out.push(total);
            }
        }
        out
    }
}

/// Rows in both sorted lists.
fn intersect(a: &[usize], b: &[usize]) -> Vec<usize> {
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            Ordering::Less => i += 1,
            Ordering::Greater => j += 1,
            Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}

/// The headers and data rows of GROUPBY's and PIVOTBY's input.
struct Prepared {
    has_headers: bool,
    show_headers: bool,
    /// Data rows that pass the filter.
    rows: Vec<usize>,
}

/// One aggregation of GROUPBY/PIVOTBY: the LAMBDA, how many arguments it
/// takes and its label in a vector of functions.
struct Agg {
    function: CalcResult,
    arity: usize,
    label: ArrayNode,
}

/// The aggregations of the `function` argument; `vertical` for a
/// VSTACK of functions (one row per function).
struct Aggs {
    list: Vec<Agg>,
    vertical: bool,
}

impl<'a> Model<'a> {
    /// The LAMBDA a function takes as an argument. A bare function name
    /// (`SUM`, from xlsx `_xleta.SUM`) is that function as a LAMBDA of
    /// `arity` arguments, as in Excel's "eta reduced" lambdas.
    pub(crate) fn lambda_arg(
        &mut self,
        node: &Node,
        cell: CellReferenceIndex,
        arity: usize,
    ) -> CalcResult {
        if let Node::NamedVariableKind { name, id: None } = node {
            if let Some(function) = eta_function(name) {
                return self.eta_lambda(function, arity);
            }
        }
        self.evaluate_node_in_context(node, cell)
    }

    fn eta_lambda(&mut self, function: Function, arity: usize) -> CalcResult {
        let names: Vec<String> = (0..arity.max(1)).map(|i| format!("_eta{i}")).collect();
        let parameters = names
            .iter()
            .map(|name| NamedVariable {
                name: name.clone(),
                id: None,
                is_optional: false,
            })
            .collect();
        let body = Node::FunctionKind {
            kind: function,
            args: names
                .into_iter()
                .map(|name| Node::NamedVariableKind { name, id: None })
                .collect(),
        };
        let id = self.get_next_lambda_id();
        self.lambdas.insert(id, (parameters, body));
        CalcResult::Lambda(id)
    }

    /// The aggregation of GROUPBY/PIVOTBY and how many arguments it takes.
    fn aggregation_arg(
        &mut self,
        node: &Node,
        cell: CellReferenceIndex,
    ) -> Result<(CalcResult, usize), CalcResult> {
        if let Node::NamedVariableKind { name, id: None } = node {
            if let Some(function) = eta_function(name) {
                let arity = if function == Function::Percentof {
                    2
                } else {
                    1
                };
                return Ok((self.eta_lambda(function, arity), arity));
            }
        }
        match self.evaluate_node_in_context(node, cell) {
            CalcResult::Lambda(id) => {
                let arity = self
                    .lambdas
                    .get(&id)
                    .map_or(1, |(p, _)| p.len())
                    .clamp(1, 2);
                Ok((CalcResult::Lambda(id), arity))
            }
            error @ CalcResult::Error { .. } => Err(error),
            _ => Err(CalcResult::new_error(
                Error::VALUE,
                cell,
                "The function must be a LAMBDA or a function name".to_string(),
            )),
        }
    }

    /// The label of a function in a vector of functions: its name, or
    /// "LAMBDA".
    fn agg_label(node: &Node) -> ArrayNode {
        match node {
            Node::NamedVariableKind { name, id: None } if eta_function(name).is_some() => {
                ArrayNode::String(name.to_uppercase())
            }
            _ => ArrayNode::String("LAMBDA".to_string()),
        }
    }

    /// The functions of an HSTACK/VSTACK of functions.
    fn agg_list(
        &mut self,
        nodes: &[Node],
        cell: CellReferenceIndex,
    ) -> Result<Vec<Agg>, CalcResult> {
        let mut list = Vec::new();
        for node in nodes {
            let (function, arity) = self.aggregation_arg(node, cell)?;
            list.push(Agg {
                function,
                arity,
                label: Self::agg_label(node),
            });
        }
        Ok(list)
    }

    /// The `function` argument: one function, or a vector of them
    /// (`HSTACK(SUM,AVERAGE)`, `VSTACK(...)`), optionally with a row or
    /// column of labels (`VSTACK(HSTACK(SUM,AVERAGE),{"Sum","Mean"})`).
    fn aggregations(&mut self, node: &Node, cell: CellReferenceIndex) -> Result<Aggs, CalcResult> {
        let bad = |message: &str| CalcResult::new_error(Error::VALUE, cell, message.to_string());
        let stack = |node: &Node| match node {
            Node::FunctionKind {
                kind: kind @ (Function::Hstack | Function::Vstack),
                args,
            } => Some((kind.clone() == Function::Vstack, args.clone())),
            _ => None,
        };
        let Some((vertical, args)) = stack(node) else {
            let (function, arity) = self.aggregation_arg(node, cell)?;
            return Ok(Aggs {
                list: vec![Agg {
                    function,
                    arity,
                    label: Self::agg_label(node),
                }],
                vertical: false,
            });
        };
        // A stack of functions, then labels: VSTACK(HSTACK(f, g), {"a","b"})
        // or HSTACK(VSTACK(f, g), {"a";"b"}).
        if args.len() == 2 {
            if let Some((inner_vertical, inner)) = stack(&args[0]) {
                if inner_vertical != vertical {
                    let mut list = self.agg_list(&inner, cell)?;
                    let labels = self.eval_to_array(&args[1], cell)?;
                    let labels: Vec<ArrayNode> = labels.into_iter().flatten().collect();
                    if labels.len() != list.len() {
                        return Err(bad("One label is needed for each function"));
                    }
                    for (agg, label) in list.iter_mut().zip(labels) {
                        if let ArrayNode::Error(e) = &label {
                            return Err(CalcResult::new_error(e.clone(), cell, String::new()));
                        }
                        agg.label = label;
                    }
                    return Ok(Aggs {
                        list,
                        vertical: inner_vertical,
                    });
                }
            }
        }
        if args.is_empty() {
            return Err(bad("No function"));
        }
        let list = self.agg_list(&args, cell)?;
        Ok(Aggs {
            vertical: vertical && list.len() > 1,
            list,
        })
    }

    /// A range or array argument; a range is read only down to its sheet's
    /// last used row. Also gives the height the argument has.
    pub(crate) fn read_grid(
        &mut self,
        node: &Node,
        cell: CellReferenceIndex,
    ) -> Result<(Grid, usize), CalcResult> {
        match self.evaluate_node_with_reference(node, cell) {
            CalcResult::Range { left, right } => {
                let height = (right.row - left.row + 1) as usize;
                let max_row = self
                    .used_dimension(left.sheet)
                    .map_or(right.row, |d| d.max_row)
                    .max(left.row);
                let last = right.row.min(max_row);
                if last < right.row {
                    self.record_seen_empty_range(
                        left.sheet,
                        last + 1,
                        left.column,
                        right.row,
                        right.column,
                    );
                }
                let grid = self.evaluate_range(left, CellReferenceIndex { row: last, ..right });
                Ok((grid, height))
            }
            result => {
                let grid = self.result_to_array(result, cell)?;
                let height = grid.len();
                Ok((grid, height))
            }
        }
    }

    /// Reads the column-oriented arguments: all must have the same height.
    /// Rows past what was read of all of them (empty rows below the data of
    /// whole-column references) become one empty row.
    fn read_fields(
        &mut self,
        nodes: &[&Node],
        cell: CellReferenceIndex,
    ) -> Result<Vec<Grid>, CalcResult> {
        let mut grids = Vec::new();
        let mut height = None;
        for node in nodes {
            let (grid, h) = self.read_grid(node, cell)?;
            if grid.is_empty() || grid[0].is_empty() {
                return Err(CalcResult::new_error(
                    Error::VALUE,
                    cell,
                    "Empty argument".to_string(),
                ));
            }
            if height.is_some_and(|x| x != h) {
                return Err(CalcResult::new_error(
                    Error::VALUE,
                    cell,
                    "The fields, values and filter must have the same number of rows".to_string(),
                ));
            }
            height = Some(h);
            grids.push(grid);
        }
        let height = height.unwrap_or(0);
        let read = grids.iter().map(Vec::len).max().unwrap_or(0);
        let wanted = if read < height { read + 1 } else { read };
        for grid in &mut grids {
            let width = grid[0].len();
            grid.resize(wanted, vec![ArrayNode::Empty; width]);
        }
        Ok(grids)
    }

    fn optional_number(
        &mut self,
        args: &[Node],
        i: usize,
        cell: CellReferenceIndex,
    ) -> Result<Option<f64>, CalcResult> {
        match args.get(i) {
            None | Some(Node::EmptyArgKind) => Ok(None),
            Some(node) => self.get_number(node, cell).map(|f| Some(f.trunc())),
        }
    }

    /// `sort_order`: one number or a vector of numbers (key columns only),
    /// 1-based over key columns then value columns, negative = descending.
    fn sort_spec(
        &mut self,
        args: &[Node],
        i: usize,
        keys: usize,
        values: usize,
        cell: CellReferenceIndex,
    ) -> Result<SortSpec, CalcResult> {
        let bad = || CalcResult::new_error(Error::VALUE, cell, "Invalid sort order".to_string());
        let numbers: Vec<f64> = match args.get(i) {
            None | Some(Node::EmptyArgKind) => vec![1.0],
            Some(node) => {
                let grid = self.eval_to_array(node, cell)?;
                let mut numbers = Vec::new();
                for value in grid.into_iter().flatten() {
                    match value {
                        ArrayNode::Number(f) => numbers.push(f.trunc()),
                        ArrayNode::Error(e) => {
                            return Err(CalcResult::new_error(e, cell, String::new()))
                        }
                        _ => return Err(bad()),
                    }
                }
                numbers
            }
        };
        let mut spec = Vec::new();
        for n in &numbers {
            let index = n.abs() as usize;
            if index == 0 || index > keys + values {
                return Err(bad());
            }
            if index > keys {
                if numbers.len() > 1 {
                    return Err(bad());
                }
                return Ok(SortSpec::Value(index - keys - 1, *n < 0.0));
            }
            spec.push((index - 1, *n < 0.0));
        }
        Ok(SortSpec::Keys(spec))
    }

    /// Headers (`field_headers`, automatic when missing) and filtered rows.
    fn prepare(
        &mut self,
        values: &Grid,
        field_headers: Option<f64>,
        filter: Option<&Grid>,
        levels: usize,
        cell: CellReferenceIndex,
    ) -> Result<Prepared, CalcResult> {
        let (has_headers, show_headers) = match field_headers {
            None => {
                let detected = matches!(values[0][0], ArrayNode::String(_))
                    && matches!(values.get(1).map(|r| &r[0]), Some(ArrayNode::Number(_)));
                (detected, detected && levels > 1)
            }
            Some(0.0) => (false, false),
            Some(1.0) => (true, false),
            Some(2.0) => (false, true),
            Some(3.0) => (true, true),
            Some(_) => {
                return Err(CalcResult::new_error(
                    Error::VALUE,
                    cell,
                    "Invalid field_headers".to_string(),
                ))
            }
        };
        let mut rows = Vec::new();
        for r in usize::from(has_headers)..values.len() {
            if let Some(filter) = filter {
                let keep = match &filter[r][0] {
                    ArrayNode::Boolean(b) => *b,
                    ArrayNode::Number(f) => *f != 0.0,
                    ArrayNode::Empty => false,
                    ArrayNode::Error(e) => {
                        return Err(CalcResult::new_error(e.clone(), cell, String::new()))
                    }
                    ArrayNode::String(_) => {
                        return Err(CalcResult::new_error(
                            Error::VALUE,
                            cell,
                            "Invalid filter".to_string(),
                        ))
                    }
                };
                if !keep {
                    continue;
                }
            }
            rows.push(r);
        }
        Ok(Prepared {
            has_headers,
            show_headers,
            rows,
        })
    }

    /// The function applied to the values of `rows` in column `v`, with the
    /// values of `relative` as a second argument when it takes two.
    #[allow(clippy::too_many_arguments)]
    fn aggregate_group(
        &mut self,
        agg: &Agg,
        values: &Grid,
        v: usize,
        rows: &[usize],
        relative: &[usize],
        cell: CellReferenceIndex,
    ) -> ArrayNode {
        let column = |rows: &[usize]| {
            if rows.is_empty() {
                // An empty group gets one blank, as an empty array can't be.
                return CalcResult::Array(vec![vec![ArrayNode::Empty]]);
            }
            CalcResult::Array(rows.iter().map(|&r| vec![values[r][v].clone()]).collect())
        };
        let mut arguments = vec![column(rows)];
        if agg.arity == 2 {
            arguments.push(column(relative));
        }
        result_to_node(self.call_lambda_with_values(agg.function.clone(), arguments, cell))
    }

    fn headers(grid: &Grid, has_headers: bool, first: usize, generated: &str) -> Vec<ArrayNode> {
        (0..grid[0].len())
            .map(|c| {
                if has_headers {
                    grid[0][c].clone()
                } else {
                    ArrayNode::String(format!("{generated} {}", first + c))
                }
            })
            .collect()
    }

    fn finish(mut out: Grid, cell: CellReferenceIndex) -> CalcResult {
        // A blank in a spilled array shows as 0; Excel leaves these cells blank.
        for node in out.iter_mut().flatten() {
            if *node == ArrayNode::Empty {
                *node = ArrayNode::String(String::new());
            }
        }
        if out.is_empty() || out[0].is_empty() {
            return CalcResult::new_error(Error::CALC, cell, "No data".to_string());
        }
        if let Some((error, message)) = check_array_size(out.len(), out[0].len()) {
            return CalcResult::new_error(error, cell, message);
        }
        CalcResult::Array(out)
    }

    fn total_depth(
        &mut self,
        args: &[Node],
        i: usize,
        keys: usize,
        hierarchy: bool,
        cell: CellReferenceIndex,
    ) -> Result<i64, CalcResult> {
        let depth = match self.optional_number(args, i, cell)? {
            Some(d) => d as i64,
            None => {
                if keys > 1 && hierarchy {
                    2
                } else {
                    1
                }
            }
        };
        if depth.unsigned_abs() as usize > keys.max(1) || (depth.abs() > 1 && !hierarchy) {
            return Err(CalcResult::new_error(
                Error::VALUE,
                cell,
                "Invalid total depth".to_string(),
            ));
        }
        Ok(depth)
    }

    pub(crate) fn fn_groupby(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        match self.groupby(args, cell) {
            Ok(r) | Err(r) => r,
        }
    }

    fn groupby(
        &mut self,
        args: &[Node],
        cell: CellReferenceIndex,
    ) -> Result<CalcResult, CalcResult> {
        if args.len() < 3 || args.len() > 8 {
            return Err(CalcResult::new_args_number_error(cell));
        }
        let mut nodes = vec![&args[0], &args[1]];
        let has_filter = !matches!(args.get(6), None | Some(Node::EmptyArgKind));
        if has_filter {
            nodes.push(&args[6]);
        }
        let grids = self.read_fields(&nodes, cell)?;
        let (fields, values) = (&grids[0], &grids[1]);
        let filter = if has_filter { grids.get(2) } else { None };
        let aggs = self.aggregations(&args[2], cell)?;
        let field_headers = self.optional_number(args, 3, cell)?;
        let hierarchy = match self.optional_number(args, 7, cell)? {
            None | Some(0.0) => true,
            Some(1.0) => false,
            Some(_) => {
                return Err(CalcResult::new_error(
                    Error::VALUE,
                    cell,
                    "Invalid field_relationship".to_string(),
                ))
            }
        };
        let k = fields[0].len();
        let nv = values[0].len();
        let nf = aggs.list.len();
        // Value columns of the result: per function, per value column (one
        // function per row when vertical).
        let columns = if aggs.vertical { nv } else { nf * nv };
        let depth = self.total_depth(args, 4, k, hierarchy, cell)?;
        let sort = self.sort_spec(args, 5, k, columns, cell)?;
        let prepared = self.prepare(values, field_headers, filter, k, cell)?;
        let rows = prepared.rows.clone();

        let axis = Axis {
            keys: fields,
            width: k,
        };
        let vertical = aggs.vertical;
        let mut value_of = |group: &[usize], c: usize| {
            let (f, v) = if vertical { (0, c) } else { (c / nv, c % nv) };
            self.aggregate_group(&aggs.list[f], values, v, group, &rows, cell)
        };
        let lines = axis.lines(&rows, depth, &sort, hierarchy, &mut value_of);

        let mut out: Grid = Vec::new();
        let value_headers = Self::headers(values, prepared.has_headers, 1, "Value");
        let key_headers = |show: bool| {
            if show {
                Self::headers(fields, prepared.has_headers, 1, "Field")
            } else {
                vec![ArrayNode::Empty; k]
            }
        };
        if vertical {
            if prepared.show_headers {
                let mut header = key_headers(true);
                header.push(ArrayNode::Empty);
                header.extend(value_headers);
                out.push(header);
            }
            for line in lines {
                for agg in &aggs.list {
                    let mut row = line.keys.clone();
                    row.push(agg.label.clone());
                    for v in 0..nv {
                        row.push(self.aggregate_group(agg, values, v, &line.rows, &rows, cell));
                    }
                    out.push(row);
                }
            }
            return Ok(Self::finish(out, cell));
        }
        if nf > 1 {
            // Function names; over a row of value headers when there are
            // several value columns.
            let mut header = key_headers(prepared.show_headers && nv == 1);
            for agg in &aggs.list {
                for v in 0..nv {
                    header.push(if v == 0 {
                        agg.label.clone()
                    } else {
                        ArrayNode::Empty
                    });
                }
            }
            out.push(header);
            if nv > 1 {
                let mut header = key_headers(prepared.show_headers);
                for _ in 0..nf {
                    header.extend(value_headers.iter().cloned());
                }
                out.push(header);
            }
        } else if prepared.show_headers {
            let mut header = key_headers(true);
            header.extend(value_headers);
            out.push(header);
        }
        for line in lines {
            let mut row = line.keys;
            for agg in &aggs.list {
                for v in 0..nv {
                    row.push(self.aggregate_group(agg, values, v, &line.rows, &rows, cell));
                }
            }
            out.push(row);
        }
        Ok(Self::finish(out, cell))
    }

    pub(crate) fn fn_pivotby(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        match self.pivotby(args, cell) {
            Ok(r) | Err(r) => r,
        }
    }

    fn pivotby(
        &mut self,
        args: &[Node],
        cell: CellReferenceIndex,
    ) -> Result<CalcResult, CalcResult> {
        if args.len() < 4 || args.len() > 11 {
            return Err(CalcResult::new_args_number_error(cell));
        }
        let mut nodes = vec![&args[0], &args[1], &args[2]];
        let has_filter = !matches!(args.get(9), None | Some(Node::EmptyArgKind));
        if has_filter {
            nodes.push(&args[9]);
        }
        let grids = self.read_fields(&nodes, cell)?;
        let (row_fields, col_fields, values) = (&grids[0], &grids[1], &grids[2]);
        let filter = if has_filter { grids.get(3) } else { None };
        let aggs = self.aggregations(&args[3], cell)?;
        let field_headers = self.optional_number(args, 4, cell)?;
        let kr = row_fields[0].len();
        let kc = col_fields[0].len();
        let nv = values[0].len();
        let nf = aggs.list.len();
        let vertical = aggs.vertical;
        // Value columns under each column line: per function, per value
        // column (one function per row when vertical).
        let per_line = if vertical { nv } else { nf * nv };
        let row_depth = self.total_depth(args, 5, kr, true, cell)?;
        let row_sort = self.sort_spec(args, 6, kr, per_line, cell)?;
        let col_depth = self.total_depth(args, 7, kc, true, cell)?;
        let col_sort = self.sort_spec(args, 8, kc, per_line, cell)?;
        let relative_to = match self.optional_number(args, 10, cell)? {
            None => 0,
            Some(r) if (0.0..=4.0).contains(&r) => r as u8,
            Some(_) => {
                return Err(CalcResult::new_error(
                    Error::VALUE,
                    cell,
                    "Invalid relative_to".to_string(),
                ))
            }
        };
        let prepared = self.prepare(values, field_headers, filter, kr.max(kc), cell)?;
        let rows = prepared.rows.clone();

        let row_axis = Axis {
            keys: row_fields,
            width: kr,
        };
        let col_axis = Axis {
            keys: col_fields,
            width: kc,
        };
        let mut value_of = |group: &[usize], c: usize| {
            let (f, v) = if vertical { (0, c) } else { (c / nv, c % nv) };
            self.aggregate_group(&aggs.list[f], values, v, group, &rows, cell)
        };
        let row_lines = row_axis.lines(&rows, row_depth, &row_sort, true, &mut value_of);
        let col_lines = col_axis.lines(&rows, col_depth, &col_sort, true, &mut value_of);

        // The rows of a line's parent (one key column fewer), within `within`.
        let parent_rows = |axis: &Axis, line: &Line, within: &[usize]| -> Vec<usize> {
            if line.prefix_len == 0 {
                return within.to_vec();
            }
            axis.rows_with_prefix(&line.keys[..line.prefix_len - 1], within)
        };

        // The header cells over the columns of one column line: (function
        // label, value name) of each.
        let value_names = Self::headers(values, prepared.has_headers, 1, "Value");
        let mut sub: Vec<(ArrayNode, ArrayNode)> = Vec::new();
        if vertical {
            for name in &value_names {
                sub.push((ArrayNode::Empty, name.clone()));
            }
        } else {
            for agg in &aggs.list {
                for (v, name) in value_names.iter().enumerate() {
                    let label = if v == 0 {
                        agg.label.clone()
                    } else {
                        ArrayNode::Empty
                    };
                    sub.push((label, name.clone()));
                }
            }
        }
        let function_row = !vertical && nf > 1;
        let value_row = nv > 1 || (prepared.show_headers && !function_row);
        let header_rows = kc + usize::from(function_row) + usize::from(value_row);
        let left = kr + usize::from(vertical);
        let mut out: Grid = Vec::new();
        for level in 0..header_rows {
            let mut row = vec![ArrayNode::Empty; left];
            let last = level + 1 == header_rows;
            if prepared.show_headers && last {
                row = Self::headers(row_fields, prepared.has_headers, 1, "Field");
                if vertical {
                    row.push(ArrayNode::Empty);
                }
            }
            for line in &col_lines {
                for (i, (label, name)) in sub.iter().enumerate() {
                    let node = if level < kc {
                        if i > 0 {
                            ArrayNode::Empty
                        } else if line.prefix_len == 0 {
                            if level == 0 {
                                line.keys[0].clone()
                            } else {
                                ArrayNode::Empty
                            }
                        } else if level < line.prefix_len {
                            line.keys[level].clone()
                        } else {
                            ArrayNode::Empty
                        }
                    } else if function_row && level == kc {
                        label.clone()
                    } else {
                        name.clone()
                    };
                    row.push(node);
                }
            }
            out.push(row);
        }
        let row_aggs: Vec<Option<usize>> = if vertical {
            (0..nf).map(Some).collect()
        } else {
            vec![None]
        };
        for row_line in &row_lines {
            for row_agg in &row_aggs {
                let mut row = row_line.keys.clone();
                if let Some(f) = row_agg {
                    row.push(aggs.list[*f].label.clone());
                }
                for col_line in &col_lines {
                    let group = intersect(&row_line.rows, &col_line.rows);
                    let functions: Vec<usize> = match row_agg {
                        Some(f) => vec![*f],
                        None => (0..nf).collect(),
                    };
                    for f in functions {
                        let agg = &aggs.list[f];
                        for v in 0..nv {
                            if group.is_empty() {
                                row.push(ArrayNode::Empty);
                                continue;
                            }
                            let relative = if agg.arity == 2 {
                                match relative_to {
                                    0 => col_line.rows.clone(),
                                    1 => row_line.rows.clone(),
                                    2 => rows.clone(),
                                    3 => parent_rows(&row_axis, row_line, &col_line.rows),
                                    _ => parent_rows(&col_axis, col_line, &row_line.rows),
                                }
                            } else {
                                Vec::new()
                            };
                            row.push(self.aggregate_group(agg, values, v, &group, &relative, cell));
                        }
                    }
                }
                out.push(row);
            }
        }
        Ok(Self::finish(out, cell))
    }
}
