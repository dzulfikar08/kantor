//! Region maps (chartex `layoutId="regionMap"`): the region outlines Office
//! caches in the part, how data points find their region, map projections
//! and the value → colour scale.
//!
//! Office looks the categories up with Bing and keeps the answers in the
//! series' `cx:layoutPr/cx:geography/cx:geoCache` ([MS-ODRAWXML] 2.24.3.35),
//! either as XML (`cx:clear`, documented) or as an opaque `cx:binary`
//! blob. The clear form holds, per region, polygons whose points are
//! compressed with Bing's Point Compression Algorithm (`pcaRings`). The
//! binary form isn't documented: Kantor tries it as compressed (zlib,
//! deflate, gzip) or plain XML of the clear form, and when that fails the
//! chart is drawn as a list of its values instead of a map.

use std::io::Read as _;

use roxmltree::{Document, Node};
use slides_engine::Rgb;
use slides_engine::theme::ColorCtx;

/// A region and its outline: rings of `[longitude, latitude]` (degrees).
/// The first ring of each polygon is its outside; rings after it in the
/// same `pcaRings` are holes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeoRegion {
    pub name: String,
    pub id: String,
    pub rings: Vec<Vec<[f64; 2]>>,
}

/// The map projections Office offers (`projectionType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Projection {
    #[default]
    Mercator,
    Miller,
    Robinson,
    Albers,
}

/// Whether region names are drawn on the map (`regionLabelLayout`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RegionLabels {
    None,
    /// Only where the label fits inside the region (default).
    #[default]
    BestFit,
    All,
}

/// Where a gradient stop sits (`cx:valueColorPositions`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StopAt {
    /// The lowest / highest value (or the midpoint for the middle stop).
    Auto,
    Number(f64),
    /// Percent of the data's range.
    Percent(f64),
}

/// The colour scale of a value map.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueColors {
    /// 2 or 3 stops.
    pub count: u8,
    pub min: Option<Rgb>,
    pub mid: Option<Rgb>,
    pub max: Option<Rgb>,
    pub at: [StopAt; 3],
}

impl Default for ValueColors {
    fn default() -> Self {
        Self {
            count: 2,
            min: None,
            mid: None,
            max: None,
            at: [StopAt::Auto; 3],
        }
    }
}

/// What became of the part's geography cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CacheState {
    /// Regions read.
    #[default]
    Read,
    /// The part has no cache (Office hadn't looked the data up).
    Missing,
    /// A `cx:binary` cache Kantor can't decode.
    Unreadable,
}

/// One answer of the cache's location lookups: the place names asked for
/// (country, state, county, postcode) and the region found.
#[derive(Clone, Debug, PartialEq)]
pub struct Lookup {
    /// Lower-case names: the most specific first.
    pub names: Vec<String>,
    pub region: usize,
}

/// A region map series' geography.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeoMap {
    pub regions: Vec<GeoRegion>,
    pub lookups: Vec<Lookup>,
    pub projection: Projection,
    /// `viewedRegionType`: `world`, `countryRegion`, `state`...
    pub viewed: String,
    pub labels: RegionLabels,
    pub colors: ValueColors,
    /// Category values (`cx:strDim type="colorStr"`): a categorical map.
    pub color_strs: Vec<String>,
    pub cache: CacheState,
    pub attribution: Option<String>,
}

fn local<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn locals<'a, 'i>(n: Node<'a, 'i>, name: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

fn num(n: Node, k: &str) -> Option<f64> {
    n.attribute(k)?
        .trim()
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite())
}

const SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-";

