//! 3D preview: ASC terrain draped with the satellite image, the water level, and the
//! road and track P3D models.
//!
//! The terrain is a quadtree of tiles: tiles near the camera use the full ASC and satellite
//! resolution, distant ones are coarser, so the whole map stays visible up to the horizon.
//! Tiles and models are built on worker threads and drawn with OpenGL inside an egui paint
//! callback with a depth buffer.
use crate::{
    document::MapSettings,
    geometry::{self, Point},
    mesh3d, paa,
    raster::Raster,
    terrain::{Meta, Terrain},
};
use anyhow::Result;
use eframe::{egui, egui_glow, glow};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, Sender, TryRecvError, channel},
    },
};

/// Cells per tile side.
const CELLS: usize = 64;
/// Satellite texture side per tile at most.
const TILE_TEXTURE: u32 = 256;
/// A tile splits into four when the camera is closer than this many tile sizes.
const SPLIT: f64 = 2.2;
/// Tiles kept in memory at most.
const MAX_TILES: usize = 900;
/// Floats per vertex: position, normal, texture coordinate, colour.
const STRIDE: usize = 12;
/// Flat road footprints float this far above the terrain, in metres.
const LIFT: f32 = 0.3;
const WORKERS: usize = 3;

static GENERATION: AtomicU64 = AtomicU64::new(1);
fn generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// A world rectangle.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Region {
    pub min: Point,
    pub max: Point,
}
impl Region {
    pub fn size(&self) -> f64 {
        (self.max[0] - self.min[0]).max(self.max[1] - self.min[1])
    }
    pub fn center(&self) -> Point {
        [
            (self.min[0] + self.max[0]) / 2.,
            (self.min[1] + self.max[1]) / 2.,
        ]
    }
}

/// Bilinear ASC height, clamped to the sampled cell centres at the edges.
pub fn height(t: &Terrain, p: Point) -> Option<f64> {
    let m = &t.meta;
    let lo = [m.east + 0.5 * m.cell, m.north + 0.5 * m.cell];
    let hi = [
        m.east + (m.cols as f64 - 0.5) * m.cell,
        m.north + (m.rows as f64 - 0.5) * m.cell,
    ];
    t.sample([p[0].clamp(lo[0], hi[0]), p[1].clamp(lo[1], hi[1])])
}

/// Hypsometric tint: green lowland, yellow and brown hills, light peaks.
fn tint(t: f32) -> [f32; 3] {
    const STOPS: [(f32, [f32; 3]); 5] = [
        (0.0, [0.27, 0.45, 0.25]),
        (0.3, [0.52, 0.62, 0.32]),
        (0.6, [0.72, 0.62, 0.38]),
        (0.85, [0.55, 0.42, 0.30]),
        (1.0, [0.92, 0.92, 0.90]),
    ];
    let t = t.clamp(0., 1.);
    for w in STOPS.windows(2) {
        if t <= w[1].0 {
            let f = (t - w[0].0) / (w[1].0 - w[0].0);
            return std::array::from_fn(|k| w[0].1[k] + (w[1].1[k] - w[0].1[k]) * f);
        }
    }
    STOPS[4].1
}

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// World rectangle covered by the image.
    pub min: Point,
    pub max: Point,
}

/// Satellite pixels under `region` from the coarsest pyramid level that still gives
/// `max_px` pixels across it.
pub fn satellite(
    raster: &Raster,
    map: &MapSettings,
    region: Region,
    max_px: u32,
) -> Result<Option<Image>> {
    if !map.valid() {
        return Ok(None);
    }
    let size = region.size();
    let level = (0..raster.levels.len())
        .rev()
        .find(|&l| size / map.width * raster.levels[l].width as f64 >= max_px as f64)
        .unwrap_or(0);
    let l = &raster.levels[level];
    let px = |x: f64| ((x - map.east) / map.width * l.width as f64).clamp(0., l.width as f64);
    let py = |y: f64| {
        ((map.north + map.height - y) / map.height * l.height as f64).clamp(0., l.height as f64)
    };
    let (x0, x1) = (
        px(region.min[0]).floor() as u32,
        px(region.max[0]).ceil() as u32,
    );
    let (y0, y1) = (
        py(region.max[1]).floor() as u32,
        py(region.min[1]).ceil() as u32,
    );
    if x1 <= x0 || y1 <= y0 {
        return Ok(None);
    }
    let rgba = raster.region(level, x0, y0, x1 - x0, y1 - y0)?;
    let wx = |x: u32| map.east + x as f64 / l.width as f64 * map.width;
    let wy = |y: u32| map.north + map.height - y as f64 / l.height as f64 * map.height;
    Ok(Some(Image {
        width: x1 - x0,
        height: y1 - y0,
        rgba,
        min: [wx(x0), wy(y1)],
        max: [wx(x1), wy(y0)],
    }))
}

/// Interleaved vertices and triangles ready for the GPU.
#[derive(Default)]
pub struct Mesh {
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
}
impl Mesh {
    fn push(&mut self, p: [f32; 3], n: [f32; 3], uv: [f32; 2], c: [f32; 4]) -> u32 {
        let i = (self.vertices.len() / STRIDE) as u32;
        self.vertices.extend(p);
        self.vertices.extend(n);
        self.vertices.extend(uv);
        self.vertices.extend(c);
        i
    }
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TileKey {
    pub level: u8,
    pub x: u32,
    pub y: u32,
}
impl TileKey {
    fn children(self) -> [TileKey; 4] {
        let (l, x, y) = (self.level + 1, self.x * 2, self.y * 2);
        [
            TileKey { level: l, x, y },
            TileKey {
                level: l,
                x: x + 1,
                y,
            },
            TileKey {
                level: l,
                x,
                y: y + 1,
            },
            TileKey {
                level: l,
                x: x + 1,
                y: y + 1,
            },
        ]
    }
}

/// The quadtree over the map: level 0 is one square tile covering everything.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub extent: Region,
    pub size: f64,
    pub max_level: u8,
}
impl Layout {
    pub fn new(extent: Region, cell: f64) -> Self {
        let size = extent.size();
        let mut max_level = 0;
        while max_level < 14 && size / (1u64 << max_level) as f64 / CELLS as f64 > cell * 1.01 {
            max_level += 1;
        }
        Self {
            extent,
            size,
            max_level,
        }
    }
    /// The tile's rectangle cut to the map; `None` when it lies outside.
    pub fn rect(&self, k: TileKey) -> Option<Region> {
        let s = self.size / (1u64 << k.level) as f64;
        let min = [
            self.extent.min[0] + k.x as f64 * s,
            self.extent.min[1] + k.y as f64 * s,
        ];
        let max = [
            (min[0] + s).min(self.extent.max[0]),
            (min[1] + s).min(self.extent.max[1]),
        ];
        (max[0] > min[0] + 1e-6 && max[1] > min[1] + 1e-6).then_some(Region { min, max })
    }
    pub fn local(&self, p: Point, z: f64) -> [f32; 3] {
        [
            (p[0] - self.extent.min[0]) as f32,
            (p[1] - self.extent.min[1]) as f32,
            z as f32,
        ]
    }
}

pub struct TileData {
    pub mesh: Mesh,
    pub image: Option<Image>,
}
pub struct Tile {
    pub zmin: f32,
    pub zmax: f32,
    /// Taken by the GPU upload.
    data: Mutex<Option<TileData>>,
}

