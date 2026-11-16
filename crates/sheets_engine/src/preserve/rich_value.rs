//! Pictures Excel saved for IMAGE formulas (and pictures placed in cells):
//! rich values.
//!
//! Excel keeps an IMAGE cell's last download in the file, so the workbook
//! shows it offline. The cell is written as `<c t="e" vm="N"><f>…</f>
//! <v>#VALUE!</v></c>` (older readers see `#VALUE!`); `vm` is a 1-based
//! index into `xl/metadata.xml`'s `<valueMetadata>`, whose record points (by
//! its `<rc t v>`) into the `XLRICHVALUE` `<futureMetadata>`, whose
//! `<xlrd:rvb i>` is the index of a rich value in
//! `xl/richData/rdrichvalue.xml`. The rich value's structure
//! (`rdrichvaluestructure.xml`) says what its values are: `_webimage` (an
//! IMAGE formula: `WebImageIdentifier` indexes the `<webImageSrd>` list of
//! `rdRichValueWebImage.xml`, whose `<blip r:id>` is the picture) or
//! `_localImage` (Place in Cell: `_rvRel:LocalImageIdentifier` indexes the
//! `<rel r:id>` list of `richValueRel.xml`); `Text` is the alt text.
//!
//! [`read`] finds those pictures when a workbook opens. On save, IronCalc
//! writes its own `metadata.xml` (dynamic arrays only) and the cells' values
//! (an IMAGE cell's alt text); the rich data parts are carried like other
//! workbook parts IronCalc doesn't know. [`keep`] adds the original's rich
//! value metadata to the saved `metadata.xml` and writes the cells that
//! still show their saved picture back as Excel does, so Excel shows them
//! too.

use std::collections::HashMap;
use std::sync::Arc;

use roxmltree::{Document, Node};

use super::{
    Context, Original, Package, Rel, SheetKind, parse_rels, rels_part, resolve, workbook_sheets,
};

const NS_RICH_DATA: &str = "http://schemas.microsoft.com/office/spreadsheetml/2017/richdata";
const RVB_EXT: &str = "{3e2802c4-a4d2-4d8b-9148-e3be6c30e623}";

/// A picture saved for a cell.
#[derive(Clone, Debug)]
pub(crate) struct RichPicture {
    pub data: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub alt: Option<String>,
}

fn local<'a>(n: Node<'a, 'a>, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name
}

fn children<'a>(n: Node<'a, 'a>, name: &'a str) -> impl Iterator<Item = Node<'a, 'a>> + 'a {
    n.children().filter(move |c| local(*c, name))
}

/// The workbook's rich value metadata (`xl/metadata.xml`).
#[derive(Default, Debug)]
struct Metadata {
    /// Per `<valueMetadata>` record (vm - 1): the index into the
    /// `XLRICHVALUE` future metadata, if the record is a rich value.
    value: Vec<Option<usize>>,
    /// The `XLRICHVALUE` future metadata: rich value indices.
    rvb: Vec<usize>,
}

fn read_metadata(xml: &str) -> Option<Metadata> {
    let doc = Document::parse(xml).ok()?;
    let root = doc.root_element();
    let types: Vec<&str> = root
        .children()
        .filter(|c| local(*c, "metadataTypes"))
        .flat_map(|t| children(t, "metadataType"))
        .map(|t| t.attribute("name").unwrap_or_default())
        .collect();
    let rich_type = types.iter().position(|t| *t == "XLRICHVALUE")? + 1;
    let rvb = children(root, "futureMetadata")
        .find(|f| f.attribute("name") == Some("XLRICHVALUE"))
        .map(|f| {
            children(f, "bk")
                .map(|bk| {
                    bk.descendants()
                        .find(|d| local(*d, "rvb"))
                        .and_then(|r| r.attribute("i")?.parse().ok())
                        .unwrap_or(usize::MAX)
                })
                .collect()
        })
        .unwrap_or_default();
    let value = children(root, "valueMetadata")
        .flat_map(|v| children(v, "bk"))
        .map(|bk| {
            children(bk, "rc").find_map(|rc| {
                let t: usize = rc.attribute("t")?.parse().ok()?;
                (t == rich_type)
                    .then(|| rc.attribute("v")?.parse().ok())
                    .flatten()
            })
        })
        .collect();
    Some(Metadata { value, rvb })
}

