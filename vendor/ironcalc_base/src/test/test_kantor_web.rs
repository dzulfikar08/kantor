#![allow(clippy::unwrap_used)]
//! KANTOR PATCH (functions): EUROCONVERT, FILTERXML, IMAGE, WEBSERVICE, the
//! online functions and the network errors.

use std::sync::Arc;

use crate::cell::CellValue;
use crate::functions::kantor_web::{WebKind, WebResult};
use crate::test::util::new_empty_model;
use crate::Model;

fn number(model: &Model, row: i32, column: i32) -> f64 {
    match model.get_cell_value_by_index(0, row, column).unwrap() {
        CellValue::Number(f) => f,
        other => panic!("not a number: {other:?}"),
    }
}

#[test]
fn euroconvert_microsoft_examples() {
    let mut model = new_empty_model();
    model._set("A1", "=EUROCONVERT(1.2,\"DEM\",\"EUR\")");
    model._set("A2", "=EUROCONVERT(1,\"FRF\",\"EUR\",TRUE,3)");
    model._set("A3", "=EUROCONVERT(1,\"FRF\",\"EUR\",FALSE,3)");
    model._set("A4", "=EUROCONVERT(1,\"FRF\",\"DEM\",TRUE,3)");
    model.evaluate();
    assert_eq!(number(&model, 1, 1), 0.61);
    assert_eq!(number(&model, 2, 1), 0.152);
    assert_eq!(number(&model, 3, 1), 0.15);
    assert!((number(&model, 4, 1) - 0.29728616).abs() < 1e-15);
}

#[test]
fn euroconvert_rounding_and_errors() {
    let mut model = new_empty_model();
    // Lira and Belgian francs round to whole units, the euro to cents.
    model._set("A1", "=EUROCONVERT(100,\"EUR\",\"ITL\")");
    model._set("A2", "=EUROCONVERT(100,\"eur\",\"bef\")");
    model._set("A3", "=EUROCONVERT(1000,\"BEF\",\"EUR\")");
    model._set("A4", "=EUROCONVERT(100,\"DEM\",\"FRF\")");
    model._set("A5", "=EUROCONVERT(100,\"DEM\",\"DEM\")");
    model._set("A6", "=EUROCONVERT(100,\"DEM\",\"FRF\",TRUE)");
    model._set("B1", "=EUROCONVERT(1,\"USD\",\"EUR\")");
    model._set("B2", "=EUROCONVERT(1,\"DEM\",\"EUR\",TRUE,2)");
    model.evaluate();
    assert_eq!(number(&model, 1, 1), 193627.0);
    assert_eq!(number(&model, 2, 1), 4034.0);
    assert_eq!(number(&model, 3, 1), 24.79);
    // 100 / 1.95583 × 6.55957, to cents.
    assert_eq!(number(&model, 4, 1), 335.39);
    assert_eq!(number(&model, 5, 1), 100.0);
    assert!((number(&model, 6, 1) - 100.0 / 1.95583 * 6.55957).abs() < 1e-12);
    assert_eq!(model._get_text("B1"), "#VALUE!");
    assert_eq!(model._get_text("B2"), "#VALUE!");
}