/// Points of a Point Compression Algorithm string as `[longitude,
/// latitude]`. `None` when it isn't one.
pub fn decode_pca(s: &str) -> Option<Vec<[f64; 2]>> {
    let mut out = Vec::new();
    let (mut x, mut y) = (0i64, 0i64);
    let mut chars = s.bytes().peekable();
    while chars.peek().is_some() {
        let mut n: u64 = 0;
        let mut k = 0u32;
        loop {
            let c = chars.next()?;
            let b = SAFE.iter().position(|s| *s == c)? as u64;
            if k > 60 {
                return None;
            }
            n += (b & 31) << k;
            k += 5;
            if b < 32 {
                break;
            }
        }
        // n = (dx + dy)(dx + dy + 1) / 2 + dy (Cantor pairing).
        let mut d = (((8.0 * n as f64 + 5.0).sqrt() - 1.0) / 2.0).floor() as u64;
        while d > 0 && d * (d + 1) / 2 > n {
            d -= 1;
        }
        while (d + 1) * (d + 2) / 2 <= n {
            d += 1;
        }
        let ny = n - d * (d + 1) / 2;
        let nx = d.checked_sub(ny)?;
        let unzig = |v: u64| ((v >> 1) as i64) ^ -((v & 1) as i64);
        x += unzig(nx);
        y += unzig(ny);
        out.push([x as f64 * 1e-5, y as f64 * 1e-5]);
    }
    Some(out)
}

/// `[longitude, latitude]` points as a Point Compression Algorithm string
/// (tests and fixtures).
pub fn encode_pca(pts: &[[f64; 2]]) -> String {
    let mut out = String::new();
    let (mut px, mut py) = (0i64, 0i64);
    for p in pts {
        let (x, y) = ((p[0] * 1e5).round() as i64, (p[1] * 1e5).round() as i64);
        let zig = |v: i64| ((v << 1) ^ (v >> 63)) as u64;
        let (dx, dy) = (zig(x - px), zig(y - py));
        (px, py) = (x, y);
        let mut i = (dy + dx) * (dy + dx + 1) / 2 + dy;
        loop {
            let mut r = i & 31;
            i >>= 5;
            if i > 0 {
                r += 32;
            }
            out.push(SAFE[r as usize] as char);
            if i == 0 {
                break;
            }
        }
    }
    out
}

/// The clear XML of a `cx:binary` cache, if it is one Kantor can read:
/// compressed with zlib, deflate or gzip, or not at all.
fn binary_xml(b64: &str) -> Option<String> {
    use base64::Engine as _;
    let clean: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(clean.as_bytes())
        .ok()?;
    let looks_xml = |s: &str| s.contains("geoData") || s.contains("geoLocation");
    let text = |raw: Vec<u8>| -> Option<String> {
        if let Ok(s) = String::from_utf8(raw.clone()) {
            return Some(s.trim_start_matches('\u{feff}').to_owned());
        }
        // UTF-16 LE with or without a BOM.
        if raw.len().is_multiple_of(2) {
            let units: Vec<u16> = raw
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            return String::from_utf16(&units)
                .ok()
                .map(|s| s.trim_start_matches('\u{feff}').to_owned());
        }
        None
    };
    let limit = 64 << 20;
    let inflate = |r: &mut dyn std::io::Read| {
        let mut out = Vec::new();
        r.take(limit).read_to_end(&mut out).ok().map(|_| out)
    };
    let tries: [Option<Vec<u8>>; 4] = [
        Some(bytes.clone()),
        inflate(&mut flate2::read::ZlibDecoder::new(&bytes[..])),
        inflate(&mut flate2::read::DeflateDecoder::new(&bytes[..])),
        inflate(&mut flate2::read::GzDecoder::new(&bytes[..])),
    ];
    tries
        .into_iter()
        .flatten()
        .filter_map(text)
        .find(|s| looks_xml(s))
}

