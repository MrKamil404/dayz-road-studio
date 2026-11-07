//! Minimal reader for binarised (ODOL) P3D models: LOD resolutions, vertex positions and
//! named selections. Enough to measure track models shipped only as ODOL (DayZ 1.30 is
//! version 56); materials, normals and animations are skipped.
use anyhow::{Context, Result, bail, ensure};

/// One LOD: its resolution, vertex positions (model x, y, z) and named selections with
/// their vertex indices.
#[derive(Debug, Default)]
pub struct Lod {
    pub resolution: f32,
    pub vertices: Vec<[f32; 3]>,
    pub selections: Vec<(String, Vec<u16>)>,
    /// Texture paths of the LOD.
    pub textures: Vec<String>,
    /// Polygons as vertex indices (3 or 4).
    pub faces: Vec<Vec<u16>>,
    /// Texture index of each face (`u16::MAX` when none).
    pub face_textures: Vec<u16>,
    /// First UV set, one per vertex.
    pub uvs: Vec<[f32; 2]>,
}
impl Lod {
    /// First vertex of a named selection (case-insensitive).
    pub fn point(&self, name: &str) -> Option<[f32; 3]> {
        self.selections
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .and_then(|(_, v)| v.first())
            .and_then(|&i| self.vertices.get(i as usize).copied())
    }
}
/// Resolution of the memory LOD.
pub const MEMORY: f32 = 1e15;

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}
impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.p.checked_add(n).context("Rozmiar ODOL")?;
        let s = self.b.get(self.p..end).context("Niepełny plik ODOL")?;
        self.p = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into()?))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.bytes(4)?.try_into()?))
    }
    fn count(&mut self) -> Result<usize> {
        let n = self.i32()?;
        ensure!((0..=50_000_000).contains(&n), "Nieprawidłowa liczba elementów ODOL");
        Ok(n as usize)
    }
    fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }
    fn cstring(&mut self) -> Result<String> {
        let rest = self.b.get(self.p..).context("Niepełny plik ODOL")?;
        let end = rest.iter().position(|&c| c == 0).context("Niezakończony napis ODOL")?;
        let s = String::from_utf8_lossy(&rest[..end]).into_owned();
        self.p += end + 1;
        Ok(s)
    }
    /// `size` bytes, LZO-compressed when 1024 bytes or more.
    fn compressed(&mut self, size: usize) -> Result<Vec<u8>> {
        if size < 1024 {
            return Ok(self.bytes(size)?.to_vec());
        }
        let rest = self.b.get(self.p..).context("Niepełny plik ODOL")?;
        let (data, used) = lzo1x_decompress(rest, size)?;
        self.p += used;
        Ok(data)
    }
    /// Compressed array: element count, then `count × element` bytes.
    fn compressed_array(&mut self, element: usize) -> Result<(usize, Vec<u8>)> {
        let n = self.count()?;
        Ok((n, self.compressed(n.checked_mul(element).context("Rozmiar ODOL")?)?))
    }
    /// Compressed array that may instead store one value repeated `count` times.
    fn compressed_fill(&mut self, element: usize) -> Result<()> {
        let n = self.count()?;
        if self.u8()? != 0 {
            self.skip(element)
        } else {
            self.compressed(n.checked_mul(element).context("Rozmiar ODOL")?).map(|_| ())
        }
    }
}

