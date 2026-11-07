//! Visual meshes of P3D models (MLOD or ODOL) with texture coordinates, and the PAA
//! textures they use, for the 3D view.
use crate::paa;
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Triangles of one texture, in model coordinates (x right, y up, z forward).
#[derive(Default)]
pub struct Surface {
    pub texture: String,
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
}

#[derive(Default)]
pub struct ModelMesh {
    pub surfaces: Vec<Surface>,
    /// Lowest vertex, so road parts can be laid on the terrain.
    pub min_y: f32,
}
impl ModelMesh {
    pub fn triangles(&self) -> usize {
        self.surfaces.iter().map(|s| s.positions.len() / 3).sum()
    }
    fn from_faces(
        vertices: &[[f32; 3]],
        faces: impl Iterator<Item = (Vec<(usize, [f32; 2])>, String)>,
    ) -> Result<Self> {
        let mut by_texture: HashMap<String, Surface> = HashMap::new();
        for (face, texture) in faces {
            let s = by_texture
                .entry(texture.to_lowercase())
                .or_insert_with(|| Surface {
                    texture: texture.clone(),
                    ..Default::default()
                });
            for i in 1..face.len().saturating_sub(1) {
                for &(v, uv) in [&face[0], &face[i], &face[i + 1]] {
                    s.positions.push(
                        *vertices
                            .get(v)
                            .context("Nieprawidłowy indeks wierzchołka")?,
                    );
                    s.uvs.push(uv);
                }
            }
        }
        let min_y = vertices.iter().map(|v| v[1]).fold(f32::INFINITY, f32::min);
        let mut surfaces: Vec<_> = by_texture.into_values().collect();
        surfaces.sort_by(|a, b| a.texture.cmp(&b.texture));
        Ok(Self {
            surfaces,
            min_y: if min_y.is_finite() { min_y } else { 0. },
        })
    }
}

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}
impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self
            .b
            .get(self.p..self.p + n)
            .context("Nieoczekiwany koniec pliku P3D")?;
        self.p += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into()?))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.bytes(4)?.try_into()?))
    }
    fn string(&mut self) -> Result<String> {
        let rest = self
            .b
            .get(self.p..)
            .context("Nieoczekiwany koniec pliku P3D")?;
        let n = rest
            .iter()
            .position(|&c| c == 0)
            .context("Niezakończony napis w P3D")?;
        let s = String::from_utf8_lossy(&rest[..n]).into_owned();
        self.p += n + 1;
        Ok(s)
    }
}

type MlodFace = (Vec<(usize, [f32; 2])>, String);

fn mlod(b: &[u8]) -> Result<ModelMesh> {
    let mut r = Reader { b, p: 0 };
    ensure!(
        r.bytes(4)? == b"MLOD" && r.u32()? == 257,
        "Nieobsługiwany MLOD"
    );
    let lods = r.u32()?;
    ensure!(lods <= 1000, "Za dużo LOD w MLOD");
    let mut best: Option<(f32, Vec<[f32; 3]>, Vec<MlodFace>)> = None;
    for _ in 0..lods {
        ensure!(r.bytes(4)? == b"P3DM", "Nieobsługiwany LOD MLOD");
        r.bytes(8)?;
        let (nv, nn, nf) = (r.u32()? as usize, r.u32()? as usize, r.u32()? as usize);
        r.u32()?;
        ensure!(
            nv <= 2_000_000 && nn <= 2_000_000 && nf <= 2_000_000,
            "MLOD przekracza limity"
        );
        let mut vertices = Vec::with_capacity(nv);
        for _ in 0..nv {
            vertices.push([r.f32()?, r.f32()?, r.f32()?]);
            r.u32()?;
        }
        r.bytes(nn * 12)?;
        let mut faces = Vec::with_capacity(nf);
        for _ in 0..nf {
            let n = r.u32()? as usize;
            ensure!((3..=4).contains(&n), "Nieobsługiwany wielokąt MLOD");
            let mut face = Vec::with_capacity(n);
            for i in 0..4 {
                let v = r.u32()? as usize;
                r.u32()?;
                let uv = [r.f32()?, r.f32()?];
                if i < n {
                    face.push((v, uv));
                }
            }
            r.u32()?;
            let texture = r.string()?;
            r.string()?;
            faces.push((face, texture));
        }
        ensure!(r.bytes(4)? == b"TAGG", "Brak TAGG w MLOD");
        loop {
            r.bytes(1)?;
            let name = r.string()?;
            let len = r.u32()? as usize;
            r.bytes(len)?;
            if name == "#EndOfFile#" {
                break;
            }
        }
        let resolution = r.f32()?;
        // Resolution LODs are below 1000; pick the most detailed one.
        if !faces.is_empty() && resolution < 1000. && best.as_ref().is_none_or(|b| resolution < b.0)
        {
            best = Some((resolution, vertices, faces));
        }
    }
    let (_, vertices, faces) = best.context("Brak widocznego LOD w MLOD")?;
    ModelMesh::from_faces(&vertices, faces.into_iter())
}

