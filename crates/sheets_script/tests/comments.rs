//! Notes (`Comment`) from macros: AddComment, Comment, NoteText,
//! ClearComments, Worksheet.Comments; threaded comments are refused.

mod support;
use support::*;

/// Notes on B2, C3 and C4 of the sales table, as a macro adds them.
const ADD_NOTES: &str = r##"
Sub AddNotes()
    Range("C4").AddComment "Check with Andi"
    Range("B2").AddComment "Region moved in March"
    With Range("C3")
        .AddComment
        .Comment.Text "Budi: discount applied"
        .Comment.Visible = True
    End With
    Debug.Print Application.UserName
End Sub
"##;

#[test]
fn add_notes_edit_and_list_them() {
    let code = format!(
        "{ADD_NOTES}{}",
        r##"
Sub ListNotes()
    Dim c As Comment
    Debug.Print CStr(ActiveSheet.Comments.Count)
    For Each c In ActiveSheet.Comments
        Debug.Print c.Parent.Address & "|" & c.Text & "|" & c.Visible
    Next c
    Debug.Print ActiveSheet.Comments(2).Parent.Address
    Debug.Print Range("B2").Comment.Next.Parent.Address
    Debug.Print TypeName(Range("C4").Comment.Next)
    Debug.Print Range("C4").Comment.Previous.Parent.Address
    Debug.Print TypeName(Range("A1").Comment)
    Debug.Print Range("B2").Comment.Author = Application.UserName
End Sub
"##
    );
    let (book, outcome, printed) = run_printed(sales(), &code, "AddNotes");
    ok(&outcome);
    let user = printed[0].clone();
    let b2 = book.note_at(0, 1, 1).expect("a note on B2");
    assert_eq!(b2.text, "Region moved in March");
    assert_eq!(b2.author, user);
    assert!(!b2.shown);
    let c3 = book.note_at(0, 2, 2).expect("a note on C3");
    assert_eq!(c3.text, "Budi: discount applied");
    assert!(c3.shown);
    assert_eq!(book.note_cells(0), [(1, 1), (2, 2), (3, 2)]);

    let (_, outcome, printed) = run_printed(book, &code, "ListNotes");
    ok(&outcome);
    assert_eq!(
        printed,
        [
            "3",
            "$B$2|Region moved in March|False",
            "$C$3|Budi: discount applied|True",
            "$C$4|Check with Andi|False",
            "$C$3",
            "$C$3",
            "Nothing",
            "$C$3",
            "Nothing",
            "True",
        ]
    );
}

#[test]
fn change_hide_and_delete_notes() {
    let code = r##"
Sub Tidy()
    Range("B2").Comment.Text = "Region moved in April"
    Range("C4").Comment.Text "Andi: ", 1, False
    Range("C3").Comment.Visible = False
    Range("C4").Comment.Visible = True
    Debug.Print Range("C4").Comment.Text
    Range("C4").Comment.Delete
    Debug.Print CStr(ActiveSheet.Comments.Count)
    Range("A1:B5").ClearComments
    Debug.Print CStr(ActiveSheet.Comments.Count)
End Sub
"##;
    let (book, outcome) = run(sales(), ADD_NOTES, "AddNotes");
    ok(&outcome);
    let (book, outcome, printed) = run_printed(book, code, "Tidy");
    ok(&outcome);
    assert_eq!(printed, ["Andi: Check with Andi", "2", "1"]);
    assert!(book.note_at(0, 1, 1).is_none());
    assert!(book.note_at(0, 3, 2).is_none());
    let c3 = book.note_at(0, 2, 2).expect("C3 keeps its note");
    assert!(!c3.shown);
}

