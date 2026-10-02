use crate::geometry::{Library, Point, Shape};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
const MAGIC: &[u8] = b"\x06\0\x0d";
#[derive(Clone)]
struct Entry {
    start: usize,
    len: usize,
    id: u32,
    kind: u16,
}
#[derive(Clone)]
struct Field {
    tag: u8,
    typ: u8,
    start: usize,
    len: usize,
    list: Option<Vec<Entry>>,
}
struct Block {
    start: usize,
    end: usize,
    entries: Vec<Entry>,
}
fn u16(b: &[u8], p: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        b.get(p..p + 2).context("Truncated TV4P u16")?.try_into()?,
    ))
}
fn u32(b: &[u8], p: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        b.get(p..p + 4).context("Truncated TV4P u32")?.try_into()?,
    ))
}
fn f64(b: &[u8], p: usize) -> Result<f64> {
    Ok(f64::from_le_bytes(
        b.get(p..p + 8).context("Truncated TV4P f64")?.try_into()?,
    ))
}
fn entries(b: &[u8], mut p: usize, end: usize, count: usize) -> Result<Vec<Entry>> {
    if count > 200_000 || end > b.len() {
        bail!("Invalid TV4P entry list")
    };
    let mut out = Vec::new();
    for _ in 0..count {
        if b.get(p..p + 3) != Some(MAGIC) {
            bail!("Invalid entry marker")
        };
        let len = u32(b, p + 3)? as usize;
        p += 7;
        if len < 6 || p + len > end {
            bail!("Invalid entry size")
        };
        out.push(Entry {
            start: p,
            len,
            id: u32(b, p + 2)?,
            kind: u16(b, p)?,
        });
        p += len;
    }
    if p != end {
        bail!("TV4P list size mismatch")
    };
    Ok(out)
}
fn blocks(b: &[u8], tag: u8) -> Vec<Block> {
    let mut v = Vec::new();
    for p in 0..b.len().saturating_sub(10) {
        if b[p..p + 3] != [tag, 0, 12] {
            continue;
        };
        let Ok(len) = u32(b, p + 3) else { continue };
        let Ok(count) = u32(b, p + 7) else { continue };
        let end = p + 7 + len as usize;
        if len < 4 {
            continue;
        };
        if let Ok(entries) = entries(b, p + 11, end, count as usize) {
            v.push(Block {
                start: p,
                end,
                entries,
            })
        }
    }
    v
}
fn block(b: &[u8], tag: u8) -> Result<Block> {
    let mut v = blocks(b, tag);
    if tag == 0x8a {
        v.retain(|v| v.entries.iter().all(|e| e.kind == 0x1a))
    };
    if v.len() != 1 {
        bail!("Expected one block 0x{tag:02X}, found {}", v.len())
    };
    Ok(v.remove(0))
}
fn fields(b: &[u8]) -> Result<Vec<Field>> {
    let mut p = 6;
    let mut out = Vec::new();
    while p < b.len() {
        let h = b.get(p..p + 3).context("Truncated field")?;
        let tag = h[0];
        let typ = h[2];
        p += 3;
        let mut start = p;
        let len;
        let mut list = None;
        match typ {
            11 => {
                len = u16(b, p)? as usize;
                p += 2;
                start = p;
            }
            5 | 8 | 13 => len = 4,
            9 => len = 1,
            20 => len = 8,
            21 => len = 1 + *b.get(p).context("Truncated array")? as usize * 8,
            32 => len = 3,
            12 => {
                let l = u32(b, p)? as usize;
                let c = u32(b, p + 4)? as usize;
                if l < 4 {
                    bail!("Invalid nested list")
                };
                len = l + 4;
                list = Some(entries(b, p + 8, p + len, c)?);
            }
            _ => bail!("Unknown field type 0x{typ:02X}"),
        }
        if p + len > b.len() {
            bail!("Field exceeds entry")
        };
        p += len;
        out.push(Field {
            tag,
            typ,
            start,
            len,
            list,
        });
    }
    Ok(out)
}
fn field<'a>(b: &'a [u8], f: &Field) -> &'a [u8] {
    &b[f.start..f.start + f.len]
}
fn text(b: &[u8], f: &Field) -> String {
    String::from_utf8_lossy(field(b, f)).into_owned()
}
#[derive(Clone, Serialize)]
pub struct Road {
    pub index: usize,
    pub id: u32,
    pub model: String,
    pub models: Vec<String>,
    pub start: Point,
    pub rotation_degrees: f64,
    pub parts: usize,
    pub shape: Shape,
    pub is_new: bool,
}
#[derive(Clone, Serialize)]
pub struct Project {
    pub path: PathBuf,
    pub roads: Vec<Road>,
}
pub fn load(path: &Path, lib: &mut Library) -> Result<Project> {
    let b = fs::read(path).with_context(|| format!("Cannot read {}", path.display()))?;
    decode(path.to_owned(), &b, lib)
}
fn decode(path: PathBuf, b: &[u8], lib: &mut Library) -> Result<Project> {
    let bl = block(b, 0x8a)?;
    let mut roads = Vec::new();
    for (index, e) in bl.entries.iter().enumerate() {
        let body = &b[e.start..e.start + e.len];
        let fs = fields(body)?;
        let mut start = [0., 0.];
        let mut rotation = 0.;
        let mut model = String::new();
        for f in &fs {
            match (f.tag, f.typ) {
                (0x8c, 20) => rotation = f64(body, f.start)?,
                (0x8e, 21) => {
                    if f.len >= 17 {
                        start = [f64(body, f.start + 1)?, f64(body, f.start + 9)?];
                    }
                }
                (0x91, 11) => model = text(body, f),
                _ => {}
            }
        }
        if !rotation.is_finite() || !start.iter().all(|v| v.is_finite()) {
            bail!("Non-finite transform for road {}", e.id)
        }
        let mut shape = Shape::default();
        let mut parts = 1;
        let mut models = vec![model.clone()];
        let key = lib.model(&model);
        let mut keyports = [None; 4];
        match key {
            Ok(m) => keyports = shape.place(&m, -rotation.rem_euclid(360.).to_radians(), start),
            Err(err) => {
                if err.is::<crate::geometry::NonMlod>() {
                    shape.non_mlod_models.push(model.clone());
                }
                shape.warnings.push(format!("{}: {err:#}", model));
            }
        }
        // These lists are independent chains extending from the key part, not
        // consecutive sections of one polyline: end, begin, left, right.
        for f in &fs {
            let Some(list) = &f.list else { continue };
            parts += list.len();
            let portindex = match f.tag {
                0x92 => 1,
                0x93 => 0,
                0x94 => 2,
                0x95 => 3,
                _ => continue,
            };
            let mut at = keyports[portindex];
            for part in list {
                let pb = &body[part.start..part.start + part.len];
                let pf = fields(pb)?;
                let cat = pf
                    .iter()
                    .find(|f| f.tag == 0x7f)
                    .map(|f| u32(pb, f.start))
                    .transpose()?
                    .unwrap_or(3);
                let name = pf
                    .iter()
                    .find(|f| f.tag == 0x33 && f.typ == 11)
                    .map(|f| text(pb, f))
                    .unwrap_or_default();
                models.push(name.clone());
                if at.is_none() {
                    if shape.warnings.is_empty() {
                        shape.warnings.push(format!(
                            "Brak punktu połączenia odgałęzienia 0x{:02X}",
                            f.tag
                        ))
                    }
                    break;
                }
                match lib
                    .model(&name)
                    .and_then(|m| shape.append(&m, at.unwrap(), cat == 7))
                {
                    Ok(p) => at = Some(p),
                    Err(err) => {
                        if err.is::<crate::geometry::NonMlod>() {
                            shape.non_mlod_models.push(name.clone());
                        }
                        shape.warnings.push(format!("{name}: {err:#}"));
                        break;
                    }
                }
            }
        }
        roads.push(Road {
            index,
            id: e.id,
            model,
            models,
            start,
            rotation_degrees: rotation,
            parts,
            shape,
            is_new: false,
        });
    }
    Ok(Project { path, roads })
}
fn normalized(body: &[u8]) -> Result<Vec<u8>> {
    let mut out = body.to_vec();
    out[2..6].fill(0);
    for f in fields(body)? {
        if let Some(list) = f.list {
            for e in list {
                let n = normalized(&body[e.start..e.start + e.len])?;
                out[e.start..e.start + e.len].copy_from_slice(&n)
            }
        }
    }
    Ok(out)
}
fn build_list(tag: u8, bodies: &[Vec<u8>]) -> Result<Vec<u8>> {
    let mut v = vec![tag, 0, 12];
    v.extend([0; 4]);
    v.extend(u32::try_from(bodies.len())?.to_le_bytes());
    for b in bodies {
        v.extend(MAGIC);
        v.extend(u32::try_from(b.len())?.to_le_bytes());
        v.extend(b)
    }
    let len = u32::try_from(v.len() - 7)?;
    v[3..7].copy_from_slice(&len.to_le_bytes());
    Ok(v)
}
fn new_id(used: &mut HashSet<u32>, state: &mut u32, step: u32) -> Result<u32> {
    while used.contains(state) {
        *state = state.checked_add(step).context("TV4P ID overflow")?
    }
    let n = *state;
    used.insert(n);
    *state = state.checked_add(step).context("TV4P ID overflow")?;
    Ok(n)
}
fn reid(body: &[u8], id: u32, used: &mut HashSet<u32>, state: &mut u32) -> Result<Vec<u8>> {
    let mut out = body.to_vec();
    out[2..6].copy_from_slice(&id.to_le_bytes());
    for f in fields(body)? {
        if let Some(list) = f.list {
            for e in list {
                let id = new_id(used, state, 128)?;
                let b = reid(&body[e.start..e.start + e.len], id, used, state)?;
                out[e.start..e.start + e.len].copy_from_slice(&b)
            }
        }
    }
    Ok(out)
}
pub fn same_path(a: &Path, b: &Path) -> bool {
    fn full(p: &Path) -> PathBuf {
        p.canonicalize().unwrap_or_else(|_| {
            p.parent()
                .and_then(|p| p.canonicalize().ok())
                .map(|d| d.join(p.file_name().unwrap_or_default()))
                .unwrap_or_else(|| p.to_owned())
        })
    }
    full(a)
        .to_string_lossy()
        .eq_ignore_ascii_case(&full(b).to_string_lossy())
}
pub fn merge(a: &Path, b: &Path, out: &Path) -> Result<String> {
    Ok(merge_report(a, b, out)?.message)
}
pub struct MergeReport {
    pub message: String,
    pub new_ids: HashSet<u32>,
}
pub fn merge_report(a: &Path, b: &Path, out: &Path) -> Result<MergeReport> {
    if same_path(a, out) || same_path(b, out) {
        bail!("Plik wynikowy musi być inny niż A i B")
    };
    let a = fs::read(a)?;
    let b = fs::read(b)?;
    let ra = block(&a, 0x8a)?;
    let rb = block(&b, 0x8a)?;
    for tag in [0x88, 0x89] {
        let aa = block(&a, tag)?;
        let bb = block(&b, tag)?;
        let n = |b: &[u8], bl: &Block| {
            bl.entries
                .iter()
                .map(|e| normalized(&b[e.start..e.start + e.len]))
                .collect::<Result<Vec<_>>>()
        };
        if n(&a, &aa)? != n(&b, &bb)? {
            bail!("Niezgodne ustawienia Road Tool 0x{tag:02X}")
        }
    }
    let mut used = HashSet::new();
    for data in [&a, &b] {
        for p in 0..data.len().saturating_sub(12) {
            if data.get(p..p + 3) == Some(MAGIC) {
                if let (Ok(len), Ok(id)) = (u32(data, p + 3), u32(data, p + 9)) {
                    if len >= 6 && p + 7 + len as usize <= data.len() {
                        used.insert(id);
                    }
                }
            }
        }
    }
    let mut part_state = used
        .iter()
        .copied()
        .max()
        .unwrap_or(0)
        .checked_add(128)
        .context("ID overflow")?;
    let mut road_state = ra
        .entries
        .iter()
        .map(|e| e.id)
        .max()
        .unwrap_or(0)
        .checked_add(992)
        .context("ID overflow")?;
    let mut bodies: Vec<_> = ra
        .entries
        .iter()
        .map(|e| a[e.start..e.start + e.len].to_vec())
        .collect();
    let mut seen = bodies
        .iter()
        .map(|b| normalized(b))
        .collect::<Result<HashSet<_>>>()?;
    let mut added = 0;
    let mut new_ids = HashSet::new();
    for e in &rb.entries {
        let body = &b[e.start..e.start + e.len];
        if seen.insert(normalized(body)?) {
            let id = new_id(&mut used, &mut road_state, 992)?;
            bodies.push(reid(body, id, &mut used, &mut part_state)?);
            new_ids.insert(id);
            added += 1
        }
    }
    let list = build_list(0x8a, &bodies)?;
    let delta = list.len() as i64 - (ra.end - ra.start) as i64;
    let mut result = a[..ra.start].to_vec();
    result.extend(list);
    result.extend(&a[ra.end..]);
    if delta != 0 {
        for tag in [0x3f, 0x18] {
            let positions: Vec<_> = (0..result.len().saturating_sub(6))
                .filter(|p| result[*p..*p + 3] == [tag, 0, 13])
                .collect();
            if positions.len() != 1 {
                bail!("Ambiguous metadata offset 0x{tag:02X}")
            };
            let p = positions[0] + 3;
            let value = u32(&result, p)? as i64 + delta;
            let value = u32::try_from(value).context("Metadata offset overflow")?;
            result[p..p + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    fs::write(out, &result)?;
    Ok(MergeReport {
        message: format!(
            "A: {} · B: {} · dodano: {} · wynik: {} dróg",
            ra.entries.len(),
            rb.entries.len(),
            added,
            bodies.len()
        ),
        new_ids,
    })
}
pub fn roundtrip(path: &Path, language: crate::i18n::Language) -> Result<()> {
    let b = fs::read(path)?;
    let bl = block(&b, 0x8a)?;
    let bodies: Vec<_> = bl
        .entries
        .iter()
        .map(|e| b[e.start..e.start + e.len].to_vec())
        .collect();
    let rebuilt = build_list(0x8a, &bodies)?;
    if rebuilt != b[bl.start..bl.end] {
        bail!("Roundtrip mismatch")
    };
    println!(
        "{}",
        language.tr(&format!(
            "Road block roundtrip: identical ({} bytes)",
            rebuilt.len()
        ))
    );
    Ok(())
}
pub fn types(path: &Path, language: crate::i18n::Language) -> Result<()> {
    let b = fs::read(path)?;
    for tag in [0x88, 0x89] {
        let bl = block(&b, tag)?;
        println!(
            "{}",
            language.tr(&format!("0x{tag:02X}: {} entries", bl.entries.len()))
        );
        for e in bl.entries {
            let body = &b[e.start..e.start + e.len];
            for f in fields(body)? {
                if f.typ == 11 {
                    println!("  {} 0x{:02X} {}", e.id, f.tag, text(body, &f));
                }
            }
        }
    }
    Ok(())
}
