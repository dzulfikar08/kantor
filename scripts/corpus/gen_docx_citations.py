#!/usr/bin/env python3
"""Write the citation fixtures (CC0, synthetic): tests/corpus/docx/cite_*.docx,
read by crates/docs_engine/tests/citations.rs.

Each file is laid out the way the reference manager's Word plug-in leaves it
(field codes and content controls as described in docs/decisions/0007-citations.md),
with made-up references by Indonesian-sounding authors. The result text is
what the plug-in would have shown; Kantor's tests re-render it themselves.

* cite_zotero.docx: Zotero Word plug-in. Three `ADDIN ZOTERO_ITEM CSL_CITATION`
  fields (one with two items and a page locator, its code split over three
  `w:instrText` runs as Word does with long codes), a `ADDIN ZOTERO_BIBL`
  bibliography whose result spans three paragraphs (italic titles), and the
  document preferences in docProps/custom.xml (`ZOTERO_PREF_1`, `_2`: APA,
  locale id-ID).
* cite_mendeley.docx: Mendeley Desktop. `ADDIN CSL_CITATION` fields (with the
  `mendeley` formatted citation), `ADDIN Mendeley Bibliography
  CSL_BIBLIOGRAPHY`, and `Mendeley Citation Style_1` (IEEE) in custom.xml.
* cite_mendeley_cite.docx: Mendeley Cite (the Office add-in). Inline content
  controls tagged `MENDELEY_CITATION_v3_` + base64 JSON, a block content
  control tagged `MENDELEY_BIBLIOGRAPHY`.
* cite_word.docx: Word's own citations: a `w:citation` content control
  around a `CITATION` field, a bibliography content control
  (docPartGallery "Bibliographies") with a heading and a nested
  `w:bibliography` control around a `BIBLIOGRAPHY` field, and the sources
  as `b:Sources` in customXml/item1.xml (with its itemProps part). Also an
  EndNote `ADDIN EN.CITE` field with a nested `ADDIN EN.CITE.DATA` field
  (`w:fldData`) and an `ADDIN EN.REFLIST` bibliography.

Regenerate with `python3 scripts/corpus/gen_docx_citations.py tests/corpus/docx`
(standard library only, deterministic: fixed zip timestamps).
"""
import base64
import json
import sys
import zipfile
from pathlib import Path
from xml.sax.saxutils import escape

out = Path(sys.argv[1] if len(sys.argv) > 1 else "tests/corpus/docx")
out.mkdir(parents=True, exist_ok=True)

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
DECL = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\r\n'
CSL_SCHEMA = "https://github.com/citation-style-language/schema/raw/master/csl-citation.json"

# ── the references ─────────────────────────────────────────────────────────

NUGROHO = {
    "id": "ITEM-NUGROHO",
    "type": "article-journal",
    "title": "Pengaruh media sosial terhadap prestasi belajar mahasiswa",
    "container-title": "Jurnal Pendidikan Indonesia",
    "volume": "10",
    "issue": "2",
    "page": "115-130",
    "DOI": "10.1234/jpi.v10i2.5678",
    "author": [{"family": "Nugroho", "given": "Adi"}],
    "issued": {"date-parts": [[2021, 6]]},
}
SANTOSO = {
    "id": "ITEM-SANTOSO",
    "type": "book",
    "title": "Metodologi penelitian kuantitatif",
    "publisher": "Penerbit Andi",
    "publisher-place": "Yogyakarta",
    "author": [
        {"family": "Santoso", "given": "Budi"},
        {"family": "Wijaya", "given": "Citra"},
    ],
    "issued": {"date-parts": [[2019]]},
}
RAHMAWATI = {
    "id": "ITEM-RAHMAWATI",
    "type": "article-journal",
    "title": "Sistem informasi akademik berbasis web",
    "container-title": "Jurnal Teknologi Informasi",
    "volume": "6",
    "issue": "1",
    "page": "45-58",
    "author": [
        {"family": "Rahmawati", "given": "Dewi"},
        {"family": "Pratama", "given": "Eko"},
        {"family": "Hidayat", "given": "Fajar"},
    ],
    "issued": {"date-parts": [[2022]]},
}


def item(data, zotero_id):
    d = dict(data)
    d["id"] = zotero_id
    return d


# ── WordprocessingML helpers ───────────────────────────────────────────────


