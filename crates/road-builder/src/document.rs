use crate::{geometry::Point, roads::PlacedPart};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct MapSettings {
    pub east: f64,
    pub north: f64,
    pub width: f64,
    pub height: f64,
}
impl Default for MapSettings {
    fn default() -> Self {
        Self {
            east: 200000.,
            north: 0.,
            width: 15360.,
            height: 15360.,
        }
    }
}
impl MapSettings {
    pub fn valid(&self) -> bool {
        [self.east, self.north, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            && self.width > 0.
            && self.height > 0.
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Route {
    pub name: String,
    pub family: String,
    pub points: Vec<Point>,
    pub parts: Vec<PlacedPart>,
    /// Imported record replaced by this route; omitted for new roads.
    pub replaces: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct RoutingSettings {
    pub max_grade: f64,
    pub slope_weight: f64,
    pub cell: f64,
    pub min_radius: f64,
    pub tolerance: f64,
}
impl Default for RoutingSettings {
    fn default() -> Self {
        Self {
            max_grade: 12.,
            slope_weight: 4.,
            cell: 20.,
            min_radius: 25.,
            tolerance: 5.,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Document {
    pub version: u32,
    pub base: Option<PathBuf>,
    pub models: PathBuf,
    pub satellite: Option<PathBuf>,
    pub terrain: Option<PathBuf>,
    #[serde(default)]
    pub satellite_cache: Option<PathBuf>,
    #[serde(default)]
    pub terrain_cache: Option<PathBuf>,
    #[serde(default)]
    pub terrain_origin: Option<Point>,
    pub map: MapSettings,
    pub routing: RoutingSettings,
    pub routes: Vec<Route>,
    pub deleted: Vec<u32>,
    #[serde(default)]
    pub transforms: Vec<(u32, Point, f64)>,
    pub forbidden: Vec<Vec<Point>>,
    #[serde(default)]
    pub road_colors: crate::road_colors::Palette,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            version: 1,
            base: None,
            models: "P:\\dz\\structures\\roads\\parts".into(),
            satellite: None,
            terrain: None,
            satellite_cache: None,
            terrain_cache: None,
            terrain_origin: None,
            map: MapSettings::default(),
            routing: RoutingSettings::default(),
            routes: vec![],
            deleted: vec![],
            transforms: vec![],
            forbidden: vec![],
            road_colors: Default::default(),
        }
    }
}
impl Document {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "Nieobsługiwana wersja projektu");
        ensure!(self.map.valid(), "Nieprawidłowe współrzędne mapy");
        let s = &self.routing;
        ensure!(
            [
                s.max_grade,
                s.slope_weight,
                s.cell,
                s.min_radius,
                s.tolerance
            ]
            .iter()
            .all(|x| x.is_finite())
                && s.max_grade > 0.
                && s.slope_weight >= 0.
                && s.cell > 0.
                && s.min_radius > 0.
                && s.tolerance > 0.,
            "Nieprawidłowe ustawienia trasy"
        );
        for r in &self.routes {
            ensure!(
                r.points
                    .iter()
                    .chain(r.parts.iter().map(|p| &p.position))
                    .flatten()
                    .all(|x| x.is_finite()),
                "Nieprawidłowe punkty drogi"
            );
            ensure!(
                r.parts.iter().all(|p| p.rotation.is_finite()),
                "Nieprawidłowa rotacja"
            );
        }
        ensure!(
            self.forbidden
                .iter()
                .flatten()
                .flatten()
                .all(|x| x.is_finite()),
            "Nieprawidłowy obszar zakazany"
        );
        ensure!(
            self.transforms
                .iter()
                .all(|(_, p, r)| p.iter().all(|v| v.is_finite()) && r.is_finite()),
            "Nieprawidłowy transform drogi"
        );
        ensure!(
            self.terrain_origin
                .is_none_or(|p| p.iter().all(|v| v.is_finite())),
            "Nieprawidłowa pozycja ASC"
        );
        Ok(())
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        crate::storage::atomic_write(path, &serde_json::to_vec_pretty(self)?)
    }
    pub fn load(path: &Path) -> Result<Self> {
        let d: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        d.validate()?;
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn save_restore_layers_offsets_and_geometry() {
        let path = std::env::temp_dir().join(format!("road-session-{}.dzroad", std::process::id()));
        let mut d = Document::default();
        d.map.width = 40960.;
        d.terrain_origin = Some([200000., 0.]);
        d.satellite = Some("sat.png".into());
        d.satellite_cache = Some("cache/sat".into());
        d.terrain = Some("height.asc".into());
        d.transforms.push((123, [10., 20.], 35.));
        d.road_colors.insert("asf2".into(), [12, 34, 56]);
        d.forbidden.push(vec![[0., 0.], [10., 0.], [0., 10.]]);
        d.save(&path).unwrap();
        let loaded = Document::load(&path).unwrap();
        assert_eq!(loaded.map.width, 40960.);
        assert_eq!(loaded.transforms, d.transforms);
        assert_eq!(loaded.road_colors, d.road_colors);
        assert_eq!(loaded.terrain_origin, d.terrain_origin);
        assert_eq!(loaded.satellite_cache, d.satellite_cache);
        let original = std::fs::read(&path).unwrap();
        d.map.width = -1.;
        assert!(d.save(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn older_projects_without_road_colors_still_load() {
        let mut old = serde_json::to_value(Document::default()).unwrap();
        old.as_object_mut().unwrap().remove("road_colors");
        let document: Document = serde_json::from_value(old).unwrap();
        document.validate().unwrap();
        assert!(document.road_colors.is_empty());
    }
}