/// The workbook's rich data parts, by the last segment of their
/// relationship type.
fn rich_parts(
    orig: &Original,
) -> Option<(HashMap<String, String>, Vec<super::SheetEntry>, String)> {
    let root = parse_rels(&orig.text("_rels/.rels").ok()??).ok()?;
    let (wb, wb_rels, _, sheets) = workbook_sheets(&root, &|n| orig.text(n)).ok()?;
    let parts = wb_rels
        .iter()
        .filter(|r| !r.external)
        .filter_map(|r| Some((r.short_kind().to_owned(), resolve(&wb, &r.target)?)))
        .collect();
    Some((parts, sheets, wb))
}

/// The picture parts a list part (`richValueRel.xml` or
/// `rdRichValueWebImage.xml`) points to, in list order.
fn picture_list(
    orig: &Original,
    part: Option<&String>,
    entry: &str,
    id_of: &str,
) -> Vec<Option<String>> {
    let Some(part) = part else {
        return Vec::new();
    };
    let Some(xml) = orig.text(part).ok().flatten() else {
        return Vec::new();
    };
    let rels: Vec<Rel> = orig
        .text(&rels_part(part))
        .ok()
        .flatten()
        .and_then(|x| parse_rels(&x).ok())
        .unwrap_or_default();
    let Ok(doc) = Document::parse(&xml) else {
        return Vec::new();
    };
    children(doc.root_element(), entry)
        .map(|e| {
            let node = if id_of.is_empty() {
                e
            } else {
                children(e, id_of).next()?
            };
            let id = node.attributes().find(|a| a.name() == "id")?.value();
            let rel = rels.iter().find(|r| r.id == id && !r.external)?;
            resolve(part, &rel.target)
        })
        .collect()
}

/// A rich value that is a picture: its part and alt text.
fn rich_pictures(
    orig: &Original,
    parts: &HashMap<String, String>,
) -> HashMap<usize, (String, Option<String>)> {
    let mut out = HashMap::new();
    let (Some(values), Some(structures)) = (
        parts
            .get("rdRichValue")
            .and_then(|p| orig.text(p).ok().flatten()),
        parts
            .get("rdRichValueStructure")
            .and_then(|p| orig.text(p).ok().flatten()),
    ) else {
        return out;
    };
    let (Ok(values), Ok(structures)) = (Document::parse(&values), Document::parse(&structures))
    else {
        return out;
    };
    let structures: Vec<(&str, Vec<&str>)> = children(structures.root_element(), "s")
        .map(|s| {
            (
                s.attribute("t").unwrap_or_default(),
                children(s, "k")
                    .map(|k| k.attribute("n").unwrap_or_default())
                    .collect(),
            )
        })
        .collect();
    let web = picture_list(
        orig,
        parts.get("rdRichValueWebImage"),
        "webImageSrd",
        "blip",
    );
    let local_list = picture_list(orig, parts.get("richValueRel"), "rel", "");
    for (i, rv) in children(values.root_element(), "rv").enumerate() {
        let Some((kind, keys)) = rv
            .attribute("s")
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(|s| structures.get(s))
        else {
            continue;
        };
        let fields: HashMap<&str, String> = keys
            .iter()
            .copied()
            .zip(children(rv, "v").map(|v| v.text().unwrap_or_default().to_owned()))
            .collect();
        let (list, key) = match *kind {
            "_webimage" => (&web, "WebImageIdentifier"),
            "_localImage" => (&local_list, "_rvRel:LocalImageIdentifier"),
            _ => continue,
        };
        let Some(part) = fields
            .get(key)
            .and_then(|v| v.parse::<usize>().ok())
            .and_then(|i| list.get(i).cloned().flatten())
        else {
            continue;
        };
        let alt = fields.get("Text").filter(|t| !t.is_empty()).cloned();
        out.insert(i, (part, alt));
    }
    out
}