#[test]
fn filterxml_paths_predicates_and_attributes() {
    let mut model = new_empty_model();
    let xml = "<r><i n=\"a\" v=\"1\">x</i><i n=\"b\" v=\"2\">y<b>z</b></i><i n=\"c\">10</i><j>2.5</j></r>";
    model._set("A1", xml);
    // A column of matches, spilled; numbers are numbers.
    model._set("C1", "=FILTERXML(A1,\"//i\")");
    model._set("D1", "=FILTERXML(A1,\"//i/@n\")");
    model._set("E1", "=FILTERXML(A1,\"/r/i[2]/@v\")");
    model._set("E2", "=FILTERXML(A1,\"//i[@n='c']\")");
    model._set("E3", "=FILTERXML(A1,\"//j\")*2");
    model._set("E4", "=FILTERXML(A1,\"//i[last()]/@n\")");
    model._set("E5", "=FILTERXML(A1,\"count(//i)\")");
    model._set("E6", "=FILTERXML(A1,\"//nothing\")");
    model._set("E7", "=FILTERXML(\"<a><b>\",\"//b\")");
    model._set("E8", "=FILTERXML(A1,\"//i[contains(.,'z')]/b/text()\")");
    model._set("E9", "=FILTERXML(A1,\"//i[@v > 1]/@n | //j\")");
    model._set(
        "E11",
        "=FILTERXML(A1,\"//*[local-name()='j']/..//@v[. = '1']\")",
    );
    model._set("E12", "=FILTERXML(A1,\"//p:i\")");
    model.evaluate();
    assert_eq!(model._get_text("C1"), "x");
    assert_eq!(model._get_text("C2"), "yz");
    assert_eq!(number(&model, 3, 3), 10.0);
    assert_eq!(model._get_text("C4"), "");
    assert_eq!(
        [
            model._get_text("D1"),
            model._get_text("D2"),
            model._get_text("D3")
        ],
        ["a", "b", "c"]
    );
    assert_eq!(number(&model, 1, 5), 2.0);
    assert_eq!(number(&model, 2, 5), 10.0);
    assert_eq!(number(&model, 3, 5), 5.0);
    assert_eq!(model._get_text("E4"), "c");
    assert_eq!(model._get_text("E5"), "#VALUE!");
    assert_eq!(model._get_text("E6"), "#VALUE!");
    assert_eq!(model._get_text("E7"), "#VALUE!");
    assert_eq!(model._get_text("E8"), "z");
    assert_eq!(model._get_text("E9"), "b");
    assert_eq!(number(&model, 10, 5), 2.5);
    assert_eq!(number(&model, 11, 5), 1.0);
    assert_eq!(model._get_text("E12"), "#VALUE!");
}

#[test]
fn filterxml_default_namespace_is_not_matched() {
    // As in Excel: an element in a namespace has no name XPath can use
    // without a prefix, and FILTERXML can't declare one.
    let mut model = new_empty_model();
    model._set(
        "A1",
        "=FILTERXML(\"<feed xmlns='http://www.w3.org/2005/Atom'><t>1</t></feed>\",\"//t\")",
    );
    model._set(
        "A2",
        "=FILTERXML(\"<feed xmlns='http://www.w3.org/2005/Atom'><t>1</t></feed>\",\"//*[local-name()='t']\")",
    );
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#VALUE!");
    assert_eq!(number(&model, 2, 1), 1.0);
}

