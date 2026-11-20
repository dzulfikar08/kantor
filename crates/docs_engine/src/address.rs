//! Word's Address Block and Greeting Line (`ADDRESSBLOCK`,
//! `GREETINGLINE`) and the address fields they draw on (Word's *Match
//! Fields*).
//!
//! Both fields name Word's address fields ("First Name", "Address 1"…)
//! through placeholders in their `\f` format (`<<_FIRST0_>>`,
//! `<<_STREET1_>>`); a field map (`w:odso/w:fieldMapData` in settings.xml,
//! or Word's guesses from the column names) says which column of the data
//! source each address field is. What Word does follows [MS-OI29500]
//! 2.1.460 and 2.1.476 and Word 365's merge of docx4j's CR-067
//! `address-probe-r1.docx` (see docs/mail-merge.md).

use std::sync::OnceLock;

use crate::fields::Instr;
use crate::merge::MergeData;
use crate::xml;

// ── address fields and the field map ─────────────────────────────────────

/// Word's address fields, in the order of its Match Fields dialog (and of
/// the `w:fieldMapData` it writes).
pub const ADDRESS_FIELDS: &[&str] = &[
    "Unique Identifier",
    "Courtesy Title",
    "First Name",
    "Last Name",
    "Middle Name",
    "Suffix",
    "Nickname",
    "Job Title",
    "Company",
    "Address 1",
    "Address 2",
    "City",
    "State",
    "Postal Code",
    "Country or Region",
    "Business Phone",
    "Business Fax",
    "Home Phone",
    "Home Fax",
    "E-mail Address",
    "Age",
    "Gender",
    "Spouse/Partner Courtesy Title",
    "Spouse/Partner First Name",
    "Spouse/Partner Last Name",
    "Spouse/Partner Middle Name",
    "Spouse/Partner Nickname",
    "Phonetic Guide for First Name",
    "Phonetic Guide for Last Name",
    "Address 3",
    "Department",
];

/// The address fields Address Block and Greeting Line use (the ones the
/// Match Fields dialog lists first, as Word's "Required information").
pub const BLOCK_FIELDS: &[&str] = &[
    "Courtesy Title",
    "First Name",
    "Middle Name",
    "Last Name",
    "Suffix",
    "Nickname",
    "Company",
    "Address 1",
    "Address 2",
    "City",
    "State",
    "Postal Code",
    "Country or Region",
    "Spouse/Partner Courtesy Title",
    "Spouse/Partner First Name",
];

/// Column names Kantor takes for an address field (lower case, letters
/// and digits only): Word's English guesses plus Indonesian, German,
/// French and Japanese headers.
fn synonyms(field: &str) -> &'static [&'static str] {
    match field {
        "Unique Identifier" => &["uniqueidentifier", "id", "uniqueid"],
        "Courtesy Title" => &[
            "courtesytitle",
            "title",
            "salutation",
            "honorific",
            "prefix",
            "mrmrs",
            "sapaan",
            "panggilan",
            "anrede",
            "civilite",
            "civilité",
            "敬称",
        ],
        "First Name" => &[
            "firstname",
            "first",
            "givenname",
            "forename",
            "fname",
            "namadepan",
            "vorname",
            "prenom",
            "prénom",
            "名",
        ],
        "Last Name" => &[
            "lastname",
            "last",
            "surname",
            "familyname",
            "lname",
            "namabelakang",
            "namakeluarga",
            "nachname",
            "familienname",
            "nomdefamille",
            "姓",
        ],
        "Middle Name" => &["middlename", "middle", "middleinitial", "namatengah"],
        "Suffix" => &["suffix", "akhiran"],
        "Nickname" => &["nickname", "nick", "namapanggilan", "spitzname", "surnom"],
        "Job Title" => &["jobtitle", "position", "jabatan", "role", "beruf", "poste"],
        "Company" => &[
            "company",
            "companyname",
            "organization",
            "organisation",
            "business",
            "perusahaan",
            "namaperusahaan",
            "instansi",
            "firma",
            "firme",
            "unternehmen",
            "societe",
            "société",
            "entreprise",
            "会社名",
            "会社",
        ],
        "Address 1" => &[
            "address1",
            "address",
            "addressline1",
            "street",
            "streetaddress",
            "street1",
            "alamat",
            "alamat1",
            "jalan",
            "strasse",
            "straße",
            "adresse",
            "adresse1",
            "住所",
            "住所1",
        ],
        "Address 2" => &[
            "address2",
            "addressline2",
            "street2",
            "alamat2",
            "adresse2",
            "住所2",
        ],
        "City" => &[
            "city",
            "town",
            "kota",
            "kotakabupaten",
            "kabupatenkota",
            "stadt",
            "ort",
            "ville",
            "市区町村",
        ],
        "State" => &[
            "state",
            "province",
            "region",
            "county",
            "stateprovince",
            "provinsi",
            "propinsi",
            "bundesland",
            "prefecture",
            "都道府県",
        ],
        "Postal Code" => &[
            "postalcode",
            "postcode",
            "zip",
            "zipcode",
            "postal",
            "kodepos",
            "plz",
            "postleitzahl",
            "codepostal",
            "郵便番号",
        ],
        "Country or Region" => &[
            "countryorregion",
            "countryregion",
            "country",
            "negara",
            "land",
            "pays",
            "国",
        ],
        "Business Phone" => &["businessphone", "workphone", "phone", "telepon", "telp"],
        "Home Phone" => &["homephone", "teleponrumah"],
        "E-mail Address" => &["emailaddress", "email", "mail", "surel", "alamatemail"],
        "Department" => &["department", "dept", "divisi", "departemen", "bagian"],
        "Address 3" => &["address3", "addressline3", "alamat3"],
        "Gender" => &["gender", "sex", "jeniskelamin"],
        "Age" => &["age", "usia", "umur"],
        _ => &[],
    }
}