/// `(row, col)` 0-based of an A1 reference.
fn cell_ref(r: &str) -> Option<(u32, u32)> {
    let split = r.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = r.split_at(split);
    if letters.is_empty() {
        return None;
    }
    let mut col = 0u32;
    for c in letters.chars() {
        if !c.is_ascii_alphabetic() {
            return None;
        }
        col = col * 26 + (c.to_ascii_uppercase() as u32 - 'A' as u32 + 1);
    }
    Some((digits.parse::<u32>().ok()?.checked_sub(1)?, col - 1))
}

/// An attribute's value in a start tag's text.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let pat = format!(" {name}=\"");
    let start = tag.find(&pat)? + pat.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

/// The cells of a worksheet's XML with a `vm` attribute: `((row, col), vm)`.
fn vm_cells(xml: &str) -> Vec<((u32, u32), usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = xml[from..].find(" vm=\"") {
        let pos = from + at;
        from = pos + 5;
        let Some(start) = xml[..pos].rfind('<') else {
            continue;
        };
        let Some(end) = xml[pos..].find('>') else {
            break;
        };
        let tag = &xml[start..pos + end];
        if !tag.starts_with("<c ") && !tag.contains(":c ") {
            continue;
        }
        if let (Some(r), Some(vm)) = (
            attr(tag, "r").and_then(cell_ref),
            attr(tag, "vm").and_then(|v| v.parse().ok()),
        ) {
            out.push((r, vm));
        }
    }
    out
}

/// The pictures Excel saved for cells, by `(sheetId, row, col)` (0-based).
pub(crate) fn read(bytes: &[u8]) -> HashMap<(u32, u32, u32), RichPicture> {
    let mut out = HashMap::new();
    let Ok(orig) = Original::new(bytes) else {
        return out;
    };
    let Some((parts, sheets, _)) = rich_parts(&orig) else {
        return out;
    };
    let Some(meta) = parts
        .get("sheetMetadata")
        .and_then(|p| orig.text(p).ok().flatten())
        .and_then(|x| read_metadata(&x))
    else {
        return out;
    };
    let pictures = rich_pictures(&orig, &parts);
    if pictures.is_empty() {
        return out;
    }
    // Each picture part read once: its bytes and size, `None` if it isn't
    // a picture Kantor reads.
    type Loaded = Option<(Arc<Vec<u8>>, u32, u32)>;
    let mut loaded: HashMap<String, Loaded> = HashMap::new();
    for s in sheets.iter().filter(|s| s.kind == SheetKind::Work) {
        let Some(xml) = orig.text(&s.part).ok().flatten() else {
            continue;
        };
        for ((row, col), vm) in vm_cells(&xml) {
            let Some((part, alt)) = vm
                .checked_sub(1)
                .and_then(|i| meta.value.get(i).copied().flatten())
                .and_then(|f| meta.rvb.get(f))
                .and_then(|rv| pictures.get(rv))
            else {
                continue;
            };
            let picture = loaded
                .entry(part.clone())
                .or_insert_with(|| {
                    let data = orig.get(part).ok().flatten()?;
                    let (w, h) = crate::web::picture_size(&data)?;
                    Some((Arc::new(data), w, h))
                })
                .clone();
            if let Some((data, width, height)) = picture {
                out.insert(
                    (s.sheet_id, row, col),
                    RichPicture {
                        data,
                        width,
                        height,
                        alt: alt.clone(),
                    },
                );
            }
        }
    }
    out
}