def t(text, rpr=""):
    space = ' xml:space="preserve"' if text != text.strip() else ""
    return f"<w:r>{rpr}<w:t{space}>{escape(text)}</w:t></w:r>"


def p(*runs, style=None):
    ppr = f'<w:pPr><w:pStyle w:val="{style}"/></w:pPr>' if style else ""
    return f"<w:p>{ppr}{''.join(runs)}</w:p>"


def fld(kind, extra=""):
    if extra:
        return f'<w:r><w:fldChar w:fldCharType="{kind}">{extra}</w:fldChar></w:r>'
    return f'<w:r><w:fldChar w:fldCharType="{kind}"/></w:r>'


def instr(*pieces):
    return "".join(
        f'<w:r><w:instrText xml:space="preserve">{escape(s)}</w:instrText></w:r>' for s in pieces
    )


def field(code_pieces, result_runs, begin_extra=""):
    """A complex field in one paragraph: begin, code runs, separate, result, end."""
    return fld("begin", begin_extra) + instr(*code_pieces) + fld("separate") + "".join(result_runs) + fld("end")


ITALIC = "<w:rPr><w:i/></w:rPr>"


def document(body):
    return (
        f'{DECL}<w:document xmlns:w="{W}" xmlns:r="{R}"><w:body>{body}'
        '<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1701" w:right="1701" '
        'w:bottom="1701" w:left="2268" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr>'
        "</w:body></w:document>"
    )


STYLES = (
    f'{DECL}<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr>'
    '<w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman"/><w:sz w:val="24"/>'
    '<w:lang w:val="id-ID"/></w:rPr></w:rPrDefault></w:docDefaults>'
    '<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>'
    '<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/>'
    '<w:pPr><w:keepNext/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="28"/></w:rPr></w:style>'
    '<w:style w:type="paragraph" w:styleId="Bibliography"><w:name w:val="Bibliography"/>'
    '<w:basedOn w:val="Normal"/><w:pPr><w:ind w:left="720" w:hanging="720"/></w:pPr></w:style>'
    "</w:styles>"
)

CORE = (
    f"{DECL}<cp:coreProperties "
    'xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" '
    'xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" '
    'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">'
    "<dc:title>Citations</dc:title><dc:creator>Kantor corpus</dc:creator>"
    '<dcterms:created xsi:type="dcterms:W3CDTF">2026-11-27T00:00:00Z</dcterms:created>'
    '<dcterms:modified xsi:type="dcterms:W3CDTF">2026-11-27T00:00:00Z</dcterms:modified>'
    "</cp:coreProperties>"
)


def custom(props):
    items = "".join(
        f'<property fmtid="{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}" pid="{i + 2}" name="{escape(k)}">'
        f"<vt:lpwstr>{escape(v)}</vt:lpwstr></property>"
        for i, (k, v) in enumerate(props)
    )
    return (
        f'{DECL}<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" '
        f'xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">{items}</Properties>'
    )


def package(name, body, custom_props=None, custom_xml=None):
    ct = [
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>',
        '<Default Extension="xml" ContentType="application/xml"/>',
        '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>',
        '<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>',
        '<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>',
    ]
    rels = [
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>',
        '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>',
    ]
    doc_rels = [
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>',
    ]
    parts = [
        ("word/document.xml", document(body)),
        ("word/styles.xml", STYLES),
        ("docProps/core.xml", CORE),
    ]
    if custom_props:
        ct.append(
            '<Override PartName="/docProps/custom.xml" ContentType="application/vnd.openxmlformats-officedocument.custom-properties+xml"/>'
        )
        rels.append(
            '<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties" Target="docProps/custom.xml"/>'
        )
        parts.append(("docProps/custom.xml", custom(custom_props)))
    if custom_xml:
        ct.append(
            '<Override PartName="/customXml/itemProps1.xml" ContentType="application/vnd.openxmlformats-officedocument.customXmlProperties+xml"/>'
        )
        doc_rels.append(
            '<Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml" Target="../customXml/item1.xml"/>'
        )
        parts.append(("customXml/item1.xml", custom_xml))
        parts.append(
            (
                "customXml/itemProps1.xml",
                f'{DECL}<ds:datastoreItem ds:itemID="{{6E2E1C3A-0B1D-4C5E-9F00-3C1A2B4D5E6F}}" '
                'xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml">'
                '<ds:schemaRefs><ds:schemaRef ds:uri="http://schemas.openxmlformats.org/officeDocument/2006/bibliography"/>'
                "</ds:schemaRefs></ds:datastoreItem>",
            )
        )
        parts.append(
            (
                "customXml/_rels/item1.xml.rels",
                f'{DECL}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
                '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps" Target="itemProps1.xml"/>'
                "</Relationships>",
            )
        )
    parts.insert(
        0,
        (
            "[Content_Types].xml",
            f'{DECL}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">{"".join(ct)}</Types>',
        ),
    )
    parts.insert(
        1,
        (
            "_rels/.rels",
            f'{DECL}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{"".join(rels)}</Relationships>',
        ),
    )
    parts.append(
        (
            "word/_rels/document.xml.rels",
            f'{DECL}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{"".join(doc_rels)}</Relationships>',
        )
    )
    with zipfile.ZipFile(out / name, "w", zipfile.ZIP_DEFLATED) as z:
        for part, text in parts:
            info = zipfile.ZipInfo(part, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, text.encode("utf-8"))