#[test]
fn note_text_reads_and_writes() {
    let code = r##"
Sub Notes()
    Range("A2").NoteText "Top seller"
    Range("A2").NoteText " of 2026", 11
    Debug.Print Range("A2").NoteText
    Debug.Print Range("A2").NoteText(Start:=5, Length:=6)
    Range("A2").NoteText "Best", 1, 3
    Debug.Print Range("A2").Comment.Text
    Debug.Print Range("A3").NoteText = ""
End Sub
"##;
    let (book, outcome, printed) = run_printed(sales(), code, "Notes");
    ok(&outcome);
    assert_eq!(
        printed,
        [
            "Top seller of 2026",
            "seller",
            "Best seller of 2026",
            "True"
        ]
    );
    assert_eq!(book.note_at(0, 1, 0).unwrap().text, "Best seller of 2026");
    assert!(book.note_at(0, 2, 0).is_none());
}

#[test]
fn adding_a_second_note_fails_as_in_excel() {
    let code = r##"
Sub Twice()
    Range("B3").AddComment "first"
    Range("B3").AddComment "second"
End Sub

Sub Trapped()
    On Error Resume Next
    Range("B3").AddComment "third"
    Debug.Print CStr(Err.Number)
    Err.Clear
    Range("B4").Comment.Text = "x"
    Debug.Print CStr(Err.Number)
End Sub
"##;
    let (book, outcome) = run(sales(), code, "Twice");
    assert_eq!(err(&outcome).number, 1004);
    let (book, outcome, printed) = run_printed(book, code, "Trapped");
    ok(&outcome);
    assert_eq!(printed, ["1004", "91"]);
    assert_eq!(book.note_at(0, 2, 1).unwrap().text, "first");
}

#[test]
fn threaded_comments_are_not_supported() {
    for code in [
        "Sub T()\n    Range(\"A1\").AddCommentThreaded \"Hi\"\nEnd Sub\n",
        "Sub T()\n    Debug.Print TypeName(Range(\"A1\").CommentThreaded)\nEnd Sub\n",
        "Sub T()\n    Debug.Print ActiveSheet.CommentsThreaded.Count\nEnd Sub\n",
    ] {
        let (_, outcome) = run(sales(), code, "T");
        let e = err(&outcome);
        assert_eq!(e.number, 445, "{code}");
        assert!(e.description.contains("Threaded"), "{}", e.description);
    }
}

#[test]
fn notes_survive_save_and_reopen() {
    let (book, outcome, printed) = run_printed(sales(), ADD_NOTES, "AddNotes");
    ok(&outcome);
    let user = printed[0].clone();
    let (book, _dir) = reopen(book, "notes.xlsx");
    assert_eq!(book.note_cells(0), [(1, 1), (2, 2), (3, 2)]);
    let b2 = book.note_at(0, 1, 1).unwrap();
    assert_eq!(b2.text, "Region moved in March");
    assert_eq!(b2.author, user);
    assert!(!b2.shown);
    let c3 = book.note_at(0, 2, 2).unwrap();
    assert_eq!(c3.text, "Budi: discount applied");
    assert!(c3.shown);
    // And a macro sees them in the reopened file.
    let code = "Sub R()\n    Debug.Print Range(\"C4\").Comment.Text\nEnd Sub\n";
    let (_, outcome, printed) = run_printed(book, code, "R");
    ok(&outcome);
    assert_eq!(printed, ["Check with Andi"]);
}

#[test]
fn undo_takes_the_notes_back_in_one_step() {
    let mut book = sales();
    book.set_note(0, 0, 0, "Kept", "Sari").unwrap();
    let code = format!(
        "{ADD_NOTES}{}",
        r##"
Sub More()
    AddNotes
    Range("A1").Comment.Text = "Changed"
    Range("A1").Comment.Visible = True
End Sub
"##
    );
    let (mut book, outcome) = run(book, &code, "More");
    ok(&outcome);
    assert_eq!(book.note_cells(0).len(), 4);
    assert!(book.undo().unwrap());
    assert_eq!(book.note_cells(0), [(0, 0)]);
    let a1 = book.note_at(0, 0, 0).unwrap();
    assert_eq!(a1.text, "Kept");
    assert!(!a1.shown);
}
