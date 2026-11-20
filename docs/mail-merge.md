# Mail merge

Kantor fills a template in once per row of a list: offer letters, invoices,
certificates, name cards. Docs works like Word's **Mailings** tab and reads and
writes Word's own merge fields, so a template made in Word merges in Kantor
and the other way round. Presentations and workbooks take `{{Field}}`
placeholders.

The list is a workbook or a text file: `.xlsx`, `.xlsm`, `.xls`, `.xlsb`,
`.ods`, `.csv`, `.tsv`. The first row names the fields (another row can be
chosen); rows that are entirely empty are left out. Values are taken as the
sheet shows them (number formats applied), and numbers and dates keep their
value for the `\#` and `\@` pictures.

## Docs: the Mailings bar

The toolbar's envelope button (or *Ctrl K → Mailings*) shows the bar:

| Button | What it does |
|---|---|
| Select Recipients… | Choose the workbook or CSV file. The document is linked to it. |
| Edit Recipient List… | Sheet, header row, *Sort by*, *Filter* (equal to, contains, is blank…, joined with And), a tick per recipient, and the *File names* for one file per recipient. |
| Insert Merge Field | A `MERGEFIELD` for one of the list's fields at the caret (shown as «Name»). |
| Highlight Merge Fields | Shades the merge fields grey. |
| Preview Results, ◀ ▶ | Shows a recipient's values in place of the field names; the arrows go to the first, previous, next and last recipient. |
| Finish & Merge | *Edit Individual Documents* (one new document, each recipient in its own section starting on a new page), *Save as Documents (.docx)* or *Save as PDF*, one per recipient in a folder, or *Print Documents*. File names follow the pattern set in the recipient list: `{Name} - Offer` (`{#}` is the record number; empty: the template's name and the number). |

The link to the list is saved in the document as Word saves it (`w:mailMerge`
in `settings.xml`, the path relative to the document, and the filter and sort
as Word's query), together with Preview Results and the record shown. Opening
the Mailings bar on such a document reads its list again; when the list was
moved, Kantor (like Word) looks for it beside the document. Unticked
recipients are not saved.

### Fields

| Field | In the merged document |
|---|---|
| `MERGEFIELD Name` | The value. `\b "text"` goes before it and `\f "text"` after it, only when there is a value; `\* Upper`, `Lower`, `FirstCap`, `Caps`; `\# "#,##0.00"` for numbers; `\@ "d MMMM yyyy"` for dates (month names in Indonesian when the text is marked Indonesian). A field the list doesn't have shows Word's `Error! MergeField was not found in header record of data source.` |
| `IF { MERGEFIELD City } = "Jakarta" "Head office" "Branch office"` | The text that applies (numbers compare as numbers, `*` and `?` work as wildcards). |
| `NEXT` | The fields after it take the next recipient (labels, lists). |
| `NEXTIF a op b` | The next recipient, when the condition holds. |
| `SKIPIF a op b` | No document for this recipient when the condition holds. |
| `MERGEREC`, `MERGESEQ` | The recipient's number in the list and the number of documents merged so far. |

Field names follow Word: a column "First Name" is `MERGEFIELD First_Name`
(case doesn't matter). Merge fields become plain text in the merged
documents; other fields (`PAGE`, `DATE`…) stay fields. Headers, footers and
text boxes are merged too; in *Edit Individual Documents* each recipient's
section gets its own header and footer when they hold merge fields. A
paragraph that holds only merge fields that all came out empty is left out
(Word's blank line suppression; a template from Word with
`doNotSuppressBlankLines` keeps them).

## Without a window

```sh
# One PDF per row, named from a field:
kantor docs merge offer.docx staff.xlsx -o out/ --pdf --name "{Name} - Offer"
# .docx and PDF, only some rows, sorted:
kantor docs merge offer.docx staff.xlsx --sheet Staff -o out/ --docx --pdf \
    --where "Dept=Sales" --where "Salary>=10000000" --sort Name
# The template's own list (as saved by Word or Kantor), into one document:
kantor docs merge offer.docx --single offers.docx
# Certificates from a presentation with {{Name}} and {{Course}} on the slide:
kantor slides merge certificate.pptx people.csv -o certificates/ --pdf
# Invoices from a workbook with {{Customer}}, {{Total}}… in its cells:
kantor sheets merge invoice.xlsx orders.xlsx --sheet Orders -o invoices/
```

`--where` takes `Field=Value`, `Field!=Value`, `<`, `>`, `<=`, `>=`,
`Field~Text` (contains), `Field=` (blank) and `Field!=` (not blank); several
must all hold. `--sort Field` or `Field:desc`, repeated for more keys.
Existing files are kept unless `-f` is given; nothing is written when one of
the names is taken. Files with the same name get ` (2)`, ` (3)`…

In presentations, `{{Field}}` may sit anywhere in a slide's text (shapes,
tables, groups), and keeps the formatting of the text it starts in, even when
PowerPoint split it across runs. In workbooks, every text cell's
placeholders are filled; a cell holding just one placeholder for a number or
a date gets the number, so formulas and the cell's number format work on it.
A placeholder the list doesn't have is left as it is (and reported).

## For AI agents

The MCP server (`kantor mcp`) has a `mail_merge` tool with the same options:
`template`, `data`, `sheet`, `header_row`, `where`, `sort`, `output_dir`,
`formats` (`docx`/`pptx`/`xlsx` and `pdf`), `name`, `single` and
`overwrite`. It runs without the app and returns the files it wrote.

## Not done yet

- Envelopes, label sheets (Word's *Labels*), e-mail merge, *Address Block*
  and *Greeting Line* aren't made in Kantor; a template from Word that uses
  them merges its merge fields, and `NEXT` fills label sheets made in Word.
- Unticked recipients live only while the document is open (Word keeps them
  in `w:odso`, which Kantor keeps as read but doesn't change).
- While Preview Results is on, merge fields in headers and footers show their
  names (they are filled in when merging).
- In *Edit Individual Documents*, footnotes and comments of the template are
  shared by every recipient's section.
- Data sources are files: Word's ODBC and Outlook contact sources aren't
  read (their link is kept).
