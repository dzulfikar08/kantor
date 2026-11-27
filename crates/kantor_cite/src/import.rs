//! Reference files to CSL-JSON items: BibLaTeX / BibTeX (`.bib`, parsed
//! with the `biblatex` crate), RIS (`.ris`, the tagged format every
//! manager exports) and CSL-JSON (`.json`, Zotero's and Better BibTeX's
//! export, an array of items or `{"items": […]}`).
//!
//! Each item gets an `id`: the `.bib` key, the RIS `ID` tag, or the one it
//! had. Fields the mapping doesn't know are left out.

use std::path::Path;

use anyhow::{Context as _, bail};
use biblatex::{ChunksExt as _, Person};
use serde_json::{Map, Value, json};

/// The kind of a reference file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bib,
    Ris,
    CslJson,
}

impl Format {
    /// By file name, else by a look at the text.
    pub fn detect(path: &Path, text: &str) -> Option<Self> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        match ext.as_deref() {
            Some("bib" | "bibtex" | "biblatex") => return Some(Self::Bib),
            Some("ris" | "txt") if text.contains("TY  -") => return Some(Self::Ris),
            Some("json" | "csljson") => return Some(Self::CslJson),
            _ => {}
        }
        let t = text.trim_start_matches('\u{feff}').trim_start();
        if t.starts_with('[') || t.starts_with('{') {
            Some(Self::CslJson)
        } else if t.starts_with("TY  -") || t.contains("\nTY  -") {
            Some(Self::Ris)
        } else if t.contains('@') {
            Some(Self::Bib)
        } else {
            None
        }
    }
}

/// Read a reference file.
pub fn read_file(path: &Path) -> anyhow::Result<Vec<Value>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let Some(format) = Format::detect(path, &text) else {
        bail!("{}: not a .bib, RIS or CSL-JSON file", path.display());
    };
    parse(&text, format).with_context(|| format!("reading {}", path.display()))
}

/// Parse reference text.
pub fn parse(text: &str, format: Format) -> anyhow::Result<Vec<Value>> {
    let text = text.trim_start_matches('\u{feff}');
    match format {
        Format::Bib => from_bib(text),
        Format::Ris => Ok(from_ris(text)),
        Format::CslJson => from_csl_json(text),
    }
}

/// CSL-JSON: an array of items, `{"items": […]}`, or one item.
pub fn from_csl_json(text: &str) -> anyhow::Result<Vec<Value>> {
    let v: Value = serde_json::from_str(text).context("not valid JSON")?;
    let items = match v {
        Value::Array(a) => a,
        Value::Object(ref o) if o.get("items").is_some_and(Value::is_array) => {
            o["items"].as_array().cloned().unwrap_or_default()
        }
        Value::Object(_) => vec![v],
        _ => bail!("CSL-JSON is an array of items"),
    };
    let items: Vec<Value> = items
        .into_iter()
        .filter(|i| i.is_object())
        .enumerate()
        .map(|(n, mut i)| {
            if let Some(o) = i.as_object_mut()
                && !o.contains_key("id")
            {
                o.insert("id".into(), Value::String(format!("item-{}", n + 1)));
            }
            i
        })
        .collect();
    if items.is_empty() {
        bail!("no items");
    }
    Ok(items)
}

fn person(p: &Person) -> Value {
    let mut o = Map::new();
    if p.given_name.is_empty() && p.prefix.is_empty() {
        // `{Badan Pusat Statistik}`: an institution.
        o.insert("literal".into(), Value::String(p.name.clone()));
        return Value::Object(o);
    }
    o.insert("family".into(), Value::String(p.name.clone()));
    if !p.given_name.is_empty() {
        o.insert("given".into(), Value::String(p.given_name.clone()));
    }
    if !p.prefix.is_empty() {
        o.insert(
            "non-dropping-particle".into(),
            Value::String(p.prefix.clone()),
        );
    }
    if !p.suffix.is_empty() {
        o.insert("suffix".into(), Value::String(p.suffix.clone()));
    }
    Value::Object(o)
}

