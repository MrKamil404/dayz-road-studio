use crate::geometry::Point;
use anyhow::{Context, Result, bail, ensure};
use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Meta {
    pub cols: usize,
    pub rows: usize,
    pub east: f64,
    pub north: f64,
    pub cell: f64,
}
pub struct Terrain {
    pub meta: Meta,
    data: Mmap,
}
impl Terrain {
    pub fn open(dir: &Path) -> Result<Self> {
        let meta: Meta = serde_json::from_slice(&std::fs::read(dir.join("terrain.json"))?)?;
        let f = File::open(dir.join("height.f32"))?;
        ensure!(
            meta.cols > 0 && meta.rows > 0 && meta.cell.is_finite() && meta.cell > 0.,
            "Nieprawidłowy teren"
        );
        let size = meta
            .cols
            .checked_mul(meta.rows)
            .and_then(|n| n.checked_mul(4))
            .context("Rozmiar ASC")?;
        ensure!(
            f.metadata()?.len() == size as u64,
            "Uszkodzona pamięć podręczna ASC"
        );
        // The cache is exclusively created before mapping and is never modified while open.
        let data = unsafe { Mmap::map(&f)? };
        Ok(Self { meta, data })
    }
    pub fn value(&self, x: usize, y: usize) -> Option<f64> {
        if x >= self.meta.cols || y >= self.meta.rows {
            return None;
        }
        let p = (y * self.meta.cols + x) * 4;
        let z = f32::from_le_bytes(self.data[p..p + 4].try_into().ok()?) as f64;
        z.is_finite().then_some(z)
    }
    pub fn point(&self, x: usize, y: usize) -> Point {
        [
            self.meta.east + (x as f64 + 0.5) * self.meta.cell,
            self.meta.north + (self.meta.rows as f64 - y as f64 - 0.5) * self.meta.cell,
        ]
    }
    pub fn sample(&self, p: Point) -> Option<f64> {
        let x = (p[0] - self.meta.east) / self.meta.cell - 0.5;
        let y = self.meta.rows as f64 - (p[1] - self.meta.north) / self.meta.cell - 0.5;
        if x < 0. || y < 0. || x > (self.meta.cols - 1) as f64 || y > (self.meta.rows - 1) as f64 {
            return None;
        }
        let (ix, iy) = (x.floor() as usize, y.floor() as usize);
        let (jx, jy) = (
            (ix + 1).min(self.meta.cols - 1),
            (iy + 1).min(self.meta.rows - 1),
        );
        let (a, b, c, d) = (
            self.value(ix, iy)?,
            self.value(jx, iy)?,
            self.value(ix, jy)?,
            self.value(jx, jy)?,
        );
        Some(
            (a * (1. - x.fract()) + b * x.fract()) * (1. - y.fract())
                + (c * (1. - x.fract()) + d * x.fract()) * y.fract(),
        )
    }
    /// Marching triangles resolves saddle cells consistently. Sample only the visible grid.
    pub fn contours(&self, bounds: [f64; 4], interval: f64) -> Vec<(Point, Point)> {
        if !interval.is_finite() || interval <= 0. {
            return vec![];
        }
        let m = &self.meta;
        let x0 = (((bounds[0] - m.east) / m.cell).floor().max(0.) as usize).min(m.cols - 1);
        let x1 = (((bounds[2] - m.east) / m.cell).ceil().max(0.) as usize).min(m.cols - 1);
        let y0 = ((m.rows as f64 - (bounds[3] - m.north) / m.cell)
            .floor()
            .max(0.) as usize)
            .min(m.rows - 1);
        let y1 = ((m.rows as f64 - (bounds[1] - m.north) / m.cell)
            .ceil()
            .max(0.) as usize)
            .min(m.rows - 1);
        let step = ((x1.saturating_sub(x0).max(y1.saturating_sub(y0)) as f64 / 160.).ceil()
            as usize)
            .max(1);
        let mut lines = Vec::new();
        for y in (y0..y1).step_by(step) {
            for x in (x0..x1).step_by(step) {
                let xx = (x + step).min(x1);
                let yy = (y + step).min(y1);
                let pts = [
                    self.point(x, y),
                    self.point(xx, y),
                    self.point(xx, yy),
                    self.point(x, yy),
                ];
                let vals = [
                    self.value(x, y),
                    self.value(xx, y),
                    self.value(xx, yy),
                    self.value(x, yy),
                ];
                for tri in [[0, 1, 2], [0, 2, 3]] {
                    let [Some(a), Some(b), Some(c)] = tri.map(|i| vals[i]) else {
                        continue;
                    };
                    let z = [a, b, c];
                    let p = tri.map(|i| pts[i]);
                    let lo = (a.min(b).min(c) / interval).ceil() as i64;
                    let hi = (a.max(b).max(c) / interval).floor() as i64;
                    // Extremely dense contours are not useful at this zoom.
                    if hi.saturating_sub(lo) > 100 {
                        continue;
                    }
                    for k in lo..=hi {
                        let level = k as f64 * interval;
                        let mut hits = Vec::new();
                        for (i, j) in [(0, 1), (1, 2), (2, 0)] {
                            if (z[i] <= level && z[j] > level) || (z[j] <= level && z[i] > level) {
                                let t = (level - z[i]) / (z[j] - z[i]);
                                hits.push([
                                    p[i][0] + (p[j][0] - p[i][0]) * t,
                                    p[i][1] + (p[j][1] - p[i][1]) * t,
                                ]);
                            }
                        }
                        if hits.len() == 2 {
                            lines.push((hits[0], hits[1]));
                        }
                    }
                }
            }
        }
        lines
    }
}