#[test]
fn webservice_and_image_wait_for_the_host() {
    let mut model = new_empty_model();
    model._set("A1", "=WEBSERVICE(\"https://example.com/rate\")");
    model._set("A2", "=IMAGE(\"https://example.com/a.png\",\"Logo\")");
    model._set("A3", "=IMAGE(\"http://example.com/a.png\")");
    model._set("A4", "=IMAGE(\"https://example.com/a.png\",,3)");
    model._set("A5", "=IMAGE(\"https://example.com/a.png\",,1,20)");
    model._set("A6", "=WEBSERVICE(\"ftp://example.com\")");
    model._set("A7", "=ISERROR(A1)");
    model.evaluate();
    // Fetching is off until the host allows it.
    assert_eq!(model._get_text("A1"), "#BLOCKED!");
    assert_eq!(model._get_text("A2"), "#BLOCKED!");
    assert_eq!(model._get_text("A3"), "#VALUE!");
    assert_eq!(model._get_text("A4"), "#VALUE!");
    assert_eq!(model._get_text("A5"), "#VALUE!");
    assert_eq!(model._get_text("A6"), "#VALUE!");
    assert!(model.kantor_web_requests().is_empty());

    model.kantor_set_web_enabled(true);
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#BUSY!");
    assert_eq!(model._get_text("A2"), "#BUSY!");
    assert_eq!(
        model.kantor_web_requests(),
        [
            ("https://example.com/a.png".to_string(), WebKind::Image),
            ("https://example.com/rate".to_string(), WebKind::Text),
        ]
    );
    assert!(model.kantor_cell_image(0, 2, 1).is_none());

    model.kantor_set_web_result(
        "https://example.com/rate",
        WebKind::Text,
        WebResult::Text("1.0842".to_string()),
    );
    model.kantor_set_web_result(
        "https://example.com/a.png",
        WebKind::Image,
        WebResult::Image {
            data: Arc::new(vec![1, 2, 3]),
            width: 40,
            height: 20,
        },
    );
    model.evaluate();
    assert!(model.kantor_web_requests().is_empty());
    assert_eq!(model._get_text("A1"), "1.0842");
    // The cell's value is the alt text; the picture is drawn over it.
    assert_eq!(model._get_text("A2"), "Logo");
    let image = model.kantor_cell_image(0, 2, 1).unwrap();
    assert_eq!(
        (image.pixel_width, image.pixel_height, image.sizing),
        (40, 20, 0)
    );
    assert_eq!(model.kantor_image_cells(), [(0, 2, 1)]);

    // Unreachable: Excel's #CONNECT!.
    model.kantor_set_web_result(
        "https://example.com/a.png",
        WebKind::Image,
        WebResult::Error(crate::expressions::token::Error::CONNECT),
    );
    model._set(
        "A2",
        "=IMAGE(\"https://example.com/a.png\",\"Logo\",3,10,30)",
    );
    model.evaluate();
    assert_eq!(model._get_text("A2"), "#CONNECT!");
    assert!(model.kantor_cell_image(0, 2, 1).is_none());
}

#[test]
fn saved_pictures_show_until_the_formula_downloads_its_own() {
    let mut model = new_empty_model();
    model._set(
        "B2",
        "=IMAGE(\"https://example.com/a.png\",\"Logo\",3,30,60)",
    );
    model._set("B3", "=IMAGE(\"https://example.com/b.png\")");
    let saved = Arc::new(vec![9, 9, 9]);
    // Before any evaluation: what the file saved, sized by the formula's
    // literal arguments.
    model.kantor_set_saved_picture(0, 2, 2, saved.clone(), 8, 4, None);
    assert!(model.kantor_has_saved_pictures());
    let shown = model.kantor_cell_image(0, 2, 2).unwrap();
    assert_eq!(
        (
            shown.sizing,
            shown.height,
            shown.width,
            shown.alt_text.as_str()
        ),
        (3, Some(30.0), Some(60.0), "Logo")
    );
    assert!(model.kantor_shows_saved_picture(0, 2, 2));
    assert_eq!(model.kantor_image_cells(), [(0, 2, 2)]);

    // Blocked: the saved picture stays; a cell without one is #BLOCKED!.
    model.evaluate();
    assert_eq!(model._get_text("B2"), "Logo");
    assert!(model.kantor_shows_saved_picture(0, 2, 2));
    assert_eq!(model._get_text("B3"), "#BLOCKED!");

    // Downloading: the saved picture while it waits, then the new one.
    model.kantor_set_web_enabled(true);
    model.evaluate();
    assert!(model.kantor_shows_saved_picture(0, 2, 2));
    assert_eq!(model._get_text("B3"), "#BUSY!");
    model.kantor_set_web_result(
        "https://example.com/a.png",
        WebKind::Image,
        WebResult::Image {
            data: Arc::new(vec![1]),
            width: 2,
            height: 2,
        },
    );
    model.evaluate();
    assert!(!model.kantor_shows_saved_picture(0, 2, 2));
    assert_eq!(model.kantor_cell_image(0, 2, 2).unwrap().pixel_width, 2);

    // Another URL in the cell: the saved picture isn't its picture.
    model.kantor_set_web_enabled(false);
    model._set("B2", "=IMAGE(\"https://example.com/other.png\")");
    model.evaluate();
    assert_eq!(model._get_text("B2"), "#BLOCKED!");
    assert!(model.kantor_cell_image(0, 2, 2).is_none());
}

