//! Formula text helpers for the editor: finding references, where a pointed
//! reference may go, F4 (relative / absolute), and function names for
//! autocomplete. Pure functions over the formula text; byte offsets
//! throughout (what the input reports as its caret).
//!
//! The scanner is deliberately small: it knows strings, quoted and plain sheet
//! prefixes, cell references and `A1:B2` ranges, function names (a word before
//! `(`), numbers and punctuation. Whole-column (`A:A`), whole-row (`1:3`) and
//! defined names are left as plain words.

use std::ops::Range;
use std::sync::OnceLock;

use crate::{CellRange, MAX_COLS, MAX_ROWS, column_name};

/// A cell or range reference in a formula.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormulaRef {
    /// Byte range of the whole reference, sheet prefix included.
    pub span: Range<usize>,
    /// The sheet named in the reference (`'Sheet 2'!A1` → `Sheet 2`).
    pub sheet: Option<String>,
    pub range: CellRange,
}

impl FormulaRef {
    /// Identity for colouring: the same cells get the same colour however
    /// they are written (`A1`, `$A$1`).
    fn key(&self) -> (Option<String>, CellRange) {
        (self.sheet.as_ref().map(|s| s.to_lowercase()), self.range)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Ref(FormulaRef),
    /// A string literal; `closed` is false when the formula ends inside it.
    Str {
        closed: bool,
    },
    Func,
    Word,
    Number,
    Space,
    Punct(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    span: Range<usize>,
    kind: Kind,
}

/// A cell written as `[$]COL[$]ROW`: `(row, col, col_abs, row_abs)`, 0-based.
fn parse_cell(s: &str) -> Option<(u32, u32, bool, bool)> {
    let b = s.as_bytes();
    let mut i = 0;
    let col_abs = b.first() == Some(&b'$');
    if col_abs {
        i += 1;
    }
    let letters = b[i..]
        .iter()
        .take_while(|c| c.is_ascii_alphabetic())
        .count();
    if !(1..=3).contains(&letters) {
        return None;
    }
    let mut col: u32 = 0;
    for c in &b[i..i + letters] {
        col = col * 26 + u32::from(c.to_ascii_uppercase() - b'A') + 1;
    }
    i += letters;
    let row_abs = b.get(i) == Some(&b'$');
    if row_abs {
        i += 1;
    }
    let digits = &s[i..];
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) || digits.starts_with('0') {
        return None;
    }
    let row: u32 = digits.parse().ok()?;
    if col > MAX_COLS || row > MAX_ROWS {
        return None;
    }
    Some((row - 1, col - 1, col_abs, row_abs))
}

fn is_ref_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'$'
}

/// A cell or `A1:B2` range starting at byte `start`: its end and cells.
fn parse_ref_at(formula: &str, start: usize) -> Option<(usize, CellRange)> {
    let b = formula.as_bytes();
    let end = start + b[start..].iter().take_while(|c| is_ref_byte(**c)).count();
    let (row, col, ..) = parse_cell(&formula[start..end])?;
    if b.get(end) == Some(&b':') {
        let start2 = end + 1;
        let end2 = start2 + b[start2..].iter().take_while(|c| is_ref_byte(**c)).count();
        if let Some((row2, col2, ..)) = parse_cell(&formula[start2..end2]) {
            return Some((end2, CellRange::from_corners((row, col), (row2, col2))));
        }
    }
    Some((end, CellRange::from_corners((row, col), (row, col))))
}

/// The end of a `"…"` string starting at `start`, and whether it was closed.
fn quoted_end(b: &[u8], start: usize, quote: u8) -> (usize, bool) {
    let mut i = start + 1;
    while i < b.len() {
        if b[i] == quote {
            if b.get(i + 1) == Some(&quote) {
                i += 2;
                continue;
            }
            return (i + 1, true);
        }
        i += 1;
    }
    (b.len(), false)
}

