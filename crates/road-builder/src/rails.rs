//! Railway track pieces measured from binarised (ODOL) models, a catalog of the pieces that
//! terrain routing can chain, and export of fitted tracks as Terrain Builder objects.
use crate::{
    document::Route,
    geometry::{Library, Model, Point, Port, add, mul, norm, sub},
    odol,
    roads::CatalogPart,
};
use anyhow::{Context, Result, bail, ensure};
use std::{fmt::Write as _, path::Path};

/// Default folder with the DayZ track models.
pub const DEFAULT_FOLDER: &str = "P:\\dz\\structures\\rail\\Tracks";

/// Marker error: an ODOL model without the `map1` / `map2` track end points.
#[derive(Debug)]
pub struct NotTrack;
impl std::fmt::Display for NotTrack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Model ODOL nie jest torem (brak punktów map1/map2)")
    }
}
impl std::error::Error for NotTrack {}

fn unit(p: Point) -> Point {
    mul(p, 1. / norm(p).max(1e-12))
}

/// Measure a track piece. The memory points `map2` → `map1` give the direction of travel;
/// the centre line comes from the rail heads (the highest vertices of the first LOD), which
/// are exact, unlike the memory points that can be off by several centimetres.
pub fn track_model(bytes: &[u8]) -> Result<Model> {
    let lods = odol::read(bytes, |i, r| i == 0 || r == odol::MEMORY)?;
    let memory = lods
        .iter()
        .find(|l| l.resolution == odol::MEMORY)
        .ok_or(NotTrack)?;
    let (Some(end), Some(start)) = (memory.point("map1"), memory.point("map2")) else {
        return Err(NotTrack.into());
    };
    let visual = &lods[0];
    ensure!(visual.resolution != odol::MEMORY, "Model toru nie ma LOD wizualnego");
    let (a, b) = ([start[0] as f64, start[2] as f64], [end[0] as f64, end[2] as f64]);
    let chord = norm(sub(b, a));
    ensure!(chord > 0.5, "Końce toru map1/map2 są za blisko siebie");
    let u = mul(sub(b, a), 1. / chord);
    // Right of the direction of travel.
    let n = [u[1], -u[0]];
    let mid = mul(add(a, b), 0.5);
    let local = |v: &[f32; 3]| {
        let d = sub([v[0] as f64, v[2] as f64], mid);
        (d[0] * u[0] + d[1] * u[1], d[0] * n[0] + d[1] * n[1])
    };
    let top = visual.vertices.iter().map(|v| v[1]).fold(f32::MIN, f32::max);
    let heads: Vec<(f64, f64)> = visual
        .vertices
        .iter()
        .filter(|v| v[1] > top - 0.02)
        .map(local)
        .collect();
    // Centre between the two rail heads, slice by slice.
    // Rail heads are long strips with vertices only where they bend (curves) or end
    // (straights). Group them along the track; a group with both rails gives a centre point.
    let mut sorted = heads.clone();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut samples = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let mut j = i;
        while j + 1 < sorted.len() && sorted[j + 1].0 - sorted[i].0 < 0.3 {
            j += 1;
        }
        let group = &sorted[i..=j];
        let lo = group.iter().map(|g| g.1).fold(f64::INFINITY, f64::min);
        let hi = group.iter().map(|g| g.1).fold(f64::NEG_INFINITY, f64::max);
        if hi - lo > 0.5 {
            let along = group.iter().map(|g| g.0).sum::<f64>() / group.len() as f64;
            samples.push((along, (lo + hi) / 2.));
        }
        i = j + 1;
    }
    ensure!(samples.len() >= 2, "Nie rozpoznano szyn w modelu toru");
    // Least-squares lateral offset: side(s) = k0 + k1 s + k2 s² (linear with few samples).
    let terms = if samples.len() >= 4 { 3 } else { 2 };
    let mut m = [[0f64; 3]; 3];
    let mut r = [0f64; 3];
    for &(s, side) in &samples {
        let p = [1., s, s * s];
        for i in 0..terms {
            for j in 0..terms {
                m[i][j] += p[i] * p[j];
            }
            r[i] += p[i] * side;
        }
    }
    if terms == 2 {
        m[2][2] = 1.;
    }
    let k = solve3(m, r).context("Nie można wyznaczyć osi toru")?;
    let curved = k[2].abs() > 2e-6;
    let k = if curved { k } else { [k[0] + k[2] * chord * chord / 12., k[1], 0.] };
    let point = |s: f64| add(add(mid, mul(u, s)), mul(n, k[0] + k[1] * s + k[2] * s * s));
    let direction = |s: f64| unit(add(u, mul(n, k[1] + 2. * k[2] * s)));
    // Rail ends: move each end until the farthest rail head along the local direction.
    let mut ends = [-chord / 2., chord / 2.];
    for (e, sign) in ends.iter_mut().zip([-1., 1.]) {
        for _ in 0..4 {
            let (p, d) = (point(*e), mul(direction(*e), sign));
            let reach = heads
                .iter()
                .map(|&(along, side)| {
                    let q = add(add(mid, mul(u, along)), mul(n, side));
                    let rel = sub(q, p);
                    rel[0] * d[0] + rel[1] * d[1]
                })
                .fold(f64::NEG_INFINITY, f64::max);
            if !reach.is_finite() {
                break;
            }
            *e += sign * reach;
        }
    }
    let steps = ((ends[1] - ends[0]) / 2.5).ceil().max(2.) as usize;
    let line: Vec<Point> = (0..=steps)
        .map(|i| point(ends[0] + (ends[1] - ends[0]) * i as f64 / steps as f64))
        .collect();
    let length: f64 = line.windows(2).map(|w| norm(sub(w[1], w[0]))).sum();
    // Footprint: the width of the whole model across its middle (sleepers and ballast).
    let (lo, hi) = visual
        .vertices
        .iter()
        .map(local)
        .filter(|(along, _)| along.abs() < 0.5)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (_, side)| (lo.min(side), hi.max(side)));
    let centre = k[0];
    let half = if hi > lo { ((hi - centre).max(centre - lo)).clamp(0.5, 10.) } else { 1.5 };
    let mut triangles = Vec::new();
    for (i, w) in line.windows(2).enumerate() {
        let s0 = ends[0] + (ends[1] - ends[0]) * i as f64 / steps as f64;
        let s1 = ends[0] + (ends[1] - ends[0]) * (i + 1) as f64 / steps as f64;
        let side = |s: f64| {
            let d = direction(s);
            [d[1] * half, -d[0] * half]
        };
        let (l0, r0) = (sub(w[0], side(s0)), add(w[0], side(s0)));
        let (l1, r1) = (sub(w[1], side(s1)), add(w[1], side(s1)));
        triangles.push([l0, r0, r1]);
        triangles.push([l0, r1, l1]);
    }
    Ok(Model {
        file: None,
        triangles,
        ports: [
            Some(Port { p: line[0], outward: mul(direction(ends[0]), -1.) }),
            Some(Port { p: *line.last().unwrap(), outward: direction(ends[1]) }),
            None,
            None,
        ],
        line,
        length,
        source: "ODOL".into(),
    })
}

