//! Tables, conditional formats, data validation, defined names, pivot
//! tables and page setup ([`super::sheets_author`]'s tools).

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Map, Value, json};
use sheets_engine::{
    AxisEntry, CfBarOptions, CfCompare, CfFormat, CfPeriod, CfRuleDef, CfSpec, CfTextOp,
    ErrorStyle, Message, Paper, PivotDestination, PivotFunc, ReportLayout, TotalFunc, Validation,
    ValidationOperator, ValidationType, Workbook, default_icon_steps,
};

use super::sheets_author::{
    get, opt_bool, opt_count, opt_num, opt_str, parse_ref, range_arg, req_str, rgb, sheet_arg,
    used_part,
};

pub(super) fn run(
    book: &mut Workbook,
    tool: &str,
    o: &Map<String, Value>,
) -> Result<(String, Value)> {
    match tool {
        "add_table" => add_table(book, o),
        "add_conditional_format" => add_cf(book, o),
        "clear_conditional_formats" => {
            let sheet = sheet_arg(book, o)?;
            let within = match get(o, "range") {
                Some(_) => Some(range_arg(book, o, "range")?.1),
                None => None,
            };
            let n = book.clear_cf_rules(sheet, within)?;
            Ok((
                format!("removed {n} conditional format(s)"),
                json!({"removed": n}),
            ))
        }
        "add_validation" => add_validation(book, o),
        "clear_validation" => {
            let (sheet, r) = range_arg(book, o, "range")?;
            book.clear_validation(sheet, &[r])?;
            Ok((format!("cleared validation of {}", r.name()), json!({})))
        }
        "define_name" => {
            let name = req_str(o, "name")?;
            let text = match get(o, "refers_to") {
                Some(Value::String(s)) => s.clone(),
                _ => bail!("`refers_to` is a reference such as Data!$B$2:$B$100"),
            };
            let scope = match get(o, "sheet") {
                Some(_) => Some(sheet_arg(book, o)?),
                None => None,
            };
            // Names of cells only (IronCalc can't evaluate names of constants
            // or formulas), written absolute with their sheet.
            let Ok((s, r)) = parse_ref(book, &text, scope.unwrap_or(book.active_sheet())) else {
                bail!(
                    "`refers_to` must be cells such as Data!$B$2:$B$100; names of values or formulas aren't supported: put the value in a cell (e.g. Inputs!$B$2) and name that cell"
                );
            };
            let r = used_part(book, s, r);
            let formula = super::sheets_chart::abs_ref(book, s, r);
            book.set_defined_name(name, scope, &formula)?;
            Ok((format!("defined {name}"), json!({"name": name})))
        }
        "delete_name" => {
            let name = req_str(o, "name")?;
            let scope = match get(o, "sheet") {
                Some(_) => Some(sheet_arg(book, o)?),
                None => None,
            };
            book.delete_defined_name(name, scope)?;
            Ok((format!("deleted name {name}"), json!({})))
        }
        "add_pivot" => add_pivot(book, o),
        "page_setup" => page_setup(book, o),
        other => bail!("unknown tool {other}"),
    }
}

// ── tables ─────────────────────────────────────────────────────────────────

fn total_func(s: &str) -> Result<TotalFunc> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "none" => TotalFunc::None,
        "sum" => TotalFunc::Sum,
        "average" | "avg" => TotalFunc::Average,
        "count" => TotalFunc::Count,
        "count_numbers" | "countnums" => TotalFunc::CountNumbers,
        "max" => TotalFunc::Max,
        "min" => TotalFunc::Min,
        "stddev" | "std_dev" => TotalFunc::StdDev,
        "var" => TotalFunc::Var,
        other => bail!(
            "a total is none, sum, average, count, count_numbers, max, min, stddev or var, not \"{other}\""
        ),
    })
}