fn tokenize(formula: &str) -> Vec<Token> {
    let b = formula.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let start = i;
        let kind = if c == b'"' {
            let (end, closed) = quoted_end(b, i, b'"');
            i = end;
            Kind::Str { closed }
        } else if c == b'\'' {
            let (end, closed) = quoted_end(b, i, b'\'');
            i = end;
            match (closed && b.get(end) == Some(&b'!'))
                .then(|| parse_ref_at(formula, end + 1))
                .flatten()
            {
                Some((ref_end, range)) => {
                    i = ref_end;
                    let sheet = formula[start + 1..end - 1].replace("''", "'");
                    Kind::Ref(FormulaRef {
                        span: start..ref_end,
                        sheet: Some(sheet),
                        range,
                    })
                }
                None => Kind::Word,
            }
        } else if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            let end = i + b[i..]
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'$'))
                .count();
            let word = &formula[i..end];
            i = end;
            match b.get(end) {
                Some(b'!') if !word.contains('$') => match parse_ref_at(formula, end + 1) {
                    Some((ref_end, range)) => {
                        i = ref_end;
                        Kind::Ref(FormulaRef {
                            span: start..ref_end,
                            sheet: Some(word.to_owned()),
                            range,
                        })
                    }
                    None => Kind::Word,
                },
                Some(b'(') => Kind::Func,
                _ => match parse_ref_at(formula, start) {
                    // The whole word must be the cell (not `A1B`).
                    Some((ref_end, range)) if ref_end >= end => {
                        i = ref_end;
                        Kind::Ref(FormulaRef {
                            span: start..ref_end,
                            sheet: None,
                            range,
                        })
                    }
                    _ => Kind::Word,
                },
            }
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            i += b[i..]
                .iter()
                .take_while(|c| c.is_ascii_digit() || **c == b'.')
                .count();
            Kind::Number
        } else if c.is_ascii_whitespace() {
            i += b[i..]
                .iter()
                .take_while(|c| c.is_ascii_whitespace())
                .count();
            Kind::Space
        } else {
            let ch = formula[i..].chars().next().unwrap_or(' ');
            i += ch.len_utf8();
            Kind::Punct(ch)
        };
        out.push(Token {
            span: start..i,
            kind,
        });
    }
    out
}

fn is_formula(text: &str) -> bool {
    text.starts_with('=')
}

/// Every reference in a formula, in order. Empty for plain values.
pub fn references(formula: &str) -> Vec<FormulaRef> {
    if !is_formula(formula) {
        return Vec::new();
    }
    tokenize(formula)
        .into_iter()
        .filter_map(|t| match t.kind {
            Kind::Ref(r) => Some(r),
            _ => None,
        })
        .collect()
}

/// A colour slot for each of `refs`: the same cells share a slot, new cells
/// take the next one (callers cycle through their palette).
pub fn reference_colors(refs: &[FormulaRef]) -> Vec<usize> {
    let mut seen: Vec<(Option<String>, CellRange)> = Vec::new();
    refs.iter()
        .map(|r| {
            let key = r.key();
            match seen.iter().position(|k| *k == key) {
                Some(ix) => ix,
                None => {
                    seen.push(key);
                    seen.len() - 1
                }
            }
        })
        .collect()
}

/// The reference the caret is in or touching (for F4).
pub fn reference_at(formula: &str, caret: usize) -> Option<FormulaRef> {
    references(formula)
        .into_iter()
        .find(|r| r.span.start <= caret && caret <= r.span.end)
}

/// Characters after which a reference can start.
const BEFORE_REF: &[char] = &[
    '=', '(', ',', ';', '+', '-', '*', '/', '^', '&', '<', '>', ':',
];
/// Characters that may follow an inserted reference.
const AFTER_REF: &[char] = &[')', ',', ';', '+', '-', '*', '/', '^', '&', '<', '>', '='];

/// Whether clicking a cell should insert its reference at `caret`: the text
/// is a formula, the caret is not inside a string, and it sits after `=`,
/// `(`, `,` or an operator, with nothing but a closing `)`, separator or
/// operator after it.
pub fn accepts_reference(formula: &str, caret: usize) -> bool {
    if !is_formula(formula) || caret == 0 || caret > formula.len() {
        return false;
    }
    if !formula.is_char_boundary(caret) {
        return false;
    }
    let in_string = tokenize(formula).iter().any(|t| {
        matches!(t.kind, Kind::Str { closed } if t.span.start < caret && (caret < t.span.end || !closed))
    });
    if in_string {
        return false;
    }
    let before = formula[..caret].trim_end().chars().next_back();
    let after = formula[caret..].trim_start().chars().next();
    before.is_some_and(|c| BEFORE_REF.contains(&c)) && after.is_none_or(|c| AFTER_REF.contains(&c))
}