/// After the merge: the original's rich value metadata in the saved
/// `metadata.xml`, and the cells that still show their saved picture
/// written as Excel writes them. Does nothing when the original has none;
/// an error leaves the package as it was.
pub(crate) fn keep(pkg: &mut Package, original: &[u8], ctx: &Context) -> Result<(), String> {
    let orig = Original::new(original)?;
    let Some((parts, orig_sheets, _)) = rich_parts(&orig) else {
        return Ok(());
    };
    let Some(meta) = parts
        .get("sheetMetadata")
        .and_then(|p| orig.text(p).ok().flatten())
        .and_then(|x| read_metadata(&x))
    else {
        return Ok(());
    };
    if !meta.value.iter().any(Option::is_some) || rich_pictures(&orig, &parts).is_empty() {
        return Ok(());
    }
    // Which cells keep their rich value: (sheetId, row, col) -> old vm.
    let mut kept: Vec<(u32, (u32, u32), usize)> = Vec::new();
    for s in orig_sheets.iter().filter(|s| s.kind == SheetKind::Work) {
        let Some(xml) = orig.text(&s.part).ok().flatten() else {
            continue;
        };
        for ((row, col), vm) in vm_cells(&xml) {
            let rich = vm
                .checked_sub(1)
                .and_then(|i| meta.value.get(i).copied().flatten())
                .is_some();
            if rich && (ctx.saved_picture)(s.sheet_id, row, col) {
                kept.push((s.sheet_id, (row, col), vm));
            }
        }
    }
    if kept.is_empty() {
        return Ok(());
    }
    // The new value metadata: the original's rich value records, in order.
    let mut renumber: HashMap<usize, usize> = HashMap::new();
    let mut records = Vec::new();
    for (i, f) in meta.value.iter().enumerate() {
        if let Some(f) = f {
            records.push(*f);
            renumber.insert(i + 1, records.len());
        }
    }
    let new_root = parse_rels(pkg.text("_rels/.rels")?.ok_or("no package relationships")?)?;
    let (new_wb, new_wb_rels, _, new_sheets) =
        workbook_sheets(&new_root, &|n| Ok(pkg.text(n)?.map(str::to_owned)))?;
    let meta_part = new_wb_rels
        .iter()
        .find(|r| r.short_kind() == "sheetMetadata" && !r.external)
        .and_then(|r| resolve(&new_wb, &r.target))
        .ok_or("the saved workbook has no metadata part")?;
    let saved_meta = pkg
        .text(&meta_part)?
        .ok_or("the saved metadata part is missing")?
        .to_owned();
    let new_meta = add_rich_metadata(&saved_meta, &meta.rvb, &records)?;
    let mut sheet_texts = Vec::new();
    for s in new_sheets.iter().filter(|s| s.kind == SheetKind::Work) {
        let cells: HashMap<(u32, u32), usize> = kept
            .iter()
            .filter(|k| k.0 == s.sheet_id)
            .filter_map(|k| Some((k.1, *renumber.get(&k.2)?)))
            .collect();
        if cells.is_empty() {
            continue;
        }
        let xml = pkg.text(&s.part)?.ok_or("a saved sheet is missing")?;
        sheet_texts.push((s.part.clone(), mark_cells(xml, &cells)));
    }
    pkg.set(&meta_part, new_meta.into_bytes());
    for (part, xml) in sheet_texts {
        pkg.set(&part, xml.into_bytes());
    }
    Ok(())
}