fn solve3(mut m: [[f64; 3]; 3], mut r: [f64; 3]) -> Option<[f64; 3]> {
    for c in 0..3 {
        let p = (c..3).max_by(|&i, &j| m[i][c].abs().total_cmp(&m[j][c].abs()))?;
        if m[p][c].abs() < 1e-12 {
            return None;
        }
        m.swap(c, p);
        r.swap(c, p);
        for i in 0..3 {
            if i != c {
                let f = m[i][c] / m[c][c];
                let pivot = m[c];
                for (v, w) in m[i].iter_mut().zip(pivot) {
                    *v -= f * w;
                }
                r[i] -= f * r[c];
            }
        }
    }
    Some([r[0] / m[0][0], r[1] / m[1][1], r[2] / m[2][2]])
}

/// Track family and piece name, e.g. `Rail_TrackE_L25_10` → (`Rail_TrackE`, `L25_10`).
fn split(stem: &str) -> Option<(String, &str)> {
    let mut parts = stem.splitn(3, '_');
    let (rail, track, piece) = (parts.next()?, parts.next()?, parts.next()?);
    (rail.eq_ignore_ascii_case("rail") && track.to_lowercase().starts_with("track"))
        .then(|| (format!("{rail}_{track}"), piece))
}
/// Whether terrain routing may chain this piece: plain straights and curves. Switches,
/// level crossings, vertical (Up/Down) pieces, type transitions and the `_lc` / `_nolc`
/// variants are placed by hand.
fn routable(piece: &str) -> bool {
    let lower = piece.to_lowercase();
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if lower.ends_with("_lc") || lower.ends_with("_nolc") {
        return false;
    }
    let curve = |s: &str| {
        s.len() > 1
            && (s.starts_with('l') || s.starts_with('r'))
            && s[1..].split_once('_').is_some_and(|(a, b)| digits(a) && digits(b))
    };
    digits(&lower)
        || lower.strip_suffix('m').is_some_and(digits)
        || lower.strip_suffix("deg").is_some_and(digits)
        || lower == "sp"
        || curve(&lower)
}
pub fn is_track_family(family: &str) -> bool {
    split(&format!("{family}_x")).is_some()
}

/// Track pieces in `folder` usable for terrain routing, with warnings for unreadable models.
pub fn catalog(folder: &Path, lib: &mut Library) -> Result<(Vec<CatalogPart>, Vec<String>)> {
    let mut files: Vec<_> = std::fs::read_dir(folder)
        .with_context(|| format!("Nie można odczytać folderu torów {}", folder.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("p3d")))
        .collect();
    files.sort();
    let mut parts = Vec::new();
    let mut warnings = Vec::new();
    for path in files {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let Some((family, piece)) = split(&stem) else { continue };
        let lower = piece.to_lowercase();
        // Level-crossing pieces are not routed but replace straights at road crossings.
        let crossing = lower.starts_with("passing_") && !lower.ends_with("_lc") && !lower.ends_with("_nolc");
        if !routable(piece) && !crossing {
            continue;
        }
        match lib.model(&path.to_string_lossy()) {
            Ok(model) if model.ports[0].is_some() && model.ports[1].is_some() => {
                let a = model.ports[0].unwrap().outward;
                let b = model.ports[1].unwrap().outward;
                let turn = (-a[0] * b[1] + a[1] * b[0]).atan2(-a[0] * b[0] - a[1] * b[1]);
                parts.push(CatalogPart {
                    path: path.to_string_lossy().into_owned(),
                    family,
                    category: if crossing {
                        5
                    } else if turn.abs() > 0.1f64.to_radians() {
                        4
                    } else {
                        3
                    },
                    index: 0,
                    road_type_index: u32::MAX,
                    model,
                });
            }
            Ok(_) => warnings.push(format!("{stem}: brak końców toru")),
            Err(e) => warnings.push(format!("{stem}: {e:#}")),
        }
    }
    ensure!(!parts.is_empty(), "W folderze nie ma modeli torów Rail_Track*");
    Ok((parts, warnings))
}