/// A region's index in `regions`, adding it when new.
fn region_of(regions: &mut Vec<GeoRegion>, geo: Node) -> Option<usize> {
    let id = geo.attribute("entityId")?.to_owned();
    if let Some(i) = regions.iter().position(|r| r.id == id) {
        return Some(i);
    }
    let mut rings = Vec::new();
    for p in local(geo, "geoPolygons")
        .into_iter()
        .flat_map(|g| locals(g, "geoPolygon"))
    {
        let Some(s) = p.attribute("pcaRings") else {
            continue;
        };
        // Several rings are separated by commas or spaces (neither is a
        // PCA character).
        for part in s.split([',', ' ', ';']).filter(|r| !r.is_empty()) {
            if let Some(ring) = decode_pca(part).filter(|r| r.len() >= 3) {
                rings.push(ring);
            }
        }
    }
    regions.push(GeoRegion {
        name: geo.attribute("entityName").unwrap_or_default().to_owned(),
        id,
        rings,
    });
    Some(regions.len() - 1)
}

/// Regions and lookups of a clear cache (any element holding the
/// `CT_Clear` children: `cx:clear` or the root of a decoded binary).
fn read_clear(clear: Node, map: &mut GeoMap) {
    let mut by_id: Vec<(String, usize)> = Vec::new();
    for res in clear
        .descendants()
        .filter(|d| d.is_element() && d.tag_name().name() == "geoDataEntityQueryResult")
    {
        let Some(geo) = local(res, "geoData") else {
            continue;
        };
        if let Some(i) = region_of(&mut map.regions, geo) {
            if let Some(q) = local(res, "geoDataEntityQuery").and_then(|q| q.attribute("entityId"))
            {
                by_id.push((q.to_owned(), i));
            }
            by_id.push((map.regions[i].id.clone(), i));
        }
    }
    let find_id = |id: &str| by_id.iter().find(|(k, _)| k == id).map(|(_, i)| *i);
    // Points (latitude, longitude) → entity ids.
    let mut point_ids: Vec<([f64; 2], String)> = Vec::new();
    for res in clear
        .descendants()
        .filter(|d| d.is_element() && d.tag_name().name() == "geoDataPointToEntityQueryResult")
    {
        let (Some(q), Some(e)) = (
            local(res, "geoDataPointQuery"),
            local(res, "geoDataPointToEntityQuery"),
        ) else {
            continue;
        };
        if let (Some(lat), Some(lon), Some(id)) = (
            num(q, "latitude"),
            num(q, "longitude"),
            e.attribute("entityId"),
        ) {
            point_ids.push(([lat, lon], id.to_owned()));
        }
    }
    let lower = |s: &str| s.trim().to_lowercase();
    for res in clear
        .descendants()
        .filter(|d| d.is_element() && d.tag_name().name() == "geoLocationQueryResult")
    {
        let q = local(res, "geoLocationQuery");
        let loc = local(res, "geoLocations").and_then(|l| local(l, "geoLocation"));
        let mut names: Vec<String> = q
            .map(|q| {
                [
                    "postalCode",
                    "adminDistrict2",
                    "adminDistrict1",
                    "countryRegion",
                ]
                .iter()
                .filter_map(|k| q.attribute(*k))
                .map(lower)
                .filter(|s| !s.is_empty())
                .collect()
            })
            .unwrap_or_default();
        let Some(loc) = loc else { continue };
        let entity = loc.attribute("entityName").map(lower);
        let region = match (num(loc, "latitude"), num(loc, "longitude")) {
            (Some(lat), Some(lon)) => point_ids
                .iter()
                .find(|(p, _)| (p[0] - lat).abs() < 1e-6 && (p[1] - lon).abs() < 1e-6)
                .and_then(|(_, id)| find_id(id)),
            _ => None,
        }
        .or_else(|| {
            let e = entity.as_deref()?;
            map.regions.iter().position(|r| lower(&r.name) == e)
        });
        let Some(region) = region else { continue };
        if let Some(e) = entity
            && !names.contains(&e)
        {
            names.insert(0, e);
        }
        map.lookups.push(Lookup { names, region });
    }
}