/// BibLaTeX / BibTeX.
pub fn from_bib(text: &str) -> anyhow::Result<Vec<Value>> {
    let bib = biblatex::Bibliography::parse(text)
        .map_err(|e| anyhow::anyhow!("not a valid .bib file: {e}"))?;
    let mut out = Vec::new();
    for e in bib.iter() {
        let field = |k: &str| {
            e.get(k)
                .map(|c| c.format_verbatim())
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        };
        let kind = e.entry_type.to_string().to_ascii_lowercase();
        let ty = match kind.as_str() {
            "article" => "article-journal",
            "book" | "mvbook" | "booklet" => "book",
            "inbook" | "incollection" | "bookinbook" | "suppbook" => "chapter",
            "inproceedings" | "conference" => "paper-conference",
            "proceedings" | "mvproceedings" => "book",
            "collection" | "mvcollection" => "book",
            "phdthesis" | "mastersthesis" | "thesis" => "thesis",
            "techreport" | "report" => "report",
            "online" | "electronic" | "www" => "webpage",
            "manual" => "report",
            "patent" => "patent",
            "dataset" => "dataset",
            "software" => "software",
            "unpublished" => "manuscript",
            _ => "document",
        };
        let mut o = Map::new();
        o.insert("id".into(), Value::String(e.key.clone()));
        o.insert("type".into(), Value::String(ty.into()));
        for (role, key) in [
            ("author", "author"),
            ("editor", "editor"),
            ("translator", "translator"),
        ] {
            if let Ok(people) = e.get_as::<Vec<Person>>(role)
                && !people.is_empty()
            {
                o.insert(
                    key.into(),
                    Value::Array(people.iter().map(person).collect()),
                );
            }
        }
        let container = match ty {
            "article-journal" => field("journaltitle").or_else(|| field("journal")),
            "chapter" | "paper-conference" => field("booktitle"),
            _ => None,
        };
        let mut set = |k: &str, v: Option<String>| {
            if let Some(v) = v {
                o.insert(k.into(), Value::String(v));
            }
        };
        set("title", field("title"));
        set("container-title", container);
        set("volume", field("volume"));
        set("issue", field("number").or_else(|| field("issue")));
        set(
            "page",
            field("pages").map(|p| p.replace("--", "-").replace('\u{2013}', "-")),
        );
        set(
            "publisher",
            field("publisher")
                .or_else(|| field("institution"))
                .or_else(|| field("school"))
                .or_else(|| field("organization")),
        );
        set(
            "publisher-place",
            field("location").or_else(|| field("address")),
        );
        set("edition", field("edition"));
        set("DOI", field("doi"));
        set("URL", field("url"));
        set("ISBN", field("isbn"));
        set("ISSN", field("issn"));
        set("abstract", field("abstract"));
        set("language", field("langid").or_else(|| field("language")));
        set("note", field("note"));
        set("collection-title", field("series"));
        if ty == "thesis" {
            let genre = match kind.as_str() {
                "phdthesis" => Some("PhD thesis".to_owned()),
                "mastersthesis" => Some("Master's thesis".to_owned()),
                _ => field("type"),
            };
            set("genre", genre);
        }
        if let Some(date) = bib_date(&field("date"), &field("year"), &field("month")) {
            o.insert("issued".into(), date);
        }
        if let Some(date) = bib_date(&field("urldate"), &None, &None) {
            o.insert("accessed".into(), date);
        }
        out.push(Value::Object(o));
    }
    if out.is_empty() {
        bail!("no entries");
    }
    Ok(out)
}

fn month_of(m: &str) -> Option<i64> {
    let m = m.trim().to_ascii_lowercase();
    if let Ok(n) = m.parse::<i64>() {
        return (1..=12).contains(&n).then_some(n);
    }
    const NAMES: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    // Indonesian months that differ: Mei, Agu(stus), Okt(ober), Des(ember).
    const ID: [(&str, i64); 4] = [("mei", 5), ("agu", 8), ("okt", 10), ("des", 12)];
    NAMES
        .iter()
        .position(|n| m.starts_with(n))
        .map(|i| i as i64 + 1)
        .or_else(|| ID.iter().find(|(p, _)| m.starts_with(p)).map(|(_, n)| *n))
}

/// A CSL date from a BibLaTeX `date` (`2021-06-15`, `2019/2020`) or BibTeX
/// `year` and `month`.
fn bib_date(date: &Option<String>, year: &Option<String>, month: &Option<String>) -> Option<Value> {
    if let Some(d) = date {
        let first = d.split('/').next().unwrap_or(d);
        let parts: Vec<i64> = first
            .split('-')
            .filter_map(|p| p.trim().parse().ok())
            .collect();
        if !parts.is_empty() {
            return Some(json!({ "date-parts": [parts] }));
        }
        return Some(json!({ "literal": d }));
    }
    let y: i64 = year
        .as_ref()?
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .get(..4)?
        .parse()
        .ok()?;
    let mut parts = vec![y];
    if let Some(m) = month.as_deref().and_then(month_of) {
        parts.push(m);
    }
    Some(json!({ "date-parts": [parts] }))
}