/// One-column names for a whole name: taken as First Name when the list
/// has no first or last name column (lists in Indonesia often have just
/// "Nama").
const FULL_NAME: &[&str] = &["name", "nama", "fullname", "namalengkap", "名前", "氏名"];

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Word's guess of the column for an address field: the column named as
/// the field or a usual synonym of it (English and Indonesian headers).
pub fn guess(field: &str, columns: &[String]) -> Option<usize> {
    let names: Vec<String> = columns.iter().map(|c| norm(c)).collect();
    let find = |list: &[&str]| {
        list.iter()
            .find_map(|s| names.iter().position(|n| n == &norm(s)))
    };
    let own = norm(field);
    if let Some(k) = names.iter().position(|n| *n == own) {
        return Some(k);
    }
    if let Some(k) = find(synonyms(field)) {
        return Some(k);
    }
    if field == "First Name"
        && find(synonyms("Last Name")).is_none()
        && !names.iter().any(|n| n == "lastname")
    {
        return find(FULL_NAME);
    }
    None
}

/// Which column each address field is (Word's *Match Fields*).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldMap {
    /// Matched as saved (`w:fieldMapData`): address field → column name.
    pub matched: Vec<(String, String)>,
    /// Whether the matching was saved: fields it doesn't name are then
    /// "(not matched)" rather than guessed.
    pub saved: bool,
}

impl FieldMap {
    /// The column of `field` in `columns`: as matched, else guessed. A
    /// saved match naming a column the list doesn't have is guessed again.
    pub fn column(&self, field: &str, columns: &[String]) -> Option<usize> {
        let field_n = norm(field);
        if let Some((_, col)) = self.matched.iter().find(|(f, _)| norm(f) == field_n) {
            let c = norm(col);
            if let Some(k) = columns.iter().position(|x| norm(x) == c) {
                return Some(k);
            }
            return guess(field, columns);
        }
        if self.saved {
            return None;
        }
        guess(field, columns)
    }

    /// Every address field's column for `columns` (by [`ADDRESS_FIELDS`]).
    pub fn resolve(&self, columns: &[String]) -> Vec<(&'static str, Option<usize>)> {
        ADDRESS_FIELDS
            .iter()
            .map(|f| (*f, self.column(f, columns)))
            .collect()
    }

    /// Read the matching saved in `w:odso`; `None` when it has none.
    pub fn from_odso(odso: &str) -> Option<Self> {
        let mut matched = Vec::new();
        let mut any = false;
        let mut rest = odso;
        while let Some(k) = rest.find("<w:fieldMapData") {
            let tail = &rest[k..];
            let end = element_end(tail);
            any = true;
            if let Some(el) = xml::parse(&tail[..end]) {
                let val = |n: &str| {
                    el.child(n)
                        .and_then(|c| c.attr("val"))
                        .map(str::to_owned)
                        .unwrap_or_default()
                };
                let ty = val("type");
                let name = val("name");
                let mapped = val("mappedName");
                if ty != "null" && !name.is_empty() && !mapped.is_empty() {
                    matched.push((mapped, name));
                }
            }
            rest = &tail[end..];
        }
        any.then_some(Self {
            matched,
            saved: true,
        })
    }

    /// `odso` (or a new `w:odso`) with this matching as its
    /// `w:fieldMapData`, every address field listed as Word writes them.
    pub fn to_odso(&self, odso: Option<&str>, columns: &[String], lid: &str) -> String {
        let esc = xml::escape;
        let mut maps = String::new();
        for f in ADDRESS_FIELDS {
            match self.column(f, columns) {
                Some(k) => maps.push_str(&format!(
                    "<w:fieldMapData><w:type w:val=\"dbColumn\"/><w:name w:val=\"{}\"/><w:mappedName w:val=\"{}\"/><w:column w:val=\"{k}\"/><w:lid w:val=\"{}\"/></w:fieldMapData>",
                    esc(&columns[k]),
                    esc(f),
                    esc(lid)
                )),
                None => maps.push_str(&format!(
                    "<w:fieldMapData><w:column w:val=\"0\"/><w:lid w:val=\"{}\"/></w:fieldMapData>",
                    esc(lid)
                )),
            }
        }
        let Some(odso) = odso.filter(|o| o.contains("</w:odso>")) else {
            return format!("<w:odso>{maps}</w:odso>");
        };
        // Drop the old matching; the new one goes where it was (before
        // `w:recipientData`, as CT_Odso orders them).
        let mut s = String::with_capacity(odso.len() + maps.len());
        let mut rest = odso;
        let mut at: Option<usize> = None;
        while let Some(k) = rest.find("<w:fieldMapData") {
            s.push_str(&rest[..k]);
            at.get_or_insert(s.len());
            let tail = &rest[k..];
            let end = element_end(tail);
            rest = &tail[end..];
        }
        s.push_str(rest);
        let at = at.unwrap_or_else(|| {
            s.find("<w:recipientData")
                .unwrap_or_else(|| s.rfind("</w:odso>").unwrap_or(s.len()))
        });
        s.insert_str(at, &maps);
        s
    }
}

