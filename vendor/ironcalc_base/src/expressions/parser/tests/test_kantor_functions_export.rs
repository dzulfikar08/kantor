//! KANTOR PATCH (functions): names of the functions Kantor adds, and functions
//! passed by name (eta LAMBDAs), as Excel writes them in xlsx.

use crate::expressions::parser::stringify::{to_english_string, to_excel_string};
use crate::expressions::parser::tests::utils::new_parser;
use crate::expressions::types::CellReferenceRC;
use std::collections::HashMap;

fn cell() -> CellReferenceRC {
    CellReferenceRC {
        sheet: "Sheet1".to_string(),
        row: 1,
        column: 1,
    }
}

fn round_trip(typed: &str, xlsx: &str) {
    let mut p = new_parser(vec!["Sheet1".to_string()], vec![], HashMap::new());
    let t = p.parse(typed, &cell());
    assert_eq!(to_excel_string(&t, &cell()), xlsx, "export of {typed}");
    let back = p.parse(xlsx, &cell());
    assert_eq!(to_english_string(&back, &cell()), typed, "import of {xlsx}");
}

#[test]
fn new_functions_get_their_xlsx_prefixes() {
    round_trip("AGGREGATE(9,6,A1:A10)", "_xlfn.AGGREGATE(9,6,A1:A10)");
    round_trip("BAHTTEXT(B2)", "BAHTTEXT(B2)");
    round_trip("USDOLLAR(B2,2)", "USDOLLAR(B2,2)");
    round_trip("DBCS(B2)", "_xlfn.DBCS(B2)");
    round_trip("JIS(B2)", "JIS(B2)");
    round_trip("PHONETIC(B2)", "PHONETIC(B2)");
}

#[test]
fn functions_passed_by_name_are_written_as_xleta() {
    round_trip(
        "GROUPBY(A2:A9,B2:B9,SUM)",
        "_xlfn.GROUPBY(A2:A9,B2:B9,_xleta.SUM)",
    );
    round_trip(
        "PIVOTBY(A2:A9,B2:B9,C2:C9,PERCENTOF,,,,,,,1)",
        "_xlfn.PIVOTBY(A2:A9,B2:B9,C2:C9,_xleta.PERCENTOF,,,,,,,1)",
    );
    round_trip(
        "GROUPBY(A2:A9,B2:B9,STDEV.S)",
        "_xlfn.GROUPBY(A2:A9,B2:B9,_xleta.STDEV.S)",
    );
    round_trip("BYROW(A1:B3,SUM)", "_xlfn.BYROW(A1:B3,_xleta.SUM)");
    // A LAMBDA parameter that happens to be called like a function stays one.
    let mut p = new_parser(vec!["Sheet1".to_string()], vec![], HashMap::new());
    let t = p.parse("LAMBDA(sum,sum+1)(2)", &cell());
    assert_eq!(
        to_excel_string(&t, &cell()),
        "_xlfn.LAMBDA(_xlpm.sum,_xlpm.sum+1)(2)"
    );
}