#[test]
fn refreshing_refetches_text_and_failures_but_keeps_pictures() {
    let mut model = new_empty_model();
    model._set("A1", "=WEBSERVICE(\"https://example.com/rate\")");
    model._set("A2", "=IMAGE(\"https://example.com/a.png\")");
    model._set("A3", "=IMAGE(\"https://example.com/gone.png\")");
    model.kantor_set_web_enabled(true);
    model.evaluate();
    model.kantor_set_web_result(
        "https://example.com/rate",
        WebKind::Text,
        WebResult::Text("1".to_string()),
    );
    model.kantor_set_web_result(
        "https://example.com/a.png",
        WebKind::Image,
        WebResult::Image {
            data: Arc::new(vec![1]),
            width: 1,
            height: 1,
        },
    );
    model.kantor_set_web_result(
        "https://example.com/gone.png",
        WebKind::Image,
        WebResult::Error(crate::expressions::token::Error::CONNECT),
    );
    model.evaluate();
    assert_eq!(model._get_text("A1"), "1");
    assert_eq!(model._get_text("A3"), "#CONNECT!");
    model.kantor_refresh_web();
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#BUSY!");
    assert_eq!(model._get_text("A2"), "");
    assert!(model.kantor_cell_image(0, 2, 1).is_some());
    assert_eq!(model._get_text("A3"), "#BUSY!");
    assert_eq!(
        model.kantor_web_requests(),
        [
            ("https://example.com/gone.png".to_string(), WebKind::Image),
            ("https://example.com/rate".to_string(), WebKind::Text),
        ]
    );

    // Blocked afterwards: WEBSERVICE keeps the value it had.
    model.kantor_set_web_result(
        "https://example.com/rate",
        WebKind::Text,
        WebResult::Text("2".to_string()),
    );
    model.evaluate();
    model.kantor_set_web_enabled(false);
    model.kantor_refresh_web();
    model.evaluate();
    assert_eq!(model._get_text("A1"), "2");
    // A formula typed while blocked has nothing to keep.
    model._set("A4", "=WEBSERVICE(\"https://example.com/new\")");
    model.evaluate();
    assert_eq!(model._get_text("A4"), "#BLOCKED!");
}

#[test]
fn online_functions_give_excels_offline_errors() {
    let mut model = new_empty_model();
    model._set("A1", "=STOCKHISTORY(\"MSFT\",TODAY())");
    model._set("A2", "=TRANSLATE(\"Hello\",\"en\",\"id\")");
    model._set("A3", "=CUBEVALUE(\"Sales\",\"[Measures].[Profit]\")");
    model._set("A4", "=RTD(\"progid\",,\"topic\")");
    model._set("A5", "=COPILOT(\"Summarize\",B1:B3)");
    model._set("A6", "=ERROR.TYPE(A2)");
    model._set("A7", "=IFERROR(A1,\"offline\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#CONNECT!");
    assert_eq!(model._get_text("A2"), "#CONNECT!");
    assert_eq!(model._get_text("A3"), "#NAME?");
    assert_eq!(model._get_text("A4"), "#N/A");
    assert_eq!(model._get_text("A5"), "#CONNECT!");
    assert_eq!(number(&model, 6, 1), 10.0);
    assert_eq!(model._get_text("A7"), "offline");
}

#[test]
fn network_errors_are_formula_literals() {
    let mut model = new_empty_model();
    model._set("A1", "=#CONNECT!");
    model._set("A2", "=ISERROR(#BUSY!)");
    model._set("A3", "=ERROR.TYPE(#BLOCKED!)");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#CONNECT!");
    assert_eq!(model._get_text("A2"), "TRUE");
    assert_eq!(number(&model, 3, 1), 11.0);
    assert_eq!(model._get_formula("A1"), "=#CONNECT!");
}