/// The length of the `w:fieldMapData` element `tail` starts with.
fn element_end(tail: &str) -> usize {
    let gt = tail.find('>').map_or(tail.len(), |g| g + 1);
    if tail[..gt].ends_with("/>") {
        return gt;
    }
    tail.find("</w:fieldMapData>")
        .map_or(tail.len(), |e| e + "</w:fieldMapData>".len())
}

/// The columns of the address fields for a data source, worked out once
/// per merge.
#[derive(Clone, Debug, Default)]
pub struct AddressColumns {
    cols: Vec<(&'static str, Option<usize>)>,
}

impl AddressColumns {
    pub fn new(map: &FieldMap, data: &MergeData) -> Self {
        Self {
            cols: map.resolve(&data.fields),
        }
    }

    /// Whether a column is matched to address field `field`.
    pub fn has(&self, field: &str) -> bool {
        self.cols.iter().any(|(f, c)| *f == field && c.is_some())
    }

    /// The column matched to address field `field`.
    pub fn column(&self, field: &str) -> Option<usize> {
        self.cols
            .iter()
            .find(|(f, _)| *f == field)
            .and_then(|(_, c)| *c)
    }

    /// The value of address field `field` (Word's mapped name) in `row`.
    pub fn value(&self, data: &MergeData, row: Option<usize>, field: &str) -> String {
        let Some(row) = row else {
            return String::new();
        };
        self.cols
            .iter()
            .find(|(f, _)| *f == field)
            .and_then(|(_, c)| *c)
            .and_then(|c| data.rows.get(row).and_then(|r| r.get(c)))
            .map(|v| v.text.trim().to_owned())
            .unwrap_or_default()
    }
}

// ── formats ──────────────────────────────────────────────────────────────

/// A placeholder of an `\f` format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ph {
    Title0,
    Nick0,
    First0,
    Middle0,
    Last0,
    Suffix0,
    Title1,
    Nick1,
    First1,
    Middle1,
    Last1,
    Suffix1,
    Company,
    Street1,
    Street2,
    City,
    State,
    Postal,
    Country,
    Before,
    After,
}

impl Ph {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "TITLE0" => Self::Title0,
            "NICK0" => Self::Nick0,
            "FIRST0" => Self::First0,
            "MIDDLE0" => Self::Middle0,
            "LAST0" => Self::Last0,
            "SUFFIX0" => Self::Suffix0,
            "TITLE1" => Self::Title1,
            "NICK1" => Self::Nick1,
            "FIRST1" => Self::First1,
            "MIDDLE1" => Self::Middle1,
            "LAST1" => Self::Last1,
            "SUFFIX1" => Self::Suffix1,
            "COMPANY" => Self::Company,
            "STREET1" => Self::Street1,
            "STREET2" => Self::Street2,
            "CITY" => Self::City,
            "STATE" => Self::State,
            "POSTAL" => Self::Postal,
            "COUNTRY" => Self::Country,
            "BEFORE" => Self::Before,
            "AFTER" => Self::After,
            _ => return None,
        })
    }

    /// The address field it stands for ([MS-OI29500]: Word takes
    /// `_SUFFIX1_` as the spouse's nickname and `_NICK1_` as nothing).
    pub fn field(self) -> &'static str {
        match self {
            Self::Title0 => "Courtesy Title",
            Self::Nick0 => "Nickname",
            Self::First0 => "First Name",
            Self::Middle0 => "Middle Name",
            Self::Last0 => "Last Name",
            Self::Suffix0 => "Suffix",
            Self::Title1 => "Spouse/Partner Courtesy Title",
            Self::Nick1 => "",
            Self::First1 => "Spouse/Partner First Name",
            Self::Middle1 => "Spouse/Partner Middle Name",
            Self::Last1 => "Spouse/Partner Last Name",
            Self::Suffix1 => "Spouse/Partner Nickname",
            Self::Company => "Company",
            Self::Street1 => "Address 1",
            Self::Street2 => "Address 2",
            Self::City => "City",
            Self::State => "State",
            Self::Postal => "Postal Code",
            Self::Country => "Country or Region",
            Self::Before | Self::After => "",
        }
    }

    fn is_name(self) -> bool {
        matches!(
            self,
            Self::Title0
                | Self::Nick0
                | Self::First0
                | Self::Middle0
                | Self::Last0
                | Self::Suffix0
                | Self::Title1
                | Self::Nick1
                | Self::First1
                | Self::Middle1
                | Self::Last1
                | Self::Suffix1
        )
    }

    fn is_address(self) -> bool {
        matches!(
            self,
            Self::Street1 | Self::Street2 | Self::City | Self::State | Self::Postal | Self::Country
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Part {
    Text(String),
    Ph(Ph),
}

/// A piece of a format: text shown as it is, or a `<< … >>` group shown
/// only when its placeholders have values.
#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Text(String),
    Group(Vec<Part>),
}

impl Piece {
    fn phs(&self) -> Vec<Ph> {
        match self {
            Piece::Text(_) => Vec::new(),
            Piece::Group(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    Part::Ph(ph) => Some(*ph),
                    Part::Text(_) => None,
                })
                .collect(),
        }
    }

    /// The text as written (placeholders as `_NAME_`), for comparing
    /// greeting formats.
    fn source(&self) -> String {
        let part = |p: &Part| match p {
            Part::Text(t) => t.clone(),
            Part::Ph(ph) => format!("_{}_", ph_name(*ph)),
        };
        match self {
            Piece::Text(t) => t.clone(),
            Piece::Group(parts) => format!("<<{}>>", parts.iter().map(part).collect::<String>()),
        }
    }
}