/// Terrain Builder "Import objects" text: `"name";X;Y;yaw;pitch;roll;scale;Z;` per piece,
/// with the heading in Terrain Builder's clockwise degrees and Z relative to the terrain.
pub fn objects(routes: &[&Route]) -> Result<String> {
    let mut out = String::new();
    let mut count = 0;
    for route in routes {
        ensure!(!route.parts.is_empty(), "Torowisko {} nie ma dopasowanych segmentów", route.name);
        for part in route.parts.iter().chain(&route.extras) {
            let name = part
                .model
                .rsplit(['\\', '/'])
                .next()
                .and_then(|f| f.rsplit_once('.').map(|(s, _)| s).or(Some(f)))
                .context("Brak nazwy modelu toru")?
                .to_lowercase();
            // A reversed piece is the same object turned around.
            let yaw = (part.rotation + if part.reverse { 180. } else { 0. }).rem_euclid(360.);
            ensure!(
                part.position.iter().all(|v| v.is_finite()) && yaw.is_finite(),
                "Nieprawidłowa pozycja toru"
            );
            writeln!(
                out,
                "\"{name}\";{:.6};{:.6};{yaw:.6};0.000000;0.000000;1.000000;0.000000;",
                part.position[0], part.position[1]
            )?;
            count += 1;
        }
    }
    if count == 0 {
        bail!("Brak torowisk do eksportu");
    }
    Ok(out)
}

/// Footprint of a binarised object without track ends: its first LOD's x/z bounding box.
pub fn object_model(bytes: &[u8]) -> Result<Model> {
    let lods = odol::read(bytes, |i, _| i == 0)?;
    let v = &lods.first().context("Model ODOL nie ma LOD")?.vertices;
    ensure!(!v.is_empty(), "Model ODOL nie ma wierzchołków");
    let (x0, x1, z0, z1) = v.iter().fold(
        (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY),
        |(a, b, c, d), p| (a.min(p[0] as f64), b.max(p[0] as f64), c.min(p[2] as f64), d.max(p[2] as f64)),
    );
    let (p, q, r, s) = ([x0, z0], [x1, z0], [x1, z1], [x0, z1]);
    Ok(Model {
        file: None,
        triangles: vec![[p, q, r], [p, r, s]],
        ports: [None; 4],
        line: vec![],
        length: z1 - z0,
        source: "ODOL".into(),
    })
}

