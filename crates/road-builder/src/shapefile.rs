//! Sequential ESRI PolyLine / PolyLineZ / PolyLineM geometry reader.
//! Z and M are deliberately ignored: heights in this editor come from ASC.
use crate::geometry::{Point, norm, sub};
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
#[derive(Debug)]
pub struct Line {
    pub record: u32,
    pub part: usize,
    pub points: Vec<Point>,
}
fn word(b: &[u8], p: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        b.get(p..p + 4).context("Niepełny rekord SHP")?.try_into()?,
    ))
}
fn double(b: &[u8], p: usize) -> Result<f64> {
    Ok(f64::from_le_bytes(
        b.get(p..p + 8)
            .context("Niepełne współrzędne SHP")?
            .try_into()?,
    ))
}
pub fn read(path: &Path, offset: Point, cancel: &AtomicBool) -> Result<Vec<Line>> {
    ensure!(
        offset.iter().all(|p| p.is_finite()),
        "Nieprawidłowe przesunięcie SHP"
    );
    let f = File::open(path)?;
    let length = f.metadata()?.len();
    let mut r = BufReader::new(f);
    let mut h = [0; 100];
    r.read_exact(&mut h).context("Niepełny nagłówek SHP")?;
    ensure!(
        u32::from_be_bytes(h[..4].try_into()?) == 9994 && word(&h, 28)? == 1000,
        "Nieprawidłowy nagłówek SHP"
    );
    ensure!(
        u32::from_be_bytes(h[24..28].try_into()?) as u64 * 2 == length,
        "Długość SHP nie zgadza się z nagłówkiem"
    );
    let declared = word(&h, 32)?;
    ensure!(
        [3, 13, 23].contains(&declared),
        "Import dróg wymaga PolyLine, PolyLineZ lub PolyLineM. Typ SHP: {declared}"
    );
    let mut position = 100u64;
    let mut lines = Vec::new();
    while position < length {
        ensure!(!cancel.load(Ordering::Relaxed), "Import SHP anulowany");
        let mut header = [0; 8];
        r.read_exact(&mut header)
            .context("Niepełny nagłówek rekordu SHP")?;
        let record = u32::from_be_bytes(header[..4].try_into()?);
        let size = u32::from_be_bytes(header[4..].try_into()?) as u64 * 2;
        ensure!(
            size >= 4 && size <= length.saturating_sub(position + 8),
            "Nieprawidłowy rozmiar rekordu SHP #{record}"
        );
        let mut b = vec![0; usize::try_from(size)?];
        r.read_exact(&mut b)?;
        position += 8 + size;
        let kind = word(&b, 0)?;
        if kind == 0 {
            ensure!(size == 4, "Nieprawidłowy pusty rekord SHP");
            continue;
        }
        ensure!(
            kind == declared,
            "Rekord #{record} ma typ inny niż nagłówek SHP"
        );
        let parts = word(&b, 36)? as usize;
        let count = word(&b, 40)? as usize;
        ensure!(
            parts > 0 && count >= parts.saturating_mul(2),
            "Nieprawidłowa liczba części lub punktów SHP #{record}"
        );
        let points_start = 44usize
            .checked_add(parts.checked_mul(4).context("Za dużo części SHP")?)
            .context("Rozmiar SHP")?;
        let xy_end = points_start
            .checked_add(count.checked_mul(16).context("Za dużo punktów SHP")?)
            .context("Rozmiar SHP")?;
        let z_end = if kind == 13 {
            xy_end
                .checked_add(16)
                .and_then(|p| p.checked_add(count.checked_mul(8)?))
                .context("Rozmiar SHP Z")?
        } else {
            xy_end
        };
        ensure!(
            b.len() >= z_end,
            "Niepełne współrzędne rekordu SHP #{record}"
        );
        let measure_size = 16usize
            .checked_add(count.checked_mul(8).context("Rozmiar SHP M")?)
            .context("Rozmiar SHP M")?;
        ensure!(
            if kind == 3 {
                b.len() == xy_end
            } else {
                b.len() == z_end || b.len() == z_end + measure_size
            },
            "Nieprawidłowe dane Z/M SHP #{record}"
        );
        let mut starts = Vec::with_capacity(parts + 1);
        for i in 0..parts {
            starts.push(word(&b, 44 + i * 4)? as usize);
        }
        starts.push(count);
        ensure!(
            starts[0] == 0 && starts.windows(2).all(|w| w[0] < w[1] && w[1] <= count),
            "Nieprawidłowe indeksy części SHP #{record}"
        );
        for (part, w) in starts.windows(2).enumerate() {
            ensure!(w[1] - w[0] >= 2, "Część SHP ma mniej niż dwa punkty");
            let mut points = Vec::with_capacity(w[1] - w[0]);
            for i in w[0]..w[1] {
                if i % 4096 == 0 {
                    ensure!(!cancel.load(Ordering::Relaxed), "Import SHP anulowany");
                }
                let p = [
                    double(&b, points_start + i * 16)? + offset[0],
                    double(&b, points_start + i * 16 + 8)? + offset[1],
                ];
                ensure!(
                    p.iter().all(|v| v.is_finite()),
                    "Nieprawidłowe współrzędne SHP #{record}"
                );
                if points.last().is_none_or(|v| norm(sub(*v, p)) > 1e-8) {
                    points.push(p);
                }
            }
            ensure!(
                points.len() >= 2,
                "Część SHP składa się z powtórzonych punktów"
            );
            lines.push(Line {
                record,
                part,
                points,
            });
        }
    }
    ensure!(!lines.is_empty(), "SHP nie zawiera linii dróg");
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(kind: u32) -> Vec<u8> {
        let mut content = kind.to_le_bytes().to_vec();
        content.extend([0; 32]);
        content.extend(2u32.to_le_bytes());
        content.extend(4u32.to_le_bytes());
        content.extend(0u32.to_le_bytes());
        content.extend(2u32.to_le_bytes());
        for p in [[0f64, 0.], [100., 0.], [20., 30.], [40., 50.]] {
            for v in p {
                content.extend(v.to_le_bytes());
            }
        }
        if kind == 13 {
            content.extend([0; 16 + 4 * 8]);
        }
        if kind == 23 {
            content.extend([0; 16 + 4 * 8]);
        }
        let mut h = vec![0; 100];
        h[..4].copy_from_slice(&9994u32.to_be_bytes());
        h[24..28].copy_from_slice(&((108 + content.len()) as u32 / 2).to_be_bytes());
        h[28..32].copy_from_slice(&1000u32.to_le_bytes());
        h[32..36].copy_from_slice(&kind.to_le_bytes());
        h.extend(1u32.to_be_bytes());
        h.extend((content.len() as u32 / 2).to_be_bytes());
        h.extend(content);
        h
    }
    #[test]
    fn multipart_xy_z_m_offsets_and_truncation() {
        let file = std::env::temp_dir().join(format!("road-shp-{}.shp", std::process::id()));
        for kind in [3, 13, 23] {
            std::fs::write(&file, fixture(kind)).unwrap();
            let lines = read(&file, [200000., 0.], &AtomicBool::new(false)).unwrap();
            assert_eq!(lines.len(), 2);
            assert_eq!(lines[0].points, vec![[200000., 0.], [200100., 0.]]);
            assert_eq!(lines[1].points[0], [200020., 30.]);
            assert_eq!(lines[1].part, 1);
        }
        let mut bad = fixture(3);
        bad.pop();
        std::fs::write(&file, bad).unwrap();
        assert!(read(&file, [0., 0.], &AtomicBool::new(false)).is_err());
        let mut bad = fixture(3);
        bad[156..160].copy_from_slice(&4u32.to_le_bytes());
        std::fs::write(&file, bad).unwrap();
        assert!(read(&file, [0., 0.], &AtomicBool::new(false)).is_err());
        std::fs::remove_file(file).unwrap();
    }
}