def dumps(v):
    return json.dumps(v, ensure_ascii=False, separators=(",", ":"))


# ── Zotero ─────────────────────────────────────────────────────────────────

ZOTERO_URI = "http://zotero.org/users/1234567/items/"


def zotero_citation(cid, cites, text):
    """`cites`: (data, zotero item id, item key, extra citation-item keys)."""
    return dumps(
        {
            "citationID": cid,
            "properties": {"formattedCitation": text, "plainCitation": text, "noteIndex": 0},
            "citationItems": [
                {"id": zid, "uris": [ZOTERO_URI + key], "itemData": item(data, zid), **extra}
                for data, zid, key, extra in cites
            ],
            "schema": CSL_SCHEMA,
        }
    )


def zotero():
    c1 = zotero_citation("a1b2c3d4", [(NUGROHO, 101, "NUGRH001", {})], "(Nugroho, 2021)")
    c2 = zotero_citation(
        "e5f6g7h8",
        [
            (NUGROHO, 101, "NUGRH001", {"locator": "12", "label": "page"}),
            (SANTOSO, 102, "SANTS002", {}),
        ],
        "(Nugroho, 2021, hlm. 12; Santoso & Wijaya, 2019)",
    )
    c3 = zotero_citation("i9j0k1l2", [(RAHMAWATI, 103, "RAHMW003", {})], "(Rahmawati dkk., 2022)")
    code2 = f" ADDIN ZOTERO_ITEM CSL_CITATION {c2} "
    # Word splits long codes over runs; cut inside the JSON.
    a, b = len(code2) // 3, 2 * len(code2) // 3
    body = "".join(
        [
            p(t("Bab 1 Pendahuluan"), style="Heading1"),
            p(
                t("Penelitian sebelumnya "),
                field([f" ADDIN ZOTERO_ITEM CSL_CITATION {c1} "], [t("(Nugroho, 2021)")]),
                t(" menunjukkan pengaruh yang nyata "),
                field([code2[:a], code2[a:b], code2[b:]], [t("(Nugroho, 2021, hlm. 12; Santoso & Wijaya, 2019)")]),
                t("."),
            ),
            p(t("Paragraf ini diubah dalam pengujian.")),
            p(
                t("Sistem serupa sudah dibangun "),
                field([f" ADDIN ZOTERO_ITEM CSL_CITATION {c3} "], [t("(Rahmawati dkk., 2022)")]),
                t("."),
            ),
            p(t("Daftar Pustaka"), style="Heading1"),
            # The bibliography: one field over three paragraphs.
            '<w:p><w:pPr><w:pStyle w:val="Bibliography"/></w:pPr>'
            + fld("begin")
            + instr(' ADDIN ZOTERO_BIBL {"uncited":[],"omitted":[],"custom":[]} CSL_BIBLIOGRAPHY ')
            + fld("separate")
            + t("Nugroho, A. (2021). Pengaruh media sosial terhadap prestasi belajar mahasiswa. ")
            + t("Jurnal Pendidikan Indonesia", ITALIC)
            + t(", ")
            + t("10", ITALIC)
            + t("(2), 115–130. https://doi.org/10.1234/jpi.v10i2.5678")
            + "</w:p>",
            p(
                t("Rahmawati, D., Pratama, E., & Hidayat, F. (2022). Sistem informasi akademik berbasis web. "),
                t("Jurnal Teknologi Informasi", ITALIC),
                t(", "),
                t("6", ITALIC),
                t("(1), 45–58."),
                style="Bibliography",
            ),
            p(
                t("Santoso, B., & Wijaya, C. (2019). "),
                t("Metodologi penelitian kuantitatif", ITALIC),
                t(". Penerbit Andi."),
                fld("end"),
                style="Bibliography",
            ),
        ]
    )
    pref = (
        '<data data-version="3" zotero-version="7.0.11"><session id="Kq3vXz9P"/>'
        '<style id="http://www.zotero.org/styles/apa" locale="id-ID" hasBibliography="1" '
        'bibliographyStyleHasBeenSet="1"/><prefs><pref name="fieldType" value="Field"/>'
        '<pref name="automaticJournalAbbreviations" value="true"/></prefs></data>'
    )
    chunks = [pref[i : i + 255] for i in range(0, len(pref), 255)]
    props = [(f"ZOTERO_PREF_{i + 1}", c) for i, c in enumerate(chunks)]
    package("cite_zotero.docx", body, custom_props=props)


