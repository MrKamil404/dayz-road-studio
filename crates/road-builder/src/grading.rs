//! Grade MLOD footprints with an optional smooth transition to surrounding terrain.
use crate::{
    geometry::{Point, Shape, add, mul, norm, sub},
    roads::{CatalogPart, PlacedPart},
    terrain::{Meta, Terrain},
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

struct Surface {
    shape: Shape,
    line: Vec<Point>,
    station: Vec<f64>,
    length: f64,
}
fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn inside(p: Point, t: [Point; 3]) -> bool {
    let area = cross(sub(t[1], t[0]), sub(t[2], t[0]));
    if area.abs() < 1e-10 {
        return false;
    }
    let sign = area.signum();
    (0..3).all(|i| sign * cross(sub(t[(i + 1) % 3], t[i]), sub(p, t[i])) >= -1e-8)
}
fn triangle_distance(p: Point, t: [Point; 3]) -> f64 {
    if inside(p, t) { return 0.; }
    (0..3).map(|i| {
        let a = t[i];
        let d = sub(t[(i + 1) % 3], a);
        let len2 = d[0] * d[0] + d[1] * d[1];
        let rel = sub(p, a);
        let fraction = if len2 > 1e-20 {
            ((rel[0] * d[0] + rel[1] * d[1]) / len2).clamp(0., 1.)
        } else { 0. };
        norm(sub(p, add(a, mul(d, fraction))))
    }).fold(f64::INFINITY, f64::min)
}
fn fraction(p: Point, s: &Surface) -> f64 {
    let mut best = f64::INFINITY;
    let mut station = 0.;
    for (i, w) in s.line.windows(2).enumerate() {
        let d = sub(w[1], w[0]);
        let len = norm(d);
        if len <= 1e-10 {
            continue;
        }
        let t = ((sub(p, w[0])[0] * d[0] + sub(p, w[0])[1] * d[1]) / (len * len)).clamp(0., 1.);
        let distance = norm(sub(p, add(w[0], mul(d, t))));
        if distance < best {
            best = distance;
            station = s.station[i] + len * t;
        }
    }
    (station / s.length).clamp(0., 1.)
}
fn height(a: f64, b: f64, da: f64, db: f64, len: f64, t: f64) -> f64 {
    let t2 = t * t;
    let t3 = t2 * t;
    (2. * t3 - 3. * t2 + 1.) * a
        + (t3 - 2. * t2 + t) * len * da
        + (-2. * t3 + 3. * t2) * b
        + (t3 - t2) * len * db
}

/// A new immutable cache is created; the currently mapped source is never changed.
pub fn apply(
    terrain: &Terrain,
    parts: &[PlacedPart],
    catalog: &[CatalogPart],
    dir: &Path,
    cancel: &AtomicBool,
) -> Result<(Terrain, usize)> {
    apply_with_blend(terrain, parts, catalog, dir, cancel, 0.)
}

pub fn apply_with_blend(
    terrain: &Terrain,
    parts: &[PlacedPart],
    catalog: &[CatalogPart],
    dir: &Path,
    cancel: &AtomicBool,
    blend_width: f64,
) -> Result<(Terrain, usize)> {
    ensure!(blend_width.is_finite() && (0.0..=500.).contains(&blend_width),
        "Nieprawidłowa szerokość wygładzania terenu");
    ensure!(
        !parts.is_empty(),
        "Najpierw wygeneruj segmenty wybranej trasy"
    );
    let mut surfaces: Vec<Surface> = Vec::new();
    let mut heights = Vec::new();
    for p in parts {
        ensure!(!cancel.load(Ordering::Relaxed), "Modyfikacja ASC anulowana");
        let m = &catalog
            .iter()
            .find(|m| m.path == p.model)
            .context("Brak modelu parta w katalogu")?
            .model;
        // Road parts (MLOD) and track pieces measured from ODOL both carry real footprints.
        ensure!(
            (m.source == "MLOD" || m.source == "ODOL") && !m.triangles.is_empty(),
            "Modyfikacja ASC wymaga rzeczywistej geometrii MLOD: {}",
            p.model
        );
        let mut shape = Shape::default();
        shape.place(m, -p.rotation.to_radians(), p.position);
        let mut line = shape.lines.first().context("Brak przebiegu parta")?.clone();
        if p.reverse {
            line.reverse();
        }
        ensure!(line.len() >= 2, "Brak portów parta");
        if let Some(last) = surfaces.last() {
            ensure!(
                norm(sub(*last.line.last().unwrap(), line[0])) < 0.01,
                "Party nie łączą się; przerwano modyfikację ASC"
            );
        }
        if surfaces.is_empty() {
            heights.push(
                terrain
                    .sample(line[0])
                    .context("Brak wysokości ASC na początku drogi")?,
            );
        }
        heights.push(
            terrain
                .sample(*line.last().unwrap())
                .context("Brak wysokości ASC na łączeniu lub końcu drogi")?,
        );
        let mut station = vec![0.];
        for w in line.windows(2) {
            station.push(station.last().unwrap() + norm(sub(w[1], w[0])));
        }
        let length = *station.last().unwrap();
        ensure!(length > 1e-8, "Part o zerowej długości");
        surfaces.push(Surface {
            shape,
            line,
            station,
            length,
        });
    }
    // Shape-preserving cubic Hermite: shared derivatives give a continuous smooth profile.
    let slopes: Vec<_> = surfaces
        .iter()
        .enumerate()
        .map(|(i, s)| (heights[i + 1] - heights[i]) / s.length)
        .collect();
    let mut derivatives = vec![slopes[0]];
    for i in 1..surfaces.len() {
        let (a, b) = (slopes[i - 1], slopes[i]);
        let d = if a * b <= 0. {
            0.
        } else {
            let (l, r) = (surfaces[i - 1].length, surfaces[i].length);
            let (w1, w2) = (2. * r + l, r + 2. * l);
            (w1 + w2) / (w1 / a + w2 / b)
        };
        derivatives.push(d);
    }
    derivatives.push(*slopes.last().unwrap());
    let mut edits: HashMap<usize, (f64, usize, f64)> = HashMap::new();
    let m = &terrain.meta;
    // Terrain heights are interpolated between cell centres, so every cell whose square
    // touches the footprint shapes the road surface and is graded in full. Otherwise a road
    // narrower than the ASC cells leaves a checkerboard of graded and untouched cells.
    let core = m.cell * std::f64::consts::FRAC_1_SQRT_2;
    for (i, s) in surfaces.iter().enumerate() {
        let mut cells: HashMap<usize, (f64, f64)> = HashMap::new();
        for tri in &s.shape.triangles {
            if cross(sub(tri[1], tri[0]), sub(tri[2], tri[0])).abs() < 1e-10 { continue; }
            ensure!(!cancel.load(Ordering::Relaxed), "Modyfikacja ASC anulowana");
            let mut bounds = tri.iter().fold(
                [
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ],
                |mut b, p| {
                    b[0] = b[0].min(p[0]);
                    b[1] = b[1].min(p[1]);
                    b[2] = b[2].max(p[0]);
                    b[3] = b[3].max(p[1]);
                    b
                },
            );
            bounds[0] -= blend_width + core;
            bounds[1] -= blend_width + core;
            bounds[2] += blend_width + core;
            bounds[3] += blend_width + core;
            let x0 = (((bounds[0] - m.east) / m.cell - 0.5).ceil().max(0.) as usize).min(m.cols);
            let x1 = (((bounds[2] - m.east) / m.cell - 0.5).floor() + 1.).max(0.) as usize;
            let y0 = ((m.rows as f64 - (bounds[3] - m.north) / m.cell - 0.5)
                .ceil()
                .max(0.) as usize)
                .min(m.rows);
            let y1 = ((m.rows as f64 - (bounds[1] - m.north) / m.cell - 0.5).floor() + 1.).max(0.)
                as usize;
            for y in y0..y1.min(m.rows) {
                ensure!(!cancel.load(Ordering::Relaxed), "Modyfikacja ASC anulowana");
                for x in x0..x1.min(m.cols) {
                    let p = terrain.point(x, y);
                    let distance = triangle_distance(p, *tri);
                    let weight = if distance <= core { 1. }
                        else if blend_width > 0. && distance < core + blend_width {
                            let t = (distance - core) / blend_width;
                            1. - t * t * (3. - 2. * t)
                        } else { continue; };
                    if terrain.value(x, y).is_some() {
                        let z = height(
                            heights[i],
                            heights[i + 1],
                            derivatives[i],
                            derivatives[i + 1],
                            s.length,
                            fraction(p, s),
                        );
                        ensure!(
                            z.is_finite() && z.abs() <= f32::MAX as f64,
                            "Wysokość poza zakresem ASC"
                        );
                        let cell = cells.entry(y * m.cols + x).or_insert((z, weight));
                        if weight > cell.1 { *cell = (z, weight); }
                    }
                }
            }
        }
        // Full grading under a road takes priority over neighbouring transition bands.
        // Equally strong overlapping surfaces retain the deterministic mean.
        for (cell, (z, weight)) in cells {
            let e = edits.entry(cell).or_insert((0., 0, weight));
            if weight > e.2 { *e = (z, 1, weight); }
            else if weight == e.2 { e.0 += z; e.1 += 1; }
        }
    }
    ensure!(
        !edits.is_empty(),
        "Brak komórek ASC wewnątrz partów. Sprawdź zasięg i rozdzielczość ASC"
    );
    std::fs::create_dir_all(dir)?;
    let mut out = BufWriter::new(File::create(dir.join("height.f32"))?);
    let mut changed = 0;
    for y in 0..m.rows {
        ensure!(!cancel.load(Ordering::Relaxed), "Modyfikacja ASC anulowana");
        for x in 0..m.cols {
            let old = terrain.value(x, y).map(|z| z as f32).unwrap_or(f32::NAN);
            let z = edits
                .get(&(y * m.cols + x))
                .map(|(sum, n, weight)| (old as f64 + (sum / *n as f64 - old as f64) * weight) as f32)
                .unwrap_or(old);
            if old.is_finite() && old.to_bits() != z.to_bits() {
                changed += 1;
            }
            out.write_all(&z.to_le_bytes())?;
        }
    }
    out.flush()?;
    drop(out);
    ensure!(!cancel.load(Ordering::Relaxed), "Modyfikacja ASC anulowana");
    crate::storage::atomic_write(&dir.join("terrain.json"), &serde_json::to_vec(m)?)?;
    Ok((Terrain::open(dir)?, changed))
}

/// Limits of a designed road or track profile and the slopes of its embankments and cuts.
#[derive(Clone, Copy, Debug)]
pub struct Earthwork {
    /// Maximum longitudinal grade, %.
    pub max_grade: f64,
    /// How far the road may lie below (cut) or above (fill) the terrain, m.
    pub max_cut: f64,
    pub max_fill: f64,
    /// Side slopes of embankments and cuts: horizontal metres per vertical metre.
    pub side_slope: f64,
}
/// What grading did: changed cells, the deepest fill and cut on the centre line, length.
#[derive(Clone, Copy, Debug, Default)]
pub struct Report {
    pub changed: usize,
    pub fill: f64,
    pub cut: f64,
    pub length: f64,
}

/// One route's temporary terrain adaptation.
#[derive(Clone)]
pub struct Layer {
    pub order: u64,
    pub parts: Vec<PlacedPart>,
    pub blend_width: f64,
    pub earthwork: Earthwork,
}
/// The base terrain with the adaptations laid over it.
pub struct Composed {
    pub dir: PathBuf,
    pub terrain: Terrain,
    pub reports: Vec<(u64, Report)>,
    /// Adaptations that cannot be built, with the reason.
    pub failed: Vec<(u64, String)>,
    /// Adaptations left out for now because their models are not loaded.
    pub skipped: Vec<u64>,
}

const COMPOSITE: &str = "asc-composite-";

/// Changed cells of one adaptation: index and new height.
struct Patch {
    report: Report,
    cells: Vec<(u32, f32)>,
}
impl Patch {
    fn read(path: &Path) -> Option<Self> {
        let b = std::fs::read(path).ok()?;
        let (head, rest) = (b.get(..32)?, b.get(32..)?);
        let f = |i: usize| f64::from_le_bytes(head[i * 8..i * 8 + 8].try_into().unwrap());
        if rest.len() % 8 != 0 || f(0) as usize != rest.len() / 8 {
            return None;
        }
        let cells = rest
            .chunks_exact(8)
            .map(|c| (u32::from_le_bytes(c[..4].try_into().unwrap()), f32::from_le_bytes(c[4..].try_into().unwrap())))
            .collect();
        Some(Self { report: Report { changed: f(0) as usize, fill: f(1), cut: f(2), length: f(3) }, cells })
    }
    fn write(&self, path: &Path) -> Result<()> {
        let mut b = Vec::with_capacity(32 + self.cells.len() * 8);
        for v in [self.cells.len() as f64, self.report.fill, self.report.cut, self.report.length] {
            b.extend(v.to_le_bytes());
        }
        for (i, z) in &self.cells {
            b.extend(i.to_le_bytes());
            b.extend(z.to_le_bytes());
        }
        crate::storage::atomic_write(path, &b)
    }
}

fn write_heights(dir: &Path, meta: &Meta, cells: &[f32]) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut out = BufWriter::new(File::create(dir.join("height.f32"))?);
    for z in cells {
        out.write_all(&z.to_le_bytes())?;
    }
    out.flush()?;
    drop(out);
    crate::storage::atomic_write(&dir.join("terrain.json"), &serde_json::to_vec(meta)?)
}