fn add_table(book: &mut Workbook, o: &Map<String, Value>) -> Result<(String, Value)> {
    let (sheet, r) = range_arg(book, o, "range")?;
    let r = used_part(book, sheet, r);
    let style = opt_str(o, "style")?.unwrap_or(sheets_engine::DEFAULT_TABLE_STYLE);
    if style != "none"
        && !sheets_engine::table_style_names()
            .iter()
            .any(|s| s == style)
    {
        bail!(
            "there is no table style \"{style}\" (TableStyleLight1-21, TableStyleMedium1-28, TableStyleDark1-11, or none)"
        );
    }
    let header = opt_bool(o, "header")?.unwrap_or(true);
    let mut name = book.create_table(
        sheet,
        r,
        header,
        if style == "none" {
            sheets_engine::DEFAULT_TABLE_STYLE
        } else {
            style
        },
    )?;
    if let Some(want) = opt_str(o, "name")?
        && want != name
    {
        if !sheets_engine::valid_table_name(want) {
            bail!(
                "\"{want}\" is not a table name (letters, digits, _ and ., starting with a letter)"
            );
        }
        book.rename_table(&name, want)?;
        name = want.to_owned();
    }
    let mut opts = book
        .table_options(&name)
        .ok_or_else(|| anyhow!("no table {name}"))?;
    let before = opts.clone();
    if style == "none" {
        opts.style = None;
    }
    if let Some(b) = opt_bool(o, "banded_rows")? {
        opts.banded_rows = b;
    }
    if let Some(b) = opt_bool(o, "first_column")? {
        opts.first_column = b;
    }
    if let Some(b) = opt_bool(o, "last_column")? {
        opts.last_column = b;
    }
    if let Some(b) = opt_bool(o, "filter_buttons")? {
        opts.filter_button = b;
    }
    let totals = get(o, "totals");
    if totals.is_some_and(|t| t != &Value::Bool(false)) {
        opts.total_row = true;
    }
    if opts != before {
        book.set_table_options(&name, opts)?;
    }
    if let Some(Value::Object(m)) = totals {
        let info = book
            .tables(sheet)
            .into_iter()
            .find(|t| t.name == name)
            .context("the new table")?;
        for (col, f) in m {
            let i = info
                .columns
                .iter()
                .position(|c| c.eq_ignore_ascii_case(col))
                .with_context(|| {
                    format!(
                        "`totals`: the table has no column \"{col}\" (columns: {})",
                        info.columns.join(", ")
                    )
                })?;
            let f = f
                .as_str()
                .context("`totals` maps column names to sum, average, count...")?;
            book.set_table_total(&name, i, total_func(f)?)?;
        }
    } else if let Some(t) = totals
        && !matches!(t, Value::Bool(_))
    {
        bail!(
            "`totals` is true (a total row) or maps column names to functions, e.g. {{\"Revenue\": \"sum\"}}"
        );
    }
    let area = book
        .tables(sheet)
        .into_iter()
        .find(|t| t.name == name)
        .map(|t| t.area.name())
        .unwrap_or_default();
    Ok((
        format!("made table {name}"),
        json!({"table": name, "range": area}),
    ))
}

// ── conditional formats ────────────────────────────────────────────────────

/// A rule's format: `{fill, font_color, bold, italic, underline, strike}`
/// or a preset name; default light red fill with dark red text.
fn cf_format(v: Option<&Value>) -> Result<CfFormat> {
    let Some(v) = v else {
        return Ok(CfFormat::PRESETS[0].1);
    };
    match v {
        Value::String(s) => {
            let presets = [
                ("red", 0usize),
                ("light_red", 0),
                ("yellow", 1),
                ("green", 2),
                ("red_fill", 3),
                ("red_text", 4),
                ("bold", 5),
            ];
            presets
                .iter()
                .find(|(n, _)| *n == s)
                .map(|(_, i)| CfFormat::PRESETS[*i].1)
                .with_context(|| {
                    format!("`format` \"{s}\": use red, yellow, green, red_fill, red_text, bold or {{fill, font_color, bold...}}")
                })
        }
        Value::Object(m) => {
            for k in m.keys() {
                if ![
                    "fill",
                    "font_color",
                    "color",
                    "bold",
                    "italic",
                    "underline",
                    "strike",
                ]
                .contains(&k.as_str())
                {
                    bail!(
                        "a rule's `format` has no \"{k}\" (keys: fill, font_color, bold, italic, underline, strike)"
                    );
                }
            }
            Ok(CfFormat {
                fill: get(m, "fill").map(|c| rgb(c, "fill")).transpose()?,
                font: get(m, "font_color")
                    .or_else(|| get(m, "color"))
                    .map(|c| rgb(c, "font_color"))
                    .transpose()?,
                bold: opt_bool(m, "bold")?.unwrap_or(false),
                italic: opt_bool(m, "italic")?.unwrap_or(false),
                underline: opt_bool(m, "underline")?.unwrap_or(false),
                strike: opt_bool(m, "strike")?.unwrap_or(false),
            })
        }
        _ => bail!("`format` is a preset name or {{fill, font_color, bold, ...}}"),
    }
}

/// A rule's operand as typed: numbers, `=formulas`, text.
fn operand(v: Option<&Value>, key: &str) -> Result<String> {
    Ok(match v {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
        None => bail!("this rule needs `{key}`"),
        Some(_) => bail!("`{key}` is a number, text or =formula"),
    })
}