fn ph_name(ph: Ph) -> &'static str {
    match ph {
        Ph::Title0 => "TITLE0",
        Ph::Nick0 => "NICK0",
        Ph::First0 => "FIRST0",
        Ph::Middle0 => "MIDDLE0",
        Ph::Last0 => "LAST0",
        Ph::Suffix0 => "SUFFIX0",
        Ph::Title1 => "TITLE1",
        Ph::Nick1 => "NICK1",
        Ph::First1 => "FIRST1",
        Ph::Middle1 => "MIDDLE1",
        Ph::Last1 => "LAST1",
        Ph::Suffix1 => "SUFFIX1",
        Ph::Company => "COMPANY",
        Ph::Street1 => "STREET1",
        Ph::Street2 => "STREET2",
        Ph::City => "CITY",
        Ph::State => "STATE",
        Ph::Postal => "POSTAL",
        Ph::Country => "COUNTRY",
        Ph::Before => "BEFORE",
        Ph::After => "AFTER",
    }
}

/// A line break in a format: Word writes the paragraph marks of the
/// dialog's layout into the code; `\n` is not one (Word reads a
/// backslash as making the next character plain, so `\n` is an "n").
fn is_break(c: char) -> bool {
    matches!(c, '\r' | '\n' | '\u{b}')
}

/// Parse an `\f` format.
fn parse_format(f: &str) -> Vec<Piece> {
    // Plain text with escapes undone; groups split off.
    let mut pieces = Vec::new();
    let mut text = String::new();
    let mut chars = f.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    text.push(n);
                }
            }
            '<' if chars.peek() == Some(&'<') => {
                chars.next();
                if !text.is_empty() {
                    pieces.push(Piece::Text(std::mem::take(&mut text)));
                }
                let mut inner = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            if let Some(n) = chars.next() {
                                inner.push(n);
                            }
                        }
                        '>' if chars.peek() == Some(&'>') => {
                            chars.next();
                            break;
                        }
                        c => inner.push(c),
                    }
                }
                pieces.push(Piece::Group(parse_group(&inner)));
            }
            c => text.push(c),
        }
    }
    if !text.is_empty() {
        pieces.push(Piece::Text(text));
    }
    pieces
}

fn parse_group(s: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        if rest.starts_with('_')
            && let Some(end) = rest[1..].find('_')
            && let Some(ph) = Ph::parse(&rest[1..1 + end])
        {
            if !text.is_empty() {
                parts.push(Part::Text(std::mem::take(&mut text)));
            }
            parts.push(Part::Ph(ph));
            rest = &rest[end + 2..];
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        text.push(c);
        rest = &rest[c.len_utf8()..];
    }
    if !text.is_empty() {
        parts.push(Part::Text(text));
    }
    parts
}

/// The text of `pieces`, a group only when every placeholder in it has a
/// value.
fn render(pieces: &[Piece], get: &dyn Fn(Ph) -> String) -> String {
    let mut out = String::new();
    for p in pieces {
        match p {
            Piece::Text(t) => out.push_str(t),
            Piece::Group(parts) => {
                let mut s = String::new();
                let mut ok = true;
                for part in parts {
                    match part {
                        Part::Text(t) => s.push_str(t),
                        Part::Ph(Ph::Before | Ph::After) => {}
                        Part::Ph(ph) => {
                            let v = get(*ph);
                            if v.is_empty() {
                                ok = false;
                                break;
                            }
                            s.push_str(&v);
                        }
                    }
                }
                if ok {
                    out.push_str(&s);
                }
            }
        }
    }
    out
}

// ── countries ────────────────────────────────────────────────────────────

fn country_groups() -> &'static Vec<Vec<String>> {
    static G: OnceLock<Vec<Vec<String>>> = OnceLock::new();
    G.get_or_init(|| {
        include_str!("address_countries.txt")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .map(|l| l.split('|').map(|s| s.trim().to_lowercase()).collect())
            .collect()
    })
}

/// The canonical (English) name of a country written in any of the ways
/// Word knows ([MS-OI29500] 2.1.460's list, with Indonesian names).
pub fn country_key(name: &str) -> Option<&'static str> {
    let n = name.trim().to_lowercase();
    if n.is_empty() {
        return None;
    }
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let ns = squash(&n);
    country_groups()
        .iter()
        .find(|g| g.iter().any(|x| *x == n || squash(x) == ns))
        .map(|g| canonical(&g[0]))
}

fn canonical(lower: &str) -> &'static str {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    let names = NAMES.get_or_init(|| {
        include_str!("address_countries.txt")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .map(|l| l.split('|').next().unwrap_or("").trim().to_owned())
            .collect()
    });
    names
        .iter()
        .find(|n| n.to_lowercase() == lower)
        .map(String::as_str)
        .unwrap_or("")
}

/// Whether two country names are the same country (Word's `\e` test).
pub fn same_country(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a.eq_ignore_ascii_case(b) || a.to_lowercase() == b.to_lowercase() {
        return true;
    }
    matches!((country_key(a), country_key(b)), (Some(x), Some(y)) if x == y)
}