/// Everything a worker needs to build tiles.
#[derive(Clone)]
struct TileSource {
    terrain: Option<(PathBuf, Meta)>,
    raster: Option<(Raster, MapSettings)>,
    layout: Layout,
    range: (f32, f32),
}

/// Builds one terrain tile: a grid of `CELLS` cells, its satellite crop and skirts that hide
/// cracks against coarser neighbours.
pub fn build_tile(
    layout: &Layout,
    key: TileKey,
    terrain: Option<&Terrain>,
    sat: Option<(&Raster, &MapSettings)>,
    range: (f32, f32),
) -> Result<Option<Tile>> {
    let Some(rect) = layout.rect(key) else {
        return Ok(None);
    };
    let n = CELLS + 1;
    let step = [
        (rect.max[0] - rect.min[0]) / CELLS as f64,
        (rect.max[1] - rect.min[1]) / CELLS as f64,
    ];
    let world = |i: usize, j: usize| {
        [
            rect.min[0] + i as f64 * step[0],
            rect.min[1] + j as f64 * step[1],
        ]
    };
    let z: Vec<f64> = (0..n * n)
        .map(|k| {
            terrain
                .and_then(|t| height(t, world(k % n, k / n)))
                .unwrap_or(0.)
        })
        .collect();
    let at = |i: usize, j: usize| z[j.min(n - 1) * n + i.min(n - 1)];
    // A missing satellite leaves the tile tinted by elevation.
    let image = sat.and_then(|(r, m)| satellite(r, m, rect, TILE_TEXTURE).ok().flatten());
    let span = (range.1 - range.0).max(1.);
    let mut mesh = Mesh::default();
    let vertex = |mesh: &mut Mesh, i: usize, j: usize, dz: f64| {
        let gx = (at(i + 1, j) - at(i.saturating_sub(1), j))
            / (step[0] * (i.min(1) + (i + 1 < n) as usize) as f64);
        let gy = (at(i, j + 1) - at(i, j.saturating_sub(1)))
            / (step[1] * (j.min(1) + (j + 1 < n) as usize) as f64);
        let l = (gx * gx + gy * gy + 1.).sqrt();
        let w = world(i, j);
        let uv = image.as_ref().map_or([-1., -1.], |im| {
            [
                ((w[0] - im.min[0]) / (im.max[0] - im.min[0])) as f32,
                ((im.max[1] - w[1]) / (im.max[1] - im.min[1])) as f32,
            ]
        });
        let c = tint((at(i, j) as f32 - range.0) / span);
        mesh.push(
            layout.local(w, at(i, j) - dz),
            [(-gx / l) as f32, (-gy / l) as f32, (1. / l) as f32],
            uv,
            [c[0], c[1], c[2], 1.],
        )
    };
    for j in 0..n {
        for i in 0..n {
            vertex(&mut mesh, i, j, 0.);
        }
    }
    for j in 0..CELLS {
        for i in 0..CELLS {
            let a = (j * n + i) as u32;
            let (b, c, d) = (a + 1, a + 1 + n as u32, a + n as u32);
            mesh.indices.extend([a, b, c, a, c, d]);
        }
    }
    // Skirts hang below the four edges.
    let depth = step[0].max(step[1]) * 1.5 + 2.;
    let edges: [Vec<(usize, usize)>; 4] = [
        (0..n).map(|i| (i, 0)).collect(),
        (0..n).map(|i| (i, n - 1)).collect(),
        (0..n).map(|j| (0, j)).collect(),
        (0..n).map(|j| (n - 1, j)).collect(),
    ];
    for edge in edges {
        let first = (mesh.vertices.len() / STRIDE) as u32;
        for &(i, j) in &edge {
            vertex(&mut mesh, i, j, depth);
        }
        for (k, w) in edge.windows(2).enumerate() {
            let (a, b) = ((w[0].1 * n + w[0].0) as u32, (w[1].1 * n + w[1].0) as u32);
            let (c, d) = (first + k as u32 + 1, first + k as u32);
            mesh.indices.extend([a, b, c, a, c, d]);
        }
    }
    let (zmin, zmax) = z
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    Ok(Some(Tile {
        zmin: (zmin - depth) as f32,
        zmax: zmax as f32,
        data: Mutex::new(Some(TileData { mesh, image })),
    }))
}

struct Work {
    queue: Vec<TileKey>,
    inflight: HashSet<TileKey>,
    stop: bool,
}
/// Worker threads building tiles from a shared priority queue.
struct Workers {
    work: Arc<(Mutex<Work>, Condvar)>,
    rx: Receiver<(TileKey, Option<Arc<Tile>>)>,
}
impl Workers {
    fn new(source: TileSource) -> Self {
        let work = Arc::new((
            Mutex::new(Work {
                queue: vec![],
                inflight: HashSet::new(),
                stop: false,
            }),
            Condvar::new(),
        ));
        let (tx, rx) = channel();
        for _ in 0..WORKERS {
            let (work, tx, source): (_, Sender<_>, _) = (work.clone(), tx.clone(), source.clone());
            std::thread::spawn(move || {
                let terrain = source.terrain.as_ref().and_then(|(dir, meta)| {
                    let mut t = Terrain::open(dir).ok()?;
                    t.meta = meta.clone();
                    Some(t)
                });
                let sat = source.raster.as_ref().map(|(r, m)| (r, m));
                loop {
                    let key = {
                        let (lock, cv) = &*work;
                        let mut w = lock.lock().unwrap_or_else(|e| e.into_inner());
                        while w.queue.is_empty() && !w.stop {
                            w = cv.wait(w).unwrap_or_else(|e| e.into_inner());
                        }
                        if w.stop {
                            return;
                        }
                        let key = w.queue.remove(0);
                        w.inflight.insert(key);
                        key
                    };
                    let tile = build_tile(&source.layout, key, terrain.as_ref(), sat, source.range)
                        .ok()
                        .flatten()
                        .map(Arc::new);
                    work.0
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .inflight
                        .remove(&key);
                    if tx.send((key, tile)).is_err() {
                        return;
                    }
                }
            });
        }
        Self { work, rx }
    }
    fn request(&self, keys: Vec<TileKey>) {
        let (lock, cv) = &*self.work;
        let mut w = lock.lock().unwrap_or_else(|e| e.into_inner());
        w.queue = keys
            .into_iter()
            .filter(|k| !w.inflight.contains(k))
            .collect();
        cv.notify_all();
    }
    fn idle(&self) -> bool {
        let w = self.work.0.lock().unwrap_or_else(|e| e.into_inner());
        w.queue.is_empty() && w.inflight.is_empty()
    }
}
impl Drop for Workers {
    fn drop(&mut self) {
        let (lock, cv) = &*self.work;
        lock.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        cv.notify_all();
    }
}

/// One road or track in the scene.
pub struct RoadItem {
    /// 2D footprint, drawn when the models are hidden or missing.
    pub triangles: Vec<[Point; 3]>,
    pub color: [u8; 3],
    /// P3D file, rotation and translation of each part.
    pub placements: Vec<(PathBuf, f64, Point)>,
    /// Tracks stand level at their origin; road parts follow the terrain.
    pub rigid: bool,
    /// The selected road keeps its highlighted footprint over the models.
    pub highlight: bool,
}