# ── Mendeley Desktop ───────────────────────────────────────────────────────


def mendeley_citation(items, text):
    return dumps(
        {
            "citationItems": [
                {
                    "id": f"ITEM-{i + 1}",
                    "itemData": item(data, f"ITEM-{i + 1}"),
                    "uris": [f"http://www.mendeley.com/documents/?uuid={uuid}"],
                    **extra,
                }
                for i, (data, uuid, extra) in enumerate(items)
            ],
            "mendeley": {
                "formattedCitation": text,
                "plainTextFormattedCitation": text,
                "previouslyFormattedCitation": text,
            },
            "properties": {"noteIndex": 0},
            "schema": CSL_SCHEMA,
        }
    )


def mendeley():
    c1 = mendeley_citation([(SANTOSO, "5f2c0d9e-1111-4a6b-9c1d-000000000001", {})], "[1]")
    c2 = mendeley_citation(
        [(RAHMAWATI, "5f2c0d9e-2222-4a6b-9c1d-000000000002", {"locator": "50", "label": "page"})], "[2, p. 50]"
    )
    body = "".join(
        [
            p(t("Tinjauan Pustaka"), style="Heading1"),
            p(
                t("Metode kuantitatif dijelaskan "),
                field([f" ADDIN CSL_CITATION {c1} "], [t("[1]")]),
                t(" dan diterapkan "),
                field([f" ADDIN CSL_CITATION {c2} "], [t("[2, p. 50]")]),
                t("."),
            ),
            p(t("Paragraf ini diubah dalam pengujian.")),
            p(t("Referensi"), style="Heading1"),
            '<w:p><w:pPr><w:pStyle w:val="Bibliography"/></w:pPr>'
            + fld("begin")
            + instr(" ADDIN Mendeley Bibliography CSL_BIBLIOGRAPHY ")
            + fld("separate")
            + t("[1]\tB. Santoso and C. Wijaya, ")
            + t("Metodologi penelitian kuantitatif", ITALIC)
            + t(". Yogyakarta: Penerbit Andi, 2019.")
            + "</w:p>",
            p(
                t("[2]\tD. Rahmawati, E. Pratama, and F. Hidayat, “Sistem informasi akademik berbasis web,” "),
                t("Jurnal Teknologi Informasi", ITALIC),
                t(", vol. 6, no. 1, pp. 45–58, 2022."),
                fld("end"),
                style="Bibliography",
            ),
        ]
    )
    props = [
        ("Mendeley Document_1", "True"),
        ("Mendeley Citation Style_1", "http://www.zotero.org/styles/ieee"),
        ("Mendeley Recent Style Id 0_1", "http://www.zotero.org/styles/ieee"),
        ("Mendeley Recent Style Name 0_1", "IEEE"),
        ("Mendeley Unique User Id_1", "00000000-0000-0000-0000-000000000000"),
    ]
    package("cite_mendeley.docx", body, custom_props=props)


# ── Mendeley Cite ──────────────────────────────────────────────────────────