/// How a country writes the address lines after the name and company
/// (Address Block's *Format address according to the destination
/// country/region*). `None`: the format's own layout.
pub fn country_layout(country: &str) -> Option<&'static str> {
    Some(match country_key(country)? {
        "United States of America"
        | "Canada"
        | "Philippines"
        | "Korea (Republic of)"
        | "Japan"
        | "Taiwan"
        | "China"
        | "Hong Kong SAR"
        | "Thailand" => {
            "<<_STREET1_\r>><<_STREET2_\r>><<_CITY_>><<, _STATE_>><< _POSTAL_>><<\r_COUNTRY_>>"
        }
        "United Kingdom" | "Ireland" => {
            "<<_STREET1_\r>><<_STREET2_\r>><<_CITY_\r>><<_STATE_\r>><<_POSTAL_>><<\r_COUNTRY_>>"
        }
        "Australia" => {
            "<<_STREET1_\r>><<_STREET2_\r>><<_CITY_>><< _STATE_>><< _POSTAL_>><<\r_COUNTRY_>>"
        }
        "New Zealand" | "Singapore" | "Malaysia" | "India" => {
            "<<_STREET1_\r>><<_STREET2_\r>><<_CITY_>><< _POSTAL_>><<\r_STATE_>><<\r_COUNTRY_>>"
        }
        "Indonesia" => {
            "<<_STREET1_\r>><<_STREET2_\r>><<_CITY_>><< _POSTAL_>><<\r_STATE_>><<\r_COUNTRY_>>"
        }
        "Germany" | "Austria" | "Switzerland" | "Netherlands" | "Belgium" | "France"
        | "Luxembourg" | "Italy" | "Spain" | "Portugal" | "Denmark" | "Norway" | "Sweden"
        | "Finland" | "Poland" | "Czech Republic" | "Slovak Republic" | "Hungary" | "Croatia"
        | "Slovenia" | "Iceland" | "Liechtenstein" | "Greece" | "Turkey" => {
            "<<_STREET1_\r>><<_STREET2_\r>><<_POSTAL_ >><<_CITY_>><<\r_COUNTRY_>>"
        }
        _ => return None,
    })
}

/// The countries [`home_format`] has a layout of its own for, by the
/// Windows language id the Address Block dialog writes in `\l`.
pub const HOME_COUNTRIES: &[(&str, u32)] = &[
    ("United States of America", 1033),
    ("United Kingdom", 2057),
    ("Indonesia", 1057),
    ("Germany", 1031),
    ("France", 1036),
    ("Japan", 1041),
    ("Australia", 3081),
    ("Netherlands", 1043),
];

/// What Address Block's dialog writes in `\f`: the name in `name` (one of
/// [`NAME_FORMATS`]), the company line when asked, then the address laid
/// out as letters to `home` are (Japan: postcode first, the name last).
pub fn home_format(home: &str, name: &str, company: bool, address: bool) -> String {
    let company = if company { "<<_COMPANY_\r>>" } else { "" };
    if country_key(home) == Some("Japan") {
        let mut s = String::new();
        if address {
            s.push_str("<<〒_POSTAL_\r>><<_STATE_>><<_CITY_>><<_STREET1_>><<\r_STREET2_>>\r");
        }
        s.push_str(company);
        s.push_str(name);
        s.push_str(" 様");
        return s;
    }
    let mut s = format!("{name}\r{company}");
    if address {
        s.push_str(country_layout(home).unwrap_or(
            "<<_STREET1_\r>><<_STREET2_\r>><<_CITY_>><<, _STATE_>><< _POSTAL_>><<\r_COUNTRY_>>",
        ));
    }
    s.trim_end_matches('\r').to_owned()
}

/// The name formats of the Address Block dialog: (sample, `\f` part).
pub const NAME_FORMATS: &[(&str, &str)] = &[
    ("Joshua", "<<_FIRST0_>>"),
    (
        "Joshua Randall Jr.",
        "<<_FIRST0_>><< _LAST0_>><< _SUFFIX0_>>",
    ),
    (
        "Joshua Q. Randall Jr.",
        "<<_FIRST0_>><< _MIDDLE0_>><< _LAST0_>><< _SUFFIX0_>>",
    ),
    (
        "Mr. Josh Randall Jr.",
        "<<_TITLE0_ >><<_FIRST0_>><< _LAST0_>><< _SUFFIX0_>>",
    ),
    (
        "Mr. Josh Q. Randall Jr.",
        "<<_TITLE0_ >><<_FIRST0_>><< _MIDDLE0_>><< _LAST0_>><< _SUFFIX0_>>",
    ),
    ("Randall, Joshua", "<<_LAST0_>><<, _FIRST0_>>"),
    ("Mr. Randall", "<<_TITLE0_ >><<_LAST0_>>"),
];

/// The default when an `ADDRESSBLOCK` has no `\f`: what Word 365 shows
/// (company first, then first and last name, the street lines, city,
/// state and postal code on a line, the country).
pub const DEFAULT_BLOCK: &str = "<<_COMPANY_\r>><<_FIRST0_>><< _LAST0_>>\r<<_STREET1_\r>><<_STREET2_\r>><<_CITY_>><< _STATE_>><< _POSTAL_>><<\r_COUNTRY_>>";

// ── the fields ───────────────────────────────────────────────────────────

/// The address fields an `ADDRESSBLOCK` or `GREETINGLINE` code uses.
pub fn fields_used(instr: &str) -> Vec<&'static str> {
    let i = Instr::parse(instr);
    let fmt = match (i.kind.as_str(), i.arg("f")) {
        (_, Some(f)) => f.to_owned(),
        ("ADDRESSBLOCK", None) => DEFAULT_BLOCK.to_owned(),
        _ => return Vec::new(),
    };
    let mut out: Vec<&'static str> = Vec::new();
    for ph in parse_format(&fmt).iter().flat_map(Piece::phs) {
        let f = ph.field();
        if !f.is_empty() && !out.contains(&f) {
            out.push(f);
        }
    }
    out
}