/// Reads a series' `cx:layoutPr` and value colours into a map.
pub fn read(series: Node, cx: &ColorCtx) -> GeoMap {
    let mut map = GeoMap::default();
    let lp = local(series, "layoutPr");
    if let Some(l) = lp.and_then(|l| local(l, "regionLabelLayout")) {
        map.labels = match l.attribute("val") {
            Some("none") => RegionLabels::None,
            Some("showAll") => RegionLabels::All,
            _ => RegionLabels::BestFit,
        };
    }
    let geo = lp.and_then(|l| local(l, "geography"));
    map.viewed = geo
        .and_then(|g| g.attribute("viewedRegionType"))
        .unwrap_or("")
        .to_owned();
    map.projection = match geo.and_then(|g| g.attribute("projectionType")) {
        Some("mercator") => Projection::Mercator,
        Some("miller") => Projection::Miller,
        Some("robinson") => Projection::Robinson,
        Some("albers") => Projection::Albers,
        // Automatic: Office shows the world with Miller.
        _ if map.viewed == "world" => Projection::Miller,
        _ => Projection::Mercator,
    };
    map.attribution = geo
        .and_then(|g| g.attribute("attribution"))
        .filter(|a| !a.is_empty())
        .map(str::to_owned);
    map.cache = CacheState::Missing;
    if let Some(cache) = geo.and_then(|g| local(g, "geoCache")) {
        if let Some(clear) = local(cache, "clear") {
            read_clear(clear, &mut map);
            map.cache = CacheState::Read;
        }
        if map.regions.is_empty()
            && let Some(bin) = local(cache, "binary")
        {
            map.cache = CacheState::Unreadable;
            let text = bin.text().unwrap_or_default();
            if let Some(xml) = binary_xml(text)
                && let Ok(doc) = Document::parse(&xml)
            {
                read_clear(doc.root_element(), &mut map);
                if !map.regions.is_empty() {
                    map.cache = CacheState::Read;
                }
            }
        }
    }
    if map.regions.is_empty() && map.cache == CacheState::Read {
        map.cache = CacheState::Missing;
    }
    if let Some(vc) = local(series, "valueColors") {
        let c = |k: &str| local(vc, k).and_then(|n| cx.first_color(n));
        map.colors.min = c("minColor");
        map.colors.mid = c("midColor");
        map.colors.max = c("maxColor");
    }
    if let Some(vp) = local(series, "valueColorPositions") {
        map.colors.count = if vp.attribute("count") == Some("3") {
            3
        } else {
            2
        };
        let at = |k: &str| {
            let Some(n) = local(vp, k) else {
                return StopAt::Auto;
            };
            if let Some(v) = local(n, "number").and_then(|v| text_num(v)) {
                StopAt::Number(v)
            } else if let Some(v) = local(n, "percent").and_then(|v| text_num(v)) {
                StopAt::Percent(v)
            } else {
                StopAt::Auto
            }
        };
        map.colors.at = [at("min"), at("mid"), at("max")];
    }
    map
}

/// A number as element text or `val` attribute.
fn text_num(n: Node) -> Option<f64> {
    n.text()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .or_else(|| n.attribute("val"))?
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite())
}

impl GeoMap {
    /// The region data point `i` is shown in: its category (with the outer
    /// levels to choose between places of the same name) looked up in the
    /// cache, else a region of that name.
    pub fn region_for(&self, leaf: &str, outer: &[&str]) -> Option<usize> {
        let leaf = leaf.trim().to_lowercase();
        if leaf.is_empty() {
            return None;
        }
        let outer: Vec<String> = outer.iter().map(|o| o.trim().to_lowercase()).collect();
        // The lookup naming the most of the outer levels too (the first
        // of equals).
        let best = self
            .lookups
            .iter()
            .filter(|l| l.names.contains(&leaf))
            .min_by_key(|l| std::cmp::Reverse(outer.iter().filter(|o| l.names.contains(o)).count()))
            .map(|l| l.region);
        best.or_else(|| {
            self.regions
                .iter()
                .position(|r| r.name.trim().to_lowercase() == leaf)
        })
    }
}

