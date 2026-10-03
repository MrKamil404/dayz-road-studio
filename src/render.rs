use crate::tv4p::Road;
use anyhow::Result;
use dayz_road_tool::png as shared;
pub use shared::{Bounds, View, color, distance, validate_dimensions};
use std::{collections::BTreeSet, path::Path};

fn geometry(road: &Road) -> shared::Road<'_> {
    shared::Road {
        index: road.index,
        is_new: road.is_new,
        color: None,
        triangles: &road.shape.triangles,
        lines: &road.shape.lines,
    }
}

pub fn bounds<'a>(roads: impl Iterator<Item = &'a Road>) -> Option<Bounds> {
    shared::bounds(roads.map(geometry))
}
pub fn png(
    path: &Path,
    roads: &[&Road],
    selection: &BTreeSet<usize>,
    width: u32,
    height: u32,
    transparent: bool,
) -> Result<()> {
    png_in_bounds(path, roads, selection, width, height, transparent, None)
}
pub fn png_in_bounds(
    path: &Path,
    roads: &[&Road],
    selection: &BTreeSet<usize>,
    width: u32,
    height: u32,
    transparent: bool,
    map_bounds: Option<Bounds>,
) -> Result<()> {
    colored_png(
        path,
        roads,
        selection,
        &Default::default(),
        PngOptions {
            width,
            height,
            transparent,
            map_bounds,
        },
    )
}

pub struct PngOptions {
    pub width: u32,
    pub height: u32,
    pub transparent: bool,
    pub map_bounds: Option<Bounds>,
}
pub fn colored_png(
    path: &Path,
    roads: &[&Road],
    selection: &BTreeSet<usize>,
    palette: &dayz_road_tool::road_colors::Palette,
    options: PngOptions,
) -> Result<()> {
    let roads: Vec<_> = roads
        .iter()
        .map(|road| {
            let mut shape = geometry(road);
            shape.color = palette
                .get(&dayz_road_tool::road_colors::family(&road.model))
                .copied();
            shape
        })
        .collect();
    shared::png_in_bounds(
        path,
        &roads,
        selection,
        options.width,
        options.height,
        options.transparent,
        options.map_bounds,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_png_uses_type_colors_even_for_selected_and_new_roads() {
        let roads: Vec<_> = [("asf2", 16., 96.), ("asf3", 80., 16.)]
            .into_iter()
            .enumerate()
            .map(|(index, (family, x, y))| Road {
                index,
                id: index as u32,
                model: format!("P:\\dz\\roads\\{family}_25.p3d"),
                models: vec![],
                start: [200000. + x, y],
                rotation_degrees: 0.,
                parts: 1,
                is_new: true,
                shape: crate::geometry::Shape {
                    triangles: vec![[[200000. + x, y], [200016. + x, y], [200000. + x, y + 16.]]],
                    ..Default::default()
                },
            })
            .collect();
        let palette = [
            ("asf2".into(), [200, 10, 30]),
            ("asf3".into(), [10, 200, 30]),
        ]
        .into_iter()
        .collect();
        let path =
            std::env::temp_dir().join(format!("merge-type-colors-{}.png", std::process::id()));
        colored_png(
            &path,
            &roads.iter().collect::<Vec<_>>(),
            &[0].into_iter().collect(),
            &palette,
            PngOptions {
                width: 128,
                height: 128,
                transparent: true,
                map_bounds: Some([200000., 0., 200128., 128.]),
            },
        )
        .unwrap();
        let image = image::open(&path).unwrap().into_rgba8();
        assert_eq!(image.get_pixel(20, 24).0, [200, 10, 30, 255]);
        assert_eq!(image.get_pixel(84, 104).0, [10, 200, 30, 255]);
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 0]);
        std::fs::remove_file(path).unwrap();
    }
}