def mendeley_cite_sdt(n, data, text):
    cid = f"MENDELEY_CITATION_0000000{n}-aaaa-4bbb-8ccc-dddddddddddd"
    payload = {
        "citationID": cid,
        "properties": {"noteIndex": 0},
        "isEdited": False,
        "manualOverride": {"isManuallyOverridden": False, "citeprocText": text, "manualOverrideText": ""},
        "citationItems": [
            {"id": f"0000000{n}-1111-2222-3333-444444444444", "itemData": item(data, f"0000000{n}-1111-2222-3333-444444444444"), "isTemporary": False}
        ],
    }
    tag = "MENDELEY_CITATION_v3_" + base64.b64encode(dumps(payload).encode("utf-8")).decode("ascii")
    return (
        f'<w:sdt><w:sdtPr><w:rPr><w:color w:val="000000"/></w:rPr><w:tag w:val="{tag}"/>'
        f'<w:id w:val="-17000000{n}"/><w:placeholder><w:docPart w:val="DefaultPlaceholder_-1854013440"/>'
        "</w:placeholder></w:sdtPr><w:sdtContent>"
        f'<w:r><w:rPr><w:color w:val="000000"/></w:rPr><w:t>{escape(text)}</w:t></w:r>'
        "</w:sdtContent></w:sdt>"
    )


def mendeley_cite():
    body = "".join(
        [
            p(t("Landasan Teori"), style="Heading1"),
            p(
                t("Media sosial memengaruhi belajar "),
                mendeley_cite_sdt(1, NUGROHO, "(Nugroho, 2021)"),
                t(" dan metode kuantitatif sudah baku "),
                mendeley_cite_sdt(2, SANTOSO, "(Santoso & Wijaya, 2019)"),
                t("."),
            ),
            p(t("Paragraf ini diubah dalam pengujian.")),
            p(t("Daftar Pustaka"), style="Heading1"),
            '<w:sdt><w:sdtPr><w:rPr><w:color w:val="000000"/></w:rPr><w:tag w:val="MENDELEY_BIBLIOGRAPHY"/>'
            '<w:id w:val="-1500000001"/><w:placeholder><w:docPart w:val="DefaultPlaceholder_-1854013440"/>'
            "</w:placeholder></w:sdtPr><w:sdtContent>"
            + p(
                t("Nugroho, A. (2021). Pengaruh media sosial terhadap prestasi belajar mahasiswa. "),
                t("Jurnal Pendidikan Indonesia", ITALIC),
                t(", "),
                t("10", ITALIC),
                t("(2), 115–130."),
                style="Bibliography",
            )
            + p(
                t("Santoso, B., & Wijaya, C. (2019). "),
                t("Metodologi penelitian kuantitatif", ITALIC),
                t(". Penerbit Andi."),
                style="Bibliography",
            )
            + "</w:sdtContent></w:sdt>",
            p(t("Lampiran")),
        ]
    )
    package("cite_mendeley_cite.docx", body)


# ── Word and EndNote ───────────────────────────────────────────────────────

B = "http://schemas.openxmlformats.org/officeDocument/2006/bibliography"
SOURCES = (
    f'{DECL}<b:Sources SelectedStyle="\\APASixthEditionOfficeOnline.xsl" StyleName="APA" Version="6" '
    f'xmlns:b="{B}" xmlns="{B}">'
    "<b:Source><b:Tag>Nug21</b:Tag><b:SourceType>JournalArticle</b:SourceType>"
    "<b:Guid>{3F1C2A90-7D4E-4B8A-9E61-0A1B2C3D4E5F}</b:Guid>"
    "<b:Title>Pengaruh media sosial terhadap prestasi belajar mahasiswa</b:Title>"
    "<b:Year>2021</b:Year><b:JournalName>Jurnal Pendidikan Indonesia</b:JournalName>"
    "<b:Pages>115-130</b:Pages><b:Volume>10</b:Volume><b:Issue>2</b:Issue>"
    "<b:Author><b:Author><b:NameList><b:Person><b:Last>Nugroho</b:Last><b:First>Adi</b:First></b:Person>"
    "</b:NameList></b:Author></b:Author><b:RefOrder>1</b:RefOrder></b:Source>"
    "<b:Source><b:Tag>San19</b:Tag><b:SourceType>Book</b:SourceType>"
    "<b:Guid>{5A6B7C8D-9E0F-4A1B-8C2D-3E4F5A6B7C8D}</b:Guid>"
    "<b:Title>Metodologi penelitian kuantitatif</b:Title><b:Year>2019</b:Year>"
    "<b:City>Yogyakarta</b:City><b:Publisher>Penerbit Andi</b:Publisher>"
    "<b:Author><b:Author><b:NameList><b:Person><b:Last>Santoso</b:Last><b:First>Budi</b:First></b:Person>"
    "<b:Person><b:Last>Wijaya</b:Last><b:First>Citra</b:First></b:Person></b:NameList></b:Author></b:Author>"
    "<b:RefOrder>2</b:RefOrder></b:Source>"
    "</b:Sources>"
)