/// An `ADDRESSBLOCK`'s lines for one record (`get`: an address field's
/// value, by Word's name). Empty lines are left out.
pub fn address_block(i: &Instr, get: &dyn Fn(&str) -> String) -> Vec<String> {
    let fmt = i.arg("f").unwrap_or(DEFAULT_BLOCK);
    let mut pieces = parse_format(fmt);
    let country = get("Country or Region");
    // Word keeps only the last `\e`.
    let excluded = i
        .switches
        .iter()
        .rev()
        .find(|(s, a)| s == "e" && a.is_some())
        .and_then(|(_, a)| a.clone());
    let home = excluded
        .as_deref()
        .is_some_and(|e| same_country(&country, e));
    // `\c 2`: the country only when it isn't the `\e` one (Word 365 shows
    // it with `\c 0` and `\c 1` alike).
    let drop_country = i.arg("c").map(str::trim) == Some("2") && home;
    // `\d`: a foreign address laid out as its country does: the name as
    // the format writes it, the company, then the country's lines (Word
    // 365 leaves the format's other text out).
    if i.has("d")
        && !home
        && let Some(layout) = country_layout(&country)
        && pieces
            .iter()
            .any(|p| p.phs().iter().any(|ph| ph.is_address()))
    {
        let name: Vec<Piece> = pieces
            .iter()
            .filter(|p| {
                let phs = p.phs();
                !phs.is_empty() && phs.iter().all(|ph| ph.is_name())
            })
            .cloned()
            .collect();
        let company = pieces.iter().any(|p| p.phs().contains(&Ph::Company));
        let mut f: Vec<Piece> = name;
        f.push(Piece::Text("\r".into()));
        if company {
            f.push(Piece::Group(vec![
                Part::Ph(Ph::Company),
                Part::Text("\r".into()),
            ]));
        }
        f.extend(parse_format(layout));
        pieces = f;
    }
    let value = |ph: Ph| {
        if ph == Ph::Country && drop_country {
            return String::new();
        }
        let f = ph.field();
        if f.is_empty() { String::new() } else { get(f) }
    };
    let text = render(&pieces, &value);
    text.split(is_break)
        .map(|l| {
            l.trim_matches(|c: char| c.is_whitespace() || c == ',')
                .to_owned()
        })
        .filter(|l| !l.is_empty())
        .collect()
}

/// The greeting line name formats Word accepts ([MS-OI29500] 2.1.476),
/// spaces left out.
const GREETING_FORMATS: &[&str] = &[
    "<<_TITLE0_>><<_LAST0_>>",
    "<<_TITLE0_>><<and_TITLE1_>><<_LAST0_>>",
    "<<_TITLE0_>><<_FIRST0_>><<_LAST0_>><<_SUFFIX0_>>",
    "<<_TITLE0_>><<_NICK0_>><<_LAST0_>><<_SUFFIX0_>>",
    "<<_FIRST0_>><<_LAST0_>><<_SUFFIX0_>>",
    "<<_NICK0_>><<_LAST0_>><<_SUFFIX0_>>",
    "<<_FIRST0_>><<and_FIRST1_>><<_LAST0_>>",
    "<<_NICK0_>><<and_FIRST1_>><<_LAST0_>>",
    "<<_FIRST0_>>",
    "<<_NICK0_>>",
    "<<_FIRST0_>><<and_FIRST1_>>",
    "<<_NICK0_>><<and_FIRST1_>>",
    "<<Mr.andMrs._LAST0_>>",
];

/// The Greeting Line dialog's name formats: (sample, `\f` part).
pub const GREETING_NAMES: &[(&str, &str)] = &[
    ("Mr. Randall", "<<_TITLE0_ >><<_LAST0_>>"),
    (
        "Mr. Josh Randall Jr.",
        "<<_TITLE0_ >><<_FIRST0_>><< _LAST0_>><< _SUFFIX0_>>",
    ),
    ("Josh Randall Jr.", "<<_FIRST0_>><< _LAST0_>><< _SUFFIX0_>>"),
    ("Josh", "<<_FIRST0_>>"),
    (
        "Mr. and Mrs. Randall",
        "<<_TITLE0_ >><<and _TITLE1_ >><<_LAST0_>>",
    ),
    ("Josh and Cynthia", "<<_FIRST0_>><< and _FIRST1_>>"),
    (
        "Josh and Cynthia Randall",
        "<<_FIRST0_>><< and _FIRST1_>><< _LAST0_>>",
    ),
];

/// The `\f` of a greeting line: `before` the name ("Dear"), the name
/// format, `after` it (",").
pub fn greeting_format(before: &str, name: &str, after: &str) -> String {
    let mut s = String::new();
    if !before.is_empty() {
        s.push_str(&format!("<<_BEFORE_ {before} >>"));
    }
    s.push_str(name);
    if !after.is_empty() {
        s.push_str(&format!("<<_AFTER_ {after}>>"));
    }
    s
}