/// LZO1X decompression of exactly `expected` bytes; returns the data and input consumed.
pub(crate) fn lzo1x_decompress(input: &[u8], expected: usize) -> Result<(Vec<u8>, usize)> {
    let mut out: Vec<u8> = Vec::with_capacity(expected);
    let mut ip = 0usize;
    let byte = |ip: &mut usize| -> Result<usize> {
        let b = *input.get(*ip).context("Uszkodzone dane LZO")?;
        *ip += 1;
        Ok(b as usize)
    };
    let copy_literal = |out: &mut Vec<u8>, ip: &mut usize, n: usize| -> Result<()> {
        let s = input.get(*ip..*ip + n).context("Uszkodzone dane LZO")?;
        out.extend_from_slice(s);
        *ip += n;
        Ok(())
    };
    let copy_match = |out: &mut Vec<u8>, distance: usize, n: usize| -> Result<()> {
        ensure!(distance >= 1 && distance <= out.len(), "Uszkodzone dane LZO");
        let start = out.len() - distance;
        for i in 0..n {
            let b = out[start + i];
            out.push(b);
        }
        Ok(())
    };
    // Number of literals copied after the previous instruction (0–3), from its low bits.
    let mut state;
    let first = *input.first().context("Puste dane LZO")? as usize;
    if first > 17 {
        ip += 1;
        let n = first - 17;
        copy_literal(&mut out, &mut ip, n)?;
        state = if n < 4 { n } else { 4 };
    } else {
        state = 0;
    }
    loop {
        ensure!(out.len() <= expected, "Uszkodzone dane LZO");
        let t = byte(&mut ip)?;
        if t < 16 {
            if state == 0 {
                // Literal run.
                let mut n = t;
                if n == 0 {
                    n = 15;
                    loop {
                        let b = byte(&mut ip)?;
                        if b != 0 {
                            n += b;
                            break;
                        }
                        n += 255;
                    }
                }
                copy_literal(&mut out, &mut ip, n + 3)?;
                state = 4;
                continue;
            }
            let b = byte(&mut ip)?;
            if state == 4 {
                // Three-byte match at distance 2049–3072 after a long literal run.
                copy_match(&mut out, 1 + 0x0800 + (t >> 2) + (b << 2), 3)?;
            } else {
                // Two-byte match after a short literal run.
                copy_match(&mut out, 1 + (t >> 2) + (b << 2), 2)?;
            }
            state = t & 3;
        } else if t >= 64 {
            let b = byte(&mut ip)?;
            copy_match(&mut out, 1 + ((t >> 2) & 7) + (b << 3), (t >> 5) + 1)?;
            state = t & 3;
        } else if t >= 32 {
            let mut n = t & 31;
            if n == 0 {
                n = 31;
                loop {
                    let b = byte(&mut ip)?;
                    if b != 0 {
                        n += b;
                        break;
                    }
                    n += 255;
                }
            }
            let (b0, b1) = (byte(&mut ip)?, byte(&mut ip)?);
            copy_match(&mut out, 1 + (b0 >> 2) + (b1 << 6), n + 2)?;
            state = b0 & 3;
        } else {
            // 16–31: far match or end of stream.
            let mut n = t & 7;
            if n == 0 {
                n = 7;
                loop {
                    let b = byte(&mut ip)?;
                    if b != 0 {
                        n += b;
                        break;
                    }
                    n += 255;
                }
            }
            let (b0, b1) = (byte(&mut ip)?, byte(&mut ip)?);
            let distance = ((t & 8) << 11) + (b0 >> 2) + (b1 << 6);
            if distance == 0 {
                break;
            }
            copy_match(&mut out, distance + 0x4000, n + 2)?;
            state = b0 & 3;
        }
        if state > 0 && state < 4 {
            copy_literal(&mut out, &mut ip, state)?;
        }
    }
    ensure!(out.len() == expected, "Rozmiar danych LZO nie zgadza się z ODOL");
    Ok((out, ip))
}

