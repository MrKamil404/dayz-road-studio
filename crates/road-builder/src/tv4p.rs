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
            if data.get(p..p + 3) == Some(MAGIC)
                && let (Ok(len), Ok(id)) = (u32(data, p + 3), u32(data, p + 9))
                && len >= 6
                && p + 7 + len as usize <= data.len()
            {
                used.insert(id);
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
    let road_start = ra.start;
    let road_end = (road_start as i64 + (ra.end - ra.start) as i64 + delta) as usize;
    if delta != 0 {
        for tag in [0x3f, 0x18] {
            let positions: Vec<_> = (0..result.len().saturating_sub(6))
                .filter(|p| {
                    // Metadata belongs outside the parsed road list. Segment IDs and
                    // other road payload can contain the same byte sequence.
                    !(*p < road_end && *p + 7 > road_start)
                        && result[*p..*p + 3] == [tag, 0, 13]
                })
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

/// Read the actual Road Tool definitions, not a guessed list of filenames.
pub fn catalog(
    path: &Path,
    lib: &mut Library,
) -> Result<(Vec<crate::roads::CatalogPart>, Vec<String>)> {
    let b = fs::read(path)?;
    let bl = block(&b, 0x88)?;
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for (road_type_index, entry) in bl.entries.into_iter().enumerate() {
        let body = &b[entry.start..entry.start + entry.len];
        for f in fields(body)? {
            let category = match f.tag {
                0x78 => 3,
                0x79 => 4,
                0x7b => 6,
                _ => continue,
            };
            let Some(list) = f.list else { continue };
            for (index, e) in list.iter().enumerate() {
                let part = &body[e.start..e.start + e.len];
                let fs = fields(part)?;
                let Some(f) = fs.iter().find(|f| f.tag == 0x7c && f.typ == 11) else {
                    continue;
                };
                let name = text(part, f);
                match lib.model(&name) {
                    Ok(model) if model.source == "MLOD" => {
                        let family = name
                            .rsplit(['\\', '/'])
                            .next()
                            .unwrap_or(&name)
                            .split('_')
                            .next()
                            .unwrap_or("")
                            .to_owned();
                        out.push(crate::roads::CatalogPart {
                            path: name,
                            family,
                            category,
                            index: index as u32,
                            road_type_index: road_type_index as u32,
                            model,
                        });
                    }
                    Ok(_) => {
                        warnings.push(format!("{name}: brak MLOD; model pominięty w edytorze"))
                    }
                    Err(e) => warnings.push(format!("{name}: {e}")),
                }
            }
        }
    }
    Ok((out, warnings))
}

fn rewrite(
    body: &[u8],
    changes: &std::collections::HashMap<u8, Vec<u8>>,
    remove: &[u8],
) -> Result<Vec<u8>> {
    let fs = fields(body)?;
    let mut out = body[..6].to_vec();
    let mut cursor = 6;
    for f in fs {
        let end = f.start + f.len;
        if !remove.contains(&f.tag) {
            if let Some(new) = changes.get(&f.tag) {
                out.extend(new);
            } else {
                out.extend(&body[cursor..end]);
            }
        }
        cursor = end;
    }
    Ok(out)
}
fn scalar(tag: u8, typ: u8, data: &[u8]) -> Vec<u8> {
    let mut b = vec![tag, 0, typ];
    b.extend(data);
    b
}
fn string_field(tag: u8, s: &str) -> Result<Vec<u8>> {
    let mut b = vec![tag, 0, 11];
    b.extend(u16::try_from(s.len())?.to_le_bytes());
    b.extend(s.as_bytes());
    Ok(b)
}

#[derive(Debug)]
pub struct DefinitionPart {
    pub road_type_index: u32,
    pub category: u32,
    pub index: u32,
    pub path: String,
}
fn definition_parts(data: &[u8]) -> Result<Vec<DefinitionPart>> {
    let mut parts = Vec::new();
    for (road_type_index, e) in block(data, 0x88)?.entries.into_iter().enumerate() {
        let body = &data[e.start..e.start + e.len];
        for f in fields(body)? {
            let category = match f.tag {
                0x78 => 3,
                0x79 => 4,
                0x7b => 6,
                _ => continue,
            };
            let Some(list) = f.list else { continue };
            for (index, e) in list.iter().enumerate() {
                let pb = &body[e.start..e.start + e.len];
                let fs = fields(pb)?;
                let path = fs
                    .iter()
                    .find(|f| f.tag == 0x7c && f.typ == 11)
                    .context("Brak ścieżki w definicji Road Tool")?;
                parts.push(DefinitionPart {
                    road_type_index: road_type_index as u32,
                    category,
                    index: index as u32,
                    path: text(pb, path),
                });
            }
        }
    }
    Ok(parts)
}
pub fn read_definition_parts(path: &Path) -> Result<Vec<DefinitionPart>> {
    definition_parts(&fs::read(path)?)
}
fn model_key(path: &str) -> String {
    let path = path.replace('/', "\\").to_lowercase();
    if path.as_bytes().get(1) == Some(&b':') {
        path[2..].trim_start_matches('\\').to_owned()
    } else {
        path
    }
}
fn field_u32(body: &[u8], fs: &[Field], tag: u8) -> Result<u32> {
    let f = fs
        .iter()
        .find(|f| f.tag == tag && f.typ == 5)
        .with_context(|| format!("Brak pola Road Tool 0x{tag:02X}"))?;
    u32(body, f.start)
}
fn field_string(body: &[u8], fs: &[Field], tag: u8) -> Result<String> {
    let f = fs
        .iter()
        .find(|f| f.tag == tag && f.typ == 11)
        .with_context(|| format!("Brak pola modelu 0x{tag:02X}"))?;
    Ok(text(body, f))
}
/// Validate native references independently of geometric reconstruction from filenames.
pub fn validate_road_bindings(path: &Path, ids: &HashSet<u32>) -> Result<()> {
    validate_bindings(&fs::read(path)?, ids)
}
fn validate_bindings(data: &[u8], ids: &HashSet<u32>) -> Result<()> {
    let defs = definition_parts(data)?;
    for e in block(data, 0x8a)?
        .entries
        .into_iter()
        .filter(|e| ids.contains(&e.id))
    {
        let body = &data[e.start..e.start + e.len];
        let fs = fields(body)?;
        let road_type = field_u32(body, &fs, 0x8f)?;
        let category = definition_category(field_u32(body, &fs, 0x90)?)?;
        let name = field_string(body, &fs, 0x91)?;
        if !defs.iter().any(|d| {
            d.road_type_index == road_type
                && d.category == category
                && model_key(&d.path) == model_key(&name)
        }) {
            bail!(
                "Droga #{}: model {} nie należy do typu Road Tool {} / kategorii {}",
                e.id,
                name,
                road_type,
                category
            );
        }
        for f in fs
            .iter()
            .filter(|f| [0x92, 0x93, 0x94, 0x95].contains(&f.tag))
        {
            if let Some(list) = &f.list {
                for child in list {
                    let pb = &body[child.start..child.start + child.len];
                    let fs = fields(pb)?;
                    let raw_category = field_u32(pb, &fs, 0x7f)?;
                    let category = definition_category(raw_category)?;
                    let index = field_u32(pb, &fs, 0x6c)?;
                    let name = field_string(pb, &fs, 0x33)?;
                    if !defs.iter().any(|d| {
                        d.road_type_index == road_type
                            && d.category == category
                            && d.index == index
                            && model_key(&d.path) == model_key(&name)
                    }) {
                        bail!(
                            "Droga #{}: segment {} ma niezgodne odwołanie do definicji typu {}",
                            e.id,
                            name,
                            road_type
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// Modify only the road block; preserve all unknown fields of unchanged records.
pub fn export_editor(
    base: &Path,
    out: &Path,
    doc: &crate::document::Document,
    catalog: &[crate::roads::CatalogPart],
) -> Result<()> {
    use std::collections::HashMap;
    doc.validate()?;
    ensure_different(base, out)?;
    let b = fs::read(base)?;
    let roads = block(&b, 0x8a)?;
    let raw: Vec<_> = roads
        .entries
        .iter()
        .map(|e| &b[e.start..e.start + e.len])
        .collect();
    let original_ids: HashSet<_> = roads.entries.iter().map(|e| e.id).collect();
    // Deletion is idempotent: a road absent from the base is already deleted.
    // Replacements and transforms still require their original records.
    if doc
        .routes.iter().filter_map(|r| r.replaces.as_ref())
        .chain(doc.transforms.iter().map(|t| &t.0))
        .any(|id| !original_ids.contains(id))
    {
        bail!("Projekt bazowy zmienił się: nie znaleziono edytowanej drogi");
    }
    let mut used = HashSet::new();
    for p in 0..b.len().saturating_sub(13) {
        if b.get(p..p + 3) == Some(MAGIC)
            && let (Ok(len), Ok(id)) = (u32(&b, p + 3), u32(&b, p + 9))
            && len >= 6
            && p + 7 + len as usize <= b.len()
        {
            used.insert(id);
        }
    }
    let mut state = used
        .iter()
        .max()
        .copied()
        .unwrap_or(0)
        .checked_add(1)
        .context("ID overflow")?;
    let replaced: HashSet<_> = doc
        .routes
        .iter()
        .filter_map(|r| r.replaces)
        .chain(doc.deleted.iter().copied())
        .collect();
    let mut bodies: Vec<Vec<u8>> = Vec::new();
    let mut new_ids = HashSet::new();
    for body in &raw {
        let id = u32(body, 2)?;
        if replaced.contains(&id) {
            continue;
        }
        if let Some((_, delta, rotation)) = doc.transforms.iter().find(|(rid, _, _)| *rid == id) {
            let mut changes = HashMap::new();
            let fs = fields(body)?;
            for f in &fs {
                if f.tag == 0x8c && f.typ == 20 {
                    changes.insert(
                        0x8c,
                        scalar(0x8c, 20, &(f64(body, f.start)? + rotation).to_le_bytes()),
                    );
                }
                if f.tag == 0x8e && f.typ == 21 {
                    let mut coords = field(body, f).to_vec();
                    if coords.len() < 17 {
                        bail!("Nieobsługiwane współrzędne");
                    }
                    for (i, translation) in delta.iter().enumerate() {
                        let p = 1 + i * 8;
                        let v = f64::from_le_bytes(coords[p..p + 8].try_into()?) + translation;
                        coords[p..p + 8].copy_from_slice(&v.to_le_bytes());
                    }
                    changes.insert(0x8e, scalar(0x8e, 21, &coords));
                }
            }
            bodies.push(rewrite(body, &changes, &[])?);
        } else {
            bodies.push(body.to_vec());
        }
    }
    for route in &doc.routes {
        if route.parts.is_empty() {
            bail!("Droga {} nie ma dopasowanych segmentów", route.name);
        }
        let key = &route.parts[0];
        if key.reverse {
            bail!("Odwrócony segment kluczowy nie jest obsługiwany");
        }
        let item = catalog
            .iter()
            .find(|m| m.path == key.model)
            .context("Model poza definicjami TV4P")?;
        for (i, part) in route.parts.iter().enumerate().skip(1) {
            let at = crate::roads::endpoint(&route.parts[..i], catalog)?;
            let m = catalog
                .iter()
                .find(|m| m.path == part.model)
                .context("Model poza definicjami TV4P")?;
            let expected = crate::roads::place(m, at, part.reverse)?;
            let rotation = (expected.rotation - part.rotation + 180.).rem_euclid(360.) - 180.;
            if crate::geometry::norm(crate::geometry::sub(expected.position, part.position)) > 0.001
                || rotation.abs() > 0.001
            {
                bail!("Droga {} zawiera rozłączone segmenty", route.name);
            }
        }
        let template=raw.iter().find(|body|fields(body).ok().is_some_and(|fs|
            fs.iter().find(|f|f.tag==0x8f && f.typ==5).is_some_and(|f|u32(body,f.start).ok()==Some(item.road_type_index)) &&
            fs.iter().find(|f|f.tag==0x91 && f.typ==11).is_some_and(|f|catalog.iter().any(|m|m.road_type_index==item.road_type_index && m.path==text(body,f))))).context("W bazowym TV4P potrzebna jest przykładowa droga wybranego typu (utwórz ją w Terrain Builder)")?;
        let mut changes = HashMap::new();
        changes.insert(0x8c, scalar(0x8c, 20, &key.rotation.to_le_bytes()));
        let posfield = fields(template)?
            .into_iter()
            .find(|f| f.tag == 0x8e && f.typ == 21)
            .context("Brak pozycji w szablonie")?;
        let mut coords = field(template, &posfield).to_vec();
        if coords.len() < 17 {
            bail!("Nieobsługiwane współrzędne TV4P");
        }
        coords[1..9].copy_from_slice(&key.position[0].to_le_bytes());
        coords[9..17].copy_from_slice(&key.position[1].to_le_bytes());
        changes.insert(0x8e, scalar(0x8e, 21, &coords));
        changes.insert(0x91, string_field(0x91, &key.model)?);
        changes.insert(0x8f, scalar(0x8f, 5, &item.road_type_index.to_le_bytes()));
        changes.insert(
            0x90,
            scalar(
                0x90,
                5,
                &native_category(item.category, false)?.to_le_bytes(),
            ),
        );
        let mut children = Vec::new();
        for part in &route.parts[1..] {
            let m = catalog
                .iter()
                .find(|m| m.path == part.model && m.road_type_index == item.road_type_index)
                .context("Nie można mieszać typów drogi bez skrzyżowania")?;
            let mut child = None;
            for body in &raw {
                for f in fields(body)? {
                    if let Some(list) = f.list {
                        for e in list {
                            let pb = &body[e.start..e.start + e.len];
                            if fields(pb)?.iter().any(|f| f.tag == 0x33 && f.typ == 11) {
                                child = Some(pb.to_vec());
                                break;
                            }
                        }
                    }
                    if child.is_some() {
                        break;
                    }
                }
                if child.is_some() {
                    break;
                }
            }
            let mut child = child
                .context("W bazowym TV4P potrzebny jest co najmniej jeden segment odgałęzienia")?;
            let mut edits = HashMap::new();
            let category = native_category(m.category, part.reverse)?;
            edits.insert(0x7f, scalar(0x7f, 5, &category.to_le_bytes()));
            edits.insert(0x6c, scalar(0x6c, 5, &m.index.to_le_bytes()));
            edits.insert(0x33, string_field(0x33, &part.model)?);
            child = rewrite(&child, &edits, &[])?;
            children.push(child);
        }
        for tag in [0x92, 0x93, 0x94, 0x95] {
            changes.insert(
                tag,
                build_list(tag, if tag == 0x92 { &children } else { &[] })?,
            );
        }
        let body = rewrite(template, &changes, &[])?;
        let id = new_id(&mut used, &mut state, 1)?;
        new_ids.insert(id);
        bodies.push(reid(&body, id, &mut used, &mut state)?);
    }
    let list = build_list(0x8a, &bodies)?;
    let delta = list.len() as i64 - (roads.end - roads.start) as i64;
    let mut result = b[..roads.start].to_vec();
    result.extend(list);
    result.extend(&b[roads.end..]);
    let road_start = roads.start;
    let road_end = (road_start as i64 + (roads.end - roads.start) as i64 + delta) as usize;
    if delta != 0 {
        for tag in [0x3f, 0x18] {
            let positions: Vec<_> = (0..result.len().saturating_sub(6))
                .filter(|p| {
                    // Metadata belongs outside the parsed road list. Segment IDs and
                    // other road payload can contain the same byte sequence.
                    !(*p < road_end && *p + 7 > road_start)
                        && result[*p..*p + 3] == [tag, 0, 13]
                })
                .collect();
            if positions.len() != 1 {
                bail!("Niejednoznaczne metadane TV4P 0x{tag:02X}");
            }
            let p = positions[0] + 3;
            let n = u32::try_from(u32(&result, p)? as i64 + delta)?;
            result[p..p + 4].copy_from_slice(&n.to_le_bytes());
        }
    }
    // Parse and rebuild before touching the destination.
    let check = block(&result, 0x8a)?;
    let bodies: Vec<_> = check
        .entries
        .iter()
        .map(|e| result[e.start..e.start + e.len].to_vec())
        .collect();
    if build_list(0x8a, &bodies)? != result[check.start..check.end] {
        bail!("Weryfikacja zapisu TV4P nie powiodła się");
    }
    validate_bindings(&result, &new_ids)?;
    crate::storage::atomic_write(out, &result)
}
fn native_category(category: u32, reverse: bool) -> Result<u32> {
    match (category, reverse) {
        (4, true) => Ok(7),
        (4, false) => Ok(8),
        (3 | 6, false) => Ok(category),
        _ => bail!("Nieobsługiwana kategoria segmentu lub kierunek"),
    }
}
fn definition_category(native: u32) -> Result<u32> {
    match native {
        7 | 8 => Ok(4),
        3 | 6 => Ok(native),
        _ => bail!("Nieobsługiwany kod wstawionego segmentu Road Tool: {native}"),
    }
}
fn ensure_different(a: &Path, b: &Path) -> Result<()> {
    if same_path(a, b) {
        bail!("Zapisz wynik do pliku innego niż projekt źródłowy");
    }
    Ok(())
}

#[cfg(test)]
mod editor_tests {
    use super::*;
    use crate::{
        document::{Document, Route},
        geometry::filename_model,
        roads::{CatalogPart, PlacedPart},
    };
    fn record(kind: u16, id: u32) -> Vec<u8> {
        let mut b = kind.to_le_bytes().to_vec();
        b.extend(id.to_le_bytes());
        b
    }
    fn fixture() -> Vec<u8> {
        let mut child = record(0x1b, 2);
        child.extend(scalar(0x7f, 5, &3u32.to_le_bytes()));
        child.extend(scalar(0x6c, 5, &3u32.to_le_bytes()));
        child.extend(string_field(0x33, "asf2_25.p3d").unwrap());
        let mut r = record(0x1a, 1);
        r.extend(scalar(0x8c, 20, &0f64.to_le_bytes()));
        r.extend(scalar(0x8d, 20, &0f64.to_le_bytes()));
        let mut coords = vec![2];
        coords.extend(200000f64.to_le_bytes());
        coords.extend(100f64.to_le_bytes());
        r.extend(scalar(0x8e, 21, &coords));
        r.extend(scalar(0x8f, 5, &0u32.to_le_bytes()));
        r.extend(scalar(0x90, 5, &3u32.to_le_bytes()));
        r.extend(string_field(0x91, "asf2_25.p3d").unwrap());
        r.extend(scalar(0xaa, 9, &[42]));
        for t in [0x92, 0x93, 0x94, 0x95] {
            r.extend(
                build_list(
                    t,
                    if t == 0x92 {
                        std::slice::from_ref(&child)
                    } else {
                        &[]
                    },
                )
                .unwrap(),
            );
        }
        let mut b = scalar(0x3f, 13, &1000u32.to_le_bytes());
        b.extend(scalar(0x18, 13, &2000u32.to_le_bytes()));
        let mut definition = record(0x12, 100);
        definition.extend(string_field(0x33, "asf2").unwrap());
        let straight: Vec<_> = [
            "asf2_6.p3d",
            "asf2_12.p3d",
            "asf2_6_crosswalk.p3d",
            "asf2_25.p3d",
        ]
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            let mut p = record(0x13, 101 + i as u32);
            p.extend(string_field(0x7c, name).unwrap());
            p
        })
        .collect();
        let mut curve = record(0x14, 110);
        curve.extend(string_field(0x7c, "asf2_30 25.p3d").unwrap());
        definition.extend(build_list(0x78, &straight).unwrap());
        definition.extend(build_list(0x79, &[curve]).unwrap());
        let mut cap = record(0x16, 111);
        cap.extend(string_field(0x7c, "asf2_6konec.p3d").unwrap());
        definition.extend(build_list(0x7b, &[cap]).unwrap());
        b.extend(build_list(0x88, &[definition]).unwrap());
        b.extend(build_list(0x89, &[]).unwrap());
        b.extend(build_list(0x8a, &[r]).unwrap());
        b.extend(b"unchanged non-road content");
        b
    }
    fn catalog() -> Vec<CatalogPart> {
        [
            ("asf2_25.p3d", 3, 3),
            ("asf2_30 25.p3d", 4, 0),
            ("asf2_6konec.p3d", 6, 0),
        ]
        .map(|(name, category, index)| CatalogPart {
            path: name.into(),
            family: "asf2".into(),
            category,
            index,
            road_type_index: 0,
            model: filename_model(name).unwrap(),
        })
        .to_vec()
    }
    #[test]
    fn native_corner_direction_codes_are_not_definition_categories() {
        // Constants observed in Terrain Builder records: 7 = reversed, 8 = forward.
        assert_eq!(native_category(4, true).unwrap(), 7);
        assert_eq!(native_category(4, false).unwrap(), 8);
        assert_eq!(definition_category(7).unwrap(), 4);
        assert_eq!(definition_category(8).unwrap(), 4);
        assert!(definition_category(4).is_err());
    }
    #[test]
    fn generated_konec_ends_survive_native_export() {
        let dir = std::env::temp_dir().join(format!("road-cap-export-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let base = dir.join("base.tv4p");
        let out = dir.join("caps.tv4p");
        fs::write(&base, fixture()).unwrap();
        let c = catalog();
        let points = vec![[200000., 200.], [200000., 312.5]];
        let parts = crate::roads::fit(
            &points,
            "asf2",
            &c,
            &crate::document::RoutingSettings::default(),
            |_, _| true,
        )
        .unwrap();
        let mut doc = Document::default();
        doc.routes.push(Route {
            name: "caps".into(),
            family: "asf2".into(),
            points,
            parts: parts.clone(),
            replaces: None,
        });
        export_editor(&base, &out, &doc, &c).unwrap();
        let b = fs::read(&out).unwrap();
        let block = block(&b, 0x8a).unwrap();
        let entry = block.entries.last().unwrap();
        let body = &b[entry.start..entry.start + entry.len];
        let fs = fields(body).unwrap();
        assert_eq!(field_u32(body, &fs, 0x90).unwrap(), 6);
        assert_eq!(field_string(body, &fs, 0x91).unwrap(), "asf2_6konec.p3d");
        let branch = fs
            .iter()
            .find(|f| f.tag == 0x92)
            .unwrap()
            .list
            .as_ref()
            .unwrap();
        let last = branch.last().unwrap();
        let child = &body[last.start..last.start + last.len];
        let cf = fields(child).unwrap();
        assert_eq!(field_u32(child, &cf, 0x7f).unwrap(), 6);
        assert_eq!(field_string(child, &cf, 0x33).unwrap(), "asf2_6konec.p3d");
        validate_bindings(&b, &HashSet::from([entry.id])).unwrap();
        let mut lib = Library::new(dir.clone());
        let decoded = load(&out, &mut lib).unwrap();
        let expected = crate::roads::shape(&parts, &mut lib).unwrap();
        assert_eq!(decoded.roads.last().unwrap().shape.lines, expected.lines);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn editor_export_roundtrip_and_real_geometry() {
        let dir = std::env::temp_dir().join(format!("road-tv4p-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let base = dir.join("base.tv4p");
        let out = dir.join("out.tv4p");
        let bytes = fixture();
        fs::write(&base, &bytes).unwrap();
        let c = catalog();
        let mut doc = Document::default();
        export_editor(&base, &out, &doc, &c).unwrap();
        assert_eq!(fs::read(&out).unwrap(), bytes);
        let mut parts = vec![PlacedPart {
            model: c[0].path.clone(),
            reverse: false,
            position: [200100., 200.],
            rotation: 35.,
        }];
        for (name, reverse) in [(&c[1].path, false), (&c[1].path, true), (&c[0].path, false)] {
            crate::roads::append(&mut parts, &c, name, reverse, [0., 0.], 0.).unwrap();
        }
        doc.routes.push(Route {
            name: "test".into(),
            family: "asf2".into(),
            points: vec![],
            parts: parts.clone(),
            replaces: None,
        });
        export_editor(&base, &out, &doc, &c).unwrap();
        let mut lib = Library::new(dir.clone());
        let p = load(&out, &mut lib).unwrap();
        assert_eq!(p.roads.len(), 2);
        let expected = crate::roads::shape(&parts, &mut lib).unwrap();
        assert_eq!(p.roads[1].shape.lines.len(), expected.lines.len());
        for (a, b) in p.roads[1]
            .shape
            .lines
            .iter()
            .flatten()
            .zip(expected.lines.iter().flatten())
        {
            assert!(crate::geometry::norm(crate::geometry::sub(*a, *b)) < 1e-6);
        }
        let output = fs::read(&out).unwrap();
        let ob = block(&output, 0x8a).unwrap();
        let added = ob.entries.last().unwrap();
        let added_body = &output[added.start..added.start + added.len];
        let added_fields = fields(added_body).unwrap();
        let branch = added_fields
            .iter()
            .find(|f| f.tag == 0x92)
            .unwrap()
            .list
            .as_ref()
            .unwrap();
        let codes: Vec<_> = branch
            .iter()
            .map(|e| {
                let pb = &added_body[e.start..e.start + e.len];
                field_u32(pb, &fields(pb).unwrap(), 0x7f).unwrap()
            })
            .collect();
        assert_eq!(
            codes,
            vec![8, 7, 3],
            "Native forward/reversed curves must use 8/7, never category 4"
        );
        assert_eq!(
            field_u32(added_body, &added_fields, 0x8f).unwrap(),
            0,
            "root refers to road type, not part index 3"
        );
        let ids = HashSet::from([added.id]);
        validate_bindings(&output, &ids).unwrap();
        let mut bad = output.clone();
        let type_field = added_fields.iter().find(|f| f.tag == 0x8f).unwrap();
        bad[added.start + type_field.start..added.start + type_field.start + 4]
            .copy_from_slice(&3u32.to_le_bytes());
        assert!(validate_bindings(&bad, &ids).is_err());
        let first_curve = &branch[0];
        let cb = &added_body[first_curve.start..first_curve.start + first_curve.len];
        let cfs = fields(cb).unwrap();
        let cf = cfs.iter().find(|f| f.tag == 0x7f).unwrap();
        let offset = added.start + first_curve.start + cf.start;
        let mut bad_corner = output.clone();
        bad_corner[offset..offset + 4].copy_from_slice(&4u32.to_le_bytes());
        assert!(validate_bindings(&bad_corner, &ids).is_err());
        let original = block(&bytes, 0x8a).unwrap();
        assert_eq!(&output[ob.end..], &bytes[original.end..]);
        assert_eq!(&output[14..ob.start], &bytes[14..original.start]);
        assert_eq!(
            &output[ob.entries[0].start..ob.entries[0].start + ob.entries[0].len],
            &bytes[original.entries[0].start..original.entries[0].start + original.entries[0].len]
        );
        assert!(export_editor(&base, &base, &doc, &c).is_err());
        doc.routes[0].parts[1].position[0] += 1.;
        assert!(export_editor(&base, &out, &doc, &c).is_err());
        doc.routes.clear();
        doc.transforms.push((1, [10., 20.], 90.));
        export_editor(&base, &out, &doc, &c).unwrap();
        let moved = load(&out, &mut lib).unwrap();
        assert_eq!(moved.roads[0].start, [200010., 120.]);
        assert_eq!(moved.roads[0].rotation_degrees, 90.);
        assert_eq!(moved.roads[0].id, 1);
        assert_eq!(moved.roads[0].parts, 2);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn export_ignores_metadata_patterns_in_segment_ids() {
        let dir = std::env::temp_dir().join(format!("road-metadata-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let base = dir.join("base.tv4p");
        let out = dir.join("out.tv4p");
        let mut bytes = fixture();
        let original = block(&bytes, 0x8a).unwrap();
        let root = &original.entries[0];
        let body = &bytes[root.start..root.start + root.len];
        let fs = fields(body).unwrap();
        let child = &fs.iter().find(|f| f.tag == 0x92).unwrap().list.as_ref().unwrap()[0];
        let id_offset = root.start + child.start + 2;
        for id in [0x0d00183cu32, 0x0d003f3c] {
            bytes[id_offset..id_offset + 4].copy_from_slice(&id.to_le_bytes());
            std::fs::write(&base, &bytes).unwrap();
            let c = catalog();
            let mut doc = Document::default();
            doc.routes.push(Route {
                name: "metadata regression".into(),
                family: "asf2".into(),
                points: vec![],
                parts: vec![PlacedPart {
                    model: c[0].path.clone(),
                    reverse: false,
                    position: [200100., 200.],
                    rotation: 0.,
                }],
                replaces: None,
            });
            export_editor(&base, &out, &doc, &c).unwrap();
            let output = std::fs::read(&out).unwrap();
            let result = block(&output, 0x8a).unwrap();
            assert_eq!(result.entries.len(), 2);
            let delta = (output.len() - bytes.len()) as u32;
            assert_eq!(u32(&output, 3).unwrap(), 1000 + delta);
            assert_eq!(u32(&output, 10).unwrap(), 2000 + delta);
            let kept = &result.entries[0];
            assert_eq!(&output[kept.start..kept.start + kept.len], &bytes[root.start..root.start + root.len]);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn optional_local_tv4p_roundtrip() {
        let Ok(path) = std::env::var("ROAD_TEST_TV4P") else {
            return;
        };
        let base = PathBuf::from(path);
        let bytes = fs::read(&base).unwrap();
        let bl = block(&bytes, 0x8a).unwrap();
        let bodies: Vec<_> = bl
            .entries
            .iter()
            .map(|e| bytes[e.start..e.start + e.len].to_vec())
            .collect();
        assert_eq!(build_list(0x8a, &bodies).unwrap(), bytes[bl.start..bl.end]);
        let out =
            std::env::temp_dir().join(format!("real-road-roundtrip-{}.tv4p", std::process::id()));
        export_editor(&base, &out, &crate::document::Document::default(), &[]).unwrap();
        assert_eq!(fs::read(&out).unwrap(), bytes);
        // Exercise metadata adjustment on the real project, not only a no-op export.
        if let Some(last) = bl.entries.last() {
            let mut doc = Document::default();
            doc.deleted.push(last.id);
            export_editor(&base, &out, &doc, &[]).unwrap();
            let output = fs::read(&out).unwrap();
            let changed = block(&output, 0x8a).unwrap();
            assert_eq!(changed.entries.len() + 1, bl.entries.len());
            assert!(!changed.entries.iter().any(|e| e.id == last.id));
            assert_eq!(&output[changed.end..], &bytes[bl.end..]);
        }
        fs::remove_file(out).unwrap();
    }

    #[test]
    fn deleting_absent_roads_is_safe_but_missing_edits_are_rejected() {
        let dir = std::env::temp_dir().join(format!("road-delete-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let base = dir.join("base.tv4p");
        let out = dir.join("out.tv4p");
        let bytes = fixture();
        fs::write(&base, &bytes).unwrap();
        let mut doc = Document::default();
        doc.deleted.push(999);
        export_editor(&base, &out, &doc, &[]).unwrap();
        assert_eq!(fs::read(&out).unwrap(), bytes);
        doc.deleted.push(1);
        export_editor(&base, &out, &doc, &[]).unwrap();
        let deleted = fs::read(&out).unwrap();
        assert!(block(&deleted, 0x8a).unwrap().entries.is_empty());
        // Re-export the same deletions using an already exported base.
        let again = dir.join("again.tv4p");
        export_editor(&out, &again, &doc, &[]).unwrap();
        assert_eq!(fs::read(&again).unwrap(), deleted);
        doc.transforms.push((999, [1., 0.], 0.));
        assert!(export_editor(&base, &out, &doc, &[]).is_err());
        assert_eq!(fs::read(&out).unwrap(), deleted);
        doc.transforms.clear();
        doc.routes.push(Route {
            name: "replacement".into(), family: "asf2".into(),
            points: vec![], parts: vec![], replaces: Some(999),
        });
        assert!(export_editor(&base, &out, &doc, &[]).is_err());
        assert_eq!(fs::read(&out).unwrap(), deleted);
        fs::remove_dir_all(dir).unwrap();
    }
}
