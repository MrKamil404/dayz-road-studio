//! Disk-backed RGBA pyramid. Import memory is proportional to a scanline, not image area.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Level {
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Raster {
    pub levels: Vec<Level>,
    pub dir: PathBuf,
}
impl Raster {
    pub fn open(dir: &Path) -> Result<Self> {
        let mut r: Self = serde_json::from_slice(&std::fs::read(dir.join("raster.json"))?)?;
        r.dir = dir.into();
        ensure!(!r.levels.is_empty(), "Pusta piramida rastra");
        for (i, l) in r.levels.iter().enumerate() {
            ensure!(
                l.width > 0
                    && l.height > 0
                    && std::fs::metadata(r.path(i))?.len() == l.width as u64 * l.height as u64 * 4,
                "Uszkodzona warstwa rastra"
            );
        }
        Ok(r)
    }
    pub fn path(&self, l: usize) -> PathBuf {
        self.dir.join(format!("level-{l}.rgba"))
    }
    pub fn tile(&self, level: usize, x: u32, y: u32) -> Result<(u32, u32, Vec<u8>)> {
        let l = self.levels.get(level).context("Brak poziomu rastra")?;
        ensure!(x < l.width && y < l.height, "Kafelek poza obrazem");
        let w = 256.min(l.width - x);
        let h = 256.min(l.height - y);
        let mut data = vec![0; w as usize * h as usize * 4];
        let mut f = File::open(self.path(level))?;
        for row in 0..h {
            f.seek(SeekFrom::Start(
                ((y + row) as u64 * l.width as u64 + x as u64) * 4,
            ))?;
            f.read_exact(
                &mut data[row as usize * w as usize * 4..(row + 1) as usize * w as usize * 4],
            )?;
        }
        Ok((w, h, data))
    }
}
fn rgba(row: &[u8], color: png::ColorType) -> Result<Vec<u8>> {
    use png::ColorType::*;
    let n = match color {
        Rgb => 3,
        Rgba => 4,
        Grayscale => 1,
        GrayscaleAlpha => 2,
        Indexed => bail!("Nie rozwinięto palety PNG"),
    };
    let mut out = Vec::with_capacity(row.len() / n * 4);
    for p in row.chunks_exact(n) {
        match color {
            Rgb => out.extend([p[0], p[1], p[2], 255]),
            Rgba => out.extend(p),
            Grayscale => out.extend([p[0], p[0], p[0], 255]),
            GrayscaleAlpha => out.extend([p[0], p[0], p[0], p[1]]),
            _ => unreachable!(),
        }
    }
    Ok(out)
}
fn png_import(source: &Path, out: &Path, cancel: &AtomicBool) -> Result<Level> {
    let mut decoder = png::Decoder::new(BufReader::new(File::open(source)?));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    decoder.set_limits(png::Limits { bytes: usize::MAX });
    let mut reader = decoder.read_info()?;
    let l = Level {
        width: reader.info().width,
        height: reader.info().height,
    };
    let color = reader.output_color_type().0;
    let mut f = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(out)?;
    f.set_len(l.width as u64 * l.height as u64 * 4)?;
    if !reader.info().interlaced {
        let mut writer = BufWriter::new(f);
        let mut rows = 0;
        while let Some(row) = reader.next_row()? {
            ensure!(!cancel.load(Ordering::Relaxed), "Import anulowany");
            writer.write_all(&rgba(row.data(), color)?)?;
            rows += 1;
        }
        ensure!(rows == l.height, "Niepełny obraz PNG");
        writer.flush()?;
    } else {
        // Adam7 has fixed pass ordering. Assemble each pass scanline on disk.
        for (sx, sy, dx, dy) in [
            (0, 0, 8, 8),
            (4, 0, 8, 8),
            (0, 4, 4, 8),
            (2, 0, 4, 4),
            (0, 2, 2, 4),
            (1, 0, 2, 2),
            (0, 1, 1, 2),
        ] {
            if sx >= l.width || sy >= l.height {
                continue;
            }
            for y in (sy..l.height).step_by(dy as usize) {
                ensure!(!cancel.load(Ordering::Relaxed), "Import anulowany");
                let row = reader.next_row()?.context("Niepełny PNG Adam7")?;
                let pixels = rgba(row.data(), color)?;
                let mut full = vec![0; l.width as usize * 4];
                let offset = y as u64 * l.width as u64 * 4;
                f.seek(SeekFrom::Start(offset))?;
                f.read_exact(&mut full)?;
                ensure!(
                    pixels.len() / 4 == ((l.width - sx).div_ceil(dx)) as usize,
                    "Niepoprawny wiersz Adam7"
                );
                for (i, x) in (sx..l.width).step_by(dx as usize).enumerate() {
                    full[x as usize * 4..x as usize * 4 + 4]
                        .copy_from_slice(&pixels[i * 4..i * 4 + 4]);
                }
                f.seek(SeekFrom::Start(offset))?;
                f.write_all(&full)?;
            }
        }
        ensure!(reader.next_row()?.is_none(), "Nadmiarowe wiersze PNG");
        f.sync_all()?;
    }
    reader.finish()?;
    Ok(l)
}
fn bmp_import(source: &Path, out: &Path, cancel: &AtomicBool) -> Result<Level> {
    let mut f = BufReader::new(File::open(source)?);
    let mut h = [0; 54];
    f.read_exact(&mut h)?;
    ensure!(&h[0..2] == b"BM", "Nieprawidłowy BMP");
    let u16at = |p| u16::from_le_bytes(h[p..p + 2].try_into().unwrap());
    let u32at = |p| u32::from_le_bytes(h[p..p + 4].try_into().unwrap());
    let offset = u32at(10);
    let dib = u32at(14);
    let width = u32at(18) as i32;
    let height = u32at(22) as i32;
    let bits = u16at(28);
    let compression = u32at(30);
    ensure!(
        dib >= 40 && width > 0 && height != 0 && height != i32::MIN && u16at(26) == 1,
        "Nieobsługiwany nagłówek BMP"
    );
    ensure!(
        compression == 0 && [8, 24, 32].contains(&bits),
        "BMP: obsługiwane nieskompresowane 8/24/32 bit. Zapisz inne BMP jako PNG"
    );
    let l = Level {
        width: width as u32,
        height: height.unsigned_abs(),
    };
    let mut palette = vec![];
    if bits == 8 {
        let n = if u32at(46) == 0 { 256 } else { u32at(46) };
        ensure!(n <= 256, "Nieprawidłowa paleta BMP");
        f.seek(SeekFrom::Start(14 + dib as u64))?;
        for _ in 0..n {
            let mut p = [0; 4];
            f.read_exact(&mut p)?;
            palette.push([p[2], p[1], p[0], 255]);
        }
    }
    let stride = (l.width as u64 * bits as u64).div_ceil(32) * 4;
    ensure!(
        offset as u64 >= 14 + dib as u64 + palette.len() as u64 * 4
            && offset as u64 + stride * l.height as u64 <= f.get_ref().metadata()?.len(),
        "Niepełny BMP"
    );
    let mut row = vec![0; usize::try_from(stride)?];
    let mut writer = BufWriter::new(File::create(out)?);
    for y in 0..l.height {
        ensure!(!cancel.load(Ordering::Relaxed), "Import anulowany");
        let source_y = if height > 0 { l.height - 1 - y } else { y };
        f.seek(SeekFrom::Start(offset as u64 + source_y as u64 * stride))?;
        f.read_exact(&mut row)?;
        let mut rgba = Vec::with_capacity(l.width as usize * 4);
        for x in 0..l.width as usize {
            if bits == 8 {
                rgba.extend(palette.get(row[x] as usize).context("Indeks palety BMP")?);
            } else {
                let i = x * (bits / 8) as usize;
                rgba.extend([row[i + 2], row[i + 1], row[i], 255]);
            }
        }
        writer.write_all(&rgba)?;
    }
    writer.flush()?;
    Ok(l)
}
pub fn import(source: &Path, dir: &Path, cancel: &AtomicBool) -> Result<Raster> {
    std::fs::create_dir_all(dir)?;
    let mut r = Raster {
        levels: vec![],
        dir: dir.into(),
    };
    let mut magic = [0; 8];
    File::open(source)?.read_exact(&mut magic)?;
    let l = if magic == *b"\x89PNG\r\n\x1a\n" {
        png_import(source, &r.path(0), cancel)?
    } else if &magic[..2] == b"BM" {
        bmp_import(source, &r.path(0), cancel)?
    } else {
        bail!("Wybierz BMP lub PNG");
    };
    r.levels.push(l);
    loop {
        let l = r.levels.last().unwrap();
        if l.width <= 256 && l.height <= 256 {
            break;
        }
        let next = Level {
            width: l.width.div_ceil(2),
            height: l.height.div_ceil(2),
        };
        let mut src = BufReader::new(File::open(r.path(r.levels.len() - 1))?);
        let mut dst = BufWriter::new(File::create(r.path(r.levels.len()))?);
        let mut a = vec![0; l.width as usize * 4];
        let mut b = a.clone();
        let mut row = vec![0; next.width as usize * 4];
        for y in 0..next.height {
            ensure!(!cancel.load(Ordering::Relaxed), "Import anulowany");
            src.read_exact(&mut a)?;
            if y * 2 + 1 < l.height {
                src.read_exact(&mut b)?;
            } else {
                b.copy_from_slice(&a);
            }
            for x in 0..next.width as usize {
                let i = x * 2 * 4;
                let j = ((x * 2 + 1).min(l.width as usize - 1)) * 4;
                for c in 0..4 {
                    row[x * 4 + c] =
                        ((a[i + c] as u16 + a[j + c] as u16 + b[i + c] as u16 + b[j + c] as u16)
                            / 4) as u8;
                }
            }
            dst.write_all(&row)?;
        }
        dst.flush()?;
        r.levels.push(next);
    }
    crate::storage::atomic_write(&dir.join("raster.json"), &serde_json::to_vec(&r)?)?;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn png_pyramid_orientation_odd_dimensions() {
        let dir = std::env::temp_dir().join(format!("road-raster-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("test.png");
        let mut e = png::Encoder::new(File::create(&src).unwrap(), 257, 3);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        let data: Vec<_> = (0..3)
            .flat_map(|y| (0..257).flat_map(move |x| [x as u8, y * 100, 25]))
            .collect();
        e.write_header().unwrap().write_image_data(&data).unwrap();
        let r = import(&src, &dir.join("cache"), &AtomicBool::new(false)).unwrap();
        assert_eq!(r.levels[1].width, 129);
        let (_, _, pixels) = r.tile(0, 256, 0).unwrap();
        assert_eq!(&pixels[..4], &[0, 0, 25, 255]);
        assert_eq!(&pixels[8..12], &[0, 200, 25, 255]);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn adam7_pixels_and_bmp_padding_orientation() {
        use flate2::{Compression, write::ZlibEncoder};
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend((data.len() as u32).to_be_bytes());
            out.extend(kind);
            out.extend(data);
            let mut crc = crc32fast::Hasher::new();
            crc.update(kind);
            crc.update(data);
            out.extend(crc.finalize().to_be_bytes());
        }
        let dir = std::env::temp_dir().join(format!("road-formats-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (width, height) = (9u32, 7u32);
        let mut scan = Vec::new();
        for (sx, sy, dx, dy) in [
            (0, 0, 8, 8),
            (4, 0, 8, 8),
            (0, 4, 4, 8),
            (2, 0, 4, 4),
            (0, 2, 2, 4),
            (1, 0, 2, 2),
            (0, 1, 1, 2),
        ] {
            if sx >= width || sy >= height {
                continue;
            }
            for y in (sy..height).step_by(dy as usize) {
                scan.push(0);
                for x in (sx..width).step_by(dx as usize) {
                    scan.extend([x as u8, y as u8, 200]);
                }
            }
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&scan).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = width.to_be_bytes().to_vec();
        header.extend(height.to_be_bytes());
        header.extend([8, 2, 0, 0, 1]);
        chunk(&mut bytes, b"IHDR", &header);
        chunk(&mut bytes, b"IDAT", &compressed);
        chunk(&mut bytes, b"IEND", &[]);
        let source = dir.join("adam.png");
        std::fs::write(&source, bytes).unwrap();
        let raster = import(&source, &dir.join("png"), &AtomicBool::new(false)).unwrap();
        let (_, _, data) = raster.tile(0, 0, 0).unwrap();
        for y in 0..height {
            for x in 0..width {
                let p = ((y * width + x) * 4) as usize;
                assert_eq!(&data[p..p + 4], &[x as u8, y as u8, 200, 255]);
            }
        }
        let mut bmp = vec![0; 54];
        bmp[..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&70u32.to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&2i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&2i32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        bmp.extend([255, 0, 0, 255, 255, 255, 0, 0, 0, 0, 255, 0, 255, 0, 0, 0]);
        let source = dir.join("sat.bmp");
        std::fs::write(&source, bmp).unwrap();
        let r = import(&source, &dir.join("bmp"), &AtomicBool::new(false)).unwrap();
        let (_, _, pixels) = r.tile(0, 0, 0).unwrap();
        assert_eq!(
            pixels,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
