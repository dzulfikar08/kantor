//! Hyperlinks from macros: added to a URL, a mail address and a cell of
//! another sheet, read back, changed and deleted; saved and undone.

mod support;

use support::*;

const LINKS: &str = r#"
Sub AddLinks()
    Dim ws As Worksheet, h As Hyperlink
    Set ws = Worksheets(1)
    Worksheets.Add After:=ws
    ws.Hyperlinks.Add Anchor:=ws.Range("E1"), Address:="https://kantor.paparanai.id", _
        ScreenTip:="Kantor site", TextToDisplay:="Kantor"
    ws.Hyperlinks.Add ws.Range("E2"), "", "Sheet2!B3", , "Go to Sheet2"
    ws.Hyperlinks.Add ws.Range("E3"), "mailto:dewi@example.com?subject=Hello"
    Debug.Print ws.Hyperlinks.Count
    For Each h In ws.Hyperlinks
        Debug.Print h.Range.Address(False, False) & "|" & h.Address & "|" & h.SubAddress & "|" & h.TextToDisplay
    Next
    Debug.Print ws.Hyperlinks(1).ScreenTip & "|" & ws.Hyperlinks(3).EmailSubject & "|" & ws.Hyperlinks(1).Type
    ws.Range("E3").Hyperlinks.Delete
    Debug.Print ws.Hyperlinks.Count & " " & ws.Range("A1:E1").Hyperlinks.Count & " " & ws.Range("A1:D9").Hyperlinks.Count
    With ws.Hyperlinks(2)
        .TextToDisplay = "Sheet 2, B3"
        .SubAddress = "Sheet2!C4"
    End With
    ws.Hyperlinks(1).Address = "https://kantor.paparanai.id/docs"
End Sub
"#;

#[test]
fn links_are_added_read_changed_and_deleted() {
    let (book, outcome, printed) = run_printed(sales(), LINKS, "AddLinks");
    ok(&outcome);
    assert_eq!(
        printed,
        vec![
            "3",
            "E1|https://kantor.paparanai.id||Kantor",
            "E2||Sheet2!B3|Go to Sheet2",
            "E3|mailto:dewi@example.com?subject=Hello||mailto:dewi@example.com?subject=Hello",
            "Kantor site|Hello|0",
            "2 1 0",
        ]
    );
    let links = book.cell_links(0);
    assert_eq!(links.len(), 2);
    assert_eq!(
        links[0],
        (
            0,
            4,
            "https://kantor.paparanai.id/docs".to_owned(),
            String::new(),
            Some("Kantor site".to_owned())
        )
    );
    assert_eq!(links[1].3, "Sheet2!C4");
    assert_eq!(cell(&book, 0, "E1"), "Kantor");
    assert_eq!(cell(&book, 0, "E2"), "Sheet 2, B3");
    // Deleting the link keeps the cell's text.
    assert_eq!(
        cell(&book, 0, "E3"),
        "mailto:dewi@example.com?subject=Hello"
    );
}

#[test]
fn links_survive_save_and_reopen() {
    let (book, outcome) = run(sales(), LINKS, "AddLinks");
    ok(&outcome);
    let (back, _dir) = reopen(book, "links.xlsx");
    let links = back.cell_links(0);
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].2, "https://kantor.paparanai.id/docs");
    assert_eq!(links[0].4.as_deref(), Some("Kantor site"));
    assert_eq!(links[1].2, "");
    assert_eq!(links[1].3, "Sheet2!C4");
    assert_eq!(cell(&back, 0, "E2"), "Sheet 2, B3");

    let (_, outcome, printed) = run_printed(
        back,
        r#"
Sub Read()
    Dim h As Hyperlink
    For Each h In Worksheets(1).Hyperlinks
        Debug.Print h.Name & "|" & h.Address & "|" & h.SubAddress
    Next
End Sub
"#,
        "Read",
    );
    ok(&outcome);
    assert_eq!(
        printed,
        vec![
            "Kantor|https://kantor.paparanai.id/docs|",
            "Sheet 2, B3||Sheet2!C4",
        ]
    );
}

#[test]
fn one_undo_takes_the_links_back() {
    let (mut book, outcome) = run(sales(), LINKS, "AddLinks");
    ok(&outcome);
    assert!(book.undo().unwrap());
    assert!(book.cell_links(0).is_empty());
    assert_eq!(cell(&book, 0, "E1"), "");
    assert_eq!(book.sheet_names().len(), 1);
}

#[test]
fn clear_hyperlinks_and_errors() {
    let code = r#"
Sub Clear()
    ActiveSheet.Hyperlinks.Add Range("A1"), "https://example.com"
    ActiveSheet.Hyperlinks.Add Range("B1"), "https://example.org"
    Range("A1:B1").ClearHyperlinks
    Debug.Print ActiveSheet.Hyperlinks.Count & " " & Range("A1").Value
End Sub

Sub Follow()
    ActiveSheet.Hyperlinks.Add Range("A1"), "https://example.com"
    ActiveSheet.Hyperlinks(1).Follow
End Sub

Sub NoItem()
    Debug.Print ActiveSheet.Hyperlinks(1).Address
End Sub

Sub NoTarget()
    ActiveSheet.Hyperlinks.Add Range("A1"), ""
End Sub

Sub Unknown()
    ActiveSheet.Hyperlinks.Add Range("A1"), "https://example.com"
    ActiveSheet.Hyperlinks(1).Frobnicate
End Sub
"#;
    let (book, outcome, printed) = run_printed(sales(), code, "Clear");
    ok(&outcome);
    assert_eq!(printed, vec!["0 Name"]);
    assert!(book.cell_links(0).is_empty());
    for (name, number) in [
        ("Follow", 445),
        ("NoItem", 9),
        ("NoTarget", 1004),
        ("Unknown", 438),
    ] {
        let (_, outcome) = run(sales(), code, name);
        assert_eq!(err(&outcome).number, number, "{name}");
    }
}