/// Excel's colour scale presets, lowest value's colour first.
const SCALES: [(&str, &[u32]); 8] = [
    ("red_yellow_green", &[0xF8696B, 0xFFEB84, 0x63BE7B]),
    ("green_yellow_red", &[0x63BE7B, 0xFFEB84, 0xF8696B]),
    ("red_white_green", &[0xF8696B, 0xFCFCFF, 0x63BE7B]),
    ("green_white_red", &[0x63BE7B, 0xFCFCFF, 0xF8696B]),
    ("red_white_blue", &[0xF8696B, 0xFCFCFF, 0x5A8AC6]),
    ("blue_white_red", &[0x5A8AC6, 0xFCFCFF, 0xF8696B]),
    ("white_green", &[0xFCFCFF, 0x63BE7B]),
    ("white_red", &[0xFCFCFF, 0xF8696B]),
];

/// Icon sets by friendly name and by Excel's.
const ICONS: [(&str, &str); 12] = [
    ("arrows", "3Arrows"),
    ("arrows_gray", "3ArrowsGray"),
    ("triangles", "3Triangles"),
    ("arrows4", "4Arrows"),
    ("arrows5", "5Arrows"),
    ("traffic_lights", "3TrafficLights1"),
    ("traffic_lights4", "4TrafficLights"),
    ("signs", "3Signs"),
    ("symbols", "3Symbols"),
    ("flags", "3Flags"),
    ("stars", "3Stars"),
    ("ratings", "5Rating"),
];

fn add_cf(book: &mut Workbook, o: &Map<String, Value>) -> Result<(String, Value)> {
    let (sheet, r) = range_arg(book, o, "range")?;
    let r = used_part(book, sheet, r);
    let ty = req_str(o, "type")?;
    let format = cf_format(get(o, "format"))?;
    let spec = match ty {
        "data_bar" => CfSpec::DataBar {
            color: get(o, "color")
                .map(|c| rgb(c, "color"))
                .transpose()?
                .unwrap_or(0x638EC6),
            gradient: opt_bool(o, "gradient")?.unwrap_or(true),
            bar: CfBarOptions {
                bar_only: opt_bool(o, "show_value")? == Some(false),
                ..CfBarOptions::default()
            },
        },
        "color_scale" => {
            let colors = match get(o, "colors") {
                None => SCALES[0].1.to_vec(),
                Some(Value::String(s)) => SCALES
                    .iter()
                    .find(|(n, _)| n == s)
                    .map(|(_, c)| c.to_vec())
                    .with_context(|| {
                        format!(
                            "`colors` \"{s}\": use {} or a list of 2 or 3 colours, lowest first",
                            SCALES.iter().map(|s| s.0).collect::<Vec<_>>().join(", ")
                        )
                    })?,
                Some(Value::Array(a)) => a
                    .iter()
                    .map(|c| rgb(c, "colors"))
                    .collect::<Result<Vec<_>>>()?,
                Some(_) => bail!("`colors` is a preset name or a list of colours"),
            };
            if !(2..=3).contains(&colors.len()) {
                bail!("a colour scale has 2 or 3 colours (lowest value's first)");
            }
            CfSpec::ColorScale { colors }
        }
        "icon_set" => {
            let want = opt_str(o, "icons")?.unwrap_or("arrows");
            let set = ICONS
                .iter()
                .find(|(n, x)| *n == want || *x == want)
                .map(|(_, x)| (*x).to_owned())
                .with_context(|| {
                    format!(
                        "`icons` \"{want}\": use {}",
                        ICONS.iter().map(|i| i.0).collect::<Vec<_>>().join(", ")
                    )
                })?;
            CfSpec::IconSet {
                steps: default_icon_steps(&set),
                set,
                reverse: opt_bool(o, "reverse")?.unwrap_or(false),
                show_value: opt_bool(o, "show_value")?.unwrap_or(true),
            }
        }
        "cell_value" => {
            let op = match opt_str(o, "operator")?.context("a cell_value rule needs `operator`")? {
                ">" | "greater_than" => CfCompare::GreaterThan,
                "<" | "less_than" => CfCompare::LessThan,
                ">=" | "greater_or_equal" => CfCompare::GreaterOrEqual,
                "<=" | "less_or_equal" => CfCompare::LessOrEqual,
                "=" | "==" | "equal" => CfCompare::EqualTo,
                "!=" | "<>" | "not_equal" => CfCompare::NotEqualTo,
                "between" => CfCompare::Between,
                "not_between" => CfCompare::NotBetween,
                other => {
                    bail!("`operator` \"{other}\": use >, <, >=, <=, =, !=, between or not_between")
                }
            };
            let value2 = if op.two_values() {
                operand(get(o, "value2"), "value2")?
            } else {
                String::new()
            };
            CfSpec::CellValue {
                op,
                value: operand(get(o, "value"), "value")?,
                value2,
            }
        }
        "text" => {
            let op = match opt_str(o, "operator")?.unwrap_or("contains") {
                "contains" => CfTextOp::Contains,
                "not_contains" => CfTextOp::DoesNotContain,
                "begins_with" => CfTextOp::BeginsWith,
                "ends_with" => CfTextOp::EndsWith,
                "equals" => CfTextOp::Equals,
                other => bail!(
                    "text `operator` \"{other}\": use contains, not_contains, begins_with, ends_with or equals"
                ),
            };
            CfSpec::Text {
                op,
                text: req_str(o, "text")?.to_owned(),
            }
        }
        "top" | "bottom" => CfSpec::TopBottom {
            bottom: ty == "bottom",
            rank: opt_count(o, "rank", 1)?.unwrap_or(10).min(1000) as u32,
            percent: opt_bool(o, "percent")?.unwrap_or(false),
        },
        "above_average" | "below_average" => CfSpec::Average {
            below: ty == "below_average",
            equal: false,
            std_dev: 0,
        },
        "duplicates" | "unique" => CfSpec::Duplicates {
            unique: ty == "unique",
        },
        "blanks" | "not_blanks" => CfSpec::Blanks {
            not: ty == "not_blanks",
        },
        "formula" => CfSpec::Formula {
            formula: req_str(o, "formula")?.trim_start_matches('=').to_owned(),
        },
        "dates" => {
            let p = opt_str(o, "period")?.context("a dates rule needs `period`")?;
            let period = match p {
                "yesterday" => CfPeriod::Yesterday,
                "today" => CfPeriod::Today,
                "tomorrow" => CfPeriod::Tomorrow,
                "last_7_days" => CfPeriod::Last7Days,
                "last_week" => CfPeriod::LastWeek,
                "this_week" => CfPeriod::ThisWeek,
                "next_week" => CfPeriod::NextWeek,
                "last_month" => CfPeriod::LastMonth,
                "this_month" => CfPeriod::ThisMonth,
                "next_month" => CfPeriod::NextMonth,
                other => bail!(
                    "`period` \"{other}\": use yesterday, today, tomorrow, last_7_days, last_week, this_week, next_week, last_month, this_month or next_month"
                ),
            };
            CfSpec::Dates { period }
        }
        other => bail!(
            "`type` \"{other}\": use data_bar, color_scale, icon_set, cell_value, text, top, bottom, above_average, below_average, duplicates, unique, blanks, not_blanks, formula or dates"
        ),
    };
    let def = CfRuleDef {
        format: if spec.has_format() {
            format
        } else {
            CfFormat::default()
        },
        spec,
        stop_if_true: opt_bool(o, "stop_if_true")?.unwrap_or(false),
        num_fmt: None,
        border: None,
    };
    book.add_cf_rule(sheet, &r.name(), &def)?;
    Ok((
        format!("added a conditional format ({ty}) to {}", r.name()),
        json!({"range": r.name()}),
    ))
}