pub struct Batch {
    pub texture: Option<Arc<paa::Texture>>,
    pub mesh: Mesh,
}
/// Road and track geometry laid on the terrain.
pub struct Roads {
    pub generation: u64,
    pub models: Vec<Batch>,
    /// Footprints of every road, for the footprint-only display.
    pub footprints: Mesh,
    /// Footprints of roads without models, and the highlighted one.
    pub fallback: Mesh,
    pub parts: usize,
    pub missing: usize,
}

fn drape(
    mesh: &mut Mesh,
    layout: &Layout,
    terrain: Option<&Terrain>,
    triangles: &[[Point; 3]],
    color: [u8; 3],
) {
    let c = [
        color[0] as f32 / 255.,
        color[1] as f32 / 255.,
        color[2] as f32 / 255.,
        1.,
    ];
    let mut stack: Vec<([Point; 3], u32)> = triangles.iter().map(|t| (*t, 0)).collect();
    while let Some((t, depth)) = stack.pop() {
        let edges = [0, 1, 2].map(|k| geometry::norm(geometry::sub(t[(k + 1) % 3], t[k])));
        let (k, longest) = edges
            .iter()
            .enumerate()
            .fold((0, 0.), |m, (k, &e)| if e > m.1 { (k, e) } else { m });
        if longest > 3. && depth < 16 {
            let (a, b, o) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
            let mid = geometry::mul(geometry::add(a, b), 0.5);
            stack.push(([a, mid, o], depth + 1));
            stack.push(([mid, b, o], depth + 1));
            continue;
        }
        for p in t {
            let z = terrain.and_then(|t| height(t, p)).unwrap_or(0.) + LIFT as f64;
            let i = mesh.push(layout.local(p, z), [0., 0., 1.], [-1., -1.], c);
            mesh.indices.push(i);
        }
    }
}

/// Places every part's P3D mesh: road parts follow the terrain under each vertex, tracks
/// stand level at the terrain height of their origin, as Terrain Builder places them.
pub fn build_roads(
    layout: &Layout,
    terrain: Option<&Terrain>,
    items: &[RoadItem],
    cache: &mesh3d::Cache,
) -> Roads {
    let mut batches: HashMap<String, Batch> = HashMap::new();
    let mut footprints = Mesh::default();
    let mut fallback = Mesh::default();
    let (mut parts, mut missing) = (0, 0);
    let ground = |p: Point| terrain.and_then(|t| height(t, p)).unwrap_or(0.);
    for item in items {
        drape(
            &mut footprints,
            layout,
            terrain,
            &item.triangles,
            item.color,
        );
        let mut complete = !item.placements.is_empty();
        for (file, angle, t) in &item.placements {
            parts += 1;
            let Some(m) = cache.mesh(file) else {
                missing += 1;
                complete = false;
                continue;
            };
            let base = ground(*t);
            for s in &m.surfaces {
                let texture = cache.texture(&s.texture, file);
                let key = s.texture.to_lowercase();
                let batch = batches.entry(key).or_insert_with(|| Batch {
                    texture: texture.clone(),
                    mesh: Mesh::default(),
                });
                let color = if texture.is_some() {
                    [1., 1., 1., 1.]
                } else {
                    [0.55, 0.55, 0.55, 1.]
                };
                for (tri, uv) in s.positions.chunks(3).zip(s.uvs.chunks(3)) {
                    let corners: Vec<[f32; 3]> = tri
                        .iter()
                        .map(|v| {
                            let w = geometry::add(
                                geometry::rotate([v[0] as f64, v[2] as f64], *angle),
                                *t,
                            );
                            let z = if item.rigid {
                                base + v[1] as f64
                            } else {
                                ground(w) + (v[1] - m.min_y) as f64 + 0.04
                            };
                            layout.local(w, z)
                        })
                        .collect();
                    let e1 = [
                        corners[1][0] - corners[0][0],
                        corners[1][1] - corners[0][1],
                        corners[1][2] - corners[0][2],
                    ];
                    let e2 = [
                        corners[2][0] - corners[0][0],
                        corners[2][1] - corners[0][1],
                        corners[2][2] - corners[0][2],
                    ];
                    let mut n = normalize(cross(e1, e2));
                    if n[2] < 0. {
                        n = [-n[0], -n[1], -n[2]];
                    }
                    for k in 0..3 {
                        let i = batch.mesh.push(corners[k], n, uv[k], color);
                        batch.mesh.indices.push(i);
                    }
                }
            }
        }
        if !complete || item.highlight {
            drape(&mut fallback, layout, terrain, &item.triangles, item.color);
        }
    }
    let mut models: Vec<_> = batches
        .into_values()
        .filter(|b| !b.mesh.indices.is_empty())
        .collect();
    models.sort_by_key(|b| b.texture.is_none());
    Roads {
        generation: generation(),
        models,
        footprints,
        fallback,
        parts,
        missing,
    }
}

/// Orbit camera around a world point.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub target: Point,
    /// Heading in radians; 0 looks north.
    pub yaw: f32,
    /// Elevation above the horizon in radians.
    pub pitch: f32,
    pub distance: f32,
}
impl Camera {
    pub fn new(target: Point, size: f64) -> Self {
        Self {
            target,
            yaw: 0.,
            pitch: 0.75,
            distance: (size * 0.75) as f32,
        }
    }
    fn eye(&self, t: [f32; 3]) -> [f32; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [
            t[0] - self.distance * sy * cp,
            t[1] - self.distance * cy * cp,
            t[2] + self.distance * sp,
        ]
    }
    /// Eye position and view-projection matrix in the layout's local coordinates.
    pub fn matrix(
        &self,
        layout: &Layout,
        target_z: f64,
        exaggeration: f32,
        aspect: f32,
    ) -> ([f32; 3], [f32; 16]) {
        let t = layout.local(self.target, target_z * exaggeration as f64);
        let eye = self.eye(t);
        let near = (self.distance * 0.004).clamp(0.3, 50.);
        let far = self.distance * 4. + layout.size as f32 * 3.;
        (
            eye,
            mul(
                &perspective(0.8, aspect.max(0.01), near, far),
                &look_at(eye, t, [0., 0., 1.]),
            ),
        )
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt().max(1e-12);
    [a[0] / l, a[1] / l, a[2] / l]
}
/// Column-major matrices, as OpenGL expects them.
#[rustfmt::skip]
fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> [f32; 16] {
    let f = 1. / (fovy / 2.).tan();
    [
        f / aspect, 0., 0., 0.,
        0., f, 0., 0.,
        0., 0., (far + near) / (near - far), -1.,
        0., 0., 2. * far * near / (near - far), 0.,
    ]
}
#[rustfmt::skip]
fn look_at(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> [f32; 16] {
    let f = normalize([center[0] - eye[0], center[1] - eye[1], center[2] - eye[2]]);
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    [
        s[0], u[0], -f[0], 0.,
        s[1], u[1], -f[1], 0.,
        s[2], u[2], -f[2], 0.,
        -dot(s, eye), -dot(u, eye), dot(f, eye), 1.,
    ]
}
fn mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    std::array::from_fn(|i| {
        let (c, r) = (i / 4, i % 4);
        (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum()
    })
}
fn clip(m: &[f32; 16], p: [f32; 3]) -> [f32; 4] {
    std::array::from_fn(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
}
/// Normalised device coordinates of a local point in front of the camera.
pub fn project(m: &[f32; 16], p: [f32; 3]) -> Option<[f32; 3]> {
    let v = clip(m, p);
    (v[3] > 0.).then(|| [v[0] / v[3], v[1] / v[3], v[2] / v[3]])
}
/// False when the box is certainly outside the view.
fn visible(m: &[f32; 16], lo: [f32; 3], hi: [f32; 3]) -> bool {
    let corners: Vec<[f32; 4]> = (0..8)
        .map(|k| {
            clip(
                m,
                [
                    if k & 1 == 0 { lo[0] } else { hi[0] },
                    if k & 2 == 0 { lo[1] } else { hi[1] },
                    if k & 4 == 0 { lo[2] } else { hi[2] },
                ],
            )
        })
        .collect();
    for axis in 0..3 {
        if corners.iter().all(|c| c[axis] > c[3]) || corners.iter().all(|c| c[axis] < -c[3]) {
            return false;
        }
    }
    true
}

/// What the scene is built from, gathered by the application.
pub struct Source<'a> {
    pub terrain: Option<&'a Terrain>,
    pub terrain_dir: Option<&'a std::path::Path>,
    pub raster: Option<&'a Raster>,
    pub map: &'a MapSettings,
    pub water_level: f64,
    /// Changes whenever roads, tracks, colours or the selection change.
    pub roads_key: u64,
}
impl Source<'_> {
    /// World rectangle with data to show: the ASC, or else the satellite map.
    pub fn extent(&self) -> Option<Region> {
        if let Some(t) = self.terrain {
            let m = &t.meta;
            return Some(Region {
                min: [m.east, m.north],
                max: [
                    m.east + m.cols as f64 * m.cell,
                    m.north + m.rows as f64 * m.cell,
                ],
            });
        }
        (self.raster.is_some() && self.map.valid()).then_some(Region {
            min: [self.map.east, self.map.north],
            max: [
                self.map.east + self.map.width,
                self.map.north + self.map.height,
            ],
        })
    }
    fn tiles_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.terrain_dir.hash(&mut h);
        if let Some(t) = self.terrain {
            [t.meta.east, t.meta.north, t.meta.cell]
                .map(f64::to_bits)
                .hash(&mut h);
        }
        self.raster.map(|r| &r.dir).hash(&mut h);
        [
            self.map.east,
            self.map.north,
            self.map.width,
            self.map.height,
        ]
        .map(f64::to_bits)
        .hash(&mut h);
        h.finish()
    }
}