fn odol(b: &[u8]) -> Result<ModelMesh> {
    let lod = crate::odol::read(b, |i, _| i == 0)?
        .into_iter()
        .next()
        .context("Brak LOD w ODOL")?;
    ensure!(
        lod.uvs.len() == lod.vertices.len(),
        "Niezgodna liczba UV w ODOL"
    );
    let faces = lod.faces.iter().zip(&lod.face_textures).map(|(f, &t)| {
        let texture = lod.textures.get(t as usize).cloned().unwrap_or_default();
        (
            f.iter()
                .map(|&v| {
                    (
                        v as usize,
                        lod.uvs.get(v as usize).copied().unwrap_or_default(),
                    )
                })
                .collect(),
            texture,
        )
    });
    ModelMesh::from_faces(&lod.vertices, faces)
}

/// Reads the most detailed visual LOD of a P3D file.
pub fn read(path: &Path) -> Result<ModelMesh> {
    let b =
        std::fs::read(path).with_context(|| format!("Nie można odczytać {}", path.display()))?;
    if b.starts_with(b"MLOD") {
        mlod(&b)
    } else if b.starts_with(b"ODOL") {
        odol(&b)
    } else {
        bail!("Nieznany format P3D {}", path.display())
    }
}

/// `#(argb,8,8,3)color(r,g,b,a,…)` procedural textures as a solid colour.
fn procedural(name: &str) -> Option<paa::Texture> {
    let args = name.split("color(").nth(1)?.split(')').next()?;
    let v: Vec<f32> = args
        .split(',')
        .take(4)
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    (v.len() >= 3).then(|| {
        let c = |x: f32| (x.clamp(0., 1.) * 255.) as u8;
        let px = [c(v[0]), c(v[1]), c(v[2]), c(*v.get(3).unwrap_or(&1.))];
        paa::Texture {
            width: 1,
            height: 1,
            rgba: px.to_vec(),
        }
    })
}

/// Texture files are given relative to the work drive (P:\); look upwards from the model.
fn resolve(name: &str, model: &Path) -> Option<PathBuf> {
    let name = name.trim_start_matches(['\\', '/']);
    let rel = Path::new(name);
    let paa = rel.with_extension("paa");
    model.ancestors().skip(1).find_map(|a| {
        [a.join(rel), a.join(&paa)]
            .into_iter()
            .find(|p| p.is_file())
    })
}

/// Meshes and textures loaded once and shared by later scene builds.
#[derive(Default, Clone)]
pub struct Cache {
    meshes: Arc<Mutex<HashMap<PathBuf, Option<Arc<ModelMesh>>>>>,
    textures: Arc<Mutex<HashMap<String, Option<Arc<paa::Texture>>>>>,
}
impl Cache {
    pub fn mesh(&self, file: &Path) -> Option<Arc<ModelMesh>> {
        if let Some(m) = self.meshes.lock().ok()?.get(file) {
            return m.clone();
        }
        let m = read(file).ok().filter(|m| m.triangles() > 0).map(Arc::new);
        self.meshes.lock().ok()?.insert(file.into(), m.clone());
        m
    }
    pub fn texture(&self, name: &str, model: &Path) -> Option<Arc<paa::Texture>> {
        if name.trim().is_empty() {
            return None;
        }
        let key = name.to_lowercase();
        if let Some(t) = self.textures.lock().ok()?.get(&key) {
            return t.clone();
        }
        let t = if name.starts_with('#') {
            procedural(name)
        } else {
            resolve(name, model).and_then(|p| paa::decode(&std::fs::read(p).ok()?, 256).ok())
        }
        .map(Arc::new);
        self.textures.lock().ok()?.insert(key, t.clone());
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procedural_colour_textures() {
        let t = procedural("#(argb,8,8,3)color(0.5,0.25,1,1,co)").unwrap();
        assert_eq!(t.rgba, vec![127, 63, 255, 255]);
        assert!(procedural("dz\\x_co.paa").is_none());
    }

    /// Real DayZ models when the P: drive is mounted; skipped otherwise.
    #[test]
    fn reads_textured_meshes_of_dayz_road_and_track_parts() {
        let road = Path::new("P:/dz/structures/roads/parts/asf2_25.p3d");
        let track = Path::new("P:/dz/structures/rail/Tracks/Rail_Track_25.p3d");
        if !road.exists() || !track.exists() {
            return;
        }
        let cache = Cache::default();
        for file in [road, track] {
            let m = cache.mesh(file).unwrap();
            assert!(m.triangles() >= 2, "{}", file.display());
            let length = m
                .surfaces
                .iter()
                .flat_map(|s| &s.positions)
                .map(|p| p[2])
                .fold((f32::MAX, f32::MIN), |r, z| (r.0.min(z), r.1.max(z)));
            assert!(
                length.1 - length.0 > 20.,
                "{} spans {length:?}",
                file.display()
            );
            let textured = m
                .surfaces
                .iter()
                .filter(|s| cache.texture(&s.texture, file).is_some())
                .count();
            assert!(
                textured > 0,
                "{}: {:?}",
                file.display(),
                m.surfaces.iter().map(|s| &s.texture).collect::<Vec<_>>()
            );
            for s in &m.surfaces {
                assert!(
                    s.uvs
                        .iter()
                        .all(|uv| uv[0].is_finite() && uv[1].is_finite())
                );
            }
        }
    }
}