// ── data validation ────────────────────────────────────────────────────────

fn message(v: Option<&Value>, key: &str) -> Result<Option<Message>> {
    Ok(match v {
        None => None,
        Some(Value::String(s)) => Some(Message {
            title: String::new(),
            text: s.clone(),
        }),
        Some(Value::Object(m)) => Some(Message {
            title: opt_str(m, "title")?.unwrap_or_default().to_owned(),
            text: opt_str(m, "text")?.unwrap_or_default().to_owned(),
        }),
        Some(_) => bail!("`{key}` is text or {{title, text}}"),
    })
}

fn add_validation(book: &mut Workbook, o: &Map<String, Value>) -> Result<(String, Value)> {
    let (sheet, r) = range_arg(book, o, "range")?;
    let ty = req_str(o, "type")?;
    let kind = match ty {
        "list" => ValidationType::List,
        "whole" => ValidationType::Whole,
        "decimal" => ValidationType::Decimal,
        "date" => ValidationType::Date,
        "time" => ValidationType::Time,
        "text_length" => ValidationType::TextLength,
        "custom" => ValidationType::Custom,
        "any" => ValidationType::Any,
        other => bail!(
            "`type` \"{other}\": use list, whole, decimal, date, time, text_length, custom or any"
        ),
    };
    let op = match opt_str(o, "operator")?.unwrap_or("between") {
        "between" => ValidationOperator::Between,
        "not_between" => ValidationOperator::NotBetween,
        "=" | "equal" => ValidationOperator::Equal,
        "!=" | "not_equal" => ValidationOperator::NotEqual,
        ">" | "greater_than" => ValidationOperator::GreaterThan,
        "<" | "less_than" => ValidationOperator::LessThan,
        ">=" | "greater_or_equal" => ValidationOperator::GreaterThanOrEqual,
        "<=" | "less_or_equal" => ValidationOperator::LessThanOrEqual,
        other => bail!("`operator` \"{other}\": use between, not_between, =, !=, >, <, >= or <="),
    };
    let (formula1, formula2) = match kind {
        ValidationType::List => {
            let src = match (get(o, "values"), opt_str(o, "source")?) {
                (Some(Value::Array(items)), None) => items
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => Ok(s.clone()),
                        Value::Number(n) => Ok(n.to_string()),
                        _ => bail!("list `values` are text or numbers"),
                    })
                    .collect::<Result<Vec<_>>>()?
                    .join(","),
                (None, Some(s)) => {
                    let s = s.trim_start_matches('=');
                    // A range (on any sheet) as an absolute reference.
                    let (s_ix, rr) = parse_ref(book, s, sheet)?;
                    let q = super::sheets_author::quoted(book, s_ix);
                    let abs = format!(
                        "${}${}:${}${}",
                        sheets_engine::column_name(rr.left),
                        rr.top + 1,
                        sheets_engine::column_name(rr.right),
                        rr.bottom + 1
                    );
                    format!(
                        "={}{abs}",
                        if s_ix == sheet {
                            String::new()
                        } else {
                            format!("{q}!")
                        }
                    )
                }
                _ => bail!(
                    "a list needs `values` (an array) or `source` (cells such as Lists!A2:A9)"
                ),
            };
            (Some(sheets_engine::list_source(&src)?), None)
        }
        ValidationType::Custom => (
            Some(req_str(o, "formula")?.trim_start_matches('=').to_owned()),
            None,
        ),
        ValidationType::Any => (None, None),
        _ => {
            let two = matches!(
                op,
                ValidationOperator::Between | ValidationOperator::NotBetween
            );
            let text = |key: &str| -> Result<String> {
                match get(o, key) {
                    Some(Value::Number(n)) => Ok(n.to_string()),
                    Some(Value::String(s)) => Ok(s.clone()),
                    _ => bail!("this validation needs `{key}`"),
                }
            };
            let v1 = sheets_engine::validation_operand(kind, &text("value")?)?;
            let v2 = if two {
                Some(sheets_engine::validation_operand(kind, &text("value2")?)?)
            } else {
                None
            };
            (Some(v1), v2)
        }
    };
    let style = match opt_str(o, "error_style")?.unwrap_or("stop") {
        "stop" => ErrorStyle::Stop,
        "warning" => ErrorStyle::Warning,
        "information" | "info" => ErrorStyle::Information,
        other => bail!("`error_style` \"{other}\": use stop, warning or information"),
    };
    let error = match message(get(o, "error"), "error")? {
        Some(m) => Some((style, m)),
        None if kind == ValidationType::Any => None,
        None => Some((
            style,
            Message {
                title: String::new(),
                text: String::new(),
            },
        )),
    };
    let rule = Validation {
        kind,
        operator: op,
        allow_blank: opt_bool(o, "allow_blank")?.unwrap_or(true),
        dropdown: opt_bool(o, "dropdown")?.unwrap_or(true),
        prompt: message(get(o, "prompt"), "prompt")?,
        error,
        formula1,
        formula2,
        ranges: vec![r],
        ime_mode: None,
    };
    book.set_validation(sheet, &[r], rule)?;
    Ok((
        format!("added {ty} validation to {}", r.name()),
        json!({"range": r.name()}),
    ))
}