/// Height range of the terrain from a coarse sample, for the elevation tint.
fn range(t: Option<&Terrain>, extent: Region) -> (f32, f32) {
    let Some(t) = t else { return (0., 1.) };
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for j in 0..=128 {
        for i in 0..=128 {
            let p = [
                extent.min[0] + (extent.max[0] - extent.min[0]) * i as f64 / 128.,
                extent.min[1] + (extent.max[1] - extent.min[1]) * j as f64 / 128.,
            ];
            if let Some(z) = height(t, p) {
                lo = lo.min(z);
                hi = hi.max(z);
            }
        }
    }
    if lo.is_finite() {
        (lo as f32, hi as f32)
    } else {
        (0., 1.)
    }
}

type RoadsJob = (u64, Receiver<Roads>);

pub struct Scene {
    pub camera: Option<Camera>,
    pub show_satellite: bool,
    pub show_water: bool,
    pub show_roads: bool,
    pub show_models: bool,
    pub exaggeration: f32,
    pub error: Option<String>,
    layout: Option<Layout>,
    range: (f32, f32),
    tiles_key: Option<u64>,
    workers: Option<Workers>,
    tiles: HashMap<TileKey, (Arc<Tile>, u64)>,
    visible: Vec<(TileKey, Arc<Tile>)>,
    frame: u64,
    epoch: u64,
    roads: Option<Arc<Roads>>,
    roads_key: Option<u64>,
    roads_job: Option<RoadsJob>,
    cache: mesh3d::Cache,
    gpu: Arc<Mutex<Gpu>>,
}
impl Default for Scene {
    fn default() -> Self {
        Self {
            camera: None,
            show_satellite: true,
            show_water: true,
            show_roads: true,
            show_models: true,
            exaggeration: 1.,
            error: None,
            layout: None,
            range: (0., 1.),
            tiles_key: None,
            workers: None,
            tiles: HashMap::new(),
            visible: vec![],
            frame: 0,
            epoch: generation(),
            roads: None,
            roads_key: None,
            roads_job: None,
            cache: Default::default(),
            gpu: Default::default(),
        }
    }
}

