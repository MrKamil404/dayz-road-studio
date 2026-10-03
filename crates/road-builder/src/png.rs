use crate::geometry::Point;

#[derive(Clone, Copy)]
pub struct Road<'a> {
    pub index: usize,
    pub is_new: bool,
    pub color: Option<[u8; 3]>,
    pub triangles: &'a [[Point; 3]],
    pub lines: &'a [Vec<Point>],
}
use anyhow::{Result, bail};
use std::{collections::BTreeSet, path::Path};
pub type Bounds = [f64; 4];
pub fn bounds<'a>(roads: impl Iterator<Item = Road<'a>>) -> Option<Bounds> {
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for r in roads {
        for p in r.triangles.iter().flatten().chain(r.lines.iter().flatten()) {
            b[0] = b[0].min(p[0]);
            b[1] = b[1].min(p[1]);
            b[2] = b[2].max(p[0]);
            b[3] = b[3].max(p[1]);
        }
    }
    b.iter().all(|v| v.is_finite()).then_some(b)
}
#[derive(Clone, Copy)]
pub struct View {
    pub center: Point,
    pub scale: f64,
    pub width: f64,
    pub height: f64,
    pub pan: Point,
}
impl View {
    pub fn new(b: Bounds, width: f64, height: f64, zoom: f64, pan: Point) -> Self {
        let scale = ((width - 60.).max(1.) / (b[2] - b[0]).max(1.))
            .min((height - 60.).max(1.) / (b[3] - b[1]).max(1.));
        Self {
            center: [(b[0] + b[2]) / 2., (b[1] + b[3]) / 2.],
            scale: scale * zoom,
            width,
            height,
            pan,
        }
    }
    pub fn point(&self, p: Point) -> Point {
        [
            self.width / 2. + self.pan[0] + (p[0] - self.center[0]) * self.scale,
            self.height / 2. + self.pan[1] - (p[1] - self.center[1]) * self.scale,
        ]
    }
}
pub fn color(selected: bool, any_selected: bool, is_new: bool) -> [u8; 4] {
    if selected {
        [255, 216, 70, 255]
    } else if is_new {
        [80, 225, 154, 255]
    } else if any_selected {
        [93, 108, 128, 255]
    } else {
        [92, 182, 236, 255]
    }
}
pub fn png(
    path: &Path,
    roads: &[Road<'_>],
    selection: &BTreeSet<usize>,
    width: u32,
    height: u32,
    transparent: bool,
) -> Result<()> {
    png_in_bounds(path, roads, selection, width, height, transparent, None)
}
pub fn validate_dimensions(width: u32, height: u32) -> Result<()> {
    if !(128..=20480).contains(&width) || !(128..=20480).contains(&height) {
        bail!("Szerokość i wysokość PNG muszą mieścić się w zakresie 128–20480 px")
    }
    if u64::from(width) * u64::from(height) > 419_430_400 {
        bail!("PNG może mieć maksymalnie 419 430 400 pikseli (np. 20480 × 20480)")
    }
    Ok(())
}

#[cfg(test)]
mod dimension_tests {
    use super::validate_dimensions;

    #[test]
    fn accepts_maximum_png_and_rejects_larger_sides() {
        assert!(validate_dimensions(20480, 20480).is_ok());
        assert!(validate_dimensions(20481, 128).is_err());
        assert!(validate_dimensions(128, 20481).is_err());
        assert!(validate_dimensions(127, 20480).is_err());
    }
}

pub fn png_in_bounds(
    path: &Path,
    roads: &[Road<'_>],
    selection: &BTreeSet<usize>,
    width: u32,
    height: u32,
    transparent: bool,
    map_bounds: Option<Bounds>,
) -> Result<()> {
    validate_dimensions(width, height)?;
    if let Some(b) = map_bounds
        && (!b.iter().all(|v| v.is_finite()) || b[2] <= b[0] || b[3] <= b[1])
    {
        bail!("Nieprawidłowe granice mapy")
    }
    let b = bounds(roads.iter().copied())
        .ok_or_else(|| anyhow::anyhow!("Brak geometrii do eksportu"))?;
    let view = View::new(b, width as f64, height as f64, 1., [0., 0.]);
    // Full map exports have no padding or recentering around the selected roads.
    // World origin is the bottom left; PNG origin is the top left.
    let pixel = |p: Point| match map_bounds {
        Some(b) => [
            (p[0] - b[0]) * width as f64 / (b[2] - b[0]),
            height as f64 - (p[1] - b[1]) * height as f64 / (b[3] - b[1]),
        ],
        None => view.point(p),
    };
    let bg = if transparent {
        [0, 0, 0, 0]
    } else {
        [15, 23, 37, 255]
    };
    let bytes = width as usize * height as usize * 4;
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(bytes)
        .map_err(|_| anyhow::anyhow!("Brak pamięci na obraz PNG o tych wymiarach"))?;
    buffer.resize(bytes, 0u8);
    let mut img = image::RgbaImage::from_raw(width, height, buffer)
        .ok_or_else(|| anyhow::anyhow!("Nie można utworzyć obrazu PNG"))?;
    if !transparent {
        for p in img.pixels_mut() {
            p.0 = bg;
        }
    }
    let any = roads.iter().any(|r| selection.contains(&r.index));
    for highlighted in [false, true] {
        for r in roads {
            let marked = selection.contains(&r.index);
            if marked != highlighted {
                continue;
            }
            let col = r
                .color
                .map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
                .unwrap_or_else(|| color(marked, any, r.is_new));
            for t in r.triangles {
                triangle(&mut img, t.map(pixel), col)
            }
            for line in r.lines {
                for pair in line.windows(2) {
                    stroke(
                        &mut img,
                        pixel(pair[0]),
                        pixel(pair[1]),
                        if marked { 2.2 } else { 1.1 },
                        col,
                    )
                }
            }
        }
    }
    img.save_with_format(path, image::ImageFormat::Png)?;
    Ok(())
}
fn region(img: &image::RgbaImage, pts: &[Point], pad: f64) -> (u32, u32, u32, u32) {
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for p in pts {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1])
    }
    (
        ((b[0] - pad).floor().max(0.) as u32).min(img.width()),
        ((b[1] - pad).floor().max(0.) as u32).min(img.height()),
        ((b[2] + pad).ceil().max(0.) as u32).min(img.width()),
        ((b[3] + pad).ceil().max(0.) as u32).min(img.height()),
    )
}
fn edge(a: Point, b: Point, p: Point) -> f64 {
    (p[0] - a[0]) * (b[1] - a[1]) - (p[1] - a[1]) * (b[0] - a[0])
}
fn triangle(img: &mut image::RgbaImage, t: [Point; 3], color: [u8; 4]) {
    let (x0, y0, x1, y1) = region(img, &t, 0.);
    let area = edge(t[0], t[1], t[2]);
    if area.abs() < 1e-12 {
        return;
    }
    for y in y0..y1 {
        for x in x0..x1 {
            let p = [x as f64 + 0.5, y as f64 + 0.5];
            let e = [
                edge(t[0], t[1], p),
                edge(t[1], t[2], p),
                edge(t[2], t[0], p),
            ];
            if e.iter().all(|v| *v >= 0.) || e.iter().all(|v| *v <= 0.) {
                img.put_pixel(x, y, image::Rgba(color))
            }
        }
    }
}
pub fn distance(p: Point, a: Point, b: Point) -> f64 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let t =
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (dx * dx + dy * dy).max(1e-12)).clamp(0., 1.);
    (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
}
fn stroke(img: &mut image::RgbaImage, a: Point, b: Point, width: f64, col: [u8; 4]) {
    let rad = width / 2.;
    let (x0, y0, x1, y1) = region(img, &[a, b], rad + 1.);
    for y in y0..y1 {
        for x in x0..x1 {
            let coverage =
                (rad + 0.5 - distance([x as f64 + 0.5, y as f64 + 0.5], a, b)).clamp(0., 1.);
            if coverage <= 0. {
                continue;
            }
            let old = img.get_pixel(x, y).0;
            let oa = old[3] as f64 / 255.;
            let alpha = coverage + oa * (1. - coverage);
            let mut value = [0u8; 4];
            for i in 0..3 {
                value[i] = ((col[i] as f64 * coverage + old[i] as f64 * oa * (1. - coverage))
                    / alpha.max(1e-12))
                .round() as u8;
            }
            value[3] = (alpha * 255.).round() as u8;
            img.put_pixel(x, y, image::Rgba(value));
        }
    }
}