/// IronCalc's `metadata.xml` with an `XLRICHVALUE` type, its future
/// metadata (`rvb`: rich value indices) and value metadata (`records`:
/// indices into the future metadata).
fn add_rich_metadata(xml: &str, rvb: &[usize], records: &[usize]) -> Result<String, String> {
    let doc = Document::parse(xml).map_err(|e| format!("metadata: {e}"))?;
    let root = doc.root_element();
    let types = children(root, "metadataTypes")
        .next()
        .ok_or("metadata without types")?;
    let count = children(types, "metadataType").count();
    if children(types, "metadataType").any(|t| t.attribute("name") == Some("XLRICHVALUE"))
        || children(root, "valueMetadata").next().is_some()
    {
        return Err("the saved metadata already has rich values".into());
    }
    let rich_type = count + 1;
    let ty = "<metadataType name=\"XLRICHVALUE\" minSupportedVersion=\"120000\" copy=\"1\" pasteAll=\"1\" pasteValues=\"1\" merge=\"1\" splitFirst=\"1\" rowColShift=\"1\" clearFormats=\"1\" clearComments=\"1\" assign=\"1\" coerce=\"1\"/>";
    let mut future = format!(
        "<futureMetadata name=\"XLRICHVALUE\" count=\"{}\">",
        rvb.len()
    );
    for i in rvb {
        future.push_str(&format!(
            "<bk><extLst><ext uri=\"{RVB_EXT}\"><xlrd:rvb i=\"{i}\"/></ext></extLst></bk>"
        ));
    }
    future.push_str("</futureMetadata>");
    let mut value = format!("<valueMetadata count=\"{}\">", records.len());
    for v in records {
        value.push_str(&format!("<bk><rc t=\"{rich_type}\" v=\"{v}\"/></bk>"));
    }
    value.push_str("</valueMetadata>");

    let mut out = String::with_capacity(xml.len() + future.len() + value.len() + 400);
    // Types: one more, count updated.
    let types_range = types.range();
    let types_xml = &xml[types_range.clone()];
    let close = types_xml.rfind("</").ok_or("metadata types are empty")?;
    let open_end = types_xml.find('>').ok_or("bad metadata types")?;
    let new_types = format!(
        "<metadataTypes count=\"{}\">{}{ty}</metadataTypes>",
        count + 1,
        &types_xml[open_end + 1..close]
    );
    // Where the future metadata goes: before the cell metadata (or the
    // value metadata, or the end).
    let insert_at = children(root, "cellMetadata")
        .next()
        .map(|n| n.range().start);
    let cell_end = children(root, "cellMetadata").next().map(|n| n.range().end);
    let root_close = xml.rfind("</").ok_or("bad metadata")?;
    out.push_str(&xml[..types_range.start]);
    out.push_str(&new_types);
    match insert_at {
        Some(at) => {
            out.push_str(&xml[types_range.end..at]);
            out.push_str(&future);
            let end = cell_end.unwrap_or(at);
            out.push_str(&xml[at..end]);
            out.push_str(&value);
            out.push_str(&xml[end..root_close]);
        }
        None => {
            out.push_str(&xml[types_range.end..root_close]);
            out.push_str(&future);
            out.push_str(&value);
        }
    }
    out.push_str(&xml[root_close..]);
    // The rich data namespace on the root.
    if !out.contains(NS_RICH_DATA) {
        let tag = out.find("<metadata").ok_or("bad metadata")?;
        let at = tag + "<metadata".len();
        out.insert_str(at, &format!(" xmlns:xlrd=\"{NS_RICH_DATA}\""));
    }
    Document::parse(&out).map_err(|e| format!("rich value metadata: {e}"))?;
    Ok(out)
}