/// A `GREETINGLINE`'s text for one record. A format Word doesn't offer,
/// or no name at all, gives the `\e` text ("Dear Sir or Madam,"). A
/// formal name with the title or last name missing falls back to the
/// first and last name, as Word 365 does ("Dear Pat Lee,", "Dear Sam,").
pub fn greeting_line(i: &Instr, get: &dyn Fn(&str) -> String) -> String {
    let invalid = i.arg("e").unwrap_or_default().to_owned();
    let Some(fmt) = i.arg("f") else {
        return invalid;
    };
    let pieces = parse_format(fmt);
    let mut before = String::new();
    let mut after = String::new();
    let mut name: Vec<Piece> = Vec::new();
    for p in pieces {
        if let Piece::Group(parts) = &p
            && let Some(Part::Ph(ph @ (Ph::Before | Ph::After))) =
                parts.iter().find(|x| matches!(x, Part::Ph(_)))
        {
            let text: String = parts
                .iter()
                .filter_map(|x| match x {
                    Part::Text(t) => Some(t.as_str()),
                    Part::Ph(_) => None,
                })
                .collect();
            if *ph == Ph::Before {
                before = text.trim().to_owned();
            } else {
                after = text.trim().to_owned();
            }
            continue;
        }
        name.push(p);
    }
    let shape: String = name
        .iter()
        .map(Piece::source)
        .collect::<String>()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if !GREETING_FORMATS.contains(&shape.as_str()) {
        return invalid;
    }
    let value = |ph: Ph| {
        let f = ph.field();
        if f.is_empty() { String::new() } else { get(f) }
    };
    let phs: Vec<Ph> = name.iter().flat_map(Piece::phs).collect();
    let formal = phs.contains(&Ph::Title0);
    let missing = phs
        .iter()
        .any(|ph| matches!(ph, Ph::Title0 | Ph::Last0) && value(*ph).is_empty());
    let first_missing = phs
        .iter()
        .any(|ph| matches!(ph, Ph::First0 | Ph::Nick0) && value(*ph).is_empty());
    let who = if formal && missing {
        [value(Ph::First0), value(Ph::Last0)]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    } else if first_missing || missing {
        String::new()
    } else {
        render(&name, &value).trim().to_owned()
    };
    if who.is_empty() {
        return invalid;
    }
    let mut s = String::new();
    if !before.is_empty() {
        s.push_str(&before);
        s.push(' ');
    }
    s.push_str(&who);
    s.push_str(&after);
    s
}

/// The code the Address Block dialog writes.
pub fn address_block_code(
    format: &str,
    lid: u32,
    country: CountryRule,
    home: &str,
    by_destination: bool,
) -> String {
    let q = |s: &str| s.replace('"', "\\\"");
    let mut code = format!("ADDRESSBLOCK \\f \"{}\" \\l {lid}", q(format));
    match country {
        CountryRule::Never => code.push_str(" \\c 0"),
        CountryRule::Always => code.push_str(" \\c 1"),
        CountryRule::UnlessHome => {
            code.push_str(&format!(" \\c 2 \\e \"{}\"", q(home)));
        }
    }
    if by_destination {
        code.push_str(" \\d");
    }
    code
}

/// The code the Greeting Line dialog writes.
pub fn greeting_line_code(format: &str, lid: u32, invalid: &str) -> String {
    let q = |s: &str| s.replace('"', "\\\"");
    let mut code = format!("GREETINGLINE \\f \"{}\" \\l {lid}", q(format));
    if !invalid.is_empty() {
        code.push_str(&format!(" \\e \"{}\"", q(invalid)));
    }
    code
}