// ── projections ────────────────────────────────────────────────────────────

/// Robinson's table: per 5° of latitude, the parallel's length and its
/// distance from the equator.
const ROBINSON: [(f64, f64); 19] = [
    (1.0000, 0.0000),
    (0.9986, 0.0620),
    (0.9954, 0.1240),
    (0.9900, 0.1860),
    (0.9822, 0.2480),
    (0.9730, 0.3100),
    (0.9600, 0.3720),
    (0.9427, 0.4340),
    (0.9216, 0.4958),
    (0.8962, 0.5571),
    (0.8679, 0.6176),
    (0.8350, 0.6769),
    (0.7986, 0.7346),
    (0.7597, 0.7903),
    (0.7186, 0.8435),
    (0.6732, 0.8936),
    (0.6213, 0.9394),
    (0.5722, 0.9761),
    (0.5322, 1.0000),
];

/// A projection set up for the regions shown (Albers needs their extent).
#[derive(Clone, Copy, Debug)]
pub struct Projector {
    kind: Projection,
    /// Albers: n, C, ρ0, λ0 (radians).
    albers: [f64; 4],
}

impl Projector {
    /// `bounds` = `[west, south, east, north]` of what is shown.
    pub fn new(kind: Projection, bounds: [f64; 4]) -> Self {
        let mut albers = [0.0; 4];
        if kind == Projection::Albers {
            let (s, n) = (bounds[1].to_radians(), bounds[3].to_radians());
            let p1 = s + (n - s) / 6.0;
            let p2 = n - (n - s) / 6.0;
            let p0 = (s + n) / 2.0;
            let mut nn = (p1.sin() + p2.sin()) / 2.0;
            if nn.abs() < 1e-6 {
                nn = 1e-6;
            }
            let c = p1.cos().powi(2) + 2.0 * nn * p1.sin();
            let rho0 = (c - 2.0 * nn * p0.sin()).max(0.0).sqrt() / nn;
            let l0 = ((bounds[0] + bounds[2]) / 2.0).to_radians();
            albers = [nn, c, rho0, l0];
        }
        Self { kind, albers }
    }

    /// `[x, y]` with y up (unitless; the caller fits it in the plot).
    pub fn project(&self, lon: f64, lat: f64) -> [f64; 2] {
        let lat = lat.clamp(-89.0, 89.0);
        let (l, f) = (lon.to_radians(), lat.to_radians());
        match self.kind {
            Projection::Mercator => {
                let f = f.clamp(-85f64.to_radians(), 85f64.to_radians());
                [l, (std::f64::consts::FRAC_PI_4 + f / 2.0).tan().ln()]
            }
            Projection::Miller => [l, 1.25 * (std::f64::consts::FRAC_PI_4 + 0.4 * f).tan().ln()],
            Projection::Robinson => {
                let a = lat.abs() / 5.0;
                let i = (a.floor() as usize).min(17);
                let t = a - i as f64;
                let (l0, d0) = ROBINSON[i];
                let (l1, d1) = ROBINSON[i + 1];
                let plen = l0 + (l1 - l0) * t;
                let pdfe = d0 + (d1 - d0) * t;
                [0.8487 * plen * l, 1.3523 * pdfe * lat.signum()]
            }
            Projection::Albers => {
                let [n, c, rho0, l0] = self.albers;
                let rho = (c - 2.0 * n * f.sin()).max(0.0).sqrt() / n;
                let th = n * (l - l0);
                [rho * th.sin(), rho0 - rho * th.cos()]
            }
        }
    }
}

// ── colours ────────────────────────────────────────────────────────────────

fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    Rgb(std::array::from_fn(|i| {
        (a.0[i] as f64 + (b.0[i] as f64 - a.0[i] as f64) * t).round() as u8
    }))
}

/// A value scale: stops (value, colour) in increasing order.
#[derive(Clone, Debug, PartialEq)]
pub struct Scale {
    pub stops: Vec<(f64, Rgb)>,
}