impl Scene {
    /// Tiles or road models are still being built.
    pub fn building(&self) -> bool {
        self.roads_job.is_some() || self.workers.as_ref().is_some_and(|w| !w.idle())
    }
    pub fn layout(&self) -> Option<&Layout> {
        self.layout.as_ref()
    }
    pub fn range(&self) -> (f32, f32) {
        self.range
    }
    pub fn roads(&self) -> Option<&Roads> {
        self.roads.as_deref()
    }
    pub fn visible_tiles(&self) -> impl Iterator<Item = TileKey> + '_ {
        self.visible.iter().map(|(k, _)| *k)
    }

    /// Starts at the part of the map the 2D view showed.
    pub fn enter(&mut self, view: Region, extent: Option<Region>) {
        if extent.is_none() {
            return;
        }
        let size = (view.size() * 1.2).clamp(300., 30000.);
        match &mut self.camera {
            Some(c) => c.target = view.center(),
            None => self.camera = Some(Camera::new(view.center(), size)),
        }
    }

    fn reset(&mut self, source: &Source, extent: Region) {
        let cell = source.terrain.map_or_else(
            || {
                if source.map.valid() {
                    source.map.width / source.raster.map_or(1, |r| r.levels[0].width) as f64
                } else {
                    1.
                }
            },
            |t| t.meta.cell,
        );
        let layout = Layout::new(extent, cell.max(0.5));
        self.range = range(source.terrain, extent);
        let tiles = TileSource {
            terrain: source
                .terrain_dir
                .map(PathBuf::from)
                .zip(source.terrain.map(|t| t.meta.clone())),
            raster: source.raster.cloned().zip(Some(source.map.clone())),
            layout,
            range: self.range,
        };
        self.workers = Some(Workers::new(tiles));
        self.layout = Some(layout);
        self.tiles.clear();
        self.visible.clear();
        self.epoch = generation();
        self.roads_key = None;
        if let Some(c) = &mut self.camera {
            c.target = [
                c.target[0].clamp(extent.min[0], extent.max[0]),
                c.target[1].clamp(extent.min[1], extent.max[1]),
            ];
        } else {
            self.camera = Some(Camera::new(extent.center(), extent.size()));
        }
    }

    /// Collects finished work, chooses the tiles to draw for the current camera and starts
    /// whatever is missing.
    pub fn update(&mut self, source: &Source, aspect: f32, roads: impl FnOnce() -> Vec<RoadItem>) {
        let Some(extent) = source.extent() else {
            self.layout = None;
            self.workers = None;
            self.visible.clear();
            return;
        };
        let key = source.tiles_key();
        if self.tiles_key != Some(key) || self.layout.is_none() {
            self.tiles_key = Some(key);
            self.reset(source, extent);
        }
        let (Some(layout), Some(camera)) = (self.layout, self.camera) else {
            return;
        };
        self.frame += 1;
        if let Some(w) = &self.workers {
            while let Ok((k, tile)) = w.rx.try_recv() {
                if let Some(t) = tile {
                    self.tiles.insert(k, (t, self.frame));
                }
            }
        }
        // Tile selection.
        let target_z = source
            .terrain
            .and_then(|t| height(t, camera.target))
            .unwrap_or(0.);
        let (eye, mvp) = camera.matrix(&layout, target_z, self.exaggeration, aspect);
        let mut out = Vec::new();
        let mut requests = Vec::new();
        self.select(
            TileKey {
                level: 0,
                x: 0,
                y: 0,
            },
            &layout,
            eye,
            &mvp,
            &mut out,
            &mut requests,
        );
        requests.sort_by(|a: &(u8, f64, TileKey), b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        requests.dedup_by_key(|r| r.2);
        if let Some(w) = &self.workers {
            w.request(requests.into_iter().map(|r| r.2).collect());
        }
        self.visible = out
            .into_iter()
            .filter_map(|k| self.tiles.get(&k).map(|t| (k, t.0.clone())))
            .collect();
        if self.tiles.len() > MAX_TILES {
            let shown: HashSet<_> = self.visible.iter().map(|v| v.0).collect();
            let mut old: Vec<_> = self
                .tiles
                .iter()
                .filter(|(k, _)| !shown.contains(k) && k.level > 0)
                .map(|(k, v)| (v.1, *k))
                .collect();
            old.sort_by_key(|o| o.0);
            let mut gpu = self.gpu.lock().unwrap_or_else(|e| e.into_inner());
            for (_, k) in old.into_iter().take(self.tiles.len() - MAX_TILES * 3 / 4) {
                self.tiles.remove(&k);
                gpu.evicted.push(k);
            }
        }
        // Road and track models.
        if let Some((_, rx)) = &self.roads_job {
            match rx.try_recv() {
                Ok(r) => {
                    let (k, _) = self.roads_job.take().unwrap();
                    self.roads_key = Some(k);
                    self.roads = Some(Arc::new(r));
                }
                Err(TryRecvError::Empty) => return,
                Err(_) => self.roads_job = None,
            }
        }
        let roads_key = source.roads_key ^ key.rotate_left(7);
        if self.roads_key != Some(roads_key) {
            let items = roads();
            let terrain = source
                .terrain_dir
                .map(PathBuf::from)
                .zip(source.terrain.map(|t| t.meta.clone()));
            let cache = self.cache.clone();
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let t = terrain.and_then(|(dir, meta)| {
                    let mut t = Terrain::open(&dir).ok()?;
                    t.meta = meta;
                    Some(t)
                });
                let _ = tx.send(build_roads(&layout, t.as_ref(), &items, &cache));
            });
            self.roads_job = Some((roads_key, rx));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn select(
        &mut self,
        key: TileKey,
        layout: &Layout,
        eye: [f32; 3],
        mvp: &[f32; 16],
        out: &mut Vec<TileKey>,
        requests: &mut Vec<(u8, f64, TileKey)>,
    ) {
        let Some(rect) = layout.rect(key) else { return };
        let ex = self.exaggeration;
        let bounds = |k: TileKey, r: Region, tiles: &HashMap<TileKey, (Arc<Tile>, u64)>| {
            let (zmin, zmax) = tiles
                .get(&k)
                .map_or((self.range.0 - 50., self.range.1), |t| (t.0.zmin, t.0.zmax));
            let lo = layout.local(r.min, (zmin * ex) as f64);
            let hi = layout.local(r.max, (zmax * ex) as f64);
            (lo, hi)
        };
        let distance = |lo: [f32; 3], hi: [f32; 3]| {
            let d: [f32; 3] = std::array::from_fn(|k| (lo[k] - eye[k]).max(eye[k] - hi[k]).max(0.));
            dot(d, d).sqrt() as f64
        };
        let (lo, hi) = bounds(key, rect, &self.tiles);
        if key.level > 0 && !visible(mvp, lo, hi) {
            return;
        }
        let d = distance(lo, hi);
        let Some(entry) = self.tiles.get_mut(&key) else {
            requests.push((key.level, d, key));
            return;
        };
        entry.1 = self.frame;
        let size = (rect.max[0] - rect.min[0]).max(rect.max[1] - rect.min[1]);
        if key.level < layout.max_level && d < size * SPLIT {
            let children: Vec<_> = key
                .children()
                .into_iter()
                .filter_map(|c| layout.rect(c).map(|r| (c, r)))
                .collect();
            let mut ready = true;
            for &(c, r) in &children {
                if !self.tiles.contains_key(&c) {
                    let (lo, hi) = bounds(c, r, &self.tiles);
                    if visible(mvp, lo, hi) {
                        ready = false;
                        requests.push((c.level, distance(lo, hi), c));
                    }
                }
            }
            if ready {
                for (c, _) in children {
                    self.select(c, layout, eye, mvp, out, requests);
                }
                return;
            }
        }
        out.push(key);
    }

    /// Blocks until tiles and models for the current view are built (screenshots and tests).
    pub fn settle(&mut self, source: &Source, aspect: f32, roads: impl Fn() -> Vec<RoadItem>) {
        for _ in 0..600 {
            self.update(source, aspect, &roads);
            if !self.building() && self.roads.is_some() {
                // One more pass picks up the last tiles.
                self.update(source, aspect, &roads);
                if !self.building() {
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Orbit with the left button, pan with the right or middle one, zoom with the wheel.
    pub fn input(&mut self, ui: &egui::Ui, response: &egui::Response) {
        let Some(c) = &mut self.camera else { return };
        let d = ui.input(|i| i.pointer.delta());
        if response.dragged_by(egui::PointerButton::Primary) {
            c.yaw += d.x * 0.008;
            c.pitch = (c.pitch + d.y * 0.006).clamp(0.03, 1.55);
        }
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            let k = c.distance as f64 / response.rect.height().max(1.) as f64 * 1.2;
            let (sy, cy) = (c.yaw as f64).sin_cos();
            let (dx, dy) = (d.x as f64 * k, d.y as f64 * k);
            c.target[0] += -dx * cy + dy * sy;
            c.target[1] += dx * sy + dy * cy;
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0. {
                c.distance = (c.distance * (-scroll * 0.0015).exp()).clamp(8., 60000.);
            }
        }
        if let Some(l) = &self.layout {
            let e = l.extent;
            c.target = [
                c.target[0].clamp(e.min[0], e.max[0]),
                c.target[1].clamp(e.min[1], e.max[1]),
            ];
        }
    }

    pub fn reset_camera(&mut self) {
        if let Some(l) = &self.layout {
            let target = self.camera.map_or(l.extent.center(), |c| c.target);
            self.camera = Some(Camera::new(target, l.size.min(3000.)));
        }
    }

    /// Paints the scene into `rect` through an OpenGL callback.
    pub fn paint(&self, ui: &egui::Ui, rect: egui::Rect, source: &Source) {
        let (Some(layout), Some(camera)) = (self.layout, self.camera) else {
            return;
        };
        let target_z = source
            .terrain
            .and_then(|t| height(t, camera.target))
            .unwrap_or(0.);
        let (_, mvp) = camera.matrix(
            &layout,
            target_z,
            self.exaggeration,
            rect.width() / rect.height().max(1.),
        );
        let frame = Frame {
            epoch: self.epoch,
            layout,
            tiles: self.visible.clone(),
            roads: self.roads.clone().filter(|_| self.show_roads),
            models: self.show_models,
            water: self.show_water.then_some(source.water_level as f32),
            textured: self.show_satellite,
            exaggeration: self.exaggeration,
            mvp,
        };
        let gpu = self.gpu.clone();
        let callback = egui_glow::CallbackFn::new(move |_info, painter| {
            let mut gpu = gpu.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: called on the UI thread with egui's current GL context.
            unsafe {
                if let Err(e) = gpu.draw(painter.gl(), &frame) {
                    gpu.error = Some(e);
                }
            }
        });
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(callback),
        });
    }

    pub fn gpu_error(&self) -> Option<String> {
        self.gpu.lock().ok().and_then(|g| g.error.clone())
    }
}

/// Everything one paint needs, moved into the GL callback.
struct Frame {
    epoch: u64,
    layout: Layout,
    tiles: Vec<(TileKey, Arc<Tile>)>,
    roads: Option<Arc<Roads>>,
    models: bool,
    water: Option<f32>,
    textured: bool,
    exaggeration: f32,
    mvp: [f32; 16],
}

const VERTEX: &str = r#"
in vec3 a_pos;
in vec3 a_normal;
in vec2 a_uv;
in vec4 a_color;
uniform mat4 u_mvp;
uniform float u_z;
out vec3 v_normal;
out vec2 v_uv;
out vec4 v_color;
void main() {
    v_normal = normalize(vec3(a_normal.xy * u_z, a_normal.z));
    v_uv = a_uv;
    v_color = a_color;
    gl_Position = u_mvp * vec4(a_pos.xy, a_pos.z * u_z, 1.0);
}
"#;
const FRAGMENT: &str = r#"
in vec3 v_normal;
in vec2 v_uv;
in vec4 v_color;
uniform sampler2D u_tex;
uniform int u_textured;
uniform int u_clamp;
uniform int u_lit;
out vec4 out_color;
void main() {
    vec4 base = v_color;
    bool inside = u_clamp == 0 || (v_uv.x >= 0.0 && v_uv.y >= 0.0 && v_uv.x <= 1.0 && v_uv.y <= 1.0);
    if (u_textured == 1 && inside) {
        vec4 t = texture(u_tex, v_uv);
        if (u_clamp == 0 && t.a < 0.35) discard;
        // Terrain vertices carry the elevation tint, which must not colour the satellite.
        base = vec4(u_clamp == 1 ? t.rgb : t.rgb * v_color.rgb, v_color.a);
    }
    float light = 1.0;
    if (u_lit == 1) {
        vec3 sun = normalize(vec3(-0.45, 0.35, 0.82));
        light = 0.5 + 0.62 * max(dot(normalize(v_normal), sun), 0.0);
    }
    out_color = vec4(base.rgb * light, base.a);
}
"#;

struct Buffers {
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    count: i32,
}
/// Road generation, textured model batches, all footprints, fallback footprints.
type GpuRoads = (u64, Vec<(Buffers, Option<glow::Texture>)>, Buffers, Buffers);
struct GpuTile {
    buffers: Buffers,
    texture: Option<glow::Texture>,
}

/// GPU objects of the scene; they live on the UI thread's GL context.
#[derive(Default)]
struct Gpu {
    program: Option<glow::Program>,
    epoch: u64,
    tiles: HashMap<TileKey, GpuTile>,
    evicted: Vec<TileKey>,
    roads: Option<GpuRoads>,
    water: Option<(u64, Buffers)>,
    error: Option<String>,
}

impl Gpu {
    unsafe fn program(&mut self, gl: &glow::Context) -> Result<glow::Program, String> {
        use glow::HasContext as _;
        if let Some(p) = self.program {
            return Ok(p);
        }
        let version = egui_glow::ShaderVersion::get(gl);
        if !version.is_new_shader_interface() {
            return Err("Widok 3D wymaga OpenGL 3.0 lub nowszego".into());
        }
        let header = format!(
            "{}\n#ifdef GL_ES\nprecision highp float;\n#endif\n",
            version.version_declaration()
        );
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = vec![];
            for (kind, source) in [
                (glow::VERTEX_SHADER, VERTEX),
                (glow::FRAGMENT_SHADER, FRAGMENT),
            ] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, &format!("{header}{source}"));
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    return Err(gl.get_shader_info_log(shader));
                }
                gl.attach_shader(program, shader);
                shaders.push(shader);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(gl.get_program_info_log(program));
            }
            for s in shaders {
                gl.detach_shader(program, s);
                gl.delete_shader(s);
            }
            self.program = Some(program);
            Ok(program)
        }
    }

    unsafe fn upload(
        gl: &glow::Context,
        program: glow::Program,
        mesh: &Mesh,
    ) -> Result<Buffers, String> {
        use glow::HasContext as _;
        unsafe {
            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            let ebo = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&mesh.vertices), glow::STATIC_DRAW);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                bytes(&mesh.indices),
                glow::STATIC_DRAW,
            );
            let stride = (STRIDE * 4) as i32;
            for (name, size, offset) in [
                ("a_pos", 3, 0),
                ("a_normal", 3, 3),
                ("a_uv", 2, 6),
                ("a_color", 4, 8),
            ] {
                if let Some(loc) = gl.get_attrib_location(program, name) {
                    gl.enable_vertex_attrib_array(loc);
                    gl.vertex_attrib_pointer_f32(loc, size, glow::FLOAT, false, stride, offset * 4);
                }
            }
            gl.bind_vertex_array(None);
            Ok(Buffers {
                vao,
                vbo,
                ebo,
                count: mesh.indices.len() as i32,
            })
        }
    }

    unsafe fn texture(
        gl: &glow::Context,
        w: u32,
        h: u32,
        rgba: &[u8],
        repeat: bool,
    ) -> Result<glow::Texture, String> {
        use glow::HasContext as _;
        unsafe {
            let t = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(t));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                w as i32,
                h as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(rgba)),
            );
            gl.generate_mipmap(glow::TEXTURE_2D);
            let wrap = if repeat {
                glow::REPEAT
            } else {
                glow::CLAMP_TO_EDGE
            } as i32;
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR_MIPMAP_LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, wrap);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, wrap);
            Ok(t)
        }
    }

    unsafe fn free(gl: &glow::Context, b: Buffers) {
        use glow::HasContext as _;
        unsafe {
            gl.delete_vertex_array(b.vao);
            gl.delete_buffer(b.vbo);
            gl.delete_buffer(b.ebo);
        }
    }

    unsafe fn free_tile(gl: &glow::Context, t: GpuTile) {
        use glow::HasContext as _;
        unsafe {
            Self::free(gl, t.buffers);
            if let Some(tex) = t.texture {
                gl.delete_texture(tex);
            }
        }
    }

    unsafe fn draw(&mut self, gl: &glow::Context, f: &Frame) -> Result<(), String> {
        use glow::HasContext as _;
        unsafe {
            let program = self.program(gl)?;
            if self.epoch != f.epoch {
                for (_, t) in self.tiles.drain() {
                    Self::free_tile(gl, t);
                }
                self.evicted.clear();
                self.epoch = f.epoch;
            }
            for k in std::mem::take(&mut self.evicted) {
                if let Some(t) = self.tiles.remove(&k) {
                    Self::free_tile(gl, t);
                }
            }
            for (k, tile) in &f.tiles {
                if self.tiles.contains_key(k) {
                    continue;
                }
                let Some(data) = tile.data.lock().ok().and_then(|mut d| d.take()) else {
                    continue;
                };
                let buffers = Self::upload(gl, program, &data.mesh)?;
                let texture = match &data.image {
                    Some(im) => Some(Self::texture(gl, im.width, im.height, &im.rgba, false)?),
                    None => None,
                };
                self.tiles.insert(*k, GpuTile { buffers, texture });
            }
            if let Some(r) = &f.roads
                && self.roads.as_ref().map(|x| x.0) != Some(r.generation)
            {
                if let Some((_, models, a, b)) = self.roads.take() {
                    for (m, t) in models {
                        Self::free(gl, m);
                        if let Some(t) = t {
                            gl.delete_texture(t);
                        }
                    }
                    Self::free(gl, a);
                    Self::free(gl, b);
                }
                let mut models = Vec::new();
                for b in &r.models {
                    let t = match &b.texture {
                        Some(t) => Some(Self::texture(gl, t.width, t.height, &t.rgba, true)?),
                        None => None,
                    };
                    models.push((Self::upload(gl, program, &b.mesh)?, t));
                }
                self.roads = Some((
                    r.generation,
                    models,
                    Self::upload(gl, program, &r.footprints)?,
                    Self::upload(gl, program, &r.fallback)?,
                ));
            }
            if self.water.as_ref().map(|w| w.0) != Some(f.epoch) {
                if let Some((_, b)) = self.water.take() {
                    Self::free(gl, b);
                }
                let e = f.layout.extent;
                let (w, h) = ((e.max[0] - e.min[0]) as f32, (e.max[1] - e.min[1]) as f32);
                let mut m = Mesh::default();
                for p in [[-w, -h], [2. * w, -h], [2. * w, 2. * h], [-w, 2. * h]] {
                    // z = 1 becomes the water level through u_z.
                    m.push(
                        [p[0], p[1], 1.],
                        [0., 0., 1.],
                        [-1., -1.],
                        [0.16, 0.42, 0.66, 0.62],
                    );
                }
                m.indices.extend([0, 1, 2, 0, 2, 3]);
                self.water = Some((f.epoch, Self::upload(gl, program, &m)?));
            }

            let loc = |name: &str| gl.get_uniform_location(program, name);
            let (u_z, u_textured, u_clamp, u_lit) =
                (loc("u_z"), loc("u_textured"), loc("u_clamp"), loc("u_lit"));
            gl.use_program(Some(program));
            gl.uniform_matrix_4_f32_slice(loc("u_mvp").as_ref(), false, &f.mvp);
            gl.uniform_1_f32(u_z.as_ref(), f.exaggeration);
            gl.uniform_1_i32(loc("u_tex").as_ref(), 0);
            gl.active_texture(glow::TEXTURE0);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
            gl.clear_depth_f32(1.);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            let draw = |b: &Buffers| {
                gl.bind_vertex_array(Some(b.vao));
                gl.draw_elements(glow::TRIANGLES, b.count, glow::UNSIGNED_INT, 0);
            };

            gl.uniform_1_i32(u_lit.as_ref(), 1);
            gl.uniform_1_i32(u_clamp.as_ref(), 1);
            for (k, _) in &f.tiles {
                if let Some(t) = self.tiles.get(k) {
                    gl.uniform_1_i32(
                        u_textured.as_ref(),
                        (f.textured && t.texture.is_some()) as i32,
                    );
                    gl.bind_texture(glow::TEXTURE_2D, t.texture);
                    draw(&t.buffers);
                }
            }
            if let Some((_, models, all, fallback)) = &self.roads
                && f.roads.is_some()
            {
                gl.enable(glow::POLYGON_OFFSET_FILL);
                gl.polygon_offset(-2., -8.);
                if f.models {
                    gl.uniform_1_i32(u_clamp.as_ref(), 0);
                    for (b, t) in models {
                        gl.uniform_1_i32(u_textured.as_ref(), t.is_some() as i32);
                        gl.bind_texture(glow::TEXTURE_2D, *t);
                        draw(b);
                    }
                }
                gl.uniform_1_i32(u_textured.as_ref(), 0);
                gl.uniform_1_i32(u_lit.as_ref(), 0);
                gl.polygon_offset(-3., -12.);
                draw(if f.models { fallback } else { all });
                gl.disable(glow::POLYGON_OFFSET_FILL);
            }
            if let (Some(level), Some((_, b))) = (f.water, &self.water) {
                gl.uniform_1_f32(u_z.as_ref(), level * f.exaggeration);
                gl.uniform_1_i32(u_textured.as_ref(), 0);
                gl.uniform_1_i32(u_lit.as_ref(), 0);
                gl.enable(glow::BLEND);
                gl.blend_func_separate(
                    glow::SRC_ALPHA,
                    glow::ONE_MINUS_SRC_ALPHA,
                    glow::ONE,
                    glow::ONE_MINUS_SRC_ALPHA,
                );
                gl.depth_mask(false);
                draw(b);
                gl.depth_mask(true);
            }
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.disable(glow::DEPTH_TEST);
            gl.enable(glow::BLEND);
            gl.use_program(None);
            Ok(())
        }
    }
}

