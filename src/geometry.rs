use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};
pub type Point = [f64; 2];
pub fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}
pub fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}
pub fn mul(a: Point, t: f64) -> Point {
    [a[0] * t, a[1] * t]
}
pub fn norm(a: Point) -> f64 {
    a[0].hypot(a[1])
}
pub fn rotate(a: Point, t: f64) -> Point {
    [
        a[0] * t.cos() - a[1] * t.sin(),
        a[0] * t.sin() + a[1] * t.cos(),
    ]
}
fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn unit(a: Point) -> Point {
    mul(a, 1.0 / norm(a).max(1e-12))
}
#[derive(Clone, Copy, Debug)]
pub struct Port {
    pub p: Point,
    pub outward: Point,
}
#[derive(Clone, Debug)]
pub struct Model {
    pub triangles: Vec<[Point; 3]>,
    pub ports: [Option<Port>; 4],
    pub line: Vec<Point>,
    pub length: f64,
    pub source: String,
}

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}
impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let e = self.p.checked_add(n).context("MLOD offset overflow")?;
        let v = self.b.get(self.p..e).context("Truncated MLOD")?;
        self.p = e;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into()?))
    }
    fn f32(&mut self) -> Result<f64> {
        Ok(f32::from_le_bytes(self.bytes(4)?.try_into()?) as f64)
    }
    fn string(&mut self) -> Result<String> {
        let mut v = Vec::new();
        loop {
            let b = self.bytes(1)?[0];
            if b == 0 {
                break;
            }
            v.push(b);
            if v.len() > 65536 {
                bail!("MLOD string too long")
            }
        }
        Ok(String::from_utf8_lossy(&v).into_owned())
    }
}
pub fn read_mlod(path: &Path) -> Result<Model> {
    let bytes = fs::read(path).with_context(|| format!("Cannot read {}", path.display()))?;
    parse_mlod(&bytes).with_context(|| format!("Invalid model {}", path.display()))
}
#[derive(Debug)]
pub struct NonMlod {
    format: String,
}
impl std::fmt::Display for NonMlod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "P3D model is not MLOD (format {}). Road preview requires unbinarized MLOD models.",
            self.format
        )
    }
}
impl std::error::Error for NonMlod {}
fn parse_mlod(bytes: &[u8]) -> Result<Model> {
    if !bytes.starts_with(b"MLOD") {
        let format = if bytes.starts_with(b"ODOL") {
            "ODOL".to_owned()
        } else {
            format!("{:02X?}", &bytes[..bytes.len().min(4)])
        };
        return Err(NonMlod { format }.into());
    }
    let mut r = Reader { b: bytes, p: 0 };
    if r.bytes(4)? != b"MLOD" {
        bail!("Expected MLOD P3D")
    };
    if r.u32()? != 257 {
        bail!("Unsupported MLOD version");
    }
    let lods = r.u32()?;
    if lods > 1000 {
        bail!("Too many MLOD LODs")
    }
    let mut visual: Option<(Vec<Point>, Vec<Vec<usize>>)> = None;
    let mut memory = HashMap::new();
    let mut visual_resolution = f64::INFINITY;
    for _ in 0..lods {
        if r.bytes(4)? != b"P3DM" {
            bail!("Unsupported MLOD LOD")
        };
        if r.u32()? != 28 || r.u32()? != 256 {
            bail!("Unsupported P3DM version")
        }
        let nv = r.u32()? as usize;
        let nn = r.u32()? as usize;
        let nf = r.u32()? as usize;
        let _flags = r.u32()?;
        if nv > 2_000_000 || nf > 2_000_000 || nn > 2_000_000 {
            bail!("MLOD exceeds limits")
        }
        let mut verts = Vec::with_capacity(nv);
        for _ in 0..nv {
            let x = r.f32()?;
            let _y = r.f32()?;
            let z = r.f32()?;
            r.u32()?;
            if !x.is_finite() || !z.is_finite() {
                bail!("Non-finite vertex")
            };
            verts.push([x, z]);
        }
        r.bytes(nn * 12)?;
        let mut faces = Vec::new();
        for _ in 0..nf {
            let n = r.u32()? as usize;
            if !(3..=4).contains(&n) {
                bail!("Unsupported MLOD polygon")
            };
            let mut f = Vec::new();
            for i in 0..4 {
                let vi = r.u32()? as usize;
                r.bytes(12)?;
                if i < n {
                    if vi >= nv {
                        bail!("Invalid vertex index")
                    };
                    f.push(vi)
                }
            }
            r.u32()?;
            r.string()?;
            r.string()?;
            faces.push(f);
        }
        if r.bytes(4)? != b"TAGG" {
            bail!("Missing TAGG")
        };
        let mut selections = HashMap::new();
        loop {
            let active = r.bytes(1)?[0] != 0;
            let name = r.string()?;
            let len = r.u32()? as usize;
            let payload = r.bytes(len)?;
            if name == "#EndOfFile#" {
                break;
            };
            if active && !name.starts_with('#') && len == nv + nf {
                let chosen: Vec<_> = payload[..nv]
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| **v != 0)
                    .map(|(i, _)| verts[i])
                    .collect();
                if !chosen.is_empty() {
                    let n = chosen.len() as f64;
                    selections.insert(
                        name.to_uppercase(),
                        mul(chosen.into_iter().fold([0., 0.], add), 1. / n),
                    );
                }
            }
        }
        let res = r.f32()?;
        if res > 9e14 && res < 1.1e15 {
            memory = selections;
        }
        if !faces.is_empty() && res < visual_resolution {
            visual_resolution = res;
            visual = Some((verts, faces));
        }
    }
    let (verts, faces) = visual.context("No visual mesh in MLOD")?;
    let triangles = faces
        .iter()
        .flat_map(|f| (1..f.len() - 1).map(|i| [verts[f[0]], verts[f[i]], verts[f[i + 1]]]))
        .collect();
    let center = mul(
        verts.iter().copied().fold([0., 0.], add),
        1. / verts.len().max(1) as f64,
    );
    let pairs = [("LB", "PB"), ("LE", "PE"), ("LH", "LD"), ("PH", "PD")];
    let mut ports = [None; 4];
    for (i, (a, b)) in pairs.iter().enumerate() {
        if let (Some(a), Some(b)) = (memory.get(*a), memory.get(*b)) {
            let p = mul(add(*a, *b), 0.5);
            let edge = sub(*b, *a);
            let mut out = unit([-edge[1], edge[0]]);
            if dot(out, sub(p, center)) < 0. {
                out = mul(out, -1.)
            }
            ports[i] = Some(Port { p, outward: out });
        }
    }
    let (line, length) = centerline(ports);
    if ports[0].is_none() || ports[1].is_none() {
        bail!("Missing LB/PB/LE/PE connection selections")
    }
    Ok(Model {
        triangles,
        ports,
        line,
        length,
        source: "MLOD".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_realistic_memory_connectors_instead_of_filename_length() {
        // Synthetic standard MLOD with a 6.25 m strip and four named ports.
        // This exercises binary parsing, selection weights and coordinate axes.
        fn word(b: &mut Vec<u8>, v: u32) {
            b.extend(v.to_le_bytes());
        }
        fn tag(b: &mut Vec<u8>, name: &str, data: &[u8]) {
            b.push(1);
            b.extend(name.as_bytes());
            b.push(0);
            word(b, data.len() as u32);
            b.extend(data);
        }
        let verts = [
            [4.5f32, 0., 0.],
            [4.5, 0., 6.25],
            [-4.5, 0., 6.25],
            [-4.5, 0., 0.],
        ];
        let mut b = b"MLOD".to_vec();
        word(&mut b, 257);
        word(&mut b, 2);
        for memory in [false, true] {
            b.extend(b"P3DM");
            for v in [28, 256, 4, 0, if memory { 0 } else { 2 }, 0] {
                word(&mut b, v);
            }
            for v in verts {
                for n in v {
                    b.extend(n.to_le_bytes());
                }
                word(&mut b, 0);
            }
            if !memory {
                for face in [[0, 1, 2], [0, 2, 3]] {
                    word(&mut b, 3);
                    for index in face.into_iter().chain(std::iter::once(0)) {
                        word(&mut b, index);
                        for _ in 0..3 {
                            word(&mut b, 0);
                        }
                    }
                    word(&mut b, 0);
                    b.extend([0, 0]);
                }
            }
            b.extend(b"TAGG");
            if memory {
                for (name, index) in [("LB", 3), ("PB", 0), ("LE", 2), ("PE", 1)] {
                    let mut weights = [0; 4];
                    weights[index] = 1;
                    tag(&mut b, name, &weights);
                }
            }
            tag(&mut b, "#EndOfFile#", &[]);
            b.extend((if memory { 1e15f32 } else { 0f32 }).to_le_bytes());
        }
        let model = parse_mlod(&b).unwrap();
        assert!((model.length - 6.25).abs() < 1e-8);
        assert_eq!(model.triangles.len(), 2);
        assert_eq!(model.ports[0].unwrap().p, [0., 0.]);
        assert_eq!(model.ports[1].unwrap().p, [0., 6.25]);
        b.truncate(b.len() - 9);
        assert!(parse_mlod(&b).is_err());
        assert!(parse_mlod(b"ODOL").is_err());
    }
    #[test]
    fn corner_can_connect_from_either_end_without_gaps() {
        let model = filename_model("asf2_30 25.p3d").unwrap();
        assert!((model.length - 25. * std::f64::consts::PI / 6.).abs() < 1e-8);
        let connection = Port {
            p: [100., 200.],
            outward: [0., 1.],
        };
        for (reverse, sign) in [(false, 1.), (true, -1.)] {
            let mut shape = Shape::default();
            let end = shape.append(&model, connection, reverse).unwrap();
            assert!(
                (end.p[0] - (100. + sign * 25. * (1. - (std::f64::consts::PI / 6.).cos()))).abs()
                    < 1e-7
            );
            assert!((end.p[1] - 212.5).abs() < 1e-7);
            let line = &shape.lines[0];
            let entry = if reverse {
                line.last().unwrap()
            } else {
                &line[0]
            };
            assert!(norm(sub(*entry, connection.p)) < 1e-8);
        }
        assert!(
            (filename_model("asf2_0 2000.p3d").unwrap().length - 2000. * 0.5f64.to_radians()).abs()
                < 1e-8
        );
        assert_eq!(filename_model("asf2_6konec.p3d").unwrap().length, 6.25);
        assert!(filename_model("unknown.p3d").is_err());
    }
}
fn centerline(ports: [Option<Port>; 4]) -> (Vec<Point>, f64) {
    let (Some(b), Some(e)) = (ports[0], ports[1]) else {
        return (vec![], 0.);
    };
    let a = mul(b.outward, -1.);
    let z = e.outward;
    let turn = (a[0] * z[1] - a[1] * z[0]).atan2(dot(a, z));
    let chord = norm(sub(e.p, b.p));
    if turn.abs() < 1e-6 {
        return (vec![b.p, e.p], chord);
    }
    let radius = chord / (2. * (turn.abs() / 2.).sin());
    let sign = turn.signum();
    let c = add(b.p, mul([-a[1], a[0]], radius * sign));
    let radial = sub(b.p, c);
    let steps = (turn.abs().to_degrees() / 2.).ceil().max(2.) as usize;
    let mut line: Vec<_> = (0..=steps)
        .map(|i| add(c, rotate(radial, turn * i as f64 / steps as f64)))
        .collect();
    *line.last_mut().unwrap() = e.p;
    (line, radius * turn.abs())
}
pub fn filename_model(name: &str) -> Result<Model> {
    let file = name
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(name)
        .to_lowercase();
    let stem = file
        .strip_suffix(".p3d")
        .unwrap_or(&file)
        .trim_end_matches("_crosswalk");
    let tail = stem
        .rsplit('_')
        .next()
        .context("Missing model parameters")?;
    let words: Vec<_> = tail.split_whitespace().collect();
    let first: f64 = words
        .first()
        .context("Missing length")?
        .trim_end_matches("konec")
        .parse()
        .context("Unknown model naming")?;
    let width = if stem.starts_with("asf1") {
        12.
    } else if stem.starts_with("asf2") {
        9.
    } else if stem.starts_with("asf3") {
        7.
    } else if stem.starts_with("grav") {
        6.
    } else {
        5.
    };
    let (len, turn) = if words.len() == 2 {
        let radius: f64 = words[1].parse()?;
        let angle = if first == 0. { 0.5 } else { first };
        let turn = -angle.to_radians();
        (radius * turn.abs(), turn)
    } else {
        let len = if first == 6. {
            6.25
        } else if first == 12. {
            12.5
        } else {
            first
        };
        (len, 0.)
    };
    if !len.is_finite() || len <= 0. || !turn.is_finite() {
        bail!("Invalid filename geometry")
    }
    let n = (turn.abs().to_degrees() / 2.).ceil().max(1.) as usize;
    let mut line = Vec::new();
    let mut sides = Vec::new();
    for i in 0..=n {
        let t = i as f64 / n as f64;
        let a = turn * t;
        let p = if turn.abs() < 1e-9 {
            [0., len * t]
        } else {
            let radius = len / turn.abs();
            [radius * (1. - a.cos()), radius * (-a.sin())]
        };
        let tangent = rotate([0., 1.], a);
        let right = [tangent[1], -tangent[0]];
        line.push(p);
        sides.push([
            add(p, mul(right, width / 2.)),
            sub(p, mul(right, width / 2.)),
        ]);
    }
    let mut triangles = Vec::new();
    for i in 0..n {
        let [a, b] = sides[i];
        let [c, d] = sides[i + 1];
        triangles.push([a, b, c]);
        triangles.push([b, d, c]);
    }
    let end = *line.last().unwrap();
    Ok(Model {
        triangles,
        ports: [
            Some(Port {
                p: [0., 0.],
                outward: [0., -1.],
            }),
            Some(Port {
                p: end,
                outward: rotate([0., 1.], turn),
            }),
            None,
            None,
        ],
        line,
        length: len,
        source: "nazwa pliku".into(),
    })
}
#[derive(Default)]
pub struct Library {
    pub root: PathBuf,
    cache: HashMap<String, Model>,
}
impl Library {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            cache: HashMap::new(),
        }
    }
    pub fn model(&mut self, name: &str) -> Result<Model> {
        let file = name
            .rsplit(['\\', '/'])
            .next()
            .context("Missing model file")?;
        let key = file.to_lowercase();
        if let Some(m) = self.cache.get(&key) {
            return Ok(m.clone());
        };
        let requested = PathBuf::from(name);
        let configured = self.root.join(file);
        let path = if configured.exists() {
            configured
        } else if requested.is_absolute() && requested.exists() {
            requested
        } else {
            configured
        };
        let model = if path.exists() {
            read_mlod(&path)?
        } else {
            filename_model(name)?
        };
        self.cache.insert(key, model.clone());
        Ok(model)
    }
}
#[derive(Clone, Default, Serialize)]
pub struct Shape {
    pub triangles: Vec<[Point; 3]>,
    pub lines: Vec<Vec<Point>>,
    pub length: f64,
    pub mlod_parts: usize,
    pub filename_parts: usize,
    pub warnings: Vec<String>,
    pub non_mlod_models: Vec<String>,
}
impl Shape {
    pub fn place(&mut self, m: &Model, angle: f64, translation: Point) -> [Option<Port>; 4] {
        let tr = |p| add(rotate(p, angle), translation);
        self.triangles.extend(m.triangles.iter().map(|t| t.map(tr)));
        if !m.line.is_empty() {
            self.lines.push(m.line.iter().copied().map(tr).collect())
        }
        self.length += m.length;
        if m.source == "MLOD" {
            self.mlod_parts += 1
        } else {
            self.filename_parts += 1
        };
        m.ports.map(|p| {
            p.map(|p| Port {
                p: tr(p.p),
                outward: rotate(p.outward, angle),
            })
        })
    }
    pub fn append(&mut self, m: &Model, at: Port, reverse: bool) -> Result<Port> {
        let entry = if reverse { 1 } else { 0 };
        let exit = 1 - entry;
        let from = m.ports[entry].context("Missing part entry port")?;
        let target = mul(at.outward, -1.);
        let angle = target[1].atan2(target[0]) - from.outward[1].atan2(from.outward[0]);
        let ports = self.place(m, angle, sub(at.p, rotate(from.p, angle)));
        ports[exit].context("Missing part exit port")
    }
}
