//! Grade MLOD footprints with an optional smooth transition to surrounding terrain.
use crate::{
    geometry::{Point, Shape, add, mul, norm, sub},
    roads::{CatalogPart, PlacedPart},
    terrain::Terrain,
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufWriter, Write},
    path::Path,
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
        ensure!(
            m.source == "MLOD" && !m.triangles.is_empty(),
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
            bounds[0] -= blend_width;
            bounds[1] -= blend_width;
            bounds[2] += blend_width;
            bounds[3] += blend_width;
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
                    let weight = if distance == 0. { 1. }
                        else if blend_width > 0. && distance < blend_width {
                            let t = distance / blend_width;
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
        // x=12.5 is one metre beyond the right edge at x=11.5.
        let old = source.value(12, 21).unwrap();
        let target = 50. + source.point(12, 21)[1];
        assert!((wide.value(12, 21).unwrap() - (old + (target - old) * 0.84375)).abs() < 1e-5);
        assert!(wide.value(12, 21).unwrap() < narrow.value(12, 21).unwrap());
        assert_eq!(narrow.value(13, 21), source.value(13, 21));
        assert_ne!(wide.value(13, 21), source.value(13, 21));
        assert_eq!(wide.value(15, 21), source.value(15, 21));
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
        for y in 0..32 {
            for x in 0..32 {
                let p = terrain.point(x, y);
                if !footprint.triangles.iter().any(|t| inside(p, *t)) {
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
}
