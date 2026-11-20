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
| Address Block… | Word's `ADDRESSBLOCK`: the recipient's name (in the format chosen), company and postal address, the country only when you ask for it, laid out by the destination country when ticked. A preview shows each recipient's block. |
| Greeting Line… | Word's `GREETINGLINE`: "Dear", "To", "Yth." or "Kepada Yth.", the name format and the punctuation, and the line for a recipient with no usable name ("Dear Sir or Madam,", "Dengan hormat,"). |
| Match Fields… | Which column of the list is each of Word's address fields (Courtesy Title, First Name, Last Name, Company, Address 1, City, Postal Code, Country or Region…). Saved in the document as Word saves it (`w:odso/w:fieldMapData`). |
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

| `ADDRESSBLOCK \f "…" \c 2 \e "Indonesia" \d` | The address lines, a paragraph each (empty lines left out). |
| `GREETINGLINE \f "…" \e "Dear Sir or Madam,"` | The greeting. |

### Address Block and Greeting Line

Both take the recipient's values through Word's address fields, not the
column names: `<<_TITLE0_>>` is Courtesy Title, `<<_FIRST0_>>` First Name,
`<<_LAST0_>>` Last Name, `<<_COMPANY_>>`, `<<_STREET1_>>` Address 1,
`<<_CITY_>>`, `<<_STATE_>>`, `<<_POSTAL_>>`, `<<_COUNTRY_>>` and so on
([MS-OI29500] 2.1.460). Text inside `<< >>` shows only when its field has a
value; a line that comes out empty is left out. Without *Match Fields*,
Kantor guesses the columns as Word does from their names, in English and
Indonesian (Title/Sapaan, First Name/Nama Depan, Last Name/Nama Belakang,
Company/Perusahaan, Address/Alamat, City/Kota, State/Provinsi, ZIP/Postal
Code/Kode Pos, Country/Negara; German, French and Japanese headers too). A
list with one *Name*/*Nama* column and no last name gets it as the first
name.

Address Block's switches: `\f` the format (no `\f`: company, name, street
lines, "City State Postcode", country, as Word 365 shows it); `\c 2` with
`\e "country"` leaves the country out when it is that country (Word knows
the country under many names: "Deutschland" is "Germany"; Kantor adds the
Indonesian names). Word 365 shows the country with `\c 0` as with `\c 1`,
so Kantor's *Never include* leaves `<<_COUNTRY_>>` out of the format. `\d`
lays a foreign address out as its country does, after the name and company:

| Country | Lines after the street |
|---|---|
| United States, Canada, Japan (in Latin letters), China, Korea, Philippines… | City, ST 12345 |
| United Kingdom, Ireland | Town / County / Postcode, each on a line |
| Australia | Suburb STATE 2000 |
| Indonesia, New Zealand, Singapore, Malaysia, India | City 12345 / Province |
| Germany, France, Netherlands, Austria, Switzerland, Italy, Spain, Nordic and central European countries | 12345 City |

then the country. Other countries keep the format's own layout. With Japan
as the home country, the dialog writes Japan's own order: 〒postcode, then
prefecture, city and street, the company and the name with 様.

Greeting Line takes one of the name formats Word offers (Mr. Randall,
Mr. Josh Randall Jr., Josh, Josh Randall Jr., Mr. and Mrs. Randall…); any
other format, or a recipient with no name, gets the `\e` text. As in Word
365, a "Mr. Randall" greeting for a recipient without a title or last name
uses the first and last name ("Dear Pat Lee,", "Dear Sam,"). With no `\f`,
Word shows nothing; so does Kantor. `\l` (language) is kept but changes
nothing in the result.

Checked against Word 365's merge of docx4j's CR-067
`address-probe-r1.docx` (four recipients, nine fields;
`crates/docs_engine/tests/merge.rs` repeats it): every greeting and block
matches, except that Word started a new paragraph after the name in three
`\f` formats written with `\n` (Word reads `\n` as a plain "n", as Kantor
does) for a reason the probe doesn't show.

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

- Envelopes, label sheets (Word's *Labels*) and e-mail merge aren't made in
  Kantor; `NEXT` fills label sheets made in Word.
- While Preview Results is on, an Address Block shows on one line (commas
  between the lines); merged documents get a paragraph per line. An
  inserted Address Block or Greeting Line is changed by inserting it again
  (Word's double-click to edit isn't there), and the alternate placeholders
  (`/f0`, `/l0`…) aren't read.
- Unticked recipients live only while the document is open (Word keeps them
  in `w:odso`, which Kantor keeps as read but doesn't change).
- While Preview Results is on, merge fields in headers and footers show their
  names (they are filled in when merging).
- In *Edit Individual Documents*, footnotes and comments of the template are
  shared by every recipient's section.
- Data sources are files: Word's ODBC and Outlook contact sources aren't
  read (their link is kept).