/// A road centre line and its half width, for level crossings. Lines with the same `road`
/// (the parts of one road) count as one road.
#[derive(Clone, Debug)]
pub struct RoadLine {
    pub points: Vec<Point>,
    pub half_width: f64,
    pub road: usize,
}
/// A road crossed by a track piece: crossing point, distance along the piece, the angle
/// between track and road (0–90°) and the road's half width.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub at: Point,
    pub along: f64,
    pub angle: f64,
    pub half_width: f64,
}
/// Minimum angle between a track and a road it crosses.
pub const MIN_CROSSING_ANGLE: f64 = 60.;
/// Extra length of a level crossing beyond the road, along the track.
const CROSSING_MARGIN: f64 = 1.;
/// A deck block: model path, length along the track and centre offset along it.
type Block = (String, f64, f64);
/// How a family's level crossings are built.
#[derive(Clone, Debug)]
enum Kit {
    /// Replace the straight `from` by the crossing piece `to` of the same geometry.
    Piece { from: String, to: String, length: f64 },
    /// Lay deck blocks (path, length along the track, centre offset) over plain straights;
    /// `tile` repeats the shortest block to cover the road.
    Blocks { blocks: Vec<(String, f64, f64)>, tile: bool },
    None,
}
/// Roads near tracks and how to build crossings for one track family.
pub struct Crossings {
    roads: Vec<RoadLine>,
    /// Road segments by 50 m cell.
    cells: std::collections::HashMap<(i64, i64), Vec<(usize, usize)>>,
    kit: Kit,
}
const CELL: f64 = 50.;
fn cell(p: Point) -> (i64, i64) {
    ((p[0] / CELL).floor() as i64, (p[1] / CELL).floor() as i64)
}
fn crossing_point(a: Point, b: Point, c: Point, d: Point) -> Option<(f64, f64)> {
    let r = sub(b, a);
    let s = sub(d, c);
    let den = r[0] * s[1] - r[1] * s[0];
    if den.abs() < 1e-12 {
        return None;
    }
    let ac = sub(c, a);
    let t = (ac[0] * s[1] - ac[1] * s[0]) / den;
    let u = (ac[0] * r[1] - ac[1] * r[0]) / den;
    ((0. ..=1.).contains(&t) && (0. ..=1.).contains(&u)).then_some((t, u))
}
fn point_segment(p: Point, a: Point, b: Point) -> f64 {
    let d = sub(b, a);
    let l = d[0] * d[0] + d[1] * d[1];
    let t = if l < 1e-12 { 0. } else { (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l).clamp(0., 1.) };
    norm(sub(p, add(a, mul(d, t))))
}
impl Crossings {
    /// No roads: tracks are never limited by crossings.
    pub fn none() -> Self {
        Self { roads: vec![], cells: Default::default(), kit: Kit::None }
    }
    /// `tracks` is the folder with the family's models; deck blocks are looked up there and
    /// in the neighbouring `Misc` folder.
    pub fn new(roads: Vec<RoadLine>, family: &str, catalog: &[CatalogPart], tracks: &Path) -> Self {
        let kit = Self::kit(family, catalog, tracks);
        Self::with_kit(roads, kit)
    }
    fn with_kit(roads: Vec<RoadLine>, kit: Kit) -> Self {
        let mut cells: std::collections::HashMap<(i64, i64), Vec<(usize, usize)>> = Default::default();
        for (i, road) in roads.iter().enumerate() {
            for (j, w) in road.points.windows(2).enumerate() {
                let reach = road.half_width + 10.;
                let (lo, hi) = (
                    cell([w[0][0].min(w[1][0]) - reach, w[0][1].min(w[1][1]) - reach]),
                    cell([w[0][0].max(w[1][0]) + reach, w[0][1].max(w[1][1]) + reach]),
                );
                for x in lo.0..=hi.0 {
                    for y in lo.1..=hi.1 {
                        cells.entry((x, y)).or_default().push((i, j));
                    }
                }
            }
        }
        Self { roads, cells, kit }
    }
    fn kit(family: &str, catalog: &[CatalogPart], tracks: &Path) -> Kit {
        let lower = family.to_lowercase();
        let block = |name: &str| -> Option<(String, f64, f64)> {
            let dirs = [tracks.to_path_buf(), tracks.parent().map(|p| p.join("Misc")).unwrap_or_default()];
            let path = dirs.iter().map(|d| d.join(name)).find(|p| p.exists())?;
            let model = object_model(&std::fs::read(&path).ok()?).ok()?;
            let (z0, z1) = model.triangles.iter().flatten().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p[1]), b.max(p[1])));
            Some((path.to_string_lossy().into_owned(), z1 - z0, (z0 + z1) / 2.))
        };
        if lower == "rail_track" {
            let find = |stem: &str| catalog.iter().find(|c| {
                c.family == family && Path::new(&c.path).file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(stem))
            });
            match (find("Rail_Track_25"), find("Rail_Track_Passing_25")) {
                (Some(a), Some(b)) => Kit::Piece { from: a.path.clone(), to: b.path.clone(), length: b.model.length },
                _ => Kit::None,
            }
        } else if lower == "rail_tracksmall" {
            block("Rail_Crossing_Block_Small.p3d").map_or(Kit::None, |b| Kit::Blocks { blocks: vec![b], tile: true })
        } else {
            let mut blocks: Vec<_> = ["Rail_Crossing_Block_10.p3d", "Rail_Crossing_Block_25.p3d"].into_iter().filter_map(block).collect();
            blocks.sort_by(|a, b| a.1.total_cmp(&b.1));
            if blocks.is_empty() { Kit::None } else { Kit::Blocks { blocks, tile: false } }
        }
    }
    pub fn is_empty(&self) -> bool {
        self.roads.is_empty()
    }
    /// Roads crossed by `line` and whether it runs inside the corridor of a road it does
    /// not cross (closer than the road's half width plus `clearance`).
    fn hits(&self, line: &[Point], clearance: f64) -> (Vec<Hit>, bool) {
        let mut hits = Vec::new();
        let mut candidates = std::collections::BTreeSet::new();
        for p in line {
            if let Some(v) = self.cells.get(&cell(*p)) {
                candidates.extend(v.iter().copied());
            }
        }
        if candidates.is_empty() {
            return (hits, false);
        }
        let mut walked = 0.;
        let mut crossed = std::collections::BTreeSet::new();
        for w in line.windows(2) {
            let len = norm(sub(w[1], w[0]));
            for &(i, j) in &candidates {
                let (c, d) = (self.roads[i].points[j], self.roads[i].points[j + 1]);
                if let Some((t, _)) = crossing_point(w[0], w[1], c, d) {
                    let (u, v) = (sub(w[1], w[0]), sub(d, c));
                    let cos = ((u[0] * v[0] + u[1] * v[1]) / (len * norm(v)).max(1e-12)).abs().min(1.);
                    let at = add(w[0], mul(u, t));
                    if !hits.iter().any(|h: &Hit| norm(sub(h.at, at)) < 0.5) {
                        hits.push(Hit { at, along: walked + t * len, angle: cos.acos(), half_width: self.roads[i].half_width });
                    }
                    crossed.insert(self.roads[i].road);
                }
            }
            walked += len;
        }
        let touches = candidates.iter().any(|&(i, j)| {
            !crossed.contains(&self.roads[i].road) && {
                let (c, d) = (self.roads[i].points[j], self.roads[i].points[j + 1]);
                let limit = self.roads[i].half_width + clearance;
                line.windows(2).any(|w| {
                    point_segment(w[0], c, d) < limit || point_segment(c, w[0], w[1]) < limit || point_segment(d, w[0], w[1]) < limit
                })
            }
        });
        (hits, touches)
    }
    /// Length of track a crossing of this road occupies, and the blocks covering it.
    fn need(&self, hit: &Hit) -> Option<(f64, Vec<Block>)> {
        let span = 2. * hit.half_width / hit.angle.sin().max(1e-6) + 2. * CROSSING_MARGIN;
        match &self.kit {
            // The crossing piece is as long as the straight it replaces; the road only
            // has to fit on it.
            Kit::Piece { length, .. } => (span <= *length).then(|| (span, vec![])),
            Kit::Blocks { blocks, tile: false } => blocks.iter().find(|b| b.1 >= span).map(|b| (b.1, vec![b.clone()])),
            Kit::Blocks { blocks, tile: true } => {
                let b = blocks.first()?;
                let n = (span / b.1).ceil().max(1.) as usize;
                Some((b.1 * n as f64, vec![b.clone(); n]))
            }
            Kit::None => None,
        }
    }
    /// Whether a piece may lie on `line`: away from roads, or a straight crossing a road at
    /// [`MIN_CROSSING_ANGLE`] or more with the whole crossing on this piece.
    pub fn allows(&self, item: &CatalogPart, line: &[Point], clearance: f64) -> bool {
        if self.roads.is_empty() {
            return true;
        }
        let (hits, touches) = self.hits(line, clearance);
        if touches {
            return false;
        }
        if hits.is_empty() {
            return true;
        }
        let crossing_piece = matches!(&self.kit, Kit::Piece { to, .. } if item.path == *to);
        if item.category != 3 && !crossing_piece {
            return false;
        }
        if let Kit::Piece { from, to, .. } = &self.kit
            && item.path != *from
            && item.path != *to
        {
            return false;
        }
        hits.iter().all(|h| {
            h.angle >= MIN_CROSSING_ANGLE.to_radians() - 1e-9
                && self.need(h).is_some_and(|(span, _)| {
                    // The neighbouring pieces must also stay out of the road's corridor.
                    let end = (span / 2.).max((h.half_width + clearance) / h.angle.sin() + 0.5);
                    h.along >= end - 1e-6 && item.model.length - h.along >= end - 1e-6
                })
        })
    }
    /// Turn crossings of a finished track into crossing pieces and deck blocks. Fails if a
    /// road is crossed on a curve, too obliquely or where the crossing does not fit.
    pub fn apply(
        &self,
        parts: &mut [crate::roads::PlacedPart],
        catalog: &[CatalogPart],
    ) -> Result<Vec<crate::roads::PlacedPart>> {
        let mut extras = Vec::new();
        if self.roads.is_empty() {
            return Ok(extras);
        }
        for part in parts.iter_mut() {
            let item = catalog.iter().find(|c| c.path == part.model).context("Brak modelu w katalogu")?;
            let angle = -part.rotation.to_radians();
            let mut line: Vec<Point> = item.model.line.iter().map(|q| add(crate::geometry::rotate(*q, angle), part.position)).collect();
            if part.reverse {
                line.reverse();
            }
            let (hits, _) = self.hits(&line, 0.);
            if hits.is_empty() {
                continue;
            }
            if !self.allows(item, &line, 0.) {
                bail!(
                    "Tor przecina drogę na łuku, pod kątem mniejszym niż {MIN_CROSSING_ANGLE}° lub bez miejsca na przejazd przy ({:.0}, {:.0})",
                    hits[0].at[0],
                    hits[0].at[1]
                );
            }
            match &self.kit {
                Kit::Piece { to, .. } => part.model = to.clone(),
                Kit::Blocks { .. } => {
                    let forward = crate::geometry::rotate([0., 1.], angle);
                    for h in &hits {
                        let (span, blocks) = self.need(h).unwrap();
                        let mut from = -span / 2.;
                        for (path, length, offset) in blocks {
                            let centre = from + length / 2.;
                            extras.push(crate::roads::PlacedPart {
                                model: path,
                                reverse: false,
                                position: sub(add(h.at, mul(forward, centre)), mul(forward, offset)),
                                rotation: part.rotation,
                            });
                            from += length;
                        }
                    }
                }
                Kit::None => bail!("Brak modeli przejazdu kolejowego dla tego typu toru"),
            }
        }
        Ok(extras)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roads::PlacedPart;

    #[test]
    fn routable_pieces_and_families() {
        for piece in ["25", "40", "L25_10", "R30_20", "15deg", "10m", "SP"] {
            assert!(routable(piece), "{piece}");
        }
        for piece in ["25_lc", "8_nolc", "TurnOutL", "Up_25", "Down_40", "LB_RE", "Passing_10", "Switch"] {
            assert!(!routable(piece), "{piece}");
        }
        assert_eq!(split("Rail_TrackE_L25_10"), Some(("Rail_TrackE".into(), "L25_10")));
        assert!(is_track_family("Rail_TrackSmall"));
        assert!(!is_track_family("asf2"));
    }

    #[test]
    fn terrain_builder_objects_turn_reversed_pieces_around() {
        let route = Route {
            name: "rail".into(),
            family: "Rail_Track".into(),
            points: vec![],
            parts: vec![
                PlacedPart { model: "P:\\dz\\structures\\rail\\Tracks\\Rail_Track_25.p3d".into(), reverse: false, position: [200100.5, 3000.25], rotation: -90. },
                PlacedPart { model: "Rail_Track_L25_10.p3d".into(), reverse: true, position: [200125., 3000.], rotation: 270. },
            ],
            replaces: None,
            rail: true, extras: vec![],
            grading: None,
        };
        let text = objects(&[&route]).unwrap();
        assert_eq!(
            text,
            "\"rail_track_25\";200100.500000;3000.250000;270.000000;0.000000;0.000000;1.000000;0.000000;\n\
             \"rail_track_l25_10\";200125.000000;3000.000000;90.000000;0.000000;0.000000;1.000000;0.000000;\n"
        );
        let empty = Route { parts: vec![], ..route };
        assert!(objects(&[&empty]).is_err());
    }

    /// Measured geometry of the DayZ track models when the P: drive is mounted.
    #[test]
    fn dayz_tracks_have_exact_lengths_curves_and_turn_directions() {
        let folder = Path::new(DEFAULT_FOLDER);
        if !folder.exists() {
            return;
        }
        let read = |name: &str| track_model(&std::fs::read(folder.join(name)).unwrap()).unwrap();
        let turn = |m: &Model| {
            let (a, b) = (m.ports[0].unwrap().outward, m.ports[1].unwrap().outward);
            (-a[0] * b[1] + a[1] * b[0]).atan2(-a[0] * b[0] - a[1] * b[1]).to_degrees()
        };
        for (name, length, angle) in [
            ("Rail_Track_25.p3d", 25., 0.),
            ("Rail_TrackE_40.p3d", 40., 0.),
            ("Rail_TrackE_8.p3d", 8., 0.),
            // Positive turns are to the left (counter-clockwise from above).
            ("Rail_Track_L25_10.p3d", 25., 10.),
            ("Rail_Track_R25_10.p3d", 25., -10.),
            ("Rail_TrackE_L25_5.p3d", 25., 5.),
            ("Rail_TrackE_R25_10.p3d", 25., -10.),
        ] {
            let m = read(name);
            assert!((m.length - length).abs() < 0.05, "{name}: length {}", m.length);
            // The measured angle is used; the old left curve is ~0.4° flatter than its name.
            assert!((turn(&m) - angle).abs() < 0.5, "{name}: turn {}", turn(&m));
            // Straights are straight along the model axis.
            if angle == 0. {
                let (a, b) = (m.ports[0].unwrap(), m.ports[1].unwrap());
                assert!((a.p[0] - b.p[0]).abs() < 0.01, "{name}: {a:?} {b:?}");
            }
        }
        assert!(track_model(&std::fs::read("P:/dz/structures/roads/parts/Centerline_25.p3d").unwrap_or_default()).is_err());
    }

    /// Terrain routing and fitting with the DayZ E-type track when the P: drive is mounted.
    #[test]
    fn terrain_routed_track_is_continuous_and_reaches_its_points() {
        use crate::{
            document::{RoutingSettings, WaterSettings},
            routing::{Obstacles, permitted, plan},
        };
        use std::sync::atomic::AtomicBool;
        let folder = Path::new(DEFAULT_FOLDER);
        if !folder.exists() {
            return;
        }
        let mut lib = Library::new(folder.into());
        let (catalog, _) = catalog(folder, &mut lib).unwrap();
        let dir = std::env::temp_dir().join(format!("rail-route-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 1210 × 1210 m: gentle waves and a 25 m hill the track has to go around or climb.
        let mut text = "ncols 122\nnrows 122\nxllcorner 0\nyllcorner 0\ncellsize 10\n".to_owned();
        for row in 0..122 {
            for col in 0..122 {
                let (x, y) = (col as f64 * 10. + 5., (121 - row) as f64 * 10. + 5.);
                let d2 = (x - 600.).powi(2) + (y - 600.).powi(2);
                let z = 100. + 3. * (x / 150.).sin() + 25. * (-d2 / 40000.).exp();
                text.push_str(&format!("{z:.3} "));
            }
            text.push('\n');
        }
        std::fs::write(dir.join("t.asc"), text).unwrap();
        let cancel = AtomicBool::new(false);
        let terrain = crate::terrain::import(&dir.join("t.asc"), &dir, &cancel).unwrap();
        let s = RoutingSettings::rail();
        let none = Obstacles::new(&[], &[], &WaterSettings::default(), Some(&terrain), &cancel).unwrap();
        for family in ["Rail_TrackE", "Rail_Track"] {
            let points = [[100., 600.], [600., 250.], [1100., 600.]];
            let parts = plan(&terrain, &points, family, &catalog, &s, &none, &Crossings::none(), &cancel).unwrap().parts;
            // Every piece starts exactly where the previous one ends.
            let model = |name: &str| &catalog.iter().find(|c| c.path == name).unwrap().model;
            for w in parts.windows(2) {
                let end = crate::roads::endpoint(std::slice::from_ref(&w[0]), &catalog).unwrap();
                let m = model(&w[1].model);
                let entry = m.ports[usize::from(w[1].reverse)].unwrap();
                let start = add(crate::geometry::rotate(entry.p, -w[1].rotation.to_radians()), w[1].position);
                assert!(norm(sub(start, end.p)) < 1e-6, "gap {}", norm(sub(start, end.p)));
            }
            let line = crate::roads::centerline(&parts, &catalog);
            let slack = catalog.iter().filter(|c| c.family == family).map(|c| c.model.length).fold(f64::INFINITY, f64::min) / 2.;
            for (i, p) in points.iter().enumerate() {
                let miss = line.iter().map(|q| norm(sub(*q, *p))).fold(f64::INFINITY, f64::min);
                let limit = if i == points.len() - 1 { s.tolerance + slack } else { s.tolerance };
                assert!(miss <= limit, "{family}: point {i} missed by {miss}");
            }
            assert!(line.windows(2).all(|w| permitted(w[0], w[1], &terrain, &s, &none)));
            assert!(parts.iter().all(|p| catalog.iter().any(|c| c.path == p.model && c.family == family)));
            let route = Route { name: "t".into(), family: family.to_owned(), points: points.to_vec(), parts: parts.clone(), replaces: None, rail: true, extras: vec![] , grading: None, };
            assert_eq!(objects(&[&route]).unwrap().lines().count(), parts.len());
        }
        drop(terrain);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn straight(path: &str) -> CatalogPart {
        CatalogPart {
            path: path.into(),
            family: "Rail_TrackE".into(),
            category: 3,
            index: 0,
            road_type_index: u32::MAX,
            model: crate::geometry::filename_model("asf2_25.p3d").unwrap(),
        }
    }
    /// A 25 m straight along +y from (0, 0) and its world centre line.
    fn piece_line(item: &CatalogPart) -> (crate::roads::PlacedPart, Vec<Point>) {
        let at = Port { p: [0., 0.], outward: [0., 1.] };
        let part = crate::roads::place(item, at, false).unwrap();
        let line = item
            .model
            .line
            .iter()
            .map(|q| add(crate::geometry::rotate(*q, -part.rotation.to_radians()), part.position))
            .collect();
        (part, line)
    }
    fn road(points: Vec<Point>) -> Vec<RoadLine> {
        vec![RoadLine { points, half_width: 3., road: 0 }]
    }

    #[test]
    fn crossings_need_a_straight_steep_crossing_that_fits_on_the_piece() {
        let item = straight("Rail_TrackE_25.p3d");
        let (_, line) = piece_line(&item);
        let blocks = || Kit::Blocks { blocks: vec![("Rail_Crossing_Block_10.p3d".into(), 9.06, 0.15)], tile: false };
        // Perpendicular road across the middle: allowed.
        let c = Crossings::with_kit(road(vec![[-20., 12.5], [20., 12.5]]), blocks());
        assert!(c.allows(&item, &line, 3.));
        // Road 3 m from the piece end: the 9 m deck does not fit on this piece.
        let c = Crossings::with_kit(road(vec![[-20., 3.], [20., 3.]]), blocks());
        assert!(!c.allows(&item, &line, 3.));
        // 45° is too oblique.
        let c = Crossings::with_kit(road(vec![[-20., -7.5], [20., 32.5]]), blocks());
        assert!(!c.allows(&item, &line, 3.));
        // Curves never cross roads.
        let curve = CatalogPart { category: 4, ..item.clone() };
        let c = Crossings::with_kit(road(vec![[-20., 12.5], [20., 12.5]]), blocks());
        assert!(!c.allows(&curve, &line, 3.));
        // Running alongside a road inside its corridor is not allowed; away from it is.
        let c = Crossings::with_kit(road(vec![[4., -50.], [4., 50.]]), blocks());
        assert!(!c.allows(&item, &line, 3.));
        let c = Crossings::with_kit(road(vec![[40., -50.], [40., 50.]]), blocks());
        assert!(c.allows(&item, &line, 3.));
        // No deck models: no crossing at all.
        let c = Crossings::with_kit(road(vec![[-20., 12.5], [20., 12.5]]), Kit::None);
        assert!(!c.allows(&item, &line, 3.));
        assert!(Crossings::none().allows(&curve, &line, 3.));
    }

    #[test]
    fn crossing_decks_are_centred_on_the_road_and_old_track_gets_its_crossing_piece() {
        let item = straight("Rail_TrackE_25.p3d");
        let (part, _) = piece_line(&item);
        let catalog = vec![item.clone()];
        let blocks = Kit::Blocks { blocks: vec![("Rail_Crossing_Block_10.p3d".into(), 9.06, 0.15)], tile: false };
        let c = Crossings::with_kit(road(vec![[-20., 12.5], [20., 12.5]]), blocks);
        let mut parts = vec![part.clone()];
        let extras = c.apply(&mut parts, &catalog).unwrap();
        assert_eq!(extras.len(), 1);
        assert_eq!(extras[0].rotation, part.rotation);
        // The deck centre (its origin plus the 0.15 m offset) is on the road.
        let forward = crate::geometry::rotate([0., 1.], -part.rotation.to_radians());
        let centre = add(extras[0].position, mul(forward, 0.15));
        assert!(norm(sub(centre, [0., 12.5])) < 1e-6, "{centre:?}");
        // Small track: 2 m planks tiled over the road.
        let tiles = Kit::Blocks { blocks: vec![("Rail_Crossing_Block_Small.p3d".into(), 2., 0.)], tile: true };
        let c = Crossings::with_kit(road(vec![[-20., 12.5], [20., 12.5]]), tiles);
        let extras = c.apply(&mut parts.clone(), &catalog).unwrap();
        assert_eq!(extras.len(), 4);
        // Old track: the straight becomes the crossing piece, nothing is added.
        let old = CatalogPart { family: "Rail_Track".into(), ..straight("Rail_Track_25.p3d") };
        let passing = CatalogPart { category: 5, ..straight("Rail_Track_Passing_25.p3d") };
        let catalog = vec![old.clone(), passing.clone()];
        let kit = Kit::Piece { from: old.path.clone(), to: passing.path.clone(), length: 25. };
        let c = Crossings::with_kit(road(vec![[-20., 12.5], [20., 12.5]]), kit);
        let mut parts = vec![crate::roads::place(&old, Port { p: [0., 0.], outward: [0., 1.] }, false).unwrap()];
        assert!(c.apply(&mut parts, &catalog).unwrap().is_empty());
        assert_eq!(parts[0].model, passing.path);
        // Exporting again keeps the crossing piece.
        assert!(c.apply(&mut parts, &catalog).is_ok());
        // A crossing on a curve is reported.
        let curve = CatalogPart { category: 4, ..item };
        let c = Crossings::with_kit(road(vec![[-20., 12.5], [20., 12.5]]), Kit::None);
        assert!(c.apply(&mut [part], &[curve]).is_err());
    }

    /// Terrain routing across an oblique road with the DayZ models when P: is mounted.
    #[test]
    fn terrain_routed_tracks_cross_oblique_roads_steeply_with_crossing_parts() {
        use crate::{
            document::{RoutingSettings, WaterSettings},
            routing::{Obstacles, plan},
        };
        use std::sync::atomic::AtomicBool;
        let folder = Path::new(DEFAULT_FOLDER);
        if !folder.exists() {
            return;
        }
        let mut lib = Library::new(folder.into());
        let (catalog, _) = catalog(folder, &mut lib).unwrap();
        let dir = std::env::temp_dir().join(format!("rail-cross-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut text = "ncols 122\nnrows 122\nxllcorner 0\nyllcorner 0\ncellsize 10\n".to_owned();
        for _ in 0..122 * 122 {
            text.push_str("50 ");
        }
        std::fs::write(dir.join("t.asc"), text).unwrap();
        let cancel = AtomicBool::new(false);
        let terrain = crate::terrain::import(&dir.join("t.asc"), &dir, &cancel).unwrap();
        let none = Obstacles::new(&[], &[], &WaterSettings::default(), Some(&terrain), &cancel).unwrap();
        let s = RoutingSettings::rail();
        // A road at 25° to the direct line, running off both edges so it must be crossed.
        let roads = road(vec![[-100., 250.], [1400., 250. + 1500. * 25f64.to_radians().tan()]]);
        let points = [[150., 620.], [1050., 520.]];
        for family in ["Rail_TrackE", "Rail_Track"] {
            let crossings = Crossings::new(roads.clone(), family, &catalog, folder);
            let planned = plan(&terrain, &points, family, &catalog, &s, &none, &crossings, &cancel).unwrap();
            let mut crossed = 0;
            for part in &planned.parts {
                let item = catalog.iter().find(|c| c.path == part.model).unwrap();
                let mut line: Vec<Point> = item.model.line.iter().map(|q| add(crate::geometry::rotate(*q, -part.rotation.to_radians()), part.position)).collect();
                if part.reverse {
                    line.reverse();
                }
                for h in crossings.hits(&line, 0.).0 {
                    crossed += 1;
                    assert!(h.angle >= MIN_CROSSING_ANGLE.to_radians() - 1e-6, "{family}: {}°", h.angle.to_degrees());
                    assert_ne!(item.category, 4, "{family}: crossing on a curve");
                    if family == "Rail_Track" {
                        assert!(part.model.contains("Passing_25"), "{}", part.model);
                    }
                }
            }
            assert_eq!(crossed, 1, "{family}");
            if family == "Rail_TrackE" {
                assert_eq!(planned.extras.len(), 1);
                assert!(planned.extras[0].model.contains("Rail_Crossing_Block"));
            } else {
                assert!(planned.extras.is_empty());
            }
        }
        drop(terrain);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
