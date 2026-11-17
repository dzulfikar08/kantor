//! What a chart part links to (its `c:externalData` workbook, linked or
//! embedded, and its `c:userShapes` drawing with that drawing's pictures),
//! carried with the chart when Move Chart puts it somewhere else.
//!
//! [`ChartLinks`] holds the chart's relationships and copies of the parts
//! they reach, renamed so they can't meet a part of the package (names
//! with a session tag). A save writes the parts and the chart's
//! relationships part ([`ChartLinks::rels_xml`]) next to the chart part,
//! wherever that is.

use super::*;

/// The chart part name [`ChartLinks::as_bundle`] uses.
pub(crate) const LINKED_CHART: &str = "xl/charts/kantor-linked.xml";

/// One relationship of a chart part.
#[derive(Debug, Clone)]
struct LinkRel {
    id: String,
    kind: String,
    /// The (renamed) part it reaches, or an external target.
    target: String,
    external: bool,
}

/// A chart part's relationships and the parts they reach (renamed copies,
/// their own relationships pointing at each other).
#[derive(Debug, Default)]
pub(crate) struct ChartLinks {
    rels: Vec<LinkRel>,
    pub parts: Vec<BundlePart>,
}

impl ChartLinks {
    pub(crate) fn is_empty(&self) -> bool {
        self.rels.is_empty()
    }

    /// The relationships part of a chart saved as `chart_part`.
    pub(crate) fn rels_xml(&self, chart_part: &str) -> String {
        let rels: Vec<Rel> = self
            .rels
            .iter()
            .map(|r| Rel {
                id: r.id.clone(),
                kind: r.kind.clone(),
                target: if r.external {
                    r.target.clone()
                } else {
                    relative(chart_part, &r.target)
                },
                external: r.external,
            })
            .collect();
        write_rels(&rels)
    }

    /// As parts of a bundle whose chart is [`LINKED_CHART`] (to copy them
    /// again with [`chart_links`]).
    pub(crate) fn as_bundle(&self) -> Vec<BundlePart> {
        let mut out = vec![BundlePart {
            name: rels_part(LINKED_CHART),
            data: self.rels_xml(LINKED_CHART).into_bytes(),
            content_type: RELS_CT.to_owned(),
        }];
        out.extend(self.parts.iter().map(|p| BundlePart {
            name: p.name.clone(),
            data: p.data.clone(),
            content_type: p.content_type.clone(),
        }));
        out
    }

    /// The `r:id`s it has (a chart part's links to keep).
    pub(crate) fn ids(&self) -> Vec<String> {
        self.rels.iter().map(|r| r.id.clone()).collect()
    }
}

/// The new name of linked part `part` (`xl/drawings/drawing3.xml` →
/// `xl/drawings/kantor<tag>drawing3.xml`).
fn renamed(part: &str, tag: &str) -> String {
    let dir = dir_of(part);
    format!("{dir}kantor{tag}{}", &part[dir.len()..])
}

/// The links of chart `chart_part`, found in `bundle` (the chart part, its
/// relationships and the parts they reach, as [`part_bundle`] gathers
/// them). `None` when it has none or one can't be followed.
pub(crate) fn chart_links(
    bundle: &[BundlePart],
    chart_part: &str,
    tag: &str,
) -> Option<ChartLinks> {
    let get = |name: &str| bundle.iter().find(|p| p.name == name);
    let rels_xml = String::from_utf8(get(&rels_part(chart_part))?.data.clone()).ok()?;
    let rels = parse_rels(&rels_xml).ok()?;
    if rels.is_empty() {
        return None;
    }
    let mut out = ChartLinks::default();
    // Every part reached, breadth first (each copied once).
    let mut todo: Vec<String> = Vec::new();
    for r in &rels {
        if r.external {
            out.rels.push(LinkRel {
                id: r.id.clone(),
                kind: r.kind.clone(),
                target: r.target.clone(),
                external: true,
            });
            continue;
        }
        let t = resolve(chart_part, &r.target)?;
        out.rels.push(LinkRel {
            id: r.id.clone(),
            kind: r.kind.clone(),
            target: renamed(&t, tag),
            external: false,
        });
        todo.push(t);
    }
    let mut done: Vec<String> = Vec::new();
    while let Some(part) = todo.pop() {
        if done.contains(&part) || part == chart_part {
            continue;
        }
        done.push(part.clone());
        let p = get(&part)?;
        let new = renamed(&part, tag);
        out.parts.push(BundlePart {
            name: new.clone(),
            data: p.data.clone(),
            content_type: p.content_type.clone(),
        });
        let Some(rp) = get(&rels_part(&part)) else {
            continue;
        };
        let sub = parse_rels(std::str::from_utf8(&rp.data).ok()?).ok()?;
        let mut moved = Vec::new();
        for r in sub {
            if r.external {
                moved.push(r);
                continue;
            }
            let t = resolve(&part, &r.target)?;
            moved.push(Rel {
                target: relative(&new, &renamed(&t, tag)),
                ..r
            });
            todo.push(t);
        }
        out.parts.push(BundlePart {
            name: rels_part(&new),
            data: write_rels(&moved).into_bytes(),
            content_type: RELS_CT.to_owned(),
        });
    }
    (!out.is_empty()).then_some(out)
}

/// The links of chart `chart_part` of package `bytes` (see
/// [`chart_links`]).
pub(crate) fn package_chart_links(bytes: &[u8], chart_part: &str, tag: &str) -> Option<ChartLinks> {
    let orig = Original::new(bytes).ok()?;
    let ct = ContentTypes::parse(&orig.text(CONTENT_TYPES).ok()??).ok()?;
    let bundle = part_bundle(&orig, &ct, chart_part);
    chart_links(&bundle, chart_part, tag)
}

/// Write `links` for a chart saved as `chart_part`: its relationships and
/// the linked parts (content types into `ct`).
pub(super) fn write_links(
    pkg: &mut Package,
    ct: &mut String,
    chart_part: &str,
    links: &ChartLinks,
) {
    pkg.set(
        &rels_part(chart_part),
        links.rels_xml(chart_part).into_bytes(),
    );
    for p in &links.parts {
        if p.name.ends_with(".rels") {
            pkg.set(&p.name, p.data.clone());
            continue;
        }
        if !pkg.has(&p.name) {
            ct.push_str(&format!(
                "<Override PartName=\"/{}\" ContentType=\"{}\"/>",
                esc_attr(&p.name),
                esc_attr(&p.content_type)
            ));
        }
        pkg.set(&p.name, p.data.clone());
    }
}