/// World box a layer can change, generous enough for its embankments and cuts.
fn layer_bounds(layer: &Layer, cell: f64) -> [f64; 4] {
    let e = &layer.earthwork;
    let reach = layer.blend_width + (e.max_cut.max(e.max_fill) * 2. + 20.) * e.side_slope + cell * 3. + 50.;
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in &layer.parts {
        b = [b[0].min(p.position[0]), b[1].min(p.position[1]), b[2].max(p.position[0]), b[3].max(p.position[1])];
    }
    [b[0] - reach, b[1] - reach, b[2] + reach, b[3] + reach]
}

/// Lays route adaptations over the unchanged `base` terrain and returns the result. Each
/// adaptation is cached in `cache` by what determines it (the base, the route, its settings
/// and the earlier adaptations it overlaps), so removing a route reuses every unrelated one.
pub fn compose(base: &Path, meta: &Meta, layers: &[Layer], catalog: &[CatalogPart], cache: &Path, cancel: &AtomicBool) -> Result<Composed> {
    use std::hash::{Hash, Hasher};
    let raw = std::fs::read(base.join("height.f32")).context("Nie można odczytać wysokości ASC")?;
    ensure!(raw.len() == meta.cols * meta.rows * 4, "Uszkodzona pamięć podręczna ASC");
    let mut cells: Vec<f32> = raw.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
    drop(raw);
    std::fs::create_dir_all(cache)?;
    let mut base_key = std::collections::hash_map::DefaultHasher::new();
    base.hash(&mut base_key);
    [meta.east, meta.north, meta.cell].map(f64::to_bits).hash(&mut base_key);
    let base_key = base_key.finish();

    let mut sorted: Vec<&Layer> = layers.iter().collect();
    sorted.sort_by_key(|l| l.order);
    let mut applied: Vec<(u64, [f64; 4])> = Vec::new();
    let (mut reports, mut failed, mut skipped) = (Vec::new(), Vec::new(), Vec::new());
    for layer in sorted {
        ensure!(!cancel.load(Ordering::Relaxed), "Modyfikacja ASC anulowana");
        let bounds = layer_bounds(layer, meta.cell);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        base_key.hash(&mut h);
        for p in &layer.parts {
            p.model.hash(&mut h);
            p.reverse.hash(&mut h);
            [p.position[0], p.position[1], p.rotation].map(f64::to_bits).hash(&mut h);
        }
        let e = &layer.earthwork;
        [layer.blend_width, e.max_grade, e.max_cut, e.max_fill, e.side_slope].map(f64::to_bits).hash(&mut h);
        for (key, b) in &applied {
            if b[0] <= bounds[2] && bounds[0] <= b[2] && b[1] <= bounds[3] && bounds[1] <= b[3] {
                key.hash(&mut h);
            }
        }
        let key = h.finish();
        let file = cache.join(format!("asc-patch-{key:016x}.bin"));
        let patch = match Patch::read(&file) {
            Some(p) => p,
            None => {
                if !layer.parts.iter().all(|p| catalog.iter().any(|c| c.path == p.model)) {
                    skipped.push(layer.order);
                    continue;
                }
                let work = cache.join(format!("asc-work-{key:016x}-{}", std::process::id()));
                write_heights(&work, meta, &cells)?;
                let result = (|| -> Result<Patch> {
                    let source = Terrain::open(&work)?;
                    let out = work.join("out");
                    let (graded, report) = apply_earthwork(&source, &layer.parts, catalog, &out, cancel, layer.blend_width, e)?;
                    drop((graded, source));
                    let new = std::fs::read(out.join("height.f32"))?;
                    let changed: Vec<(u32, f32)> = new
                        .chunks_exact(4)
                        .enumerate()
                        .filter_map(|(i, c)| {
                            let z = f32::from_le_bytes(c.try_into().unwrap());
                            (z.to_bits() != cells[i].to_bits()).then_some((i as u32, z))
                        })
                        .collect();
                    Ok(Patch { report: Report { changed: changed.len(), ..report }, cells: changed })
                })();
                let _ = std::fs::remove_dir_all(&work);
                match result {
                    Ok(p) => {
                        p.write(&file)?;
                        p
                    }
                    Err(e) => {
                        ensure!(!cancel.load(Ordering::Relaxed), "Modyfikacja ASC anulowana");
                        failed.push((layer.order, format!("{e:#}")));
                        continue;
                    }
                }
            }
        };
        for (i, z) in &patch.cells {
            if let Some(c) = cells.get_mut(*i as usize) {
                *c = *z;
            }
        }
        reports.push((layer.order, patch.report));
        applied.push((key, bounds));
    }

    // Nothing applied: the untouched base is the result.
    if applied.is_empty() {
        let mut terrain = Terrain::open(base)?;
        terrain.meta = meta.clone();
        return Ok(Composed { dir: base.to_owned(), terrain, reports, failed, skipped });
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    base_key.hash(&mut h);
    for (key, _) in &applied {
        key.hash(&mut h);
    }
    let dir = cache.join(format!("{COMPOSITE}{:016x}", h.finish()));
    if !dir.join("terrain.json").is_file() {
        let tmp = cache.join(format!("asc-tmp-{:016x}-{}", h.finish(), std::process::id()));
        write_heights(&tmp, meta, &cells)?;
        if std::fs::rename(&tmp, &dir).is_err() {
            let _ = std::fs::remove_dir_all(&tmp);
        }
    }
    // Earlier composites are rebuilt from their patches on demand; open ones stay locked.
    if let Ok(entries) = std::fs::read_dir(cache) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path != dir && entry.file_name().to_string_lossy().starts_with(COMPOSITE) {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }
    let mut terrain = Terrain::open(&dir)?;
    terrain.meta = meta.clone();
    Ok(Composed { dir, terrain, reports, failed, skipped })
}

/// Longitudinal profile along samples `z` spaced by `step` metres: stays within
/// `[z − cut, z + fill]`, never exceeds the grade and is rounded into gentle vertical curves
/// where the band allows it. Fails when no such profile exists.
pub fn design_profile(z: &[f64], step: &[f64], e: &Earthwork) -> Result<Vec<f64>> {
    ensure!(z.len() == step.len() && !z.is_empty(), "Brak profilu drogi");
    let n = z.len();
    let g = e.max_grade / 100.;
    let d = |i: usize| g * step[i] + 1e-9;
    // Heights reachable from the start within the band and the grade...
    let mut lo: Vec<f64> = z.iter().map(|z| z - e.max_cut).collect();
    let mut hi: Vec<f64> = z.iter().map(|z| z + e.max_fill).collect();
    for i in 1..n {
        lo[i] = lo[i].max(lo[i - 1] - d(i));
        hi[i] = hi[i].min(hi[i - 1] + d(i));
        // Routing samples the terrain more sparsely; centimetres of difference are allowed.
        if lo[i] > hi[i] && lo[i] - hi[i] < 0.05 {
            let m = (lo[i] + hi[i]) / 2.;
            (lo[i], hi[i]) = (m, m);
        }
        if lo[i] > hi[i] {
            bail!(
                "Nie da się poprowadzić niwelety w granicach wykopu i nasypu: brakuje {:.2} m. Zwiększ wykop lub nasyp maks. albo spadek maks.",
                lo[i] - hi[i]
            );
        }
    }
    // ...from which the end can still be reached.
    for i in (0..n - 1).rev() {
        lo[i] = lo[i].max(lo[i + 1] - d(i + 1));
        hi[i] = hi[i].min(hi[i + 1] + d(i + 1));
    }
    // Pick each height as close to the target as the previous one allows; always feasible.
    let pick = |target: &[f64]| -> Vec<f64> {
        let mut h = vec![0.; n];
        h[0] = target[0].clamp(lo[0], hi[0]);
        for i in 1..n {
            let (a, b) = (lo[i].max(h[i - 1] - d(i)), hi[i].min(h[i - 1] + d(i)));
            h[i] = target[i].clamp(a, b.max(a));
        }
        h
    };
    let mut h = pick(z);
    if e.max_cut + e.max_fill > 1e-9 {
        // Round the profile into vertical curves within the band.
        for _ in 0..30 {
            let mut target = h.clone();
            for _ in 0..3 {
                let prev = target.clone();
                for i in 1..n - 1 {
                    target[i] = 0.25 * prev[i - 1] + 0.5 * prev[i] + 0.25 * prev[i + 1];
                }
            }
            h = pick(&target);
        }
    }
    Ok(h)
}

/// Grade the terrain under a route along a designed profile. The footprint (and every
/// cell whose square touches it) is set to the profile; beyond it embankments and cuts
/// with `side_slope` meet the terrain, and `blend_width` softens the shoulders. Each cell
/// takes the height of the nearest part of the route. A new cache is written.
pub fn apply_earthwork(
    terrain: &Terrain,
    parts: &[PlacedPart],
    catalog: &[CatalogPart],
    dir: &Path,
    cancel: &AtomicBool,
    blend_width: f64,
    e: &Earthwork,
) -> Result<(Terrain, Report)> {
    ensure!(blend_width.is_finite() && (0.0..=500.).contains(&blend_width),
        "Nieprawidłowa szerokość wygładzania terenu");
    ensure!(
        [e.max_grade, e.max_cut, e.max_fill, e.side_slope].iter().all(|v| v.is_finite())
            && e.max_grade > 0.
            && e.max_cut >= 0.
            && e.max_fill >= 0.
            && e.side_slope > 0.,
        "Nieprawidłowe ustawienia robót ziemnych"
    );
    ensure!(!parts.is_empty(), "Najpierw wygeneruj segmenty wybranej trasy");
    let m = &terrain.meta;
    // Footprint triangles and the centre line of the whole route.
    let mut triangles: Vec<[Point; 3]> = Vec::new();
    let mut line: Vec<Point> = Vec::new();
    for p in parts {
        ensure!(!cancel.load(Ordering::Relaxed), "Modyfikacja ASC anulowana");
        let model = &catalog
            .iter()
            .find(|c| c.path == p.model)
            .context("Brak modelu parta w katalogu")?
            .model;
        ensure!(
            (model.source == "MLOD" || model.source == "ODOL") && !model.triangles.is_empty(),
            "Modyfikacja ASC wymaga rzeczywistej geometrii MLOD: {}",
            p.model
        );
        let mut shape = Shape::default();
        shape.place(model, -p.rotation.to_radians(), p.position);
        triangles.extend(shape.triangles.iter().filter(|t| cross(sub(t[1], t[0]), sub(t[2], t[0])).abs() > 1e-10));
        let mut part_line = shape.lines.first().context("Brak przebiegu parta")?.clone();
        if p.reverse {
            part_line.reverse();
        }
        for q in part_line {
            if line.last().is_none_or(|l| norm(sub(*l, q)) > 1e-6) {
                line.push(q);
            }
        }
    }
    ensure!(line.len() >= 2, "Brak przebiegu trasy");
    // Profile samples along the centre line.
    let spacing = (m.cell / 2.).clamp(0.5, 2.5);
    let mut samples: Vec<Point> = vec![line[0]];
    let mut step = vec![0.];
    for w in line.windows(2) {
        let len = norm(sub(w[1], w[0]));
        let n = (len / spacing).ceil().max(1.) as usize;
        for i in 1..=n {
            let f = i as f64 / n as f64;
            samples.push([w[0][0] + (w[1][0] - w[0][0]) * f, w[0][1] + (w[1][1] - w[0][1]) * f]);
            step.push(len / n as f64);
        }
    }
    let z: Vec<f64> = samples
        .iter()
        .map(|p| terrain.sample(*p).context("Brak wysokości ASC pod trasą"))
        .collect::<Result<_>>()?;
    let h = design_profile(&z, &step, e)?;
    let mut report = Report {
        fill: (0..h.len()).map(|i| h[i] - z[i]).fold(0., f64::max),
        cut: (0..h.len()).map(|i| z[i] - h[i]).fold(0., f64::max),
        length: step.iter().sum(),
        ..Default::default()
    };
    // Spatial bins of triangles and profile segments.
    let bin = 16.;
    let key = |p: Point| ((p[0] / bin).floor() as i64, (p[1] / bin).floor() as i64);
    let mut tri_bins: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, t) in triangles.iter().enumerate() {
        let (lo, hi) = t.iter().fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(lo, hi), p| {
            ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])
        });
        let (a, b) = (key(lo), key(hi));
        for x in a.0..=b.0 {
            for y in a.1..=b.1 {
                tri_bins.entry((x, y)).or_default().push(i);
            }
        }
    }
    let mut seg_bins: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for i in 1..samples.len() {
        let (a, b) = (key(samples[i - 1]), key(samples[i]));
        for x in a.0.min(b.0)..=a.0.max(b.0) {
            for y in a.1.min(b.1)..=a.1.max(b.1) {
                seg_bins.entry((x, y)).or_default().push(i);
            }
        }
    }
    let core = m.cell * std::f64::consts::FRAC_1_SQRT_2;
    let depth = (e.max_cut.max(e.max_fill) + (0..h.len()).map(|i| (h[i] - z[i]).abs()).fold(0., f64::max)).max(3.);
    let reach = core + blend_width + depth * e.side_slope + m.cell;
    let rings = (reach / bin).ceil() as i64 + 1;
    // Nearest footprint distance and profile height at a point.
    let surface = |p: Point| -> Option<(f64, f64)> {
        let c = key(p);
        let mut d = f64::INFINITY;
        let mut best = (f64::INFINITY, 0usize, 0.);
        for x in c.0 - rings..=c.0 + rings {
            for y in c.1 - rings..=c.1 + rings {
                if let Some(v) = tri_bins.get(&(x, y)) {
                    for &i in v {
                        d = d.min(triangle_distance(p, triangles[i]));
                    }
                }
                if let Some(v) = seg_bins.get(&(x, y)) {
                    for &i in v {
                        let (a, b) = (samples[i - 1], samples[i]);
                        let ab = sub(b, a);
                        let l2 = ab[0] * ab[0] + ab[1] * ab[1];
                        let t = if l2 > 1e-12 { ((sub(p, a)[0] * ab[0] + sub(p, a)[1] * ab[1]) / l2).clamp(0., 1.) } else { 0. };
                        let dist = norm(sub(p, add(a, mul(ab, t))));
                        if dist < best.0 {
                            best = (dist, i, t);
                        }
                    }
                }
            }
        }
        (d.is_finite() && best.0.is_finite() && d <= reach)
            .then(|| (d, h[best.1 - 1] + (h[best.1] - h[best.1 - 1]) * best.2))
    };
    // Candidate cells: the corridor around the footprint.
    let mut edits: HashMap<usize, f64> = HashMap::new();
    let (lo, hi) = triangles.iter().flatten().fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(lo, hi), p| {
        ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])
    });
    let x0 = (((lo[0] - reach - m.east) / m.cell - 0.5).floor().max(0.) as usize).min(m.cols);
    let x1 = ((((hi[0] + reach - m.east) / m.cell - 0.5).ceil() + 1.).max(0.) as usize).min(m.cols);
    let y0 = (((m.rows as f64 - (hi[1] + reach - m.north) / m.cell - 0.5).floor()).max(0.) as usize).min(m.rows);
    let y1 = (((m.rows as f64 - (lo[1] - reach - m.north) / m.cell - 0.5).ceil() + 1.).max(0.) as usize).min(m.rows);
    for y in y0..y1 {
        ensure!(!cancel.load(Ordering::Relaxed), "Modyfikacja ASC anulowana");
        for x in x0..x1 {
            let Some(old) = terrain.value(x, y) else { continue };
            let p = terrain.point(x, y);
            let Some((d, road)) = surface(p) else { continue };
            let new = if d <= core {
                road
            } else {
                let out = d - core;
                // Embankment where the road is higher, cut where it is lower.
                let side = if road > old {
                    (road - out / e.side_slope).max(old)
                } else {
                    (road + out / e.side_slope).min(old)
                };
                if blend_width > 0. && out < blend_width {
                    let t = out / blend_width;
                    side + (road - side) * (1. - t * t * (3. - 2. * t))
                } else {
                    side
                }
            };
            ensure!(new.is_finite() && new.abs() <= f32::MAX as f64, "Wysokość poza zakresem ASC");
            if (new - old).abs() > 1e-9 {
                edits.insert(y * m.cols + x, new);
            }
        }
    }
    ensure!(!edits.is_empty(), "Brak komórek ASC wewnątrz partów. Sprawdź zasięg i rozdzielczość ASC");
    std::fs::create_dir_all(dir)?;
    let mut out = BufWriter::new(File::create(dir.join("height.f32"))?);
    for y in 0..m.rows {
        ensure!(!cancel.load(Ordering::Relaxed), "Modyfikacja ASC anulowana");
        for x in 0..m.cols {
            let old = terrain.value(x, y).map(|z| z as f32).unwrap_or(f32::NAN);
            let z = edits.get(&(y * m.cols + x)).map(|z| *z as f32).unwrap_or(old);
            if old.is_finite() && old.to_bits() != z.to_bits() {
                report.changed += 1;
            }
            out.write_all(&z.to_le_bytes())?;
        }
    }
    out.flush()?;
    drop(out);
    ensure!(!cancel.load(Ordering::Relaxed), "Modyfikacja ASC anulowana");
    crate::storage::atomic_write(&dir.join("terrain.json"), &serde_json::to_vec(m)?)?;
    Ok((Terrain::open(dir)?, report))
}