pub fn import(source: &Path, dir: &Path, cancel: &AtomicBool) -> Result<Terrain> {
    std::fs::create_dir_all(dir)?;
    let mut headers = HashMap::<String, String>::new();
    let mut count = 0usize;
    let mut meta = None;
    let mut no_data = -9999.;
    let mut out = BufWriter::new(File::create(dir.join("height.f32"))?);
    // Lines may wrap arbitrarily. No complete text or grid allocation.
    for line in BufReader::new(File::open(source)?).lines() {
        ensure!(!cancel.load(Ordering::Relaxed), "Import anulowany");
        let line = line?;
        let mut words = line.split_whitespace();
        let Some(first) = words.next() else { continue };
        let key = first.to_ascii_lowercase();
        if meta.is_none()
            && [
                "ncols",
                "nrows",
                "xllcorner",
                "yllcorner",
                "xllcenter",
                "yllcenter",
                "cellsize",
                "nodata_value",
            ]
            .contains(&key.as_str())
        {
            let v = words.next().context("Brak wartości nagłówka ASC")?;
            ensure!(
                words.next().is_none() && headers.insert(key, v.into()).is_none(),
                "Powtórzony lub niepoprawny nagłówek ASC"
            );
            continue;
        }
        if meta.is_none() {
            let get = |k: &str| -> Result<f64> {
                let v: f64 = headers
                    .get(k)
                    .with_context(|| format!("Brak {k}"))?
                    .parse()?;
                ensure!(v.is_finite(), "Nieprawidłowe {k}");
                Ok(v)
            };
            let cols = get("ncols")?;
            let rows = get("nrows")?;
            let cell = get("cellsize")?;
            ensure!(
                cols >= 1. && rows >= 1. && cols.fract() == 0. && rows.fract() == 0. && cell > 0.,
                "Nieprawidłowy rozmiar ASC"
            );
            let corner = |axis: &str| -> Result<f64> {
                let a = format!("{axis}llcorner");
                let b = format!("{axis}llcenter");
                ensure!(
                    !(headers.contains_key(&a) && headers.contains_key(&b)),
                    "Sprzeczny nagłówek ASC"
                );
                if headers.contains_key(&a) {
                    get(&a)
                } else {
                    Ok(get(&b)? - cell / 2.)
                }
            };
            no_data = headers
                .get("nodata_value")
                .map(|s| s.parse())
                .transpose()?
                .unwrap_or(-9999.);
            let m = Meta {
                cols: cols as usize,
                rows: rows as usize,
                cell,
                east: corner("x")?,
                north: corner("y")?,
            };
            m.cols
                .checked_mul(m.rows)
                .and_then(|n| n.checked_mul(4))
                .context("ASC za duży dla tej platformy")?;
            meta = Some(m);
        }
        let m = meta.as_ref().unwrap();
        for word in std::iter::once(first).chain(words) {
            let z: f64 = word
                .parse()
                .with_context(|| format!("Nieprawidłowa wysokość: {word}"))?;
            if count >= m.cols * m.rows {
                bail!("Za dużo komórek ASC");
            }
            let v = if z == no_data || !z.is_finite() {
                f32::NAN
            } else {
                ensure!(z.abs() <= f32::MAX as f64, "Wysokość poza zakresem");
                z as f32
            };
            out.write_all(&v.to_le_bytes())?;
            count += 1;
        }
    }
    let meta = meta.context("Pusty plik ASC")?;
    ensure!(
        count == meta.cols * meta.rows,
        "ASC: oczekiwano {} komórek, odczytano {count}",
        meta.cols * meta.rows
    );
    out.flush()?;
    drop(out);
    crate::storage::atomic_write(&dir.join("terrain.json"), &serde_json::to_vec(&meta)?)?;
    Terrain::open(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn asc_centers_north_and_nodata() {
        let dir = std::env::temp_dir().join(format!("road-asc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("test.asc");
        std::fs::write(&file,"ncols 2\nnrows 2\nxllcenter 100\nyllcenter 200\ncellsize 10\nNODATA_value -9999\n10 20\n30 -9999\n").unwrap();
        let t = import(&file, &dir, &AtomicBool::new(false)).unwrap();
        assert_eq!(t.point(0, 0), [100., 210.]);
        assert_eq!(t.value(1, 1), None);
        assert_eq!(t.value(0, 1), Some(30.));
        assert!(t.sample([105., 205.]).is_none());
        drop(t);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