ENDNOTE = (
    "<EndNote><Cite><Author>Rahmawati</Author><Year>2022</Year><RecNum>7</RecNum>"
    "<DisplayText>(Rahmawati et al., 2022)</DisplayText><record><rec-number>7</rec-number>"
    '<ref-type name="Journal Article">17</ref-type><contributors><authors>'
    "<author>Rahmawati, Dewi</author><author>Pratama, Eko</author><author>Hidayat, Fajar</author>"
    "</authors></contributors><titles><title>Sistem informasi akademik berbasis web</title>"
    "<secondary-title>Jurnal Teknologi Informasi</secondary-title></titles>"
    "<pages>45-58</pages><volume>6</volume><number>1</number><dates><year>2022</year></dates>"
    "</record></Cite></EndNote>"
)


def word():
    cite = (
        '<w:sdt><w:sdtPr><w:id w:val="1220000001"/><w:citation/></w:sdtPr><w:sdtContent>'
        + field([" CITATION Nug21 \\l 1057 "], [t("(Nugroho, 2021)")])
        + "</w:sdtContent></w:sdt>"
    )
    cite2 = (
        '<w:sdt><w:sdtPr><w:id w:val="1220000002"/><w:citation/></w:sdtPr><w:sdtContent>'
        + field([" CITATION San19 \\p 23 \\l 1057 "], [t("(Santoso & Wijaya, 2019, p. 23)")])
        + "</w:sdtContent></w:sdt>"
    )
    endnote_data = base64.b64encode(ENDNOTE.encode("utf-8")).decode("ascii")
    en_cite = (
        fld("begin")
        + instr(f" ADDIN EN.CITE {ENDNOTE}")
        + fld("begin", f"<w:fldData xml:space=\"preserve\">{endnote_data}</w:fldData>")
        + instr(" ADDIN EN.CITE.DATA ")
        + fld("end")
        + fld("separate")
        + t("(Rahmawati et al., 2022)")
        + fld("end")
    )
    bibliography = (
        '<w:sdt><w:sdtPr><w:id w:val="1220000010"/><w:docPartObj>'
        '<w:docPartGallery w:val="Bibliographies"/><w:docPartUnique/></w:docPartObj></w:sdtPr>'
        "<w:sdtContent>"
        + p(t("Daftar Pustaka"), style="Heading1")
        + '<w:sdt><w:sdtPr><w:id w:val="1220000011"/><w:bibliography/></w:sdtPr><w:sdtContent>'
        + '<w:p><w:pPr><w:pStyle w:val="Bibliography"/></w:pPr>'
        + fld("begin")
        + instr(" BIBLIOGRAPHY ")
        + fld("separate")
        + t("Nugroho, A. (2021). Pengaruh media sosial terhadap prestasi belajar mahasiswa. ")
        + t("Jurnal Pendidikan Indonesia, 10", ITALIC)
        + t("(2), 115-130.")
        + "</w:p>"
        + p(
            t("Santoso, B., & Wijaya, C. (2019). "),
            t("Metodologi penelitian kuantitatif", ITALIC),
            t(". Yogyakarta: Penerbit Andi."),
            fld("end"),
            style="Bibliography",
        )
        + "</w:sdtContent></w:sdt></w:sdtContent></w:sdt>"
    )
    body = "".join(
        [
            p(t("Pendahuluan"), style="Heading1"),
            p(t("Menurut penelitian "), cite, t(", media sosial berpengaruh "), cite2, t(".")),
            p(t("Paragraf ini diubah dalam pengujian.")),
            p(t("EndNote juga dipakai "), en_cite, t(".")),
            bibliography,
            p(
                fld("begin")
                + instr(" ADDIN EN.REFLIST ")
                + fld("separate")
                + t("Rahmawati, D., Pratama, E., & Hidayat, F. (2022). Sistem informasi akademik berbasis web.")
                + fld("end"),
                style="Bibliography",
            ),
        ]
    )
    package("cite_word.docx", body, custom_xml=SOURCES)


zotero()
mendeley()
mendeley_cite()
word()