/// A saved sheet with `cells` (`(row, col)` -> new vm) written as Excel
/// writes a cell showing a rich value: `t="e" vm="N"`, value `#VALUE!`.
fn mark_cells(xml: &str, cells: &HashMap<(u32, u32), usize>) -> String {
    let mut out = String::with_capacity(xml.len() + cells.len() * 24);
    let mut from = 0;
    while let Some(at) = xml[from..].find("<c ") {
        let start = from + at;
        let Some(tag_len) = xml[start..].find('>') else {
            break;
        };
        let tag_end = start + tag_len;
        let tag = &xml[start..tag_end];
        let vm = attr(tag, "r")
            .and_then(cell_ref)
            .and_then(|r| cells.get(&r));
        let (Some(vm), false) = (vm, tag.ends_with('/')) else {
            out.push_str(&xml[from..=tag_end]);
            from = tag_end + 1;
            continue;
        };
        let Some(close) = xml[tag_end..].find("</c>").map(|i| tag_end + i) else {
            break;
        };
        let body = &xml[tag_end + 1..close];
        // The formula stays; the value is #VALUE!.
        let formula = body
            .find("<f")
            .and_then(|f| {
                let end = body[f..]
                    .find("</f>")
                    .map(|e| f + e + 4)
                    .or_else(|| body[f..].find("/>").map(|e| f + e + 2))?;
                Some(&body[f..end])
            })
            .unwrap_or_default();
        let mut new_tag = String::from("<c");
        for (name, value) in tag_attrs(tag) {
            if name != "t" && name != "vm" {
                new_tag.push_str(&format!(" {name}=\"{value}\""));
            }
        }
        new_tag.push_str(&format!(" t=\"e\" vm=\"{vm}\">"));
        out.push_str(&xml[from..start]);
        out.push_str(&new_tag);
        out.push_str(formula);
        out.push_str("<v>#VALUE!</v></c>");
        from = close + 4;
    }
    out.push_str(&xml[from..]);
    out
}

/// The attributes of a start tag `<c a="1" b="2"` (no `>`).
fn tag_attrs(tag: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = tag.trim_start_matches('<');
    rest = rest.split_once(char::is_whitespace).map_or("", |(_, r)| r);
    while let Some(eq) = rest.find("=\"") {
        let name = rest[..eq].trim();
        let after = &rest[eq + 2..];
        let Some(end) = after.find('"') else { break };
        out.push((name, &after[..end]));
        rest = &after[end + 1..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_references() {
        assert_eq!(cell_ref("A1"), Some((0, 0)));
        assert_eq!(cell_ref("AB12"), Some((11, 27)));
        assert_eq!(cell_ref("12"), None);
    }

    #[test]
    fn finds_cells_with_value_metadata() {
        let xml = r#"<sheetData><row r="2"><c r="B2" t="e" vm="1"><f>_xlfn.IMAGE("x")</f><v>#VALUE!</v></c><c r="C2" vm="3"/></row></sheetData>"#;
        assert_eq!(vm_cells(xml), [((1, 1), 1), ((1, 2), 3)]);
    }

    #[test]
    fn marks_kept_cells_as_excel_writes_them() {
        let xml = r#"<sheetData><row r="2"><c r="B2" s="3" t="str"><f>_xlfn.IMAGE("x","Logo")</f><v>Logo</v></c><c r="C2"><v>1</v></c></row></sheetData>"#;
        let cells = HashMap::from([((1, 1), 2)]);
        assert_eq!(
            mark_cells(xml, &cells),
            r#"<sheetData><row r="2"><c r="B2" s="3" t="e" vm="2"><f>_xlfn.IMAGE("x","Logo")</f><v>#VALUE!</v></c><c r="C2"><v>1</v></c></row></sheetData>"#
        );
    }

    #[test]
    fn rich_metadata_goes_after_ironcalcs() {
        let ironcalc = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<metadata xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:xda="http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray"><metadataTypes count="1"><metadataType name="XLDAPR" minSupportedVersion="120000"/></metadataTypes><futureMetadata name="XLDAPR" count="1"><bk/></futureMetadata><cellMetadata count="1"><bk><rc t="1" v="0"/></bk></cellMetadata></metadata>"#;
        let out = add_rich_metadata(ironcalc, &[0, 1], &[0, 1]).unwrap();
        let meta = read_metadata(&out).unwrap();
        assert_eq!(meta.rvb, [0, 1]);
        assert_eq!(meta.value, [Some(0), Some(1)]);
        assert!(out.contains("<metadataTypes count=\"2\">"), "{out}");
        let cell = out.find("<cellMetadata").unwrap();
        assert!(out.find("XLRICHVALUE\" count").unwrap() < cell);
        assert!(out.find("<valueMetadata").unwrap() > cell);
    }
}