/// The text for a reference: `A1`, `A1:B7`, or with a sheet `'Sheet 2'!A1`.
pub fn format_reference(sheet: Option<&str>, range: CellRange) -> String {
    let cells = range.name();
    match sheet {
        Some(sheet) => format!("'{}'!{cells}", sheet.replace('\'', "''")),
        None => cells,
    }
}

/// F4: cycle a reference `A1 → $A$1 → A$1 → $A1 → A1`. Ranges change both
/// corners together, following the first one; a sheet prefix is kept.
/// `None` if `text` is not a reference.
pub fn cycle_absolute(text: &str) -> Option<String> {
    let tokens = tokenize(text);
    let [
        Token {
            kind: Kind::Ref(r), ..
        },
    ] = tokens.as_slice()
    else {
        return None;
    };
    // The cells start after the sheet prefix's `!`.
    let cells_start = if r.sheet.is_some() {
        let b = text.as_bytes();
        let bang = if b[0] == b'\'' {
            quoted_end(b, 0, b'\'').0
        } else {
            text.find('!')?
        };
        bang + 1
    } else {
        0
    };
    let (prefix, cells) = text.split_at(cells_start);
    let parts: Vec<_> = cells
        .split(':')
        .map(parse_cell)
        .collect::<Option<Vec<_>>>()?;
    let (_, _, col_abs, row_abs) = parts[0];
    let (col_abs, row_abs) = match (col_abs, row_abs) {
        (false, false) => (true, true),
        (true, true) => (false, true),
        (false, true) => (true, false),
        (true, false) => (false, false),
    };
    let cells: Vec<String> = parts
        .iter()
        .map(|&(row, col, ..)| {
            format!(
                "{}{}{}{}",
                if col_abs { "$" } else { "" },
                column_name(col),
                if row_abs { "$" } else { "" },
                row + 1
            )
        })
        .collect();
    Some(format!("{prefix}{}", cells.join(":")))
}

/// The function name being typed at `caret` (`=SU|` → the range of `SU`):
/// a word of letters (then letters, digits or `.`) right after `=`, `(`, `,`,
/// an operator or a space, with the caret at its end.
pub fn function_prefix(formula: &str, caret: usize) -> Option<Range<usize>> {
    if !is_formula(formula) || caret > formula.len() || !formula.is_char_boundary(caret) {
        return None;
    }
    let tokens = tokenize(formula);
    let ix = tokens
        .iter()
        .position(|t| t.span.end == caret && matches!(t.kind, Kind::Word | Kind::Ref(_)))?;
    let word = &tokens[ix];
    let text = &formula[word.span.clone()];
    if !text.starts_with(|c: char| c.is_ascii_alphabetic())
        || !text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
    {
        return None;
    }
    let before = tokens[..ix]
        .iter()
        .rev()
        .find(|t| t.kind != Kind::Space)
        .map(|t| t.kind.clone());
    let ok_before = match before {
        Some(Kind::Punct(c)) => BEFORE_REF.contains(&c),
        _ => false,
    };
    ok_before.then(|| word.span.clone())
}

