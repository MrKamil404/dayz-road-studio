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
    /// Railway track: fitted with track models, exported as Terrain Builder objects.
    #[serde(default)]
    pub rail: bool,
    /// Objects along a track that are not chained pieces (level-crossing decks).
    #[serde(default)]
    pub extras: Vec<PlacedPart>,
    /// Temporary terrain adaptation under this route. It lives with the route: the source
    /// ASC stays untouched and removing the route restores the terrain.
    #[serde(default)]
    pub grading: Option<RouteGrading>,
    /// Bridges in gaps of the route; the parts on either side are separate stretches.
    #[serde(default)]
    pub bridges: Vec<Bridge>,
}
impl Route {
    /// The connected runs of parts between bridges.
    pub fn stretches(&self) -> Vec<&[PlacedPart]> {
        let mut out = Vec::with_capacity(self.bridges.len() + 1);
        let mut from = 0;
        for b in &self.bridges {
            let to = b.after.clamp(from, self.parts.len());
            out.push(&self.parts[from..to]);
            from = to;
        }
        out.push(&self.parts[from..]);
        out.retain(|p| !p.is_empty());
        out
    }
}
/// A bridge made of identical objects, standing in a gap of its route.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Bridge {
    /// Height of the driving surface above the model's origin.
    pub deck: f64,
    /// Number of route parts before the bridge.
    pub after: usize,
    pub pieces: Vec<PlacedPart>,
}
/// Settings of a route's terrain adaptation; the cells follow from these and the route.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct RouteGrading {
    /// Adaptations overlap in this order; later ones win.
    pub order: u64,
    pub blend_width: f64,
    pub max_grade: f64,
    pub max_cut: f64,
    pub max_fill: f64,
    pub side_slope: f64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct RoutingSettings {
    pub max_grade: f64,
    pub slope_weight: f64,
    pub cell: f64,
    pub min_radius: f64,
    pub tolerance: f64,
    /// How far the road may be cut into (below) or built up above the terrain, m.
    #[serde(default)]
    pub max_cut: f64,
    #[serde(default)]
    pub max_fill: f64,
    /// Highest a bridge may stand above the ground, m; it takes over where an embankment
    /// of `max_fill` is not enough. 0 turns bridges off.
    #[serde(default)]
    pub bridge_height: f64,
    /// Longest bridge the routing may build, m; 0 turns bridges off.
    #[serde(default)]
    pub bridge_max: f64,
    /// Bridges may also cross forbidden areas, rivers and water.
    #[serde(default)]
    pub bridge_over_forbidden: bool,
    /// Cost of a metre of bridge in metres of road.
    #[serde(default = "default_bridge_weight")]
    pub bridge_weight: f64,
    /// Cost of earthwork in metres of road per m² of cut or fill along the route.
    #[serde(default = "default_earth_weight")]
    pub earth_weight: f64,
}
fn default_earth_weight() -> f64 {
    0.5
}
impl Default for RoutingSettings {
    fn default() -> Self {
        Self {
            max_grade: 12.,
            slope_weight: 4.,
            cell: 20.,
            min_radius: 25.,
            tolerance: 5.,
            max_cut: 0.,
            max_fill: 0.,
            earth_weight: default_earth_weight(),
            bridge_height: 20.,
            bridge_max: 150.,
            bridge_over_forbidden: false,
            bridge_weight: default_bridge_weight(),
        }
    }
}
impl RoutingSettings {
    /// Railway defaults: gentle grades and wide curves (the DayZ 10° / 25 m curve is R 143 m).
    pub fn rail() -> Self {
        Self {
            max_grade: 3.,
            slope_weight: 8.,
            cell: 25.,
            min_radius: 110.,
            tolerance: 3.,
            max_cut: 0.,
            max_fill: 0.,
            earth_weight: default_earth_weight(),
            bridge_height: 20.,
            bridge_max: 150.,
            bridge_over_forbidden: false,
            bridge_weight: default_bridge_weight(),
        }
    }
    fn valid(&self) -> bool {
        [
            self.max_grade,
            self.slope_weight,
            self.cell,
            self.min_radius,
            self.tolerance,
            self.max_cut,
            self.max_fill,
            self.earth_weight,
        ]
        .iter()
        .all(|x| x.is_finite())
            && (0.0..=100.).contains(&self.max_cut)
            && (0.0..=100.).contains(&self.max_fill)
            && self.earth_weight >= 0.
            && self.max_grade > 0.
            && self.slope_weight >= 0.
            && self.cell > 0.
            && self.min_radius > 0.
            && self.tolerance > 0.
    }
}
/// Forbidden corridor along a line, e.g. a river imported from SHP.
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct ForbiddenLine {
    pub points: Vec<Point>,
    /// Full width of the corridor in metres.
    pub width: f64,
}
/// Terrain below `level` (ASC metres) counts as a forbidden area when `enabled`.
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug, Default)]
pub struct WaterSettings {
    pub enabled: bool,
    pub level: f64,
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
    /// Constraints for railway tracks.
    #[serde(default = "RoutingSettings::rail")]
    pub rail_routing: RoutingSettings,
    /// Folder with the `Rail_Track*.p3d` models.
    #[serde(default)]
    pub rail_models: Option<PathBuf>,
    /// Smooth transition width beyond the MLOD footprint, in metres per side.
    #[serde(default)]
    pub grading_blend_width: f64,
    /// Embankment and cut slopes when grading: horizontal metres per vertical metre.
    #[serde(default = "default_side_slope")]
    pub grading_side_slope: f64,
    /// Generated routes adapt the terrain under them right away.
    #[serde(default = "default_auto_grade")]
    pub auto_grade: bool,
    /// Sign of pitch and roll written to the Terrain Builder object file.
    #[serde(default)]
    pub rail_invert_pitch: bool,
    #[serde(default)]
    pub rail_invert_roll: bool,
    pub routes: Vec<Route>,
    pub deleted: Vec<u32>,
    #[serde(default)]
    pub transforms: Vec<(u32, Point, f64)>,
    pub forbidden: Vec<Vec<Point>>,
    #[serde(default)]
    pub forbidden_lines: Vec<ForbiddenLine>,
    #[serde(default)]
    pub water: WaterSettings,
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
            rail_routing: RoutingSettings::rail(),
            rail_models: None,
            grading_blend_width: 0.,
            grading_side_slope: default_side_slope(),
            auto_grade: true,
            rail_invert_pitch: false,
            rail_invert_roll: false,
            routes: vec![],
            deleted: vec![],
            transforms: vec![],
            forbidden: vec![],
            forbidden_lines: vec![],
            water: WaterSettings::default(),
            road_colors: Default::default(),
        }
    }
}
fn default_bridge_weight() -> f64 {
    4.
}
fn default_auto_grade() -> bool {
    true
}
fn default_side_slope() -> f64 {
    1.5
}
impl Document {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.grading_side_slope.is_finite() && (0.5..=10.).contains(&self.grading_side_slope),
            "Nieprawidłowe nachylenie skarp"
        );
        ensure!(self.grading_blend_width.is_finite() && (0.0..=500.).contains(&self.grading_blend_width),
            "Nieprawidłowa szerokość wygładzania terenu");
        ensure!(self.version == 1, "Nieobsługiwana wersja projektu");
        ensure!(self.map.valid(), "Nieprawidłowe współrzędne mapy");
        ensure!(
            self.routing.valid() && self.rail_routing.valid(),
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
            self.forbidden_lines.iter().all(|l| l.points.len() >= 2
                && l.points.iter().flatten().all(|x| x.is_finite())
                && l.width.is_finite()
                && l.width > 0.
                && l.width <= 2000.),
            "Nieprawidłowa linia zakazana"
        );
        ensure!(self.water.level.is_finite(), "Nieprawidłowy poziom wody");
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
        old.as_object_mut().unwrap().remove("grading_blend_width");
        let document: Document = serde_json::from_value(old).unwrap();
        document.validate().unwrap();
        assert!(document.road_colors.is_empty());
        assert_eq!(document.grading_blend_width, 0.);
        let mut updated = document;
        updated.grading_blend_width = 12.5;
        let restored: Document = serde_json::from_slice(&serde_json::to_vec(&updated).unwrap()).unwrap();
        assert_eq!(restored.grading_blend_width, 12.5);
    }
}