impl Scale {
    /// The scale for values `lo..=hi`: Office's defaults (a light and a
    /// dark shade of `accent`, white in the middle of 3) where the part
    /// gives no colour.
    pub fn new(c: &ValueColors, lo: f64, hi: f64, accent: Rgb) -> Self {
        let white = Rgb([255, 255, 255]);
        let min_c = c.min.unwrap_or(mix(accent, white, 0.8));
        let max_c = c.max.unwrap_or(mix(accent, Rgb([0, 0, 0]), 0.25));
        let mid_c = c.mid.unwrap_or(mix(min_c, max_c, 0.5));
        let at = |s: StopAt, auto: f64| match s {
            StopAt::Auto => auto,
            StopAt::Number(v) => v,
            StopAt::Percent(p) => lo + (hi - lo) * p / 100.0,
        };
        let a = at(c.at[0], lo);
        let b = at(c.at[2], hi);
        let mut stops = vec![(a, min_c)];
        if c.count == 3 {
            stops.push((at(c.at[1], (a + b) / 2.0), mid_c));
        }
        stops.push((b, max_c));
        stops.sort_by(|x, y| x.0.total_cmp(&y.0));
        Self { stops }
    }

    pub fn color(&self, v: f64) -> Rgb {
        let first = self.stops[0];
        if v <= first.0 {
            return first.1;
        }
        for w in self.stops.windows(2) {
            let ((a, ca), (b, cb)) = (w[0], w[1]);
            if v <= b {
                let t = if b > a { (v - a) / (b - a) } else { 1.0 };
                return mix(ca, cb, t);
            }
        }
        self.stops[self.stops.len() - 1].1
    }

    pub fn range(&self) -> (f64, f64) {
        (self.stops[0].0, self.stops[self.stops.len() - 1].0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pca_round_trips_and_matches_bing_example() {
        // The first point of Bing's documentation example, (35.894309,
        // -110.72522), compresses to "vx1vilihnM".
        assert_eq!(encode_pca(&[[-110.72522, 35.894309]]), "vx1vilihnM");
        let pts = [
            [-110.72522, 35.894309],
            [-110.72495, 35.8942],
            [-110.72475, 35.89417],
            [-110.72454, 35.8943],
            [2.35, -48.85],
        ];
        let s = encode_pca(&pts);
        let back = decode_pca(&s).unwrap();
        assert_eq!(back.len(), pts.len());
        for (a, b) in back.iter().zip(pts) {
            assert!((a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5);
        }
        assert!(decode_pca("vx1!").is_none());
    }

    #[test]
    fn scale_interpolates_between_stops() {
        let c = ValueColors {
            count: 3,
            min: Some(Rgb([0, 0, 0])),
            mid: Some(Rgb([100, 100, 100])),
            max: Some(Rgb([200, 200, 200])),
            at: [StopAt::Auto, StopAt::Percent(25.0), StopAt::Auto],
        };
        let s = Scale::new(&c, 0.0, 100.0, Rgb([0, 0, 255]));
        assert_eq!(s.color(25.0), Rgb([100, 100, 100]));
        assert_eq!(s.color(12.5), Rgb([50, 50, 50]));
        assert_eq!(s.color(-5.0), Rgb([0, 0, 0]));
        assert_eq!(s.color(1000.0), Rgb([200, 200, 200]));
    }

    #[test]
    fn projections_keep_north_up_and_east_right() {
        for k in [
            Projection::Mercator,
            Projection::Miller,
            Projection::Robinson,
            Projection::Albers,
        ] {
            let p = Projector::new(k, [-125.0, 25.0, -65.0, 50.0]);
            let a = p.project(-100.0, 30.0);
            let b = p.project(-90.0, 45.0);
            assert!(b[0] > a[0] && b[1] > a[1], "{k:?} {a:?} {b:?}");
        }
    }
}