/// LOD resolutions and byte ranges. The table follows the variable-length model info, so it
/// is located by its own consistency: start/end offsets, flags and metadata must lead exactly
/// to the first LOD.
/// ODOL version and (resolution, start, end) of every LOD.
type LodTable = (u32, Vec<(f32, usize, usize)>);
fn lod_table(b: &[u8]) -> Result<LodTable> {
    ensure!(b.starts_with(b"ODOL"), "To nie jest model ODOL");
    let mut r = Reader { b, p: 4 };
    let version = r.u32()?;
    ensure!((53..=60).contains(&version), "Nieobsługiwana wersja ODOL {version}");
    let n = r.count()?;
    ensure!((1..=64).contains(&n), "Nieprawidłowa liczba LOD w ODOL");
    let resolutions: Vec<f32> = (0..n).map(|_| r.f32()).collect::<Result<_>>()?;
    let word = |p: usize| b.get(p..p + 4).map(|w| u32::from_le_bytes(w.try_into().unwrap()) as usize);
    'scan: for p in r.p..b.len().saturating_sub(n * 9) {
        let mut ranges = Vec::with_capacity(n);
        for i in 0..n {
            let (Some(s), Some(e)) = (word(p + i * 4), word(p + (n + i) * 4)) else {
                continue 'scan;
            };
            if s <= p || e <= s || e > b.len() {
                continue 'scan;
            }
            ranges.push((s, e));
        }
        let mut q = p + n * 8;
        let mut meta = 0;
        for i in 0..n {
            match b.get(q + i) {
                Some(0) => meta += 1,
                Some(1) => {}
                _ => continue 'scan,
            }
        }
        q += n + meta * 25;
        if ranges.iter().map(|r| r.0).min() != Some(q) {
            continue;
        }
        let mut sorted = ranges.clone();
        sorted.sort();
        if sorted.windows(2).any(|w| w[0].1 > w[1].0) {
            continue;
        }
        return Ok((
            version,
            resolutions.into_iter().zip(ranges).map(|(res, (s, e))| (res, s, e)).collect(),
        ));
    }
    bail!("Nie znaleziono tabeli LOD w ODOL")
}

fn read_lod(b: &[u8], start: usize, resolution: f32) -> Result<Lod> {
    let mut r = Reader { b, p: start };
    for _ in 0..r.count()? {
        // Proxy: model name, 4×3 matrix, four indices.
        r.cstring()?;
        r.skip(48 + 16)?;
    }
    let n = r.count()?;
    r.skip(n * 4)?;
    for _ in 0..r.count()? {
        let n = r.count()?;
        r.skip(n * 4)?;
    }
    // Vertex count, face area, hints, bounding box, centre and radius.
    r.skip(4 + 4 + 8 + 36 + 4)?;
    let mut textures = Vec::new();
    for _ in 0..r.count()? {
        textures.push(r.cstring()?);
    }
    for _ in 0..r.count()? {
        skip_material(&mut r)?;
    }
    r.compressed_array(2)?;
    r.compressed_array(2)?;
    // Faces: count, allocation size, reserved word, then (vertex count, indices).
    let count = r.count()?;
    r.skip(4 + 2)?;
    let mut faces = Vec::with_capacity(count);
    for _ in 0..count {
        let k = r.u8()? as usize;
        let data = r.bytes(k * 2)?;
        faces.push((0..k).map(|i| u16::from_le_bytes([data[i * 2], data[i * 2 + 1]])).collect::<Vec<_>>());
    }
    // Sections give face ranges as byte offsets: 8 per triangle, 10 per quad.
    let mut offsets = Vec::with_capacity(faces.len());
    let mut offset = 0;
    for f in &faces {
        offsets.push(offset);
        offset += if f.len() == 4 { 10 } else { 8 };
    }
    let mut face_textures = vec![u16::MAX; faces.len()];
    for _ in 0..r.count()? {
        let lower = r.i32()?;
        let upper = r.i32()?;
        r.skip(12)?;
        let texture = u16::from_le_bytes(r.bytes(2)?.try_into().unwrap());
        r.skip(4)?;
        if r.i32()? == -1 {
            r.cstring()?;
        }
        let n = r.count()?;
        r.skip(n * 4)?;
        for (i, &o) in offsets.iter().enumerate() {
            if o >= lower && o < upper {
                face_textures[i] = texture;
            }
        }
    }
    let mut selections = Vec::new();
    for _ in 0..r.count()? {
        let name = r.cstring()?;
        r.compressed_array(2)?;
        r.skip(4 + 1)?;
        r.compressed_array(4)?;
        let (count, data) = r.compressed_array(2)?;
        let indices = (0..count)
            .map(|i| u16::from_le_bytes([data[i * 2], data[i * 2 + 1]]))
            .collect();
        let weights = r.count()?;
        r.compressed(weights)?;
        selections.push((name, indices));
    }
    for _ in 0..r.count()? {
        r.cstring()?;
        r.cstring()?;
    }
    for _ in 0..r.count()? {
        r.skip(4)?;
        let n = r.count()?;
        r.skip(n * 12)?;
    }
    // Icon colour, selected colour, special flags.
    r.skip(12)?;
    // Vertex data: simple-bone flag, size, clip flags, UV sets, then positions.
    r.skip(1 + 4)?;
    r.compressed_fill(4)?;
    let mut uv_sets = 1;
    let mut i = 0;
    let mut uvs = Vec::new();
    while i < uv_sets {
        let range: Vec<f32> = (0..4).map(|_| r.f32()).collect::<Result<_>>()?;
        let n = r.count()?;
        let raw = if r.u8()? != 0 { r.bytes(4)?.repeat(n) } else { r.compressed(n * 4)? };
        if i == 0 {
            let decode = |v: i16, min: f32, max: f32| (v as f32 + 32767.) / 65536. * (max - min) + min;
            uvs = (0..n)
                .map(|k| {
                    let u = i16::from_le_bytes([raw[k * 4], raw[k * 4 + 1]]);
                    let v = i16::from_le_bytes([raw[k * 4 + 2], raw[k * 4 + 3]]);
                    [decode(u, range[0], range[2]), decode(v, range[1], range[3])]
                })
                .collect();
            uv_sets = r.count()?.max(1);
        }
        i += 1;
    }
    let (count, data) = r.compressed_array(12)?;
    let f = |o: usize| f32::from_le_bytes(data[o..o + 4].try_into().unwrap());
    let vertices = (0..count).map(|i| [f(i * 12), f(i * 12 + 4), f(i * 12 + 8)]).collect();
    Ok(Lod { resolution, vertices, selections, textures, faces, face_textures, uvs })
}