fn bytes<T: Copy>(v: &[T]) -> &[u8] {
    // SAFETY: f32 and u32 have no padding and any byte view of them is valid.
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast(), std::mem::size_of_val(v)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 64 x 64 cells of 10 m: a 100 m hill in the middle, below sea level at the corners.
    fn hill(dir: &std::path::Path) -> Terrain {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).unwrap();
        let n = 64;
        let mut asc =
            format!("ncols {n}\nnrows {n}\nxllcorner 1000\nyllcorner 2000\ncellsize 10\n");
        for y in 0..n {
            let row: Vec<String> = (0..n)
                .map(|x| {
                    let (dx, dy) = (x as f64 - 31.5, y as f64 - 31.5);
                    format!("{:.2}", 100. * (-(dx * dx + dy * dy) / 200.).exp() - 5.)
                })
                .collect();
            asc += &row.join(" ");
            asc.push('\n');
        }
        let src = dir.join("hill.asc");
        std::fs::write(&src, asc).unwrap();
        crate::terrain::import(&src, &dir.join("cache"), &Default::default()).unwrap()
    }

    fn mesh_of(t: &Tile) -> std::sync::MutexGuard<'_, Option<TileData>> {
        t.data.lock().unwrap()
    }

    #[test]
    fn tiles_follow_the_asc_and_split_down_to_its_cells() {
        let dir = std::env::temp_dir().join(format!("view3d-{}", std::process::id()));
        let t = hill(&dir);
        let extent = Region {
            min: [1000., 2000.],
            max: [1640., 2640.],
        };
        let layout = Layout::new(extent, 10.);
        // 640 m / 64 cells = 10 m already at the root.
        assert_eq!(layout.max_level, 0);
        let fine = Layout::new(extent, 2.5);
        assert_eq!(fine.max_level, 2);
        assert_eq!(
            fine.rect(TileKey {
                level: 2,
                x: 3,
                y: 3
            }),
            Some(Region {
                min: [1480., 2480.],
                max: [1640., 2640.]
            })
        );
        let tile = build_tile(
            &layout,
            TileKey {
                level: 0,
                x: 0,
                y: 0,
            },
            Some(&t),
            None,
            (-5., 95.),
        )
        .unwrap()
        .unwrap();
        let data = mesh_of(&tile);
        let mesh = &data.as_ref().unwrap().mesh;
        let grid = (CELLS + 1) * (CELLS + 1);
        assert_eq!(mesh.vertices.len() / STRIDE, grid + 4 * (CELLS + 1));
        for k in [0, 31 * 65 + 32, grid - 1] {
            let v = &mesh.vertices[k * STRIDE..];
            let world = [v[0] as f64 + 1000., v[1] as f64 + 2000.];
            assert!((v[2] as f64 - height(&t, world).unwrap()).abs() < 1e-3);
        }
        assert!(tile.zmax > 90. && tile.zmin < -5.);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn near_tiles_are_fine_and_far_ones_coarse_with_the_whole_map_covered() {
        // 16 km map with 2 m cells and no terrain: flat at 0.
        let extent = Region {
            min: [0., 0.],
            max: [16384., 16384.],
        };
        let source_map = MapSettings {
            east: 0.,
            north: 0.,
            width: 16384.,
            height: 16384.,
        };
        let raster = Raster {
            levels: vec![crate::raster::Level {
                width: 8192,
                height: 8192,
            }],
            dir: "missing".into(),
        };
        let source = Source {
            terrain: None,
            terrain_dir: None,
            raster: Some(&raster),
            map: &source_map,
            water_level: 0.,
            roads_key: 0,
        };
        let mut scene = Scene::default();
        scene.enter(
            Region {
                min: [8000., 8000.],
                max: [8200., 8200.],
            },
            Some(extent),
        );
        scene.camera.as_mut().unwrap().pitch = 0.25;
        scene.camera.as_mut().unwrap().distance = 150.;
        scene.settle(&source, 1.6, Vec::new);
        let layout = *scene.layout().unwrap();
        assert_eq!(layout.max_level, 7);
        let keys: Vec<_> = scene.visible_tiles().collect();
        let deepest = keys.iter().map(|k| k.level).max().unwrap();
        let shallowest = keys.iter().map(|k| k.level).min().unwrap();
        assert_eq!(deepest, 7, "{keys:?}");
        assert!(
            shallowest <= 3,
            "distant terrain uses coarse tiles: {keys:?}"
        );
        // The tile under the camera target is drawn at full detail.
        let s = layout.size / 128.;
        assert!(keys.contains(&TileKey {
            level: 7,
            x: (8100. / s) as u32,
            y: (8100. / s) as u32
        }));
        assert!(keys.len() < 200, "{}", keys.len());
    }

    #[test]
    fn camera_looks_at_its_target() {
        let layout = Layout::new(
            Region {
                min: [0., 0.],
                max: [15360., 15360.],
            },
            7.5,
        );
        let mut c = Camera::new([1000., 14000.], 2000.);
        for yaw in [0., 1., 2.5, -2.] {
            c.yaw = yaw;
            let (_, m) = c.matrix(&layout, 0., 1., 1.5);
            let centre = project(&m, layout.local(c.target, 0.)).unwrap();
            assert!(
                centre[0].abs() < 1e-3 && centre[1].abs() < 1e-3,
                "{centre:?}"
            );
        }
        c.yaw = 0.;
        let (_, m) = c.matrix(&layout, 0., 1., 1.5);
        let north = project(&m, layout.local([1000., 14300.], 0.)).unwrap();
        let east = project(&m, layout.local([1300., 14000.], 0.)).unwrap();
        assert!(north[1] > 0.05 && east[0] > 0.05, "{north:?} {east:?}");
        assert!(visible(
            &m,
            layout.local([900., 13900.], -10.),
            layout.local([1100., 14100.], 10.)
        ));
        assert!(
            !visible(
                &m,
                layout.local([1000., 2000.], -10.),
                layout.local([1100., 2100.], 10.)
            ),
            "behind the camera"
        );
    }

    #[test]
    fn road_parts_follow_the_terrain_and_tracks_stand_level() {
        let dir = std::env::temp_dir().join(format!("view3d-roads-{}", std::process::id()));
        let t = hill(&dir);
        let layout = Layout::new(
            Region {
                min: [1000., 2000.],
                max: [1640., 2640.],
            },
            10.,
        );
        // A synthetic MLOD-free check: footprints drape, models come from P: when present.
        let item = RoadItem {
            triangles: vec![[[1220., 2317.], [1420., 2317.], [1420., 2323.]]],
            color: [200, 50, 50],
            placements: vec![],
            rigid: false,
            highlight: false,
        };
        let r = build_roads(&layout, Some(&t), &[item], &Default::default());
        assert!(r.footprints.triangles() > 40);
        assert_eq!(
            r.fallback.triangles(),
            r.footprints.triangles(),
            "no models, so the footprint stays"
        );
        for v in r.footprints.vertices.chunks(STRIDE) {
            let world = [v[0] as f64 + 1000., v[1] as f64 + 2000.];
            assert!((v[2] as f64 - height(&t, world).unwrap() - LIFT as f64).abs() < 1e-3);
        }
        let road = std::path::Path::new("P:/dz/structures/roads/parts/asf2_25.p3d");
        let track = std::path::Path::new("P:/dz/structures/rail/Tracks/Rail_Track_25.p3d");
        if road.exists() && track.exists() {
            let at = [1300., 2300.];
            let items: Vec<RoadItem> = [(road, false), (track, true)]
                .into_iter()
                .map(|(f, rigid)| RoadItem {
                    triangles: vec![],
                    color: [0; 3],
                    placements: vec![(f.into(), 0.3, at)],
                    rigid,
                    highlight: false,
                })
                .collect();
            let r = build_roads(&layout, Some(&t), &items, &Default::default());
            assert_eq!((r.parts, r.missing), (2, 0));
            assert!(r.models.iter().any(|b| b.texture.is_some()));
            assert!(r.fallback.indices.is_empty());
            // Road vertices sit on the hill; track vertices keep their model heights over the origin.
            let base = height(&t, at).unwrap();
            let zs: Vec<f64> = r
                .models
                .iter()
                .flat_map(|b| b.mesh.vertices.chunks(STRIDE).map(|v| v[2] as f64))
                .collect();
            assert!(zs.iter().any(|z| (z - base).abs() < 3.));
            assert!(zs.iter().all(|z| z.is_finite()));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn satellite_crop_matches_the_region() {
        let dir = std::env::temp_dir().join(format!("view3d-sat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 512 x 512 pixels over a 1024 m map: red channel is x / 2, green is y / 2.
        let mut data = Vec::new();
        for y in 0..512u32 {
            for x in 0..512u32 {
                data.extend([(x / 2) as u8, (y / 2) as u8, 0, 255]);
            }
        }
        std::fs::write(dir.join("level-0.rgba"), &data).unwrap();
        let raster = Raster {
            levels: vec![crate::raster::Level {
                width: 512,
                height: 512,
            }],
            dir: dir.clone(),
        };
        let map = MapSettings {
            east: 0.,
            north: 0.,
            width: 1024.,
            height: 1024.,
        };
        let im = satellite(
            &raster,
            &map,
            Region {
                min: [200., 600.],
                max: [400., 800.],
            },
            4096,
        )
        .unwrap()
        .unwrap();
        assert_eq!((im.width, im.height), (100, 100));
        assert_eq!(im.min, [200., 600.]);
        assert_eq!(im.max, [400., 800.]);
        // Top-left pixel: x = 100 px, y = (1024 - 800) / 2 = 112 px.
        assert_eq!(&im.rgba[..2], &[50, 56]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