/// IronCalc's function names (English), sorted.
pub fn function_names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let f = &ironcalc_base::language::get_default_language().functions;
        macro_rules! names {
            ($($field:ident),* $(,)?) => { vec![$(f.$field.clone()),*] };
        }
        let mut names: Vec<String> = names!(
            and,
            r#false,
            r#if,
            iferror,
            ifna,
            ifs,
            not,
            or,
            switch,
            r#true,
            xor,
            r#let,
            lambda,
            log,
            log10,
            ln,
            sin,
            cos,
            tan,
            asin,
            acos,
            atan,
            sinh,
            cosh,
            tanh,
            asinh,
            acosh,
            atanh,
            acot,
            acoth,
            cot,
            coth,
            csc,
            csch,
            sec,
            sech,
            abs,
            pi,
            sqrt,
            sqrtpi,
            atan2,
            power,
            max,
            min,
            product,
            rand,
            randbetween,
            round,
            rounddown,
            roundup,
            sum,
            sumif,
            sumifs,
            choose,
            column,
            columns,
            index,
            indirect,
            hlookup,
            hyperlink,
            lookup,
            r#match,
            offset,
            row,
            rows,
            vlookup,
            xlookup,
            sort,
            sortby,
            unique,
            filter,
            sequence,
            randarray,
            textsplit,
            concatenate,
            exact,
            value,
            t,
            valuetotext,
            concat,
            find,
            left,
            len,
            lower,
            mid,
            right,
            search,
            text,
            trim,
            unicode,
            unichar,
            r#char,
            clean,
            code,
            asc,
            arraytotext,
            dollar,
            findb,
            fixed,
            leftb,
            lenb,
            midb,
            numbervalue,
            proper,
            replace,
            replaceb,
            rightb,
            searchb,
            upper,
            isnumber,
            isnontext,
            istext,
            islogical,
            isblank,
            iserr,
            iserror,
            isna,
            na,
            isref,
            isodd,
            iseven,
            errortype,
            formulatext,
            isformula,
            isomitted,
            r#type,
            sheet,
            average,
            averagea,
            avedev,
            averageif,
            averageifs,
            count,
            counta,
            countblank,
            countif,
            countifs,
            maxifs,
            minifs,
            geomean,
            year,
            day,
            month,
            eomonth,
            date,
            datedif,
            datevalue,
            edate,
            networkdays,
            networkdaysintl,
            time,
            timevalue,
            hour,
            minute,
            second,
            today,
            now,
            days,
            days360,
            weekday,
            weeknum,
            workday,
            workdayintl,
            yearfrac,
            isoweeknum,
            pmt,
            pv,
            rate,
            nper,
            fv,
            ppmt,
            ipmt,
            npv,
            mirr,
            irr,
            xirr,
            xnpv,
            rept,
            textafter,
            textbefore,
            textjoin,
            substitute,
            regexextract,
            regexreplace,
            regextest,
            ispmt,
            rri,
            sln,
            syd,
            nominal,
            effect,
            pduration,
            tbillyield,
            tbillprice,
            tbilleq,
            dollarde,
            dollarfr,
            ddb,
            db,
            cumprinc,
            cumipmt,
            accrint,
            accrintm,
            besseli,
            besselj,
            besselk,
            bessely,
            erf,
            erfprecise,
            erfc,
            erfcprecise,
            bin2dec,
            bin2hex,
            bin2oct,
            dec2bin,
            dec2hex,
            dec2oct,
            hex2bin,
            hex2dec,
            hex2oct,
            oct2bin,
            oct2dec,
            oct2hex,
            bitand,
            bitlshift,
            bitor,
            bitrshift,
            bitxor,
            complex,
            imabs,
            imaginary,
            imargument,
            imconjugate,
            imcos,
            imcosh,
            imcot,
            imcsc,
            imcsch,
            imdiv,
            imexp,
            imln,
            imlog10,
            imlog2,
            impower,
            improduct,
            imreal,
            imsec,
            imsech,
            imsin,
            imsinh,
            imsqrt,
            imsub,
            imsum,
            imtan,
            convert,
            delta,
            gestep,
            subtotal,
            exp,
            fact,
            factdouble,
            sign,
            radians,
            degrees,
            int,
            even,
            odd,
            ceiling,
            ceilingmath,
            ceilingprecise,
            floor,
            floormath,
            floorprecise,
            isoceiling,
            r#mod,
            quotient,
            mround,
            trunc,
            gcd,
            lcm,
            base,
            decimal,
            roman,
            arabic,
            combin,
            combina,
            sumsq,
            n,
            cell,
            info,
            sheets,
            daverage,
            dcount,
            dget,
            dmax,
            dmin,
            dsum,
            dcounta,
            dproduct,
            dstdev,
            dvar,
            dvarp,
            dstdevp,
            betadist,
            betainv,
            binomdist,
            binomdistrange,
            binominv,
            chisqdist,
            chisqdistrt,
            chisqinv,
            chisqinvrt,
            chisqtest,
            confidencenorm,
            confidencet,
            covariancep,
            covariances,
            devsq,
            expondist,
            fdist,
            fdistrt,
            finv,
            finvrt,
            fisher,
            fisherinv,
            ftest,
            gamma,
            gammadist,
            gammainv,
            gammaln,
            gammalnprecise,
            hypgeomdist,
            lognormdist,
            lognorminv,
            negbinomdist,
            normdist,
            norminv,
            normsdist,
            normsinv,
            pearson,
            phi,
            poissondist,
            standardize,
            stdev,
            disc,
            fvschedule,
            intrate,
            pricedisc,
            pricemat,
            received,
            yielddisc,
            yieldmat,
            duration,
            mduration,
            price,
            r#yield,
            oddfprice,
            oddfyield,
            oddlprice,
            oddlyield,
            coupdaybs,
            coupdays,
            coupdaysnc,
            coupncd,
            coupnum,
            couppcd,
            amordegrc,
            amorlinc,
            vdb,
            stdevp,
            stdevs,
            stdeva,
            stdevpa,
            tdist,
            tdist2t,
            tdistrt,
            tinv,
            tinv2t,
            ttest,
            varp,
            vars,
            varpa,
            vara,
            weibulldist,
            ztest,
            chidist,
            chiinv,
            chitest,
            confidence,
            covar,
            critbinom,
            loginv,
            betainvcompat,
            binomdistcompat,
            expondistcompat,
            fdistcompat,
            finvcompat,
            ftestcompat,
            gammadistcompat,
            gammainvcompat,
            modecompat,
            normdistcompat,
            norminvcompat,
            normsinvcompat,
            percentilecompat,
            percentrankcompat,
            poissoncompat,
            quartilecompat,
            rankcompat,
            stdevpcompat,
            tinvcompat,
            ttestcompat,
            varcompat,
            varpcompat,
            weibullcompat,
            ztestcompat,
            betadistcompat,
            hypgeomdistcompat,
            lognormdistcompat,
            negbinomdistcompat,
            normsdistcompat,
            tdistcompat,
            sumx2my2,
            sumx2py2,
            sumxmy2,
            correl,
            rsq,
            intercept,
            slope,
            steyx,
            forecast,
            forecastlinear,
            forecastets,
            forecastetsconfint,
            forecastetsseasonality,
            forecastetsstat,
            frequency,
            growth,
            linest,
            logest,
            modemult,
            modesngl,
            percentileexc,
            percentileinc,
            percentrankexc,
            percentrankinc,
            permut,
            permutationa,
            prob,
            quartileexc,
            quartileinc,
            trend,
            trimmean,
            gauss,
            harmean,
            kurt,
            large,
            maxa,
            median,
            mina,
            rankavg,
            rankeq,
            skew,
            skewp,
            small,
            take,
            drop,
            tocol,
            torow,
            transpose,
            mmult,
            bycol,
            byrow,
            map,
            reduce,
            scan,
            makearray,
            address,
            areas,
            choosecols,
            chooserows,
            expand,
            hstack,
            vstack,
            wrapcols,
            wraprows,
            xmatch,
            trimrange,
            mdeterm,
            minverse,
            munit,
            multinomial,
            seriessum,
            sumproduct,
            percentof,
            encodeurl,
            ecma_ceiling,
        );
        names.retain(|n| !n.is_empty());
        names.sort();
        names.dedup();
        names
    })
}