fn skip_material(r: &mut Reader) -> Result<()> {
    r.cstring()?;
    let version = r.u32()?;
    // Eight colours, specular power, two words, one colour.
    r.skip(8 * 16 + 4 + 8 + 16)?;
    if version >= 20 {
        r.skip(12 * 4)?;
    }
    r.skip(16)?;
    r.cstring()?;
    r.skip(8)?;
    let stages = r.count()?;
    let transforms = r.count()?;
    for _ in 0..stages {
        r.skip(4)?;
        r.cstring()?;
        r.skip(5)?;
    }
    r.skip(transforms * 52)?;
    // Texture stage TI.
    r.skip(4)?;
    r.cstring()?;
    r.skip(5)
}

/// The LODs whose resolution `want` accepts, read from an ODOL file.
pub fn read(b: &[u8], want: impl Fn(usize, f32) -> bool) -> Result<Vec<Lod>> {
    let (_, table) = lod_table(b)?;
    let mut lods = Vec::new();
    for (i, &(resolution, start, end)) in table.iter().enumerate() {
        if want(i, resolution) {
            let lod = read_lod(&b[..end], start, resolution)
                .with_context(|| format!("LOD {resolution} modelu ODOL"))?;
            lods.push(lod);
        }
    }
    Ok(lods)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lzo_literals_matches_and_end_marker() {
        // "abcabcabcabc": literal run "abc", then a 9-byte match at distance 3, end marker.
        let stream = [17 + 3, b'a', b'b', b'c', 32 | 7, (2 << 2) as u8, 0, 17, 0, 0];
        let (out, used) = lzo1x_decompress(&stream, 12).unwrap();
        assert_eq!(out, b"abcabcabcabc");
        assert_eq!(used, stream.len());
        assert!(lzo1x_decompress(&stream, 11).is_err());
    }

    /// Real DayZ models when the P: drive is mounted; skipped otherwise.
    #[test]
    fn reads_track_memory_points_and_vertices_from_dayz_odol() {
        let path = std::path::Path::new("P:/dz/structures/rail/Tracks/Rail_Track_25.p3d");
        let Ok(bytes) = std::fs::read(path) else { return };
        let lods = read(&bytes, |i, r| i == 0 || r == MEMORY).unwrap();
        assert_eq!(lods.len(), 2);
        assert!(lods[0].vertices.len() > 1000);
        let memory = &lods[1];
        let (a, b) = (memory.point("map1").unwrap(), memory.point("map2").unwrap());
        assert!((a[2] - 12.497).abs() < 0.01 && (b[2] + 12.4825).abs() < 0.01, "{a:?} {b:?}");
    }
}