/// RIS: `TAG  - value` lines, records from `TY` to `ER`.
pub fn from_ris(text: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut rec: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let tagged = line.len() >= 6
            && line.as_bytes()[2] == b' '
            && line.get(3..5) == Some(" -")
            && line[..2].chars().all(|c| c.is_ascii_alphanumeric());
        if !tagged {
            // A value continued on the next line.
            if let Some(last) = rec.last_mut()
                && !line.trim().is_empty()
            {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
            continue;
        }
        let tag = line[..2].to_owned();
        let value = line[5..].trim().to_owned();
        if tag == "ER" {
            if !rec.is_empty() {
                out.push(ris_record(&rec, out.len() + 1));
            }
            rec.clear();
            continue;
        }
        if tag == "TY" {
            rec.clear();
        }
        rec.push((tag, value));
    }
    if rec.iter().any(|(t, _)| t == "TY") {
        out.push(ris_record(&rec, out.len() + 1));
    }
    out
}

fn ris_record(rec: &[(String, String)], n: usize) -> Value {
    let get = |tags: &[&str]| {
        tags.iter()
            .find_map(|t| rec.iter().find(|(k, v)| k == t && !v.is_empty()))
            .map(|(_, v)| v.clone())
    };
    let all = |tags: &[&str]| -> Vec<String> {
        rec.iter()
            .filter(|(k, v)| tags.contains(&k.as_str()) && !v.is_empty())
            .map(|(_, v)| v.clone())
            .collect()
    };
    let ty = match get(&["TY"]).unwrap_or_default().as_str() {
        "JOUR" | "JFULL" | "EJOUR" => "article-journal",
        "MGZN" => "article-magazine",
        "NEWS" => "article-newspaper",
        "BOOK" | "EBOOK" | "EDBOOK" => "book",
        "CHAP" | "ECHAP" => "chapter",
        "CONF" | "CPAPER" => "paper-conference",
        "THES" => "thesis",
        "RPRT" => "report",
        "ELEC" | "WEB" | "BLOG" => "webpage",
        "PAT" => "patent",
        "DATA" => "dataset",
        "COMP" => "software",
        "UNPB" | "MANSCPT" => "manuscript",
        _ => "document",
    };
    let names = |tags: &[&str]| -> Vec<Value> {
        all(tags)
            .into_iter()
            .map(|a| match a.split_once(',') {
                Some((f, g)) => {
                    json!({ "family": f.trim(), "given": g.trim().trim_end_matches(',') })
                }
                None => json!({ "literal": a.trim() }),
            })
            .collect()
    };
    let mut o = Map::new();
    o.insert(
        "id".into(),
        Value::String(get(&["ID"]).unwrap_or_else(|| format!("ris-{n}"))),
    );
    o.insert("type".into(), Value::String(ty.into()));
    let authors = names(&["AU", "A1"]);
    if !authors.is_empty() {
        o.insert("author".into(), Value::Array(authors));
    }
    let editors = names(&["ED", "A2"]);
    if !editors.is_empty() && ty != "article-journal" {
        o.insert("editor".into(), Value::Array(editors));
    }
    let mut set = |k: &str, v: Option<String>| {
        if let Some(v) = v {
            o.insert(k.into(), Value::String(v));
        }
    };
    set("title", get(&["TI", "T1"]));
    set("container-title", get(&["T2", "JO", "JF", "JA", "BT"]));
    set("volume", get(&["VL"]));
    set("issue", get(&["IS"]));
    let page = match (get(&["SP"]), get(&["EP"])) {
        (Some(a), Some(b)) if !a.contains('-') => Some(format!("{a}-{b}")),
        (a, _) => a,
    };
    set("page", page);
    set("publisher", get(&["PB"]));
    set("publisher-place", get(&["CY", "PP"]));
    set("DOI", get(&["DO"]));
    set("URL", get(&["UR", "L2"]));
    set(
        "ISBN",
        get(&["SN"]).filter(|_| matches!(ty, "book" | "chapter")),
    );
    set("ISSN", get(&["SN"]).filter(|_| ty == "article-journal"));
    set("abstract", get(&["AB", "N2"]));
    set("edition", get(&["ET"]));
    set("language", get(&["LA"]));
    if let Some(d) = get(&["PY", "Y1", "DA"]) {
        // `2021`, `2021/06/15/`, `2021///`.
        let parts: Vec<i64> = d
            .split('/')
            .take(3)
            .filter_map(|p| p.trim().parse().ok())
            .collect();
        if !parts.is_empty() {
            o.insert("issued".into(), json!({ "date-parts": [parts] }));
        }
    }
    Value::Object(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIB: &str = r#"
@article{nugroho2021,
  author = {Nugroho, Adi},
  title = {Pengaruh media sosial terhadap prestasi belajar mahasiswa},
  journal = {Jurnal Pendidikan Indonesia},
  year = {2021},
  month = jun,
  volume = {10},
  number = {2},
  pages = {115--130},
  doi = {10.1234/jpi.v10i2.5678}
}
@book{santoso2019,
  author = {Santoso, Budi and Wijaya, Citra},
  title = {Metodologi Penelitian Kuantitatif},
  publisher = {Penerbit Andi},
  address = {Yogyakarta},
  year = 2019
}
@misc{bps2023,
  author = {{Badan Pusat Statistik}},
  title = {Statistik Indonesia 2023},
  date = {2023-02-28},
  url = {https://www.bps.go.id}
}
"#;

    #[test]
    fn bib_entries() {
        let items = from_bib(BIB).unwrap();
        assert_eq!(items.len(), 3);
        let a = &items[0];
        assert_eq!(a["id"], "nugroho2021");
        assert_eq!(a["type"], "article-journal");
        assert_eq!(a["container-title"], "Jurnal Pendidikan Indonesia");
        assert_eq!(a["page"], "115-130");
        assert_eq!(a["author"][0]["family"], "Nugroho");
        assert_eq!(a["author"][0]["given"], "Adi");
        assert_eq!(a["issued"]["date-parts"], json!([[2021, 6]]));
        assert_eq!(a["DOI"], "10.1234/jpi.v10i2.5678");
        let b = &items[1];
        assert_eq!(b["type"], "book");
        assert_eq!(b["author"][1]["family"], "Wijaya");
        assert_eq!(b["publisher-place"], "Yogyakarta");
        assert_eq!(b["issued"]["date-parts"], json!([[2019]]));
        let c = &items[2];
        assert_eq!(c["author"][0]["literal"], "Badan Pusat Statistik");
        assert_eq!(c["issued"]["date-parts"], json!([[2023, 2, 28]]));
    }

    #[test]
    fn ris_records() {
        let ris = "TY  - JOUR\r\nAU  - Rahmawati, Dewi\r\nAU  - Pratama, Eko\r\nTI  - Sistem informasi akademik\r\n  berbasis web\r\nT2  - Jurnal Teknologi Informasi\r\nPY  - 2022\r\nVL  - 6\r\nIS  - 1\r\nSP  - 45\r\nEP  - 58\r\nER  - \r\nTY  - BOOK\r\nAU  - Santoso, Budi\r\nTI  - Metodologi\r\nPY  - 2019///\r\nPB  - Penerbit Andi\r\nER  - \r\n";
        let items = from_ris(ris);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["type"], "article-journal");
        assert_eq!(items[0]["title"], "Sistem informasi akademik berbasis web");
        assert_eq!(items[0]["page"], "45-58");
        assert_eq!(items[0]["author"][1]["given"], "Eko");
        assert_eq!(items[1]["issued"]["date-parts"], json!([[2019]]));
        assert_eq!(items[1]["id"], "ris-2");
    }

    #[test]
    fn csl_json_shapes() {
        let a = from_csl_json(r#"[{"id":"x","type":"book","title":"A"},{"type":"book"}]"#).unwrap();
        assert_eq!(a[1]["id"], "item-2");
        let b = from_csl_json(r#"{"items":[{"id":"y","title":"B"}]}"#).unwrap();
        assert_eq!(b[0]["id"], "y");
        assert!(from_csl_json("[]").is_err());
    }

    #[test]
    fn formats_detected() {
        assert_eq!(Format::detect(Path::new("a.bib"), ""), Some(Format::Bib));
        assert_eq!(
            Format::detect(Path::new("export"), "TY  - JOUR\n"),
            Some(Format::Ris)
        );
        assert_eq!(
            Format::detect(Path::new("x"), " [ {} ]"),
            Some(Format::CslJson)
        );
    }
}