pub fn export(terrain: &Terrain, path: &Path, cancel: &AtomicBool) -> Result<()> {
    let mut minimum = f64::INFINITY;
    for y in 0..terrain.meta.rows {
        ensure!(!cancel.load(Ordering::Relaxed), "Eksport ASC anulowany");
        for x in 0..terrain.meta.cols {
            if let Some(z) = terrain.value(x, y) {
                minimum = minimum.min(z);
            }
        }
    }
    let nodata = if minimum <= -9999. {
        minimum - (minimum.abs() * 1e-6).max(1.)
    } else {
        -9999.
    };
    crate::storage::atomic_stream(path, |file| {
        let mut out = BufWriter::new(file);
        let m = &terrain.meta;
        writeln!(
            out,
            "ncols {}\nnrows {}\nxllcorner {}\nyllcorner {}\ncellsize {}\nNODATA_value {}",
            m.cols, m.rows, m.east, m.north, m.cell, nodata
        )?;
        for y in 0..m.rows {
            ensure!(!cancel.load(Ordering::Relaxed), "Eksport ASC anulowany");
            for x in 0..m.cols {
                if x > 0 {
                    write!(out, " ")?;
                }
                if let Some(z) = terrain.value(x, y) {
                    write!(out, "{}", z as f32)?;
                } else {
                    write!(out, "{nodata}")?;
                }
            }
            writeln!(out)?;
        }
        ensure!(!cancel.load(Ordering::Relaxed), "Eksport ASC anulowany");
        out.flush()?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        geometry::Model,
        terrain::{Meta, import},
    };
    fn fixture(dir: &Path) -> Terrain {
        std::fs::create_dir_all(dir).unwrap();
        let meta = Meta {
            cols: 32,
            rows: 32,
            east: 0.,
            north: 0.,
            cell: 1.,
        };
        let mut bytes = Vec::new();
        for y in 0..32 {
            for x in 0..32 {
                let v = if (x, y) == (6, 10) {
                    f32::NAN
                } else if (x, y) == (0, 0) {
                    -9999.
                } else {
                    (50. + 31.5 - y as f64 + (x as f64 - 8.).powi(2)) as f32
                };
                bytes.extend(v.to_le_bytes());
            }
        }
        std::fs::write(dir.join("height.f32"), bytes).unwrap();
        std::fs::write(dir.join("terrain.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
        Terrain::open(dir).unwrap()
    }
    fn catalog() -> Vec<CatalogPart> {
        // Deliberately disconnected visual bands: a centreline buffer would fill the gap.
        let mut triangles = Vec::new();
        for (a, b) in [(0., 3.), (5., 8.)] {
            triangles.extend([[[-3., a], [3., a], [3., b]], [[-3., a], [3., b], [-3., b]]]);
        }
        vec![CatalogPart {
            path: "fixture.p3d".into(),
            family: "test".into(),
            category: 3,
            index: 0,
            road_type_index: 0,
            model: Model {
                file: None,
                triangles,
                ports: [None; 4],
                line: vec![[0., 0.], [0., 8.]],
                length: 8.,
                source: "MLOD".into(),
            },
        }]
    }
    #[test]
    fn blend_width_controls_transition_and_preserves_outer_terrain() {
        let root = std::env::temp_dir().join(format!("road-grade-blend-{}", std::process::id()));
        let source = fixture(&root.join("source"));
        let original = std::fs::read(root.join("source/height.f32")).unwrap();
        let parts = vec![PlacedPart {
            model: "fixture.p3d".into(), reverse: false,
            position: [8.5, 8.5], rotation: 0.,
        }];
        let c = catalog();
        let cancel = AtomicBool::new(false);
        let (narrow, _) = apply_with_blend(&source, &parts, &c, &root.join("narrow"), &cancel, 2.).unwrap();
        let (wide, _) = apply_with_blend(&source, &parts, &c, &root.join("wide"), &cancel, 4.).unwrap();
        // x=12.5 is one metre beyond the right edge at x=11.5; the blend starts after the
        // fully graded band of 0.71 cells that shapes the interpolated road surface.
        let core = std::f64::consts::FRAC_1_SQRT_2;
        let weight = |d: f64, width: f64| { let t: f64 = (d - core) / width; 1. - t * t * (3. - 2. * t) };
        let old = source.value(12, 21).unwrap();
        let target = 50. + source.point(12, 21)[1];
        assert!((wide.value(12, 21).unwrap() - (old + (target - old) * weight(1., 4.))).abs() < 1e-4);
        let old = source.value(13, 21).unwrap();
        assert!((narrow.value(13, 21).unwrap() - (old + (50. + source.point(13, 21)[1] - old) * weight(2., 2.))).abs() < 1e-4);
        assert!(wide.value(13, 21).unwrap() < narrow.value(13, 21).unwrap());
        assert_eq!(narrow.value(14, 21), source.value(14, 21));
        assert_ne!(wide.value(15, 21), source.value(15, 21));
        assert_eq!(wide.value(16, 21), source.value(16, 21));
        assert!((wide.value(10, 21).unwrap() - (50. + source.point(10, 21)[1])).abs() < 1e-5);
        assert_eq!(wide.value(6, 10), None);
        assert_eq!(std::fs::read(root.join("source/height.f32")).unwrap(), original);
        for width in [-1., f64::NAN, 501.] {
            assert!(apply_with_blend(&source, &parts, &c, &root.join("invalid"), &cancel, width).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn footprint_gaps_nodata_profile_and_asc_roundtrip() {
        let root = std::env::temp_dir().join(format!("road-grade-{}", std::process::id()));
        let source = fixture(&root.join("source"));
        let original = std::fs::read(root.join("source/height.f32")).unwrap();
        let catalog = catalog();
        let parts = vec![
            PlacedPart {
                model: "fixture.p3d".into(),
                reverse: false,
                position: [8.5, 8.5],
                rotation: 0.,
            },
            PlacedPart {
                model: "fixture.p3d".into(),
                reverse: true,
                position: [8.5, 24.5],
                rotation: 180.,
            },
        ];
        let (graded, count) = apply(
            &source,
            &parts,
            &catalog,
            &root.join("graded"),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(count > 0);
        let mut triangles = Vec::new();
        for part in &parts {
            let mut shape = Shape::default();
            shape.place(
                &catalog[0].model,
                -part.rotation.to_radians(),
                part.position,
            );
            triangles.extend(shape.triangles);
        }
        for y in 0..32 {
            for x in 0..32 {
                let p = source.point(x, y);
                if triangles.iter().any(|t| inside(p, *t)) && source.value(x, y).is_some() {
                    assert!(
                        (graded.value(x, y).unwrap() - (50. + p[1])).abs() < 1e-5,
                        "{p:?}"
                    );
                } else {
                    assert_eq!(source.value(x, y), graded.value(x, y), "{p:?}");
                }
            }
        }
        assert_eq!(source.value(8, 19), graded.value(8, 19)); // gap right on the axis
        assert_eq!(
            original,
            std::fs::read(root.join("source/height.f32")).unwrap()
        );
        let asc = root.join("export.asc");
        export(&graded, &asc, &AtomicBool::new(false)).unwrap();
        let restored = import(&asc, &root.join("restored"), &AtomicBool::new(false)).unwrap();
        for y in 0..32 {
            for x in 0..32 {
                assert_eq!(graded.value(x, y), restored.value(x, y));
            }
        }
        assert_eq!(restored.value(0, 0), Some(-9999.));
        std::fs::write(&asc, "previous").unwrap();
        assert!(export(&graded, &asc, &AtomicBool::new(true)).is_err());
        assert_eq!(std::fs::read_to_string(&asc).unwrap(), "previous");
        drop(restored);
        drop(graded);
        drop(source);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn hermite_ports_are_continuous_and_do_not_overshoot() {
        for i in 0..=100 {
            let t = i as f64 / 100.;
            let z = height(10., 20., 0., 0.2, 50., t);
            assert!((10. - 1e-8..=20. + 1e-8).contains(&z));
        }
        assert_eq!(height(10., 20., 0.1, 0.2, 50., 0.), 10.);
        assert_eq!(height(10., 20., 0.1, 0.2, 50., 1.), 20.);
    }

    #[test]
    fn reversed_curve_and_caps_grade_their_transformed_meshes() {
        let root = std::env::temp_dir().join(format!("road-grade-curve-{}", std::process::id()));
        let terrain = fixture(&root.join("source"));
        let mut catalog = Vec::new();
        for path in ["asf2_6konec.p3d", "asf2_30 25.p3d"] {
            let mut model = crate::geometry::filename_model(path).unwrap();
            model.source = "MLOD".into(); // test mesh; production rejects filename approximations
            catalog.push(CatalogPart {
                path: path.into(),
                family: "asf2".into(),
                category: 3,
                index: 0,
                road_type_index: 0,
                model,
            });
        }
        let mut parts = Vec::new();
        for (path, reverse) in [
            ("asf2_6konec.p3d", false),
            ("asf2_30 25.p3d", true),
            ("asf2_6konec.p3d", false),
        ] {
            crate::roads::append(
                &mut parts,
                &catalog,
                path,
                reverse,
                [16.5, 5.5],
                std::f64::consts::FRAC_PI_2,
            )
            .unwrap();
        }
        let (graded, count) = apply(
            &terrain,
            &parts,
            &catalog,
            &root.join("graded"),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(count > 0);
        let mut footprint = Shape::default();
        for part in &parts {
            footprint.place(
                &catalog.iter().find(|c| c.path == part.model).unwrap().model,
                -part.rotation.to_radians(),
                part.position,
            );
        }
        // Only cells within 0.71 cells of the footprint change.
        for y in 0..32 {
            for x in 0..32 {
                let p = terrain.point(x, y);
                if footprint.triangles.iter().all(|t| triangle_distance(p, *t) > std::f64::consts::FRAC_1_SQRT_2 + 1e-9) {
                    assert_eq!(terrain.value(x, y), graded.value(x, y));
                }
            }
        }
        catalog[0].model.source = "filename".into();
        assert!(
            apply(
                &terrain,
                &parts,
                &catalog,
                &root.join("rejected"),
                &AtomicBool::new(false)
            )
            .is_err()
        );
        drop(graded);
        drop(terrain);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn profile_stays_in_the_cut_and_fill_band_and_respects_the_grade() {
        // A 4 m hump over 30 m: 27 % flanks, far above a 10 % limit.
        let z: Vec<f64> = (0..=100).map(|i| { let x = i as f64 - 50.; 100. + 4. * (-x * x / 60.).exp() }).collect();
        let step: Vec<f64> = (0..=100).map(|i| if i == 0 { 0. } else { 1. }).collect();
        let e = Earthwork { max_grade: 10., max_cut: 3., max_fill: 3., side_slope: 1.5 };
        let h = design_profile(&z, &step, &e).unwrap();
        for i in 0..h.len() {
            assert!(h[i] >= z[i] - 3. - 1e-6 && h[i] <= z[i] + 3. + 1e-6, "{i}: {} vs {}", h[i], z[i]);
            if i > 0 {
                assert!((h[i] - h[i - 1]).abs() <= 0.1 + 1e-6, "{i}");
            }
        }
        // The hump is cut, not followed.
        assert!(z[50] - h[50] > 1.);
        // Without a band the same terrain cannot be graded to 10 %.
        let none = Earthwork { max_cut: 0., max_fill: 0., ..e };
        assert!(design_profile(&z, &step, &none).is_err());
        // Terrain already within the grade stays as it is.
        let gentle: Vec<f64> = (0..=100).map(|i| 100. + i as f64 * 0.05).collect();
        assert_eq!(design_profile(&gentle, &step, &none).unwrap(), gentle);
    }

    /// Flat terrain at 100 m with a 2 m deep, 6 m wide ditch across the road.
    fn ditch(dir: &Path) -> Terrain {
        std::fs::create_dir_all(dir).unwrap();
        let meta = Meta { cols: 64, rows: 64, east: 0., north: 0., cell: 1. };
        let mut bytes = Vec::new();
        for y in 0..64 {
            for x in 0..64 {
                let north = 63.5 - y as f64;
                let z: f64 = if (29. ..35.).contains(&north) && x > 10 && x < 54 { 98. } else { 100. };
                bytes.extend((z as f32).to_le_bytes());
            }
        }
        std::fs::write(dir.join("height.f32"), bytes).unwrap();
        std::fs::write(dir.join("terrain.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
        Terrain::open(dir).unwrap()
    }
    fn strip(source: &str) -> Vec<CatalogPart> {
        // A 4 m wide, 50 m long straight along +y.
        vec![CatalogPart {
            path: "strip.p3d".into(),
            family: "test".into(),
            category: 3,
            index: 0,
            road_type_index: 0,
            model: Model {
                file: None,
                triangles: vec![[[-2., 0.], [2., 0.], [2., 50.]], [[-2., 0.], [2., 50.], [-2., 50.]]],
                ports: [None; 4],
                line: vec![[0., 0.], [0., 50.]],
                length: 50.,
                source: source.into(),
            },
        }]
    }

    #[test]
    fn embankment_crosses_a_ditch_with_side_slopes_and_tracks_grade_too() {
        let root = std::env::temp_dir().join(format!("road-earth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let terrain = ditch(&root.join("source"));
        let parts = vec![PlacedPart { model: "strip.p3d".into(), reverse: false, position: [32., 7.], rotation: 0. }];
        let cancel = AtomicBool::new(false);
        let e = Earthwork { max_grade: 5., max_cut: 0., max_fill: 2.5, side_slope: 2. };
        for source in ["MLOD", "ODOL"] {
            let catalog = strip(source);
            let (graded, report) = apply_earthwork(&terrain, &parts, &catalog, &root.join(source), &cancel, 0., &e).unwrap();
            // The road bridges the ditch on an embankment: about 2 m of fill.
            assert!(report.fill > 1.5 && report.fill <= 2.5 + 1e-6, "{report:?}");
            assert_eq!(report.cut, 0.);
            let (x, y) = (32, 63 - 32);
            assert!(graded.value(x, y).unwrap() > 99.5, "{:?}", graded.value(x, y));
            // Beyond the road the embankment falls 1 m per 2 m until it meets the ditch.
            let core = std::f64::consts::FRAC_1_SQRT_2;
            let road = graded.value(32, y).unwrap();
            for dx in 3..8usize {
                let d = (32. + dx as f64 + 0.5) - 34.;
                let expected = (road - (d - core) / 2.).max(98.);
                assert!((graded.value(32 + dx, y).unwrap() - expected).abs() < 0.05, "{dx}: {:?} vs {expected}", graded.value(32 + dx, y));
            }
            // Far from the road the ditch stays.
            assert_eq!(graded.value(50, y), terrain.value(50, y));
            // The profile keeps the 5 % grade into and out of the embankment.
            for yy in 10..60 {
                let (a, b) = (graded.value(32, yy).unwrap(), graded.value(32, yy + 1).unwrap());
                assert!((a - b).abs() <= 0.05 + 1e-4, "{yy}: {a} {b}");
            }
            drop(graded);
        }
        // Without fill the ditch is too steep for 5 %.
        let flat = Earthwork { max_fill: 0., ..e };
        assert!(apply_earthwork(&terrain, &parts, &strip("MLOD"), &root.join("none"), &cancel, 0., &flat).is_err());
        drop(terrain);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn layers_keep_the_source_cached_and_disappear_with_their_route() {
        let root = std::env::temp_dir().join(format!("road-layers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let base = root.join("source");
        let source = ditch(&base);
        let meta = source.meta.clone();
        let cache = root.join("cache");
        let cancel = AtomicBool::new(false);
        let catalog = strip("ODOL");
        let original = std::fs::read(base.join("height.f32")).unwrap();
        let layer = |order, x: f64, fill| Layer {
            order,
            parts: vec![PlacedPart { model: "strip.p3d".into(), reverse: false, position: [x, 7.], rotation: 0. }],
            blend_width: 0.,
            earthwork: Earthwork { max_grade: 5., max_cut: 0., max_fill: fill, side_slope: 2. },
        };
        let patches = || std::fs::read_dir(&cache).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("asc-patch-")).count();
        let y = 63 - 32;

        let one = compose(&base, &meta, &[layer(1, 32., 2.5)], &catalog, &cache, &cancel).unwrap();
        assert_ne!(one.dir, base);
        assert!(one.terrain.value(32, y).unwrap() > 99.5);
        assert_eq!(one.reports.len(), 1);
        assert!(one.reports[0].1.changed > 10);
        assert_eq!(std::fs::read(base.join("height.f32")).unwrap(), original, "the source ASC is never written");
        assert_eq!(patches(), 1);
        let graded = one.terrain.value(32, y);
        let dir = one.dir.clone();
        drop(one);

        // The same route again comes from the cache, with the same result and report.
        let again = compose(&base, &meta, &[layer(1, 32., 2.5)], &catalog, &cache, &cancel).unwrap();
        assert_eq!((again.dir.clone(), again.terrain.value(32, y)), (dir, graded));
        assert!(again.reports[0].1.fill > 1.5);
        assert_eq!(patches(), 1);
        drop(again);

        // A later overlapping route is built on the earlier one and wins where they meet.
        let two = compose(&base, &meta, &[layer(2, 34., 2.5), layer(1, 32., 2.5)], &catalog, &cache, &cancel).unwrap();
        assert_eq!(two.reports.iter().map(|r| r.0).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(patches(), 2);
        drop(two);
        // Without the earlier route the later one depends on nothing and is rebuilt.
        let alone = compose(&base, &meta, &[layer(2, 34., 2.5)], &catalog, &cache, &cancel).unwrap();
        assert_eq!(patches(), 3);
        // Cells outside the remaining route are back at their source height.
        assert_eq!(alone.terrain.value(20, y), source.value(20, y));
        drop(alone);

        // An impossible route is reported and leaves the source as the result.
        let failed = compose(&base, &meta, &[layer(3, 32., 0.)], &catalog, &cache, &cancel).unwrap();
        assert_eq!(failed.dir, base);
        assert_eq!(failed.failed.len(), 1);
        drop(failed);
        // Models that are not loaded yet leave the layer out without failing it.
        let waiting = compose(&base, &meta, &[layer(4, 30., 2.5)], &[], &cache, &cancel).unwrap();
        assert_eq!((waiting.dir.clone(), waiting.skipped.clone(), waiting.failed.len()), (base.clone(), vec![4], 0));
        drop((waiting, source));
        std::fs::remove_dir_all(root).unwrap();
    }
}