/// Address Block's country options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CountryRule {
    /// Never include the country (Kantor leaves `_COUNTRY_` out of the
    /// format too: Word 365 shows the country with `\c 0`).
    Never,
    Always,
    /// Only when it isn't the home country (`\c 2 \e "…"`).
    UnlessHome,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(pairs: &[(&str, &str)]) -> impl Fn(&str) -> String {
        let v: Vec<(String, String)> = pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        move |f: &str| {
            v.iter()
                .find(|(k, _)| k == f)
                .map(|(_, x)| x.clone())
                .unwrap_or_default()
        }
    }

    #[test]
    fn formats_parse_groups_and_escapes() {
        let p = parse_format("<<_TITLE0_ >><<_FIRST0_>>\\n<<, _STATE_>>");
        assert_eq!(
            p,
            vec![
                Piece::Group(vec![Part::Ph(Ph::Title0), Part::Text(" ".into())]),
                Piece::Group(vec![Part::Ph(Ph::First0)]),
                Piece::Text("n".into()),
                Piece::Group(vec![Part::Text(", ".into()), Part::Ph(Ph::State)]),
            ]
        );
    }

    #[test]
    fn countries_match_in_many_languages() {
        assert!(same_country("Deutschland", "Germany"));
        assert!(same_country("Jerman", "allemagne"));
        assert!(same_country("USA", "United States"));
        assert!(same_country("Amerika Serikat", "Etats-Unis"));
        assert!(same_country("UK", "Inggris"));
        assert!(!same_country("Australia", "New Zealand"));
        assert_eq!(country_key("NZ"), Some("New Zealand"));
        assert_eq!(country_key("Jepang"), Some("Japan"));
    }

    #[test]
    fn guesses_follow_english_and_indonesian_headers() {
        let cols: Vec<String> = ["Nama", "Alamat", "Kota", "Kode Pos", "Provinsi", "Negara"]
            .map(String::from)
            .to_vec();
        assert_eq!(guess("First Name", &cols), Some(0));
        assert_eq!(guess("Address 1", &cols), Some(1));
        assert_eq!(guess("City", &cols), Some(2));
        assert_eq!(guess("Postal Code", &cols), Some(3));
        assert_eq!(guess("State", &cols), Some(4));
        assert_eq!(guess("Country or Region", &cols), Some(5));
        assert_eq!(guess("Last Name", &cols), None);
        let en: Vec<String> = ["Title", "First_Name", "Surname", "ZIP"]
            .map(String::from)
            .to_vec();
        assert_eq!(guess("Courtesy Title", &en), Some(0));
        assert_eq!(guess("First Name", &en), Some(1));
        assert_eq!(guess("Last Name", &en), Some(2));
        assert_eq!(guess("Postal Code", &en), Some(3));
    }

    #[test]
    fn odso_matching_round_trips() {
        let cols: Vec<String> = ["Nama", "Kota", "Kode Pos"].map(String::from).to_vec();
        let map = FieldMap {
            matched: vec![("Postal Code".into(), "Kode Pos".into())],
            saved: true,
        };
        let odso = map.to_odso(
            Some("<w:odso><w:udl w:val=\"x\"/><w:recipientData r:id=\"rId1\"/></w:odso>"),
            &cols,
            "id-ID",
        );
        assert!(odso.starts_with("<w:odso><w:udl w:val=\"x\"/><w:fieldMapData>"));
        assert!(odso.ends_with("<w:recipientData r:id=\"rId1\"/></w:odso>"));
        let back = FieldMap::from_odso(&odso).unwrap();
        assert!(back.saved);
        assert_eq!(back.column("Postal Code", &cols), Some(2));
        assert_eq!(back.column("City", &cols), None, "saved as not matched");
        // Rewriting replaces the old matching.
        let again = back.to_odso(Some(&odso), &cols, "id-ID");
        assert_eq!(
            again.matches("<w:fieldMapData>").count(),
            ADDRESS_FIELDS.len()
        );
        assert_eq!(FieldMap::from_odso("<w:odso/>"), None);
    }

    #[test]
    fn address_block_lines() {
        let us = rec(&[
            ("Courtesy Title", "Mr."),
            ("First Name", "Josh"),
            ("Last Name", "Randall"),
            ("Company", "Blue Sky Airlines"),
            ("Address 1", "1 Main St"),
            ("City", "Seattle"),
            ("State", "WA"),
            ("Postal Code", "98052"),
            ("Country or Region", "United States"),
        ]);
        let code = address_block_code(
            &home_format("United States of America", NAME_FORMATS[3].1, true, true),
            1033,
            CountryRule::UnlessHome,
            "United States",
            true,
        );
        let i = Instr::parse(&code);
        assert_eq!(
            address_block(&i, &us),
            vec![
                "Mr. Josh Randall",
                "Blue Sky Airlines",
                "1 Main St",
                "Seattle, WA 98052"
            ]
        );
        // To Germany with \d: postcode before the town, the country kept.
        let de = rec(&[
            ("First Name", "Anna"),
            ("Last Name", "Weber"),
            ("Address 1", "Hauptstraße 5"),
            ("City", "Berlin"),
            ("Postal Code", "10115"),
            ("Country or Region", "Deutschland"),
        ]);
        assert_eq!(
            address_block(&i, &de),
            vec!["Anna Weber", "Hauptstraße 5", "10115 Berlin", "Deutschland"]
        );
        // Indonesia: city and postcode, then the province.
        let id = rec(&[
            ("First Name", "Budi Santoso"),
            ("Address 1", "Jl. Merdeka No. 1"),
            ("City", "Bandung"),
            ("State", "Jawa Barat"),
            ("Postal Code", "40115"),
            ("Country or Region", "Indonesia"),
        ]);
        let code = address_block_code(
            &home_format("Indonesia", NAME_FORMATS[1].1, true, true),
            1057,
            CountryRule::UnlessHome,
            "Indonesia",
            true,
        );
        assert_eq!(
            address_block(&Instr::parse(&code), &id),
            vec![
                "Budi Santoso",
                "Jl. Merdeka No. 1",
                "Bandung 40115",
                "Jawa Barat"
            ]
        );
        // Japan: postcode first, the name last.
        let jp = rec(&[
            ("First Name", "太郎"),
            ("Last Name", "山田"),
            ("Address 1", "千代田1-1"),
            ("City", "千代田区"),
            ("State", "東京都"),
            ("Postal Code", "100-0001"),
        ]);
        let f = home_format("Japan", "<<_LAST0_>><< _FIRST0_>>", false, true);
        let code = address_block_code(&f, 1041, CountryRule::Never, "", false);
        assert_eq!(
            address_block(&Instr::parse(&code), &jp),
            vec!["〒100-0001", "東京都千代田区千代田1-1", "山田 太郎 様"]
        );
    }

    #[test]
    fn greeting_lines() {
        let i = Instr::parse(&greeting_line_code(
            &greeting_format("Dear", GREETING_NAMES[0].1, ","),
            1033,
            "Dear Sir or Madam,",
        ));
        let full = rec(&[
            ("Courtesy Title", "Mr"),
            ("First Name", "John"),
            ("Last Name", "Smith"),
        ]);
        assert_eq!(greeting_line(&i, &full), "Dear Mr Smith,");
        let no_title = rec(&[("First Name", "Pat"), ("Last Name", "Lee")]);
        assert_eq!(greeting_line(&i, &no_title), "Dear Pat Lee,");
        assert_eq!(greeting_line(&i, &rec(&[])), "Dear Sir or Madam,");
        // A format Word doesn't offer: the \e text.
        let odd = Instr::parse(
            r#"GREETINGLINE \f "<<_BEFORE_ To >><<_TITLE0_ >><<_FIRST0_>><< _LAST0_>><<_AFTER_ :>>" \e "To whom it may concern:""#,
        );
        assert_eq!(greeting_line(&odd, &full), "To whom it may concern:");
        assert_eq!(greeting_line(&Instr::parse("GREETINGLINE"), &full), "");
        let yth = Instr::parse(&greeting_line_code(
            &greeting_format("Yth.", GREETING_NAMES[3].1, ""),
            1057,
            "Dengan hormat,",
        ));
        assert_eq!(
            greeting_line(&yth, &rec(&[("First Name", "Budi")])),
            "Yth. Budi"
        );
    }
}