/// Function names starting with `prefix` (any case), at most `limit`.
pub fn matching_functions(prefix: &str, limit: usize) -> Vec<&'static str> {
    if prefix.is_empty() {
        return Vec::new();
    }
    let prefix = prefix.to_ascii_uppercase();
    function_names()
        .iter()
        .filter(|n| n.starts_with(&prefix))
        .take(limit)
        .map(String::as_str)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(top: u32, left: u32, bottom: u32, right: u32) -> CellRange {
        CellRange {
            top,
            left,
            bottom,
            right,
        }
    }

    #[test]
    fn finds_references() {
        let f = "=SUM(A1:B3)+$C$2*'My Sheet'!D4-Sheet2!E5:F6&\"A1\"";
        let refs = references(f);
        let texts: Vec<_> = refs.iter().map(|x| &f[x.span.clone()]).collect();
        assert_eq!(texts, ["A1:B3", "$C$2", "'My Sheet'!D4", "Sheet2!E5:F6"]);
        assert_eq!(refs[0].range, r(0, 0, 2, 1));
        assert_eq!(refs[1].range, r(1, 2, 1, 2));
        assert_eq!(refs[2].sheet.as_deref(), Some("My Sheet"));
        assert_eq!(refs[3].sheet.as_deref(), Some("Sheet2"));
        assert_eq!(refs[3].range, r(4, 4, 5, 5));
    }

    #[test]
    fn plain_values_and_non_references() {
        assert!(references("A1+B2").is_empty());
        assert!(references("=SUM(1,2)").is_empty());
        // Function names that look like cells, out-of-range cells, words.
        assert!(references("=LOG10(2)").is_empty());
        assert!(references("=XFE1+A0+TRUE+A1B").is_empty());
        assert_eq!(references("=b2").len(), 1);
    }

    #[test]
    fn colours_follow_cells() {
        let refs = references("=A1+B2+$A$1+C3:D4+b2");
        assert_eq!(reference_colors(&refs), [0, 1, 0, 2, 1]);
    }

    #[test]
    fn reference_positions() {
        assert!(accepts_reference("=", 1));
        assert!(accepts_reference("=SUM(", 5));
        assert!(accepts_reference("=SUM(A1, ", 9));
        assert!(accepts_reference("=A1+", 4));
        assert!(accepts_reference("=SUM()", 5));
        assert!(accepts_reference("=1*(2+)", 6));
        assert!(!accepts_reference("=A1", 3));
        assert!(!accepts_reference("=SUM", 4));
        assert!(!accepts_reference("12", 2));
        assert!(!accepts_reference("=\"a,", 4));
        assert!(!accepts_reference("=(B", 2));
        assert!(!accepts_reference("", 0));
    }

    #[test]
    fn formats_references() {
        assert_eq!(format_reference(None, r(2, 0, 2, 0)), "A3");
        assert_eq!(format_reference(None, r(2, 0, 6, 1)), "A3:B7");
        assert_eq!(
            format_reference(Some("Sheet2"), r(0, 0, 0, 0)),
            "'Sheet2'!A1"
        );
        assert_eq!(
            format_reference(Some("Bob's"), r(0, 0, 0, 0)),
            "'Bob''s'!A1"
        );
    }

    #[test]
    fn f4_cycles() {
        let mut s = "A1".to_owned();
        let mut seen = Vec::new();
        for _ in 0..4 {
            s = cycle_absolute(&s).unwrap();
            seen.push(s.clone());
        }
        assert_eq!(seen, ["$A$1", "A$1", "$A1", "A1"]);
        assert_eq!(cycle_absolute("A1:$B$2").unwrap(), "$A$1:$B$2");
        assert_eq!(cycle_absolute("'My Sheet'!B3").unwrap(), "'My Sheet'!$B$3");
        assert_eq!(cycle_absolute("Sheet2!$B3").unwrap(), "Sheet2!B3");
        assert_eq!(cycle_absolute("SUM"), None);
        assert_eq!(cycle_absolute("A1+B1"), None);
    }

    #[test]
    fn reference_under_caret() {
        let f = "=A1+$B$2";
        assert_eq!(reference_at(f, 2).unwrap().span, 1..3);
        assert_eq!(reference_at(f, 3).unwrap().span, 1..3);
        assert_eq!(reference_at(f, 5).unwrap().span, 4..8);
        assert_eq!(reference_at("=1+2", 2), None);
    }

    #[test]
    fn function_prefixes() {
        assert_eq!(function_prefix("=SU", 3), Some(1..3));
        assert_eq!(function_prefix("=A1+su", 6), Some(4..6));
        assert_eq!(function_prefix("=SUM(LO", 7), Some(5..7));
        assert_eq!(function_prefix("=SU", 2), None);
        assert_eq!(function_prefix("SU", 2), None);
        assert_eq!(function_prefix("=\"SU", 4), None);
        assert_eq!(function_prefix("=1SU", 4), None);
    }

    #[test]
    fn function_names_come_from_ironcalc() {
        let names = function_names();
        assert!(names.len() > 300);
        assert!(names.iter().any(|n| n == "SUM"));
        let su = matching_functions("su", 20);
        assert!(su.starts_with(&["SUBSTITUTE", "SUBTOTAL", "SUM", "SUMIF"]));
        assert!(su.contains(&"SUMPRODUCT"));
        assert!(matching_functions("", 5).is_empty());
        assert_eq!(matching_functions("su", 2).len(), 2);
    }
}