// ── pivot tables ───────────────────────────────────────────────────────────

fn pivot_func(s: &str) -> Result<PivotFunc> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "sum" => PivotFunc::Sum,
        "count" => PivotFunc::Count,
        "average" | "avg" => PivotFunc::Average,
        "max" => PivotFunc::Max,
        "min" => PivotFunc::Min,
        "product" => PivotFunc::Product,
        "count_numbers" => PivotFunc::CountNumbers,
        "stddev" => PivotFunc::StdDev,
        "stddevp" => PivotFunc::StdDevp,
        "var" => PivotFunc::Var,
        "varp" => PivotFunc::Varp,
        other => bail!(
            "a pivot value's `func` is sum, count, average, max, min, product, count_numbers, stddev, stddevp, var or varp, not \"{other}\""
        ),
    })
}

fn names_of(v: Option<&Value>, key: &str) -> Result<Vec<String>> {
    Ok(match v {
        None => Vec::new(),
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| {
                x.as_str()
                    .map(str::to_owned)
                    .with_context(|| format!("`{key}` holds field names"))
            })
            .collect::<Result<_>>()?,
        Some(_) => bail!("`{key}` is a field name or a list of them"),
    })
}

fn add_pivot(book: &mut Workbook, o: &Map<String, Value>) -> Result<(String, Value)> {
    let source = req_str(o, "source")?;
    let active = book.active_sheet();
    // A table name, or a range (its sheet's name added when missing).
    let table_sheet = (0..book.sheet_names().len() as u32).find(|&s| {
        book.tables(s)
            .iter()
            .any(|t| t.name.eq_ignore_ascii_case(source))
    });
    let (src_text, src_sheet) = if let Some(ts) = table_sheet {
        (source.to_owned(), ts)
    } else {
        let (s, r) = parse_ref(book, source, active)?;
        let r = used_part(book, s, r);
        (
            format!(
                "{}!{}",
                super::sheets_author::quoted(book, s),
                sheets_engine::formula::format_reference(None, r)
            ),
            s,
        )
    };
    let dest = match (opt_str(o, "at")?, opt_str(o, "new_sheet")?) {
        (Some(at), None) => {
            let (s, r) = parse_ref(book, at, active)?;
            PivotDestination::Cell {
                sheet: s,
                row: r.top,
                col: r.left,
            }
        }
        (None, _) => PivotDestination::NewSheet,
        (Some(_), Some(_)) => bail!("give `at` or `new_sheet`, not both"),
    };
    let (sheet, row, col) = book.insert_pivot(&src_text, src_sheet, dest)?;
    if let Some(name) = opt_str(o, "new_sheet")? {
        book.rename_sheet(sheet, name)?;
    }
    let mut l = book
        .pivot_layout(sheet, row, col)
        .context("the new pivot table")?;
    let field = |l: &sheets_engine::PivotLayout, name: &str| -> Result<usize> {
        l.fields
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(name.trim()))
            .with_context(|| {
                format!(
                    "the source has no field \"{name}\" (fields: {})",
                    l.fields
                        .iter()
                        .map(|f| f.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    };
    for n in names_of(get(o, "rows"), "rows")? {
        let f = field(&l, &n)?;
        l.rows.push(AxisEntry::Field(f));
    }
    for n in names_of(get(o, "columns"), "columns")? {
        let f = field(&l, &n)?;
        l.columns.push(AxisEntry::Field(f));
    }
    for n in names_of(get(o, "filters"), "filters")? {
        let f = field(&l, &n)?;
        l.filters.push(f);
        l.filter_item.push(None);
    }
    let values = match get(o, "values") {
        Some(Value::Array(a)) => a.clone(),
        Some(v) => vec![v.clone()],
        None => Vec::new(),
    };
    if values.is_empty() {
        bail!("a pivot table needs `values`: [{{\"field\": \"Revenue\", \"func\": \"sum\"}}]");
    }
    for v in &values {
        let (fname, func, name, fmt) = match v {
            Value::String(s) => (s.clone(), None, None, None),
            Value::Object(m) => {
                for k in m.keys() {
                    if !["field", "func", "name", "number_format"].contains(&k.as_str()) {
                        bail!(
                            "a pivot value has no \"{k}\" (keys: field, func, name, number_format)"
                        );
                    }
                }
                (
                    req_str(m, "field")?.to_owned(),
                    opt_str(m, "func")?.map(pivot_func).transpose()?,
                    opt_str(m, "name")?.map(str::to_owned),
                    opt_str(m, "number_format")?.map(super::sheets_author::number_format_code),
                )
            }
            _ => bail!("a pivot value is a field name or {{field, func, name, number_format}}"),
        };
        let f = field(&l, &fname)?;
        l.add_value(f);
        let last = l.values.last_mut().expect("just added");
        if let Some(func) = func {
            last.func = func;
        }
        if name.is_some() {
            last.name = name;
        }
        if fmt.is_some() {
            last.num_fmt = fmt;
        }
    }
    if l.values.len() > 1
        && !l.rows.contains(&AxisEntry::Values)
        && !l.columns.contains(&AxisEntry::Values)
    {
        l.columns.push(AxisEntry::Values);
    }
    l.layout = match opt_str(o, "layout")?.unwrap_or("compact") {
        "compact" => ReportLayout::Compact,
        "outline" => ReportLayout::Outline,
        "tabular" => ReportLayout::Tabular,
        other => bail!("`layout` \"{other}\": use compact, outline or tabular"),
    };
    if let Some(g) = opt_bool(o, "grand_totals")? {
        l.grand_total_row = g;
        l.grand_total_column = g;
    }
    if let Some(s) = opt_str(o, "style")? {
        l.style.name = (s != "none").then(|| s.to_owned());
    }
    book.set_pivot_layout(sheet, row, col, &l)?;
    let info = book.pivot_at(sheet, row, col).context("the pivot table")?;
    let sheet_name = book.sheet_names()[sheet as usize].clone();
    Ok((
        format!("made pivot table {} on {sheet_name}", info.name),
        json!({"pivot": info.name, "sheet": sheet_name, "range": info.area.name()}),
    ))
}

// ── page setup ─────────────────────────────────────────────────────────────

fn page_setup(book: &mut Workbook, o: &Map<String, Value>) -> Result<(String, Value)> {
    let sheet = sheet_arg(book, o)?;
    let mut p = book.print_settings(sheet);
    if let Some(l) = opt_bool(o, "landscape")? {
        p.setup.landscape = l;
    }
    if let Some(name) = opt_str(o, "paper")? {
        p.setup.paper = Paper::from_name(name).with_context(|| {
            format!("`paper` \"{name}\": a4, letter, legal, f4, a3, a5, or WxHmm")
        })?;
    }
    match get(o, "fit") {
        None => {}
        Some(Value::String(s)) if s == "width" => p.setup.fit = Some((1, 0)),
        Some(Value::String(s)) if s == "page" => p.setup.fit = Some((1, 1)),
        Some(Value::String(s)) if s == "none" => p.setup.fit = None,
        Some(Value::Array(a)) if a.len() == 2 => {
            let n = |v: &Value| v.as_u64().filter(|n| *n <= 32767).map(|n| n as u32);
            p.setup.fit = Some((
                n(&a[0]).context("`fit` is [pages wide, pages tall]")?,
                n(&a[1]).context("`fit` is [pages wide, pages tall]")?,
            ));
        }
        Some(_) => bail!("`fit` is \"width\", \"page\", \"none\" or [pages wide, pages tall]"),
    }
    if let Some(s) = opt_num(o, "scale")? {
        if !(10.0..=400.0).contains(&s) {
            bail!("`scale` is 10 to 400 (%)");
        }
        p.setup.scale = s.round() as u32;
        p.setup.fit = None;
    }
    if let Some(a) = opt_str(o, "print_area")? {
        p.print_area = if a.is_empty() {
            Vec::new()
        } else {
            vec![parse_ref(book, a, sheet)?.1]
        };
    }
    if let Some(t) = get(o, "title_rows") {
        p.title_rows = match t {
            Value::Null => None,
            v => Some(super::sheets_author::rows_arg(v, "title_rows")?),
        };
    }
    if let Some(g) = opt_bool(o, "gridlines")? {
        p.options.grid_lines = g;
    }
    if let Some(c) = opt_bool(o, "center")? {
        p.options.horizontal_centered = c;
    }
    book.set_print_settings(sheet, p)?;
    Ok(("changed the page setup".into(), json!({})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::sheets_author::{cell_value, run_tool};

    fn run_ok(b: &mut Workbook, tool: &str, args: Value) -> Value {
        run_tool(b, tool, &args, None)
            .unwrap_or_else(|e| panic!("{tool}: {e:#}"))
            .1
    }

    fn data() -> Workbook {
        let mut b = Workbook::new_empty().unwrap();
        run_ok(
            &mut b,
            "write_range",
            json!({"top_left": "A1", "values": [
                ["Region", "Product", "Units", "Revenue"],
                ["North", "Tea", 10, 120],
                ["South", "Tea", 4, 48],
                ["North", "Coffee", 7, 140],
                ["East", "Coffee", 3, 60],
            ]}),
        );
        b
    }

    #[std::prelude::v1::test]
    fn tables_with_totals() {
        let mut b = data();
        let out = run_ok(
            &mut b,
            "add_table",
            json!({"range": "A1:D5", "name": "Sales", "style": "TableStyleMedium9", "totals": {"Revenue": "sum", "Units": "average"}}),
        );
        assert_eq!(out["table"], "Sales");
        assert_eq!(out["range"], "A1:D6");
        assert_eq!(cell_value(&b, 0, 5, 3), json!(368));
        assert_eq!(b.tables(0)[0].style.as_deref(), Some("TableStyleMedium9"));
        assert!(
            run_tool(
                &mut b,
                "add_table",
                &json!({"range": "F1:G3", "style": "Nope"}),
                None
            )
            .is_err()
        );
        assert!(
            run_tool(&mut b, "add_table", &json!({"range": "A1:B3"}), None).is_err(),
            "overlaps"
        );
    }

    #[std::prelude::v1::test]
    fn conditional_formats_of_every_kind() {
        let mut b = data();
        for rule in [
            json!({"range": "C2:C5", "type": "data_bar", "color": "#5B9BD5"}),
            json!({"range": "D2:D5", "type": "color_scale", "colors": "red_yellow_green"}),
            json!({"range": "D2:D5", "type": "icon_set", "icons": "traffic_lights"}),
            json!({"range": "C2:C5", "type": "cell_value", "operator": ">", "value": 5, "format": "green"}),
            json!({"range": "C2:C5", "type": "cell_value", "operator": "between", "value": 1, "value2": 4}),
            json!({"range": "A2:A5", "type": "text", "text": "No", "format": {"fill": "#FFF2CC", "bold": true}}),
            json!({"range": "D2:D5", "type": "top", "rank": 2}),
            json!({"range": "D2:D5", "type": "below_average"}),
            json!({"range": "B2:B5", "type": "duplicates"}),
            json!({"range": "B2:B5", "type": "blanks"}),
            json!({"range": "A2:D5", "type": "formula", "formula": "=$D2>100"}),
            json!({"range": "A2:A5", "type": "dates", "period": "this_month"}),
        ] {
            run_ok(&mut b, "add_conditional_format", rule);
        }
        assert_eq!(b.cf_rules(0, None).len(), 12);
        assert!(
            run_tool(
                &mut b,
                "add_conditional_format",
                &json!({"range": "A1", "type": "sparkle"}),
                None
            )
            .is_err()
        );
        assert!(
            run_tool(
                &mut b,
                "add_conditional_format",
                &json!({"range": "A1", "type": "cell_value", "operator": ">"}),
                None
            )
            .is_err()
        );
        let out = run_ok(
            &mut b,
            "clear_conditional_formats",
            json!({"range": "C2:C5"}),
        );
        assert!(out["removed"].as_u64().unwrap() >= 3);
    }

    #[std::prelude::v1::test]
    fn validation_names_pivots_and_pages() {
        let mut b = data();
        run_ok(
            &mut b,
            "add_validation",
            json!({"range": "B2:B20", "type": "list", "values": ["Tea", "Coffee"], "prompt": "Pick one", "error": {"title": "No", "text": "Tea or Coffee"}}),
        );
        run_ok(
            &mut b,
            "add_validation",
            json!({"range": "C2:C20", "type": "whole", "operator": ">=", "value": 0}),
        );
        run_ok(
            &mut b,
            "add_validation",
            json!({"range": "A2:A20", "type": "list", "source": "A2:A5"}),
        );
        let v = b.sheet_validations(0);
        assert_eq!(v.len(), 3);
        assert!(
            v.iter()
                .any(|v| v.formula1.as_deref() == Some("\"Tea,Coffee\""))
        );
        run_ok(&mut b, "clear_validation", json!({"range": "C2:C20"}));
        assert_eq!(b.sheet_validations(0).len(), 2);

        run_ok(
            &mut b,
            "define_name",
            json!({"name": "Revenue", "refers_to": "Sheet1!$D$2:$D$5"}),
        );
        run_ok(
            &mut b,
            "write_range",
            json!({"top_left": "F1", "values": [["=SUM(Revenue)"]]}),
        );
        assert_eq!(cell_value(&b, 0, 0, 5), json!(368));
        run_ok(
            &mut b,
            "define_name",
            json!({"name": "Revenue", "refers_to": "Sheet1!$D$2:$D$3"}),
        );
        assert_eq!(cell_value(&b, 0, 0, 5), json!(168));
        run_ok(&mut b, "delete_name", json!({"name": "Revenue"}));

        let out = run_ok(
            &mut b,
            "add_pivot",
            json!({"source": "Sheet1!A1:D5", "new_sheet": "Pivot", "rows": ["Region"], "values": [{"field": "Revenue", "func": "sum", "number_format": "integer"}]}),
        );
        assert_eq!(out["sheet"], "Pivot");
        let s = 1;
        assert_eq!(b.display_value(s, 2, 0), "Row Labels");
        assert_eq!(b.display_value(s, 3, 0), "East");
        assert!(
            run_tool(
                &mut b,
                "add_pivot",
                &json!({"source": "Sheet1!A1:D5", "rows": ["Nope"], "values": ["Units"]}),
                None
            )
            .is_err()
        );

        run_ok(
            &mut b,
            "page_setup",
            json!({"sheet": 1, "landscape": true, "fit": "width", "paper": "a4", "gridlines": true, "title_rows": "1"}),
        );
        let p = b.print_settings(0);
        assert!(p.setup.landscape);
        assert_eq!(p.setup.fit, Some((1, 0)));
        assert_eq!(p.title_rows, Some((0, 0)));
    }
}
