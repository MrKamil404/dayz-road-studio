use anyhow::{Context, Result, ensure};
use dayz_road_tool::{
    document::{Document, Route},
    geometry::{self, Library, Point, Shape},
    png,
    raster::Raster,
    road_colors,
    roads::{self, CatalogPart},
    routing,
    terrain::Terrain,
    tv4p,
};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, TextureHandle, Vec2};
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const INK: Color32 = Color32::from_rgb(210, 218, 230);
const ROAD: Color32 = Color32::from_rgb(92, 182, 236);
const SELECT: Color32 = Color32::from_rgb(255, 216, 70);

struct PngSettings {
    width: u32,
    height: u32,
    transparent: bool,
    full_map: bool,
    selected_only: bool,
}
impl Default for PngSettings {
    fn default() -> Self {
        Self {
            width: 15360,
            height: 15360,
            transparent: true,
            full_map: true,
            selected_only: false,
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Select,
    Edit,
    Draw,
    Live,
    Segments,
    Forbidden,
}
enum Loaded {
    Base(PathBuf, tv4p::Project, Vec<CatalogPart>, Vec<String>),
    Satellite(PathBuf, Raster),
    Terrain(PathBuf, PathBuf, Terrain),
    Route(usize, Vec<Point>, Vec<roads::PlacedPart>),
    Export(PathBuf),
    Shp(Vec<dayz_road_tool::shapefile::Line>, String, String),
    Graded(PathBuf, Terrain, usize),
    AscExport(PathBuf),
}
#[derive(Clone)]
struct Generated {
    parts: Vec<roads::PlacedPart>,
    shape: Shape,
}
#[derive(Clone)]
struct Prediction {
    key: Vec<u8>,
    points: Vec<Point>,
    result: std::result::Result<Generated, String>,
}
struct LiveJob {
    key: Vec<u8>,
    points: Vec<Point>,
    rx: Receiver<std::result::Result<Generated, String>>,
    cancel: Arc<AtomicBool>,
}
struct ShpDialog {
    path: PathBuf,
    offset: Point,
    family: String,
    projection: Option<String>,
}
struct Job {
    rx: Receiver<Result<Loaded>>,
    cancel: Arc<AtomicBool>,
    label: String,
}
struct Preview {
    family: String,
    id: Selection,
    shape: Shape,
    bounds: [f64; 4],
}
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Selection {
    Existing(u32),
    Route(usize),
}

pub struct App {
    language: dayz_road_tool::i18n::Language,
    doc: Document,
    file: Option<PathBuf>,
    dirty: bool,
    undo: Vec<Document>,
    redo: Vec<Document>,
    base: Option<tv4p::Project>,
    catalog: Vec<CatalogPart>,
    lib: Library,
    warnings: Vec<String>,
    raster: Option<Raster>,
    terrain: Option<Terrain>,
    terrain_dir: Option<PathBuf>,
    jobs: Vec<Job>,
    center: Point,
    scale: f64,
    tool: Tool,
    selected: Option<Selection>,
    draft: Vec<Point>,
    family: String,
    model: usize,
    reverse: bool,
    angle: f64,
    previews: Vec<Preview>,
    index: HashMap<(i32, i32), Vec<usize>>,
    tiles: HashMap<(usize, u32, u32), TextureHandle>,
    tile_age: Vec<(usize, u32, u32)>,
    status: String,
    contours: bool,
    interval: f64,
    opacity: f32,
    show_sat: bool,
    show_roads: bool,
    drag_point: Option<usize>,
    drag_snapshot: bool,
    contour_cache: Vec<(Point, Point)>,
    contour_key: Option<[u64; 5]>,
    live_epoch: u64,
    live_job: Option<LiveJob>,
    live_candidate: Option<Prediction>,
    live_confirmed: Option<Prediction>,
    live_action: Option<(Vec<Point>, bool)>,
    live_tick: Instant,
    shp_dialog: Option<ShpDialog>,
    png_settings: PngSettings,
    png_job: Option<Receiver<Result<String>>>,
}
impl App {
    fn color_controls(&mut self, ui: &mut egui::Ui) {
        let types = self
            .catalog
            .iter()
            .map(|part| part.family.clone())
            .chain(self.doc.routes.iter().map(|route| route.family.clone()))
            .chain(self.previews.iter().map(|preview| preview.family.clone()))
            .chain(self.doc.road_colors.keys().cloned())
            .collect();
        let previous = self.doc.road_colors.clone();
        if road_colors::editor(ui, &types, &mut self.doc.road_colors, self.language) {
            let edited = std::mem::replace(&mut self.doc.road_colors, previous);
            self.checkpoint();
            self.doc.road_colors = edited;
        }
    }

    fn png_previews(&self) -> Vec<&Preview> {
        self.previews
            .iter()
            .filter(|preview| !self.png_settings.selected_only || self.selected == Some(preview.id))
            .collect()
    }

    fn png_controls(&mut self, ctx: &egui::Context) {
        let lang = self.language;
        egui::TopBottomPanel::bottom("builder-png").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("PNG:");
                egui::ComboBox::from_id_salt("builder-png-scope")
                    .selected_text(lang.tr(if self.png_settings.selected_only { "Zaznaczone" } else { "Wszystkie drogi" }))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.png_settings.selected_only, false, lang.tr("Wszystkie drogi"));
                        ui.selectable_value(&mut self.png_settings.selected_only, true, lang.tr("Tylko zaznaczona droga"));
                    });
                ui.checkbox(&mut self.png_settings.transparent, lang.tr("Alfa"));
                ui.label("px");
                ui.add(egui::DragValue::new(&mut self.png_settings.width).range(128..=20480));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut self.png_settings.height).range(128..=20480));
                if ui.button("15360²").clicked() { self.png_settings.width = 15360; self.png_settings.height = 15360; }
                ui.checkbox(&mut self.png_settings.full_map, lang.tr("Mapa"))
                    .on_hover_text(lang.tr("Pełny zasięg mapy z ustawień projektu; wyłącz, aby dopasować obraz do dróg."));
                let count = self.png_previews().len();
                ui.label(lang.tr(&format!("{count} dróg")));
                if ui.add_enabled(count > 0 && self.png_job.is_none() && self.jobs.is_empty(),
                    egui::Button::new(lang.tr(if self.png_job.is_some() { "Eksportowanie…" } else { "Eksportuj PNG…" })))
                    .on_hover_text(lang.tr("Eksport obejmuje istniejące drogi i dopasowane segmenty. Satelita, poziomice i szkice nie są eksportowane."))
                    .clicked() { self.export_png(); }
            });
        });
    }

    fn export_png(&mut self) {
        let lang = self.language;
        if let Err(error) =
            png::validate_dimensions(self.png_settings.width, self.png_settings.height)
        {
            self.status = error.to_string();
            return;
        }
        if self.png_settings.full_map && !self.doc.map.valid() {
            self.status = "Wymiary mapy muszą być dodatnie, a współrzędne skończone".into();
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .set_file_name("roads.png")
            .set_title(lang.tr("Eksportuj PNG…"))
            .save_file()
            && let Err(error) = self.start_png(path)
        {
            self.status = error.to_string();
        }
    }

    fn start_png(&mut self, path: PathBuf) -> Result<()> {
        ensure!(self.png_job.is_none(), "Eksport PNG już trwa");
        png::validate_dimensions(self.png_settings.width, self.png_settings.height)?;
        ensure!(
            !self.png_settings.full_map || self.doc.map.valid(),
            "Wymiary mapy muszą być dodatnie, a współrzędne skończone"
        );
        for source in [
            &self.file,
            &self.doc.base,
            &self.doc.satellite,
            &self.doc.terrain,
        ]
        .into_iter()
        .flatten()
        {
            ensure!(
                !tv4p::same_path(source, &path),
                "Eksport PNG musi mieć inną ścieżkę niż plik źródłowy"
            );
        }
        let previews = self.png_previews();
        ensure!(
            !previews.is_empty(),
            "Brak dróg w wybranym zakresie eksportu"
        );
        let shapes: Vec<_> = previews
            .iter()
            .enumerate()
            .map(|(index, preview)| {
                (
                    preview.shape.clone(),
                    index,
                    self.selected == Some(preview.id),
                    self.doc.road_colors.get(&preview.family).copied(),
                )
            })
            .collect();
        let map = &self.doc.map;
        let bounds = self.png_settings.full_map.then_some([
            map.east,
            map.north,
            map.east + map.width,
            map.north + map.height,
        ]);
        let (width, height, alpha) = (
            self.png_settings.width,
            self.png_settings.height,
            self.png_settings.transparent,
        );
        let count = shapes.len();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("builder-png-export".into())
            .spawn(move || {
                let roads: Vec<_> = shapes
                    .iter()
                    .map(|(shape, index, _, color)| png::Road {
                        index: *index,
                        is_new: false,
                        color: *color,
                        triangles: &shape.triangles,
                        lines: &shape.lines,
                    })
                    .collect();
                let selected = shapes
                    .iter()
                    .filter(|(_, _, selected, _)| *selected)
                    .map(|(_, index, _, _)| *index)
                    .collect();
                let result = png::png_in_bounds(
                    &path, &roads, &selected, width, height, alpha, bounds,
                )
                .map(|_| {
                    format!(
                        "PNG: {count} dróg · {width} × {height} px · {}",
                        path.display()
                    )
                });
                let _ = tx.send(result);
            })?;
        self.png_job = Some(rx);
        self.status = format!("Eksportowanie {count} dróg do PNG {width} × {height}…");
        Ok(())
    }

    pub fn new() -> Self {
        Self::initial()
    }
    pub fn set_language(&mut self, language: dayz_road_tool::i18n::Language) {
        self.language = language;
    }
    pub fn style() -> egui::Style {
        egui::Style {
            visuals: egui::Visuals::dark(),
            ..Default::default()
        }
    }

    pub fn tick(&mut self, ctx: &egui::Context) {
        self.poll();
        if let Some(job) = &self.png_job {
            match job.try_recv() {
                Ok(result) => {
                    self.png_job = None;
                    self.status = result.unwrap_or_else(|e| format!("{e:#}"));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.png_job = None;
                    self.status = "Eksport PNG został przerwany".into();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(100))
                }
            }
        }
        if !self.jobs.is_empty() || self.live_job.is_some() || self.live_action.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
    pub fn has_unsaved_changes(&self) -> bool {
        self.png_job.is_some()
            || self.dirty
            || !self.draft.is_empty()
            || self.live_action.is_some()
            || self
                .jobs
                .iter()
                .any(|job| !job.cancel.load(Ordering::Relaxed))
    }
    pub fn confirm_close(&self, ctx: &egui::Context) {
        let lang = self.language;
        self.guard_close(ctx, || {
            rfd::MessageDialog::new()
                .set_title(lang.tr("Niezapisane zmiany"))
                .set_description(lang.tr("Zamknąć aplikację bez zapisu projektu?"))
                .set_buttons(rfd::MessageButtons::YesNo)
                .show()
                == rfd::MessageDialogResult::Yes
        });
    }
    fn guard_close(&self, ctx: &egui::Context, confirm: impl FnOnce() -> bool) {
        if self.has_unsaved_changes() && ctx.input(|i| i.viewport().close_requested()) && !confirm()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }
    fn initial() -> Self {
        Self {png_settings: PngSettings::default(),png_job: None,language: Default::default(),doc:Document::default(),file:None,dirty:false,undo:vec![],redo:vec![],base:None,catalog:vec![],lib:Library::new(Document::default().models),warnings:vec![],raster:None,terrain:None,terrain_dir:None,jobs:vec![],center:[207680.,7680.],scale:0.05,tool:Tool::Draw,selected:None,draft:vec![],family:String::new(),model:0,reverse:false,angle:90.,previews:vec![],index:HashMap::new(),tiles:HashMap::new(),tile_age:vec![],status:"Otwórz TV4P, wybierz modele MLOD i wczytaj mapę. Punkty trasy dodajesz kliknięciem.".into(),contours:true,interval:10.,opacity:1.,show_sat:true,show_roads:true,drag_point:None,drag_snapshot:false,contour_cache:vec![],contour_key:None,live_epoch:0,live_job:None,live_candidate:None,live_confirmed:None,live_action:None,live_tick:Instant::now()-Duration::from_secs(1),shp_dialog:None}
    }
    fn checkpoint(&mut self) {
        self.undo.push(self.doc.clone());
        if self.undo.len() > 32 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
    }
    fn undo(&mut self, redo: bool) {
        if self.tool == Tool::Live && !self.draft.is_empty() && !redo {
            self.draft.pop();
            self.clear_live();
            return;
        }
        let d = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some(d) = d {
            if d.terrain_cache != self.doc.terrain_cache {
                let restored = d
                    .terrain_cache
                    .as_ref()
                    .map(|p| Terrain::open(p))
                    .transpose();
                match restored {
                    Ok(t) => {
                        self.terrain = t;
                        self.terrain_dir = d.terrain_cache.clone();
                    }
                    Err(e) => {
                        self.status = format!("Nie można przywrócić ASC: {e:#}");
                        if redo {
                            self.redo.push(d);
                        } else {
                            self.undo.push(d);
                        }
                        return;
                    }
                }
                self.clear_live();
                self.live_epoch = self.live_epoch.wrapping_add(1);
            }
            if redo {
                self.undo.push(self.doc.clone());
            } else {
                self.redo.push(self.doc.clone());
            }
            self.doc.routes = d.routes;
            self.doc.deleted = d.deleted;
            self.doc.transforms = d.transforms;
            self.doc.forbidden = d.forbidden;
            self.doc.terrain = d.terrain;
            self.doc.terrain_cache = d.terrain_cache;
            self.doc.terrain_origin = d.terrain_origin;
            self.doc.grading_blend_width = d.grading_blend_width;
            self.dirty = true;
            self.selected = None;
            self.rebuild();
        }
    }
    fn start(
        &mut self,
        label: &str,
        f: impl FnOnce(Arc<AtomicBool>) -> Result<Loaded> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(flag)))
                .unwrap_or_else(|_| {
                    Err(anyhow::anyhow!("Zadanie przerwane przez błąd wewnętrzny"))
                });
            let _ = tx.send(r);
        });
        self.jobs.push(Job {
            rx,
            cancel,
            label: label.into(),
        });
    }
    fn cache_dir(kind: &str) -> Result<PathBuf> {
        let dir = std::env::current_dir()?.join(".road-cache").join(format!(
            "{kind}-{}",
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }
    fn load_base(&mut self, path: PathBuf) {
        let root = self.doc.models.clone();
        self.start("Odczyt TV4P i modeli", move |_| {
            let mut lib = Library::new(root);
            let p = tv4p::load(&path, &mut lib)?;
            let (c, w) = tv4p::catalog(&path, &mut lib)?;
            Ok(Loaded::Base(path, p, c, w))
        });
    }
    fn import_sat(&mut self, path: PathBuf) {
        self.start("Import satelity i poziomów powiększenia", move |cancel| {
            let dir = Self::cache_dir("sat")?;
            let r = dayz_road_tool::raster::import(&path, &dir, &cancel)?;
            Ok(Loaded::Satellite(path, r))
        });
    }
    fn import_asc(&mut self, path: PathBuf) {
        self.start("Import wysokości ASC", move |cancel| {
            let dir = Self::cache_dir("asc")?;
            let t = dayz_road_tool::terrain::import(&path, &dir, &cancel)?;
            Ok(Loaded::Terrain(path, dir, t))
        });
    }
    fn poll(&mut self) {
        self.poll_live();
        let mut done = Vec::new();
        for (i, j) in self.jobs.iter().enumerate() {
            match j.rx.try_recv() {
                Ok(r) => done.push((i, r)),
                Err(mpsc::TryRecvError::Disconnected) => {
                    done.push((i, Err(anyhow::anyhow!("Zadanie przerwane"))))
                }
                Err(_) => {}
            }
        }
        for (i, r) in done.into_iter().rev() {
            let job = self.jobs.remove(i);
            if job.cancel.load(Ordering::Relaxed) {
                self.status = "Anulowano zadanie".into();
                continue;
            }
            match r {
                Ok(Loaded::Base(path, p, c, w)) => {
                    self.live_epoch = self.live_epoch.wrapping_add(1);
                    self.clear_live();
                    self.doc.base = Some(path);
                    self.base = Some(p);
                    self.catalog = c;
                    self.warnings = w;
                    self.lib = Library::new(self.doc.models.clone());
                    self.family = self
                        .catalog
                        .first()
                        .map(|p| p.family.clone())
                        .unwrap_or_default();
                    self.status = format!(
                        "Wczytano {} dróg i {} modeli MLOD",
                        self.base.as_ref().unwrap().roads.len(),
                        self.catalog.len()
                    );
                    self.rebuild();
                }
                Ok(Loaded::Satellite(path, r)) => {
                    self.doc.satellite = Some(path);
                    self.doc.satellite_cache = Some(r.dir.clone());
                    self.raster = Some(r);
                    self.tiles.clear();
                    self.tile_age.clear();
                    self.dirty = true;
                    self.status =
                        "Satelita gotowa. Ustaw zasięg w metrach; obraz jest wyrównany do mapy."
                            .into();
                }
                Ok(Loaded::Terrain(path, dir, mut t)) => {
                    self.checkpoint();
                    if self.doc.terrain.as_ref() != Some(&path) {
                        self.doc.terrain_origin = None;
                    }
                    self.doc.terrain = Some(path);
                    self.doc.terrain_cache = Some(dir.clone());
                    if let Some(p) = self.doc.terrain_origin {
                        t.meta.east = p[0];
                        t.meta.north = p[1];
                    }
                    self.doc.terrain_origin = Some([t.meta.east, t.meta.north]);
                    self.terrain_dir = Some(dir);
                    self.terrain = Some(t);
                    self.contour_key = None;
                    self.dirty = true;
                    self.status =
                        "ASC gotowy. Współrzędne pochodzą z nagłówka; można je dopasować w panelu."
                            .into();
                }
                Ok(Loaded::Graded(dir, t, count)) => {
                    self.checkpoint();
                    self.clear_live();
                    self.live_epoch = self.live_epoch.wrapping_add(1);
                    self.doc.terrain_cache = Some(dir.clone());
                    self.doc.terrain_origin = Some([t.meta.east, t.meta.north]);
                    self.terrain_dir = Some(dir);
                    self.terrain = Some(t);
                    self.contour_key = None;
                    self.status = format!(
                        "Zmieniono {count} komórek ASC pod partami. Ctrl+Z cofa operację. Zapisz wynik przyciskiem Eksport ASC."
                    );
                }
                Ok(Loaded::AscExport(path)) => {
                    self.status = format!("Zapisano zmodyfikowany teren: {}", path.display())
                }
                Ok(Loaded::Route(index, points, parts)) => {
                    if index < self.doc.routes.len() {
                        self.checkpoint();
                        self.doc.routes[index].points = points;
                        self.doc.routes[index].parts = parts;
                        self.rebuild();
                        self.status = "Trasa wyznaczona i złożona z modeli".into();
                    }
                }
                Ok(Loaded::Export(path)) => {
                    self.status = format!(
                        "Zapisano {}. Sprawdź wynik w Terrain Builder.",
                        path.display()
                    )
                }
                Ok(Loaded::Shp(lines, family, stem)) => {
                    self.checkpoint();
                    let first = self.doc.routes.len();
                    let count = lines.len();
                    let mut b = [
                        f64::INFINITY,
                        f64::INFINITY,
                        f64::NEG_INFINITY,
                        f64::NEG_INFINITY,
                    ];
                    for line in lines {
                        for p in &line.points {
                            b[0] = b[0].min(p[0]);
                            b[1] = b[1].min(p[1]);
                            b[2] = b[2].max(p[0]);
                            b[3] = b[3].max(p[1]);
                        }
                        self.doc.routes.push(Route {
                            name: format!("{stem} #{}.{}", line.record, line.part + 1),
                            family: family.clone(),
                            points: line.points,
                            parts: vec![],
                            replaces: None,
                        });
                    }
                    self.selected = Some(Selection::Route(first));
                    self.center = [b[0] / 2. + b[2] / 2., b[1] / 2. + b[3] / 2.];
                    self.scale =
                        (600. / (b[2] - b[0]).max(b[3] - b[1]).max(20.)).clamp(0.0001, 100.);
                    self.rebuild();
                    self.status =
                        format!("Zaimportowano {count} tras SHP. Wybierz trasę i dopasuj modele.");
                }
                Err(e) => self.status = format!("{e:#}"),
            }
        }
    }
    fn rebuild(&mut self) {
        if let Some(t) = &mut self.terrain
            && let Some(p) = self.doc.terrain_origin
        {
            t.meta.east = p[0];
            t.meta.north = p[1];
        }
        self.contour_key = None;
        self.previews.clear();
        self.index.clear();
        if let Some(base) = self.base.clone() {
            for r in &base.roads {
                if self.doc.deleted.contains(&r.id)
                    || self.doc.routes.iter().any(|v| v.replaces == Some(r.id))
                {
                    continue;
                }
                let mut shape = r.shape.clone();
                if let Some((_, d, rotation)) =
                    self.doc.transforms.iter().find(|(id, _, _)| *id == r.id)
                {
                    let tr = |p: Point| {
                        geometry::add(
                            geometry::add(
                                geometry::rotate(geometry::sub(p, r.start), -rotation.to_radians()),
                                r.start,
                            ),
                            *d,
                        )
                    };
                    for t in &mut shape.triangles {
                        for p in t {
                            *p = tr(*p);
                        }
                    }
                    for line in &mut shape.lines {
                        for p in line {
                            *p = tr(*p);
                        }
                    }
                }
                self.push_preview(
                    Selection::Existing(r.id),
                    shape,
                    road_colors::family(&r.model),
                );
            }
        }
        // Avoid borrowing the document while updating spatial bins.
        let routes = self.doc.routes.clone();
        for (i, r) in routes.iter().enumerate() {
            if let Ok(s) = roads::shape(&r.parts, &mut self.lib) {
                self.push_preview(Selection::Route(i), s, r.family.clone());
            }
        }
    }
    fn push_preview(&mut self, id: Selection, shape: Shape, family: String) {
        let mut b = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for p in shape
            .lines
            .iter()
            .flatten()
            .chain(shape.triangles.iter().flatten())
        {
            b[0] = b[0].min(p[0]);
            b[1] = b[1].min(p[1]);
            b[2] = b[2].max(p[0]);
            b[3] = b[3].max(p[1]);
        }
        if !b[0].is_finite() {
            return;
        }
        let index = self.previews.len();
        let bins = ((b[2] - b[0]) / 512. + 2.) * ((b[3] - b[1]) / 512. + 2.);
        if bins < 10000. {
            for x in (b[0] / 512.).floor() as i32..=(b[2] / 512.).floor() as i32 {
                for y in (b[1] / 512.).floor() as i32..=(b[3] / 512.).floor() as i32 {
                    self.index.entry((x, y)).or_default().push(index);
                }
            }
        } else {
            self.index
                .entry((i32::MIN, i32::MIN))
                .or_default()
                .push(index);
        }
        self.previews.push(Preview {
            family,
            id,
            shape,
            bounds: b,
        });
    }
    fn save(&mut self) {
        let lang = self.language;
        if self.tool == Tool::Live && !self.draft.is_empty() {
            self.finish();
            if !self.draft.is_empty() {
                self.status = "Zakończ generowanie trasy przed zapisem projektu".into();
                return;
            }
        }
        let p = self.file.clone().or_else(|| {
            rfd::FileDialog::new()
                .add_filter(lang.tr("Projekt dróg"), &["dzroad"])
                .save_file()
        });
        if let Some(p) = p {
            match self.doc.save(&p) {
                Ok(()) => {
                    self.status = format!("Zapisano {}", p.display());
                    self.file = Some(p);
                    self.dirty = false;
                }
                Err(e) => self.status = e.to_string(),
            }
        }
    }
    fn open(&mut self) {
        let lang = self.language;
        if let Some(p) = rfd::FileDialog::new()
            .add_filter(lang.tr("Projekt dróg"), &["dzroad"])
            .pick_file()
        {
            match Document::load(&p) {
                Ok(d) => {
                    for j in &self.jobs {
                        j.cancel.store(true, Ordering::Relaxed);
                    }
                    self.jobs.clear();
                    self.doc = d;
                    self.file = Some(p);
                    self.undo.clear();
                    self.redo.clear();
                    self.selected = None;
                    self.draft.clear();
                    self.clear_live();
                    self.base = None;
                    self.catalog.clear();
                    self.raster = None;
                    self.terrain = None;
                    self.terrain_dir = None;
                    self.tiles.clear();
                    self.contour_key = None;
                    self.dirty = false;
                    if let Some(p) = self.doc.base.clone() {
                        self.load_base(p);
                    }
                    self.raster = self
                        .doc
                        .satellite_cache
                        .as_ref()
                        .and_then(|p| Raster::open(p).ok());
                    if self.raster.is_none()
                        && let Some(p) = self.doc.satellite.clone()
                    {
                        self.import_sat(p);
                    }
                    self.terrain = self
                        .doc
                        .terrain_cache
                        .as_ref()
                        .and_then(|p| Terrain::open(p).ok());
                    self.terrain_dir = self
                        .doc
                        .terrain_cache
                        .clone()
                        .filter(|_| self.terrain.is_some());
                    if self.terrain.is_none()
                        && let Some(p) = self.doc.terrain.clone()
                    {
                        self.import_asc(p);
                    }
                    self.rebuild();
                }
                Err(e) => self.status = e.to_string(),
            }
        }
    }
    fn finish(&mut self) {
        if self.tool == Tool::Live {
            self.request_live_finish();
            return;
        }
        if self.draft.len() < 2 {
            self.status = "Dodaj przynajmniej dwa punkty".into();
            return;
        }
        if self.tool == Tool::Forbidden {
            if self.draft.len() < 3 {
                self.status = "Obszar wymaga trzech punktów".into();
                return;
            }
            self.checkpoint();
            self.doc.forbidden.push(std::mem::take(&mut self.draft));
            return;
        }
        self.checkpoint();
        let index = self.doc.routes.len();
        self.doc.routes.push(Route {
            name: format!("Droga {}", index + 1),
            family: self.family.clone(),
            points: std::mem::take(&mut self.draft),
            parts: vec![],
            replaces: None,
        });
        self.selected = Some(Selection::Route(index));
        self.status = "Trasa zapisana. Dopasuj modele lub uruchom automat.".into();
    }
    fn generate(&mut self, automatic: bool) {
        let Some(Selection::Route(index)) = self.selected else {
            self.status = "Wybierz narysowaną trasę".into();
            return;
        };
        let route = self.doc.routes[index].clone();
        let settings = self.doc.routing.clone();
        let catalog = self.catalog.clone();
        let forbidden = self.doc.forbidden.clone();
        let dir = self.terrain_dir.clone();
        let meta = self.terrain.as_ref().map(|t| t.meta.clone());
        self.start(
            if automatic {
                "Szukanie trasy i dopasowanie modeli"
            } else {
                "Dopasowanie modeli"
            },
            move |cancel| {
                let terrain = dir.map(|d| Terrain::open(&d)).transpose()?.map(|mut t| {
                    if let Some(m) = meta {
                        t.meta = m;
                    }
                    t
                });
                let points = if automatic {
                    routing::find(
                        terrain
                            .as_ref()
                            .ok_or_else(|| anyhow::anyhow!("Wczytaj ASC"))?,
                        &route.points,
                        &settings,
                        &forbidden,
                        &cancel,
                    )?
                } else {
                    route.points.clone()
                };
                let parts = roads::fit(&points, &route.family, &catalog, &settings, |a, b| {
                    !cancel.load(Ordering::Relaxed)
                        && terrain
                            .as_ref()
                            .map(|t| routing::permitted(a, b, t, &settings, &forbidden))
                            .unwrap_or_else(|| !routing::blocked_segment(a, b, &forbidden))
                })?;
                ensure!(!cancel.load(Ordering::Relaxed), "Anulowano");
                Ok(Loaded::Route(index, points, parts))
            },
        );
    }
    fn insert_route_point(&mut self, index: usize, segment: usize, point: Point) {
        if self.doc.routes.get(index).is_none_or(|r| segment + 1 >= r.points.len()) {
            return;
        }
        self.checkpoint();
        let route = &mut self.doc.routes[index];
        route.points.insert(segment + 1, point);
        route.parts.clear();
        self.rebuild();
    }

    fn remove_route_point(&mut self, index: usize, point: usize) {
        if self.doc.routes.get(index).is_none_or(|r| r.points.len() <= 2 || point >= r.points.len()) {
            return;
        }
        self.checkpoint();
        let route = &mut self.doc.routes[index];
        route.points.remove(point);
        route.parts.clear();
        self.rebuild();
    }

    fn export(&mut self) {
        let lang = self.language;
        if self.tool == Tool::Live && !self.draft.is_empty() {
            self.finish();
            if !self.draft.is_empty() {
                self.status = "Zakończ generowanie trasy przed eksportem TV4P".into();
                return;
            }
        }
        if self.doc.routes.iter().any(|r| r.parts.is_empty()) {
            self.status = "Dopasuj modele do wszystkich tras przed eksportem".into();
            return;
        }
        let Some(base) = self.doc.base.clone() else {
            self.status = "Wczytaj bazowy TV4P".into();
            return;
        };
        if let Some(out) = rfd::FileDialog::new()
            .add_filter(lang.tr("Terrain Builder"), &["tv4p"])
            .set_file_name("roads-edited.tv4p")
            .save_file()
        {
            let d = self.doc.clone();
            let c = self.catalog.clone();
            let terrain_dir = self.terrain_dir.clone();
            let meta = self.terrain.as_ref().map(|t| t.meta.clone());
            self.start("Zapis i weryfikacja TV4P", move |cancel| {
                let terrain = terrain_dir
                    .map(|p| Terrain::open(&p))
                    .transpose()?
                    .map(|mut t| {
                        if let Some(m) = meta {
                            t.meta = m;
                        }
                        t
                    });
                for route in &d.routes {
                    let line = roads::centerline(&route.parts, &c);
                    for w in line.windows(2) {
                        ensure!(!cancel.load(Ordering::Relaxed), "Anulowano eksport");
                        ensure!(
                            terrain
                                .as_ref()
                                .map(|t| routing::permitted(
                                    w[0],
                                    w[1],
                                    t,
                                    &d.routing,
                                    &d.forbidden
                                ))
                                .unwrap_or_else(|| !routing::blocked_segment(
                                    w[0],
                                    w[1],
                                    &d.forbidden
                                )),
                            "Droga {} przecina obszar zakazany lub nie spełnia ograniczeń ASC",
                            route.name
                        );
                    }
                }
                ensure!(!cancel.load(Ordering::Relaxed), "Anulowano eksport");
                tv4p::export_editor(&base, &out, &d, &c)?;
                Ok(Loaded::Export(out))
            });
        }
    }
    fn delete(&mut self) {
        let Some(s) = self.selected else { return };
        self.checkpoint();
        match s {
            Selection::Route(i) => {
                self.doc.routes.remove(i);
            }
            Selection::Existing(id) => self.doc.deleted.push(id),
        }
        self.selected = None;
        self.rebuild();
    }
    fn controls(&mut self, ctx: &egui::Context) {
        let lang = self.language;
        self.png_controls(ctx);
        let busy = !self.jobs.is_empty() || self.shp_dialog.is_some();
        egui::TopBottomPanel::top("toolbar").show(ctx,|ui|{ui.horizontal_wrapped(|ui|{
            ui.heading(lang.tr("DAYZ / ROAD TOOL"));ui.separator();
            ui.add_enabled_ui(!busy,|ui|{
                if ui.button(lang.tr("Otwórz TV4P")).clicked() && let Some(p)=rfd::FileDialog::new().add_filter(lang.tr("Terrain Builder"),&["tv4p"]).pick_file() {if self.doc.base.as_ref()!=Some(&p) && (self.doc.base.is_some() || !self.doc.routes.is_empty()) {self.status="Aby zmienić bazowy TV4P, uruchom nową sesję aplikacji lub otwórz zapisany projekt.".into();}else{self.load_base(p);}}
                if ui.button(lang.tr("Otwórz projekt")).clicked(){if self.dirty && !rfd::MessageDialog::new().set_title(lang.tr("Niezapisane zmiany")).set_description(lang.tr("Odrzucić niezapisane zmiany i otworzyć projekt?")).set_buttons(rfd::MessageButtons::YesNo).show().eq(&rfd::MessageDialogResult::Yes) {}else{self.open();}}
                if ui.button(lang.tr("Zapisz projekt")).clicked(){self.save();}
if ui.button(lang.tr("Eksport TV4P")).clicked(){self.export();}
                ui.separator();if ui.button(lang.tr("Cofnij")).on_hover_text(lang.tr("Cofnij · Ctrl+Z")).clicked(){self.undo(false);}
if ui.button(lang.tr("Ponów")).on_hover_text(lang.tr("Ponów · Ctrl+Y")).clicked(){self.undo(true);}
            });
            if self.dirty {ui.label(lang.tr("● zmiany"));}
        });});
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if busy {
                    ui.spinner();
                    let labels = self
                        .jobs
                        .iter()
                        .map(|j| lang.tr(&j.label))
                        .collect::<Vec<_>>()
                        .join(" · ");
                    ui.label(labels);
                    if ui.button(lang.tr("Anuluj")).clicked() {
                        for j in &self.jobs {
                            j.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                } else {
                    ui.label(lang.tr(&self.status));
                }
            });
        });
        egui::SidePanel::left("library").default_width(260.).show(ctx,|ui|{
            ui.heading(lang.tr("Warsztat dróg"));self.color_controls(ui);ui.add_enabled_ui(!busy,|ui|{ui.horizontal_wrapped(|ui|{for (tool,label) in [(Tool::Select,"Wybierz"),(Tool::Edit,"Edytuj punkty"),(Tool::Draw,"Rysuj"),(Tool::Live,"Na żywo"),(Tool::Segments,"Segmenty"),(Tool::Forbidden,"Zakaz")] {if ui.selectable_label(self.tool==tool,lang.tr(label)).clicked(){self.tool=tool;self.draft.clear();self.clear_live();}}});
            if ui.button(lang.tr("Folder modeli MLOD…")).clicked() && let Some(p)=rfd::FileDialog::new().pick_folder(){self.doc.models=p;self.dirty=true;if let Some(p)=self.doc.base.clone(){self.load_base(p);}}
            ui.small(lang.tr(&(self.doc.models.display().to_string())));
            let families:BTreeSet<_>=self.catalog.iter().map(|p|p.family.clone()).collect();egui::ComboBox::from_label(lang.tr("Typ")).selected_text(lang.tr(&self.family)).show_ui(ui,|ui|{for f in families {ui.selectable_value(&mut self.family,f.clone(),lang.tr(&(f)));}});
            if ui.button(lang.tr("Import dróg SHP…")).clicked() && let Some(path)=rfd::FileDialog::new().add_filter(lang.tr("Shapefile"),&["shp"]).pick_file(){let projection=std::fs::read_to_string(path.with_extension("prj")).ok().map(|p|p.chars().take(4000).collect());self.shp_dialog=Some(ShpDialog{path,offset:[0.,0.],family:self.family.clone(),projection});}
            if self.tool==Tool::Segments {
                egui::ComboBox::from_label(lang.tr("Model")).selected_text(lang.tr(self.catalog.get(self.model).map(|p|p.path.rsplit(['\\','/']).next().unwrap_or("")).unwrap_or("Brak MLOD"))).show_ui(ui,|ui|{for (i,p) in self.catalog.iter().enumerate().filter(|(_,p)|p.family==self.family) {ui.selectable_value(&mut self.model,i,lang.tr(p.path.rsplit(['\\','/']).next().unwrap_or("") ));}});
                ui.checkbox(&mut self.reverse,lang.tr("Odwróć zakręt"));ui.horizontal(|ui|{ui.label(lang.tr("Kierunek startu °"));ui.add(egui::DragValue::new(&mut self.angle).speed(1.));});ui.small(lang.tr("Kliknij początek. Kolejne segmenty dodasz przyciskiem."));
                if ui.button(lang.tr("Dodaj segment")).clicked(){self.add_segment();}
            }else if self.tool==Tool::Draw || self.tool==Tool::Live || self.tool==Tool::Forbidden {
                ui.small(lang.tr("Klik: punkt · Enter: zakończ · Esc: anuluj\nŚrodkowy/prawy przycisk: przesuwanie"));
                if ui.button(lang.tr("Zakończ rysowanie")).clicked(){self.finish();}
if ui.button(lang.tr("Usuń ostatni punkt")).clicked(){self.draft.pop();self.clear_live();}
                if self.tool==Tool::Live{ui.small(lang.tr("Zielone segmenty: przewidywany przebieg. Klik zatwierdza poprawne dopasowanie. Enter zapisuje gotową drogę."));}
            }
            if self.tool==Tool::Edit {ui.small(lang.tr("Przeciągnij punkt, aby zmienić trasę. Podwójny klik na linii dodaje punkt; podwójny prawy klik na punkcie usuwa go. Po edycji dopasuj modele ponownie."));}
            ui.separator();ui.heading(lang.tr("Trasy projektu"));
            let rows:Vec<_>=self.doc.routes.iter().enumerate().map(|(i,r)|(i,format!("{} · {} części",r.name,r.parts.len()))).collect();
            egui::ScrollArea::vertical().max_height(240.).show(ui,|ui|{for (i,label) in rows {if ui.selectable_label(self.selected==Some(Selection::Route(i)),lang.tr(&label)).clicked(){self.selected=Some(Selection::Route(i));}}});
            if let Some(Selection::Route(i))=self.selected {
                if ui.button(lang.tr("Edytuj punkty")).clicked(){self.tool=Tool::Edit;self.draft.clear();self.clear_live();}
                if let Some(r)=self.doc.routes.get_mut(i) && ui.text_edit_singleline(&mut r.name).changed(){self.dirty=true;}
if ui.button(lang.tr("Ustaw typ wybranej trasy")).clicked(){self.checkpoint();self.doc.routes[i].family=self.family.clone();self.doc.routes[i].parts.clear();self.rebuild();}
                if ui.button(lang.tr("Dopasuj modele do punktów")).clicked(){self.generate(false);}
if ui.button(lang.tr("Wyznacz trasę po terenie")).clicked(){self.generate(true);}
                if ui.button(lang.tr("Usuń ostatni segment")).clicked(){self.checkpoint();self.doc.routes[i].parts.pop();self.rebuild();}
            }
            if let Some(Selection::Existing(id))=self.selected {ui.label(lang.tr(&(format!("Droga TV4P #{id}"))));ui.small(lang.tr("Przeciągnij drogę w trybie Wybierz, aby ją przesunąć."));if ui.button(lang.tr("Obróć o 5°")).clicked(){self.checkpoint();if let Some(t)=self.doc.transforms.iter_mut().find(|t|t.0==id){t.2+=5.;}else{self.doc.transforms.push((id,[0.,0.],5.));}self.rebuild();}}
            if self.selected.is_some() && ui.button(lang.tr("Usuń wybraną drogę")).clicked(){self.delete();}
            if !self.doc.forbidden.is_empty() && ui.button(lang.tr("Usuń ostatni obszar zakazany")).clicked(){self.checkpoint();self.doc.forbidden.pop();}
            });
            if !self.warnings.is_empty(){ui.separator();ui.collapsing(lang.tr(&(format!("Uwagi modeli ({})",self.warnings.len()))),|ui|{egui::ScrollArea::vertical().max_height(160.).show(ui,|ui|{for w in &self.warnings{ui.small(lang.tr(w));}});});}
        });
        egui::SidePanel::right("terrain")
            .default_width(270.)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading(lang.tr("Mapa i teren"));
                    ui.add_enabled_ui(!busy, |ui| {
                        if ui.button(lang.tr("Satelita BMP / PNG…")).clicked()
                            && let Some(p) = rfd::FileDialog::new()
                                .add_filter(lang.tr("Satelita"), &["bmp", "png"])
                                .pick_file()
                        {
                            self.import_sat(p);
                        }
                        if ui.button(lang.tr("Wysokości ASC…")).clicked()
                            && let Some(p) = rfd::FileDialog::new()
                                .add_filter(lang.tr("ESRI ASCII Grid"), &["asc"])
                                .pick_file()
                        {
                            self.import_asc(p);
                        }
                        ui.checkbox(&mut self.show_sat,lang.tr("Satelita"));
                        ui.add(egui::Slider::new(&mut self.opacity, 0.0..=1.0).text("Widoczność"));
                        ui.checkbox(&mut self.show_roads,lang.tr("Modele dróg"));
                        ui.checkbox(&mut self.contours,lang.tr("Poziomice"));
                        ui.horizontal(|ui| {
                            ui.label(lang.tr("Odstęp m"));
                            ui.add(
                                egui::DragValue::new(&mut self.interval)
                                    .range(0.5..=500.)
                                    .speed(0.5),
                            );
                        });
                        ui.separator();
                        ui.label(lang.tr("Lewy dolny róg / metry"));
                        let m = &mut self.doc.map;
                        for (label, value) in [
                            ("E", &mut m.east),
                            ("N", &mut m.north),
                            ("Szerokość", &mut m.width),
                            ("Wysokość", &mut m.height),
                        ] {
                            ui.horizontal(|ui| {
                                ui.label(lang.tr(label));
                                if ui.add(egui::DragValue::new(value).speed(10.)).changed() {
                                    self.dirty = true;
                                }
                            });
                        }
                        if ui.button(lang.tr("1 piksel = 1 metr")).clicked()
                            && let Some(r) = &self.raster
                        {
                            self.doc.map.width = r.levels[0].width as f64;
                            self.doc.map.height = r.levels[0].height as f64;
                            self.dirty = true;
                        }
                        if ui.button(lang.tr("Dopasuj widok mapy")).clicked() {
                            let m = &self.doc.map;
                            self.center = [m.east + m.width / 2., m.north + m.height / 2.];
                            self.scale = 600. / m.width.max(m.height);
                        }
                        if let Some(t) = &mut self.terrain {
                            ui.separator();
                            ui.label(lang.tr(&(format!(
                                "ASC {} × {} · komórka {} m",
                                t.meta.cols, t.meta.rows, t.meta.cell
                            ))));
                            let mut changed = false;
                            ui.horizontal(|ui| {
                                ui.label(lang.tr("ASC E"));
                                changed |= ui
                                    .add(egui::DragValue::new(&mut t.meta.east).speed(10.))
                                    .changed();
                            });
                            ui.horizontal(|ui| {
                                ui.label(lang.tr("ASC N"));
                                changed |= ui
                                    .add(egui::DragValue::new(&mut t.meta.north).speed(10.))
                                    .changed();
                            });
                            if ui.button(lang.tr("Wyrównaj ASC do początku mapy")).clicked() {
                                t.meta.east = self.doc.map.east;
                                t.meta.north = self.doc.map.north;
                                changed = true;
                            }
                            if changed {
                                self.doc.terrain_origin = Some([t.meta.east, t.meta.north]);
                                self.dirty = true;
                                self.contour_key = None;
                            }
                        }
                        ui.separator();
                        ui.heading(lang.tr("Teren pod partami"));
                        ui.small(lang.tr("Płynny profil pod drogą i łagodne przejście do otaczającego terenu. Szerokość liczona od krawędzi partów; 0 m zmienia tylko obrys MLOD."));
                        ui.horizontal(|ui| {
                            ui.label(lang.tr("Szerokość wygładzania z każdej strony [m]"));
                            let mut width = self.doc.grading_blend_width;
                            if ui.add(egui::DragValue::new(&mut width).range(0.0..=500.).speed(0.5)).changed() {
                                self.checkpoint();
                                self.doc.grading_blend_width = width;
                            }
                        });
                        let can_grade = self.terrain.is_some() && self.draft.is_empty() && matches!(self.selected, Some(Selection::Route(i)) if !self.doc.routes[i].parts.is_empty());
                        if ui.add_enabled(can_grade, egui::Button::new("Dopasuj ASC pod wybraną drogą")).clicked() { self.grade_asc(); }
                        if ui.add_enabled(self.terrain.is_some(), egui::Button::new("Eksport ASC…")).clicked() { self.export_asc(); }
                        ui.small(lang.tr("Ctrl+Z cofa modyfikację terenu. Źródłowy ASC pozostaje zachowany."));
                        ui.separator();
                        ui.heading(lang.tr("Ograniczenia drogi"));
                        let s = &mut self.doc.routing;
                        for (label, value, range) in [
                            ("Spadek maks. %", &mut s.max_grade, 0.1..=100.),
                            ("Kara za spadek", &mut s.slope_weight, 0.0..=100.),
                            ("Krok automatu m", &mut s.cell, 1.0..=500.),
                            ("Promień min. m", &mut s.min_radius, 1.0..=2000.),
                            ("Tolerancja trasy m", &mut s.tolerance, 0.1..=100.),
                        ] {
                            ui.horizontal(|ui| {
                                ui.label(lang.tr(label));
                                if ui
                                    .add(egui::DragValue::new(value).range(range).speed(0.5))
                                    .changed()
                                {
                                    self.dirty = true;
                                }
                            });
                        }
                    });
                    ui.separator();
                    self.profile(ui);
                });
            });
    }
    fn add_segment(&mut self) {
        let Some(item) = self.catalog.get(self.model).cloned() else {
            self.status = "Wczytaj modele MLOD".into();
            return;
        };
        if self.reverse && item.category != 4 {
            self.status = "Odwrócić można zakręt".into();
            return;
        }
        let Some(Selection::Route(i)) = self.selected else {
            self.status = "Kliknij na mapie początek nowej drogi".into();
            return;
        };
        if self.doc.routes[i].family != item.family {
            self.status = "Wybierz model tego samego typu co droga".into();
            return;
        }
        if self.doc.routes[i].parts.is_empty() && self.reverse {
            self.status = "Pierwszy segment musi być ustawiony bez odwrócenia".into();
            return;
        }
        let route = &self.doc.routes[i];
        let Some(start) = route.points.first().copied() else {
            return;
        };
        let mut parts = route.parts.clone();
        match roads::append(
            &mut parts,
            &self.catalog,
            &item.path,
            self.reverse,
            start,
            self.angle.to_radians(),
        ) {
            Ok(()) => {
                self.checkpoint();
                self.doc.routes[i].parts = parts;
                self.rebuild();
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    fn clear_live(&mut self) {
        if let Some(job) = self.live_job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.live_candidate = None;
        self.live_confirmed = None;
        self.live_action = None;
        self.live_tick = Instant::now() - Duration::from_secs(1);
    }
    fn live_key(&self, points: &[Point]) -> Vec<u8> {
        serde_json::to_vec(&(
            points,
            &self.family,
            &self.doc.routing,
            &self.doc.forbidden,
            &self.doc.terrain_origin,
            &self.terrain_dir,
            &self.doc.models,
            self.live_epoch,
        ))
        .unwrap_or_default()
    }
    fn start_live(&mut self, points: Vec<Point>) {
        if self.live_job.is_some() {
            return;
        }
        let key = self.live_key(&points);
        let input = points.clone();
        let family = self.family.clone();
        let catalog = self.catalog.clone();
        let settings = self.doc.routing.clone();
        let forbidden = self.doc.forbidden.clone();
        let dir = self.terrain_dir.clone();
        let meta = self.terrain.as_ref().map(|t| t.meta.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Generated> {
                    let terrain = dir.map(|p| Terrain::open(&p)).transpose()?.map(|mut t| {
                        if let Some(m) = meta {
                            t.meta = m;
                        }
                        t
                    });
                    let parts = roads::fit(&input, &family, &catalog, &settings, |a, b| {
                        !flag.load(Ordering::Relaxed)
                            && terrain
                                .as_ref()
                                .map(|t| routing::permitted(a, b, t, &settings, &forbidden))
                                .unwrap_or_else(|| !routing::blocked_segment(a, b, &forbidden))
                    })?;
                    ensure!(!flag.load(Ordering::Relaxed), "Podgląd anulowany");
                    let mut shape = Shape::default();
                    for p in &parts {
                        let model = &catalog
                            .iter()
                            .find(|m| m.path == p.model)
                            .context("Brak modelu podglądu")?
                            .model;
                        shape.place(model, -p.rotation.to_radians(), p.position);
                    }
                    Ok(Generated { parts, shape })
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Błąd podglądu segmentów")))
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
        });
        self.live_job = Some(LiveJob {
            key,
            points,
            rx,
            cancel,
        });
        self.live_tick = Instant::now();
    }
    fn poll_live(&mut self) {
        let result = self.live_job.as_ref().and_then(|j| match j.rx.try_recv() {
            Ok(r) => Some(r),
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("Przerwano podgląd".into())),
            Err(_) => None,
        });
        let Some(result) = result else {
            return;
        };
        let job = self.live_job.take().unwrap();
        if job.cancel.load(Ordering::Relaxed)
            || self.tool != Tool::Live
            || job.key != self.live_key(&job.points)
        {
            return;
        }
        let prediction = Prediction {
            key: job.key,
            points: job.points,
            result,
        };
        if let Some((points, finish)) = &self.live_action
            && prediction.points == *points
        {
            let finish = *finish;
            self.live_action = None;
            match &prediction.result {
                Ok(g) => {
                    self.draft = prediction.points.clone();
                    self.live_confirmed = Some(prediction.clone());
                    if finish {
                        self.store_live(g.clone());
                        return;
                    }
                }
                Err(e) => self.status = e.clone(),
            }
        } else if prediction.points == self.draft && prediction.result.is_ok() {
            self.live_confirmed = Some(prediction.clone());
        }
        self.live_candidate = Some(prediction);
    }
    fn store_live(&mut self, g: Generated) {
        self.checkpoint();
        let i = self.doc.routes.len();
        self.doc.routes.push(Route {
            name: format!("Droga {}", i + 1),
            family: self.family.clone(),
            points: std::mem::take(&mut self.draft),
            parts: g.parts,
            replaces: None,
        });
        self.selected = Some(Selection::Route(i));
        self.clear_live();
        self.rebuild();
        self.status =
            "Zapisano trasę wygenerowaną podczas rysowania, z konec na obu końcach.".into();
    }
    fn request_live_finish(&mut self) {
        if let Some((_, finish)) = &mut self.live_action {
            *finish = true;
            return;
        }
        if self.draft.len() < 2 {
            self.status = "Zatwierdź co najmniej dwa punkty trasy".into();
            return;
        }
        let key = self.live_key(&self.draft);
        if let Some(p) = &self.live_confirmed
            && p.key == key
            && let Ok(g) = &p.result
        {
            self.store_live(g.clone());
            return;
        }
        self.queue_live(self.draft.clone(), true);
    }
    fn queue_live(&mut self, points: Vec<Point>, finish: bool) {
        let key = self.live_key(&points);
        if let Some(p) = &self.live_candidate
            && p.key == key
        {
            match &p.result {
                Ok(g) => {
                    let g = g.clone();
                    let p = p.clone();
                    self.draft = points;
                    self.live_confirmed = Some(p);
                    if finish {
                        self.store_live(g);
                    }
                    return;
                }
                Err(e) => {
                    self.status = e.clone();
                    return;
                }
            }
        }
        self.live_action = Some((points, finish));
        if let Some(j) = &self.live_job
            && j.key != key
        {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }
    fn live_points(&self, pos: Pos2, rect: Rect) -> Vec<Point> {
        let grid = (0.5 / self.scale).min(0.1);
        let p = self.world(pos, rect);
        let p = [(p[0] / grid).round() * grid, (p[1] / grid).round() * grid];
        let mut points = self.draft.clone();
        if points
            .last()
            .is_none_or(|q| geometry::norm(geometry::sub(*q, p)) > 0.01)
        {
            points.push(p);
        }
        points
    }
    fn draw_generated(&self, painter: &egui::Painter, g: &Generated, rect: Rect, color: Color32) {
        let mut mesh = egui::Mesh::default();
        for tri in &g.shape.triangles {
            let n = mesh.vertices.len() as u32;
            for p in tri {
                mesh.colored_vertex(self.screen(*p, rect), color);
            }
            mesh.add_triangle(n, n + 1, n + 2);
        }
        painter.add(egui::Shape::mesh(mesh));
        for line in &g.shape.lines {
            if line.len() > 1 {
                painter.add(egui::Shape::line(
                    line.iter().map(|p| self.screen(*p, rect)).collect(),
                    Stroke::new(1.5_f32, color),
                ));
            }
        }
    }
    fn update_live(&mut self, ui: &egui::Ui, rect: Rect, hover: Option<Pos2>) {
        let lang = self.language;
        let painter = ui.painter_at(rect);
        if let Some(p) = &self.live_confirmed
            && let Ok(g) = &p.result
        {
            self.draw_generated(&painter, g, rect, ROAD);
        }
        let points = self
            .live_action
            .as_ref()
            .map(|a| a.0.clone())
            .unwrap_or_else(|| {
                hover
                    .map(|pos| self.live_points(pos, rect))
                    .unwrap_or_else(|| self.draft.clone())
            });
        if points.len() < 2 {
            return;
        }
        let key = self.live_key(&points);
        if let Some(job) = &self.live_job
            && job.key != key
        {
            job.cancel.store(true, Ordering::Relaxed);
        }
        let ready = self.live_candidate.as_ref().is_some_and(|p| p.key == key);
        if !ready
            && self.live_job.is_none()
            && (self.live_action.is_some()
                || self.live_tick.elapsed() >= Duration::from_millis(120))
        {
            self.start_live(points.clone());
        }
        if ready {
            let p = self.live_candidate.as_ref().unwrap();
            match &p.result {
                Ok(g) => {
                    self.draw_generated(
                        &painter,
                        g,
                        rect,
                        Color32::from_rgba_unmultiplied(39, 148, 111, 145),
                    );
                    painter.text(
                        rect.left_top() + Vec2::new(16., 16.),
                        egui::Align2::LEFT_TOP,
                        lang.tr(&format!(
                            "Podgląd: {} segmentów · klik: zatwierdź · Enter: zakończ",
                            g.parts.len()
                        )),
                        egui::FontId::proportional(14.),
                        INK,
                    );
                }
                Err(e) => {
                    self.draw_points(&painter, &points, rect, Color32::LIGHT_RED, false);
                    painter.text(
                        rect.left_top() + Vec2::new(16., 16.),
                        egui::Align2::LEFT_TOP,
                        lang.tr(e),
                        egui::FontId::proportional(12.),
                        Color32::LIGHT_RED,
                    );
                }
            }
        } else {
            self.draw_points(
                &painter,
                &points,
                rect,
                Color32::from_rgb(110, 120, 125),
                false,
            );
            painter.text(
                rect.left_top() + Vec2::new(16., 16.),
                egui::Align2::LEFT_TOP,
                lang.tr("Obliczanie podglądu segmentów…"),
                egui::FontId::proportional(14.),
                INK,
            );
        }
        if self.live_job.is_some() || !ready {
            ui.ctx().request_repaint_after(Duration::from_millis(20));
        }
    }
    fn import_shp_dialog(&mut self, ctx: &egui::Context) {
        let lang = self.language;
        let mut launch = None;
        let mut close = false;
        if let Some(d) = &mut self.shp_dialog {
            egui::Window::new(lang.tr("Import dróg SHP")).collapsible(false).resizable(true).show(ctx,|ui|{
            ui.label(lang.tr(&(d.path.display().to_string())));ui.label(lang.tr("PolyLine / PolyLineZ / PolyLineM. Każda część staje się osobną trasą."));
            ui.horizontal(|ui| {
                if ui.button(lang.tr("Współrzędne świata")).clicked() { d.offset = [0., 0.]; }
                if ui.button(lang.tr("Lokalne współrzędne mapy")).clicked() { d.offset = [self.doc.map.east, self.doc.map.north]; }
            });
            ui.horizontal(|ui|{ui.label(lang.tr("Przesunięcie E"));ui.add(egui::DragValue::new(&mut d.offset[0]).speed(10.));ui.label(lang.tr("N"));ui.add(egui::DragValue::new(&mut d.offset[1]).speed(10.));});
            egui::ComboBox::from_label(lang.tr("Typ drogi dla importowanych tras")).selected_text(lang.tr(&d.family)).show_ui(ui,|ui|{for f in self.catalog.iter().map(|p|p.family.clone()).collect::<BTreeSet<_>>(){ui.selectable_value(&mut d.family,f.clone(),lang.tr(&(f)));}});
            ui.small(lang.tr("Dane muszą być w metrach i w układzie mapy. Brak automatycznej reprojekcji. Z/M oraz atrybuty DBF nie są importowane."));
            if let Some(prj)=&d.projection {ui.collapsing(lang.tr("Układ współrzędnych z PRJ"),|ui|{ui.label(lang.tr(prj));});}
            ui.horizontal(|ui| {
                if ui.add_enabled(!d.family.is_empty() && self.jobs.is_empty(), egui::Button::new(lang.tr("Importuj trasy"))).clicked() { launch = Some((d.path.clone(), d.offset, d.family.clone())); }
                if ui.button(lang.tr("Anuluj")).clicked() { close = true; }
            });
        });
        }
        if let Some((path, offset, family)) = launch {
            self.shp_dialog = None;
            self.clear_live();
            self.draft.clear();
            self.tool = Tool::Select;
            self.start("Import tras SHP", move |cancel| {
                let lines = dayz_road_tool::shapefile::read(&path, offset, &cancel)?;
                let stem = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                Ok(Loaded::Shp(lines, family, stem))
            });
        } else if close {
            self.shp_dialog = None;
        }
    }
    fn grade_asc(&mut self) {
        if !self.draft.is_empty() {
            self.status = "Zakończ rysowanie przed modyfikacją ASC".into();
            return;
        }
        let Some(Selection::Route(i)) = self.selected else {
            self.status = "Wybierz trasę projektu z wygenerowanymi partami".into();
            return;
        };
        let parts = self.doc.routes[i].parts.clone();
        if parts.is_empty() {
            self.status = "Najpierw dopasuj modele do wybranej trasy".into();
            return;
        }
        let (Some(dir), Some(meta)) = (
            self.terrain_dir.clone(),
            self.terrain.as_ref().map(|t| t.meta.clone()),
        ) else {
            self.status = "Wczytaj wysokości ASC".into();
            return;
        };
        let catalog = self.catalog.clone();
        self.clear_live();
        let blend_width = self.doc.grading_blend_width;
        self.start("Modyfikacja ASC pod partami", move |cancel| {
            let mut terrain = Terrain::open(&dir)?;
            terrain.meta = meta;
            let output = Self::cache_dir("asc-graded")?;
            let (t, count) =
                dayz_road_tool::grading::apply_with_blend(&terrain, &parts, &catalog, &output, &cancel, blend_width)?;
            Ok(Loaded::Graded(output, t, count))
        });
    }
    fn export_asc(&mut self) {
        let lang = self.language;
        let (Some(dir), Some(meta)) = (
            self.terrain_dir.clone(),
            self.terrain.as_ref().map(|t| t.meta.clone()),
        ) else {
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter(lang.tr("ESRI ASCII Grid"), &["asc"])
            .set_file_name("terrain-roads.asc")
            .save_file()
        else {
            return;
        };
        if self
            .doc
            .terrain
            .as_ref()
            .is_some_and(|p| tv4p::same_path(p, &path))
        {
            self.status = "Zapisz zmodyfikowany ASC pod inną nazwą niż źródłowy".into();
            return;
        }
        self.start("Eksport ASC", move |cancel| {
            let mut terrain = Terrain::open(&dir)?;
            terrain.meta = meta;
            dayz_road_tool::grading::export(&terrain, &path, &cancel)?;
            Ok(Loaded::AscExport(path))
        });
    }
    fn profile(&self, ui: &mut egui::Ui) {
        let lang = self.language;
        ui.heading(lang.tr("Profil wysokościowy"));
        let Some(t) = &self.terrain else {
            ui.small(lang.tr("Wczytaj ASC, aby sprawdzić wysokości."));
            return;
        };
        let Some(selection) = self.selected else {
            ui.small(lang.tr("Wybierz drogę."));
            return;
        };
        let lines = match selection {
            Selection::Route(i) => {
                let r = &self.doc.routes[i];
                if r.parts.is_empty() {
                    vec![r.points.clone()]
                } else {
                    vec![roads::centerline(&r.parts, &self.catalog)]
                }
            }
            Selection::Existing(_) => self
                .previews
                .iter()
                .find(|p| p.id == selection)
                .map(|p| p.shape.lines.clone())
                .unwrap_or_default(),
        };
        let mut samples = Vec::new();
        let mut distance = 0.;
        let mut max_grade: f64 = 0.;
        let mut missing = false;
        for line in lines {
            let mut previous: Option<(f64, f64)> = None;
            for w in line.windows(2) {
                let d = geometry::norm(geometry::sub(w[1], w[0]));
                let steps = (d / (t.meta.cell / 2.).clamp(0.1, 5.)).ceil().max(1.) as usize;
                for i in 0..=steps {
                    let f = i as f64 / steps as f64;
                    let p = geometry::add(w[0], geometry::mul(geometry::sub(w[1], w[0]), f));
                    let along = distance + d * f;
                    if let Some(z) = t.sample(p) {
                        if let Some((old_x, old_z)) = previous
                            && along > old_x
                        {
                            max_grade = max_grade.max((z - old_z).abs() / (along - old_x) * 100.);
                        }
                        samples.push((along, z));
                        previous = Some((along, z));
                    } else {
                        missing = true;
                        previous = None;
                    }
                }
                distance += d;
            }
        }
        ui.label(lang.tr(&(format!("{distance:.0} m · maks. spadek {max_grade:.1}%"))));
        if missing {
            ui.colored_label(
                Color32::LIGHT_RED,
                lang.tr("Brak wysokości na części trasy"),
            );
        }
        if samples.len() < 2 {
            return;
        }
        let min = samples.iter().map(|s| s.1).fold(f64::INFINITY, f64::min);
        let max = samples
            .iter()
            .map(|s| s.1)
            .fold(f64::NEG_INFINITY, f64::max);
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 100.), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2., Color32::from_rgb(15, 23, 37));
        painter.add(egui::Shape::line(
            samples
                .iter()
                .map(|(x, z)| {
                    Pos2::new(
                        rect.left() + (*x / distance.max(1.)) as f32 * rect.width(),
                        rect.bottom() - ((*z - min) / (max - min).max(1.)) as f32 * rect.height(),
                    )
                })
                .collect(),
            Stroke::new(1.5_f32, SELECT),
        ));
        ui.small(lang.tr(&(format!("{min:.1} — {max:.1} m n.p.m."))));
    }
    fn map(&mut self, ui: &mut egui::Ui) {
        let lang = self.language;
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0., Color32::from_rgb(15, 23, 37));
        if self.raster.is_none()
            && self.base.is_none()
            && self.doc.routes.is_empty()
            && self.draft.is_empty()
        {
            painter.text(
                rect.center() - Vec2::new(0., 28.),
                egui::Align2::CENTER_CENTER,
                lang.tr("Wytycz pierwszą drogę"),
                egui::FontId::proportional(25.),
                INK,
            );
            painter.text(
                rect.center() + Vec2::new(0., 10.),
                egui::Align2::CENTER_CENTER,
                lang.tr("Otwórz TV4P i satelitę, wybierz typ drogi i klikaj punkty na mapie."),
                egui::FontId::proportional(14.),
                INK,
            );
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.
                && let Some(pos) = response.hover_pos()
            {
                let before = self.world(pos, rect);
                self.scale = (self.scale * (scroll as f64 * 0.002).exp()).clamp(0.0001, 100.);
                let after = self.world(pos, rect);
                self.center = geometry::add(self.center, geometry::sub(before, after));
            }
        }
        if response.dragged_by(egui::PointerButton::Middle)
            || response.dragged_by(egui::PointerButton::Secondary)
        {
            let d = ui.input(|i| i.pointer.delta());
            self.center[0] -= d.x as f64 / self.scale;
            self.center[1] += d.y as f64 / self.scale;
        }
        let sw = self.world(rect.left_bottom(), rect);
        let ne = self.world(rect.right_top(), rect);
        let bounds = [sw[0], sw[1], ne[0], ne[1]];
        if self.show_sat {
            self.draw_satellite(ui, rect, bounds);
        }
        if self.contours
            && let Some(t) = &self.terrain
        {
            let key = [
                bounds[0].to_bits(),
                bounds[1].to_bits(),
                bounds[2].to_bits(),
                bounds[3].to_bits(),
                self.interval.to_bits(),
            ];
            if self.contour_key != Some(key) {
                self.contour_cache = t.contours(bounds, self.interval);
                self.contour_key = Some(key);
            }
            for (a, b) in &self.contour_cache {
                painter.line_segment(
                    [self.screen(*a, rect), self.screen(*b, rect)],
                    Stroke::new(0.7_f32, Color32::from_rgba_unmultiplied(120, 150, 175, 125)),
                );
            }
        }
        for poly in &self.doc.forbidden {
            let pts: Vec<_> = poly.iter().map(|p| self.screen(*p, rect)).collect();
            if pts.len() > 2 {
                painter.add(egui::Shape::closed_line(
                    pts,
                    Stroke::new(2.0_f32, Color32::from_rgb(181, 77, 60)),
                ));
            }
        }
        let mut visible = BTreeSet::new();
        let bins = ((bounds[2] - bounds[0]) / 512. + 2.) * ((bounds[3] - bounds[1]) / 512. + 2.);
        if bins > 20000. {
            visible.extend(0..self.previews.len());
        } else {
            for x in (bounds[0] / 512.).floor() as i32..=(bounds[2] / 512.).floor() as i32 {
                for y in (bounds[1] / 512.).floor() as i32..=(bounds[3] / 512.).floor() as i32 {
                    if let Some(items) = self.index.get(&(x, y)) {
                        visible.extend(items.iter().copied());
                    }
                }
            }
            if let Some(items) = self.index.get(&(i32::MIN, i32::MIN)) {
                visible.extend(items.iter().copied());
            }
        }
        if self.show_roads {
            for i in &visible {
                let p = &self.previews[*i];
                if p.bounds[2] < bounds[0]
                    || p.bounds[0] > bounds[2]
                    || p.bounds[3] < bounds[1]
                    || p.bounds[1] > bounds[3]
                {
                    continue;
                }
                let color = if self.selected == Some(p.id) {
                    SELECT
                } else {
                    self.doc
                        .road_colors
                        .get(&p.family)
                        .map(|rgb| Color32::from_rgb(rgb[0], rgb[1], rgb[2]))
                        .unwrap_or(ROAD)
                };
                let mut mesh = egui::Mesh::default();
                for tri in &p.shape.triangles {
                    let n = mesh.vertices.len() as u32;
                    for v in tri {
                        mesh.colored_vertex(self.screen(*v, rect), color);
                    }
                    mesh.add_triangle(n, n + 1, n + 2);
                }
                painter.add(egui::Shape::mesh(mesh));
                for line in &p.shape.lines {
                    if line.len() > 1 {
                        painter.add(egui::Shape::line(
                            line.iter().map(|p| self.screen(*p, rect)).collect(),
                            Stroke::new(1.5_f32, color),
                        ));
                    }
                }
            }
        }
        for (i, r) in self.doc.routes.iter().enumerate() {
            if r.parts.is_empty() || self.selected == Some(Selection::Route(i)) {
                self.draw_points(
                    &painter,
                    &r.points,
                    rect,
                    SELECT,
                    self.selected == Some(Selection::Route(i)),
                );
            }
        }
        self.draw_points(
            &painter,
            &self.draft,
            rect,
            Color32::from_rgb(63, 110, 168),
            true,
        );
        if self.tool == Tool::Live && self.jobs.is_empty() && self.shp_dialog.is_none() {
            self.update_live(ui, rect, response.hover_pos());
        }
        let busy = !self.jobs.is_empty() || self.shp_dialog.is_some();
        if !busy {
            let editing_click = self.tool == Tool::Edit
                && (response.double_clicked() || response.double_clicked_by(egui::PointerButton::Secondary));
            if editing_click
                && let Some(Selection::Route(index)) = self.selected
                && let Some(pos) = response.interact_pointer_pos()
            {
                let points = &self.doc.routes[index].points;
                if response.double_clicked_by(egui::PointerButton::Secondary) {
                    if let Some(point) = points.iter().position(|p| self.screen(*p, rect).distance(pos) < 12.) {
                        self.remove_route_point(index, point);
                    }
                } else if let Some((segment, _)) = points.windows(2).enumerate()
                    .map(|(i, w)| (i, distance_to_segment(pos, self.screen(w[0], rect), self.screen(w[1], rect))))
                    .filter(|(_, d)| *d < 12.)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                {
                    if !points.iter().any(|p| self.screen(*p, rect).distance(pos) < 8.) {
                        let world = self.world(pos, rect);
                        self.insert_route_point(index, segment, world);
                    }
                }
            }
            if response.clicked()
                && !editing_click
                && let Some(pos) = response.interact_pointer_pos()
            {
                let world = self.world(pos, rect);
                match self.tool {
                    Tool::Draw | Tool::Forbidden => self.draft.push(world),
                    Tool::Live => {
                        if self.live_action.is_none() {
                            let points = self.live_points(pos, rect);
                            if self.draft.is_empty() {
                                self.draft = points;
                            } else if points.len() > self.draft.len() {
                                self.queue_live(points, false);
                            }
                        }
                    }
                    Tool::Segments => {
                        self.checkpoint();
                        let i = self.doc.routes.len();
                        self.doc.routes.push(Route {
                            name: format!("Droga {}", i + 1),
                            family: self.family.clone(),
                            points: vec![world],
                            parts: vec![],
                            replaces: None,
                        });
                        self.selected = Some(Selection::Route(i));
                        self.add_segment();
                    }
                    Tool::Select | Tool::Edit => {
                        let mut best = 10.;
                        let mut selected = None;
                        for i in &visible {
                            let p = &self.previews[*i];
                            for line in &p.shape.lines {
                                for w in line.windows(2) {
                                    let d = distance_to_segment(
                                        pos,
                                        self.screen(w[0], rect),
                                        self.screen(w[1], rect),
                                    );
                                    if d < best {
                                        best = d;
                                        selected = Some(p.id);
                                    }
                                }
                            }
                        }
                        for (i, r) in self.doc.routes.iter().enumerate() {
                            for w in r.points.windows(2) {
                                let d = distance_to_segment(pos, self.screen(w[0], rect), self.screen(w[1], rect));
                                if d < best {
                                    best = d;
                                    selected = Some(Selection::Route(i));
                                }
                            }
                            for p in &r.points {
                                if self.screen(*p, rect).distance(pos) < best {
                                    selected = Some(Selection::Route(i));
                                    best = self.screen(*p, rect).distance(pos);
                                }
                            }
                        }
                        self.selected = selected;
                    }
                }
            }
            if response.drag_started_by(egui::PointerButton::Primary) && matches!(self.tool, Tool::Select | Tool::Edit) {
                self.drag_snapshot = false;
                self.drag_point = None;
                if let Some(Selection::Route(i)) = self.selected
                    && let Some(pos) = response.interact_pointer_pos()
                {
                    self.drag_point = self.doc.routes[i]
                        .points
                        .iter()
                        .position(|p| self.screen(*p, rect).distance(pos) < 12.);
                }
            }
            if response.dragged_by(egui::PointerButton::Primary)
                && matches!(self.tool, Tool::Select | Tool::Edit)
                && let Some(selection) = self.selected
            {
                if !self.drag_snapshot {
                    self.checkpoint();
                    self.drag_snapshot = true;
                }
                let delta = ui.input(|i| i.pointer.delta());
                let d = [delta.x as f64 / self.scale, -delta.y as f64 / self.scale];
                match selection {
                    Selection::Existing(id) => {
                        if let Some(t) = self.doc.transforms.iter_mut().find(|t| t.0 == id) {
                            t.1 = geometry::add(t.1, d);
                        } else {
                            self.doc.transforms.push((id, d, 0.));
                        }
                    }
                    Selection::Route(i) => {
                        let r = &mut self.doc.routes[i];
                        if let Some(p) = self.drag_point {
                            r.points[p] = geometry::add(r.points[p], d);
                            r.parts.clear();
                        } else {
                            for p in &mut r.points {
                                *p = geometry::add(*p, d);
                            }
                            for p in &mut r.parts {
                                p.position = geometry::add(p.position, d);
                            }
                        }
                    }
                }
                self.rebuild();
            }
        }
        if let Some(pos) = response.hover_pos() {
            let p = self.world(pos, rect);
            let h = self.terrain.as_ref().and_then(|t| t.sample(p));
            let label = lang.tr(&format!(
                "E {:.2}   N {:.2}{}",
                p[0],
                p[1],
                h.map(|h| format!("   H {h:.2} m")).unwrap_or_default()
            ));
            painter.text(
                rect.left_bottom() + Vec2::new(16., -16.),
                egui::Align2::LEFT_BOTTOM,
                label,
                egui::FontId::monospace(13.),
                INK,
            );
        }
        let metres = 10f64.powf((100. / self.scale).log10().floor());
        let a = rect.right_bottom() + Vec2::new(-20., -32.);
        let b = a - Vec2::new((metres * self.scale) as f32, 0.);
        painter.line_segment([a, b], Stroke::new(2.0_f32, INK));
        painter.text(
            b - Vec2::new(0., 5.),
            egui::Align2::LEFT_BOTTOM,
            format!("{metres:.0} m"),
            egui::FontId::monospace(12.),
            INK,
        );
    }
    fn world(&self, p: Pos2, r: Rect) -> Point {
        [
            self.center[0] + (p.x - r.center().x) as f64 / self.scale,
            self.center[1] - (p.y - r.center().y) as f64 / self.scale,
        ]
    }
    fn screen(&self, p: Point, r: Rect) -> Pos2 {
        Pos2::new(
            r.center().x + ((p[0] - self.center[0]) * self.scale) as f32,
            r.center().y - ((p[1] - self.center[1]) * self.scale) as f32,
        )
    }
    fn draw_points(
        &self,
        p: &egui::Painter,
        line: &[Point],
        rect: Rect,
        color: Color32,
        handles: bool,
    ) {
        if line.len() > 1 {
            p.add(egui::Shape::line(
                line.iter().map(|p| self.screen(*p, rect)).collect(),
                Stroke::new(2.0_f32, color),
            ));
        }
        if handles {
            for (i, point) in line.iter().enumerate() {
                let q = self.screen(*point, rect);
                p.circle_filled(q, 4., Color32::WHITE);
                p.circle_stroke(q, 5., Stroke::new(2.0_f32, color));
                p.text(
                    q + Vec2::new(8., -8.),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{}", i + 1),
                    egui::FontId::monospace(11.),
                    INK,
                );
            }
        }
    }
    fn draw_satellite(&mut self, ui: &egui::Ui, rect: Rect, bounds: [f64; 4]) {
        let Some(r) = &self.raster else {
            return;
        };
        let m = &self.doc.map;
        if !m.valid() {
            return;
        }
        let ratio = (r.levels[0].width as f64 / (m.width * self.scale))
            .max(r.levels[0].height as f64 / (m.height * self.scale));
        let level = (ratio.log2().floor().max(0.) as usize).min(r.levels.len() - 1);
        let l = &r.levels[level];
        let x0 =
            (((bounds[0] - m.east) / m.width * l.width as f64).max(0.) as u32).min(l.width) / 256;
        let x1 = (((bounds[2] - m.east) / m.width * l.width as f64).max(0.) as u32)
            .min(l.width - 1)
            / 256;
        let y0 = (((m.north + m.height - bounds[3]) / m.height * l.height as f64).max(0.) as u32)
            .min(l.height)
            / 256;
        let y1 = (((m.north + m.height - bounds[1]) / m.height * l.height as f64).max(0.) as u32)
            .min(l.height - 1)
            / 256;
        let mut loaded = 0;
        for x in x0..=x1 {
            for y in y0..=y1 {
                let key = (level, x, y);
                if let std::collections::hash_map::Entry::Vacant(entry) = self.tiles.entry(key) {
                    if loaded >= 2 {
                        ui.ctx().request_repaint();
                        continue;
                    }
                    match r.tile(level, x * 256, y * 256) {
                        Ok((w, h, data)) => {
                            let texture = ui.ctx().load_texture(
                                format!("sat-{level}-{x}-{y}"),
                                egui::ColorImage::from_rgba_unmultiplied(
                                    [w as usize, h as usize],
                                    &data,
                                ),
                                egui::TextureOptions::LINEAR,
                            );
                            entry.insert(texture);
                            self.tile_age.push(key);
                            loaded += 1;
                        }
                        Err(_) => continue,
                    }
                }
                let tex = &self.tiles[&key];
                let a = [
                    m.east + x as f64 * 256. / l.width as f64 * m.width,
                    m.north + m.height - y as f64 * 256. / l.height as f64 * m.height,
                ];
                let b = [
                    a[0] + tex.size()[0] as f64 / l.width as f64 * m.width,
                    a[1] - tex.size()[1] as f64 / l.height as f64 * m.height,
                ];
                ui.painter_at(rect).image(
                    tex.id(),
                    Rect::from_two_pos(self.screen(a, rect), self.screen(b, rect)),
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                    Color32::from_white_alpha((self.opacity * 255.) as u8),
                );
            }
        }
        while self.tiles.len() > 256 {
            if !self.tile_age.is_empty() {
                let k = self.tile_age.remove(0);
                self.tiles.remove(&k);
            } else {
                break;
            }
        }
    }
}
impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for App {
    fn drop(&mut self) {
        for job in &self.jobs {
            job.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(job) = &self.live_job {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl App {
    pub fn show(&mut self, ctx: &egui::Context) {
        if !ctx.wants_keyboard_input() && self.jobs.is_empty() && self.shp_dialog.is_none() {
            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.finish();
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.draft.clear();
                self.clear_live();
            }
            if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Z)) {
                self.undo(false);
            }
            if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Y)) {
                self.undo(true);
            }
        }
        self.controls(ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            self.map(ui);
        });
        self.import_shp_dialog(ctx);
    }
}
fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let d = b - a;
    let t = ((p - a).dot(d) / d.length_sq().max(1e-6)).clamp(0., 1.);
    p.distance(a + d * t)
}

#[cfg(test)]
mod tests {
    #[test]
    fn editing_line_points_preserves_route_and_supports_undo_redo() {
        let mut app = super::App::new();
        app.doc.routes.push(dayz_road_tool::document::Route {
            name: "Existing line".into(), family: "asf2".into(),
            points: vec![[0., 0.], [100., 0.]],
            parts: vec![dayz_road_tool::roads::PlacedPart {
                model: "asf2_25.p3d".into(), reverse: false,
                position: [0., 0.], rotation: 0.,
            }], replaces: Some(42),
        });
        app.insert_route_point(0, 0, [50., 10.]);
        assert_eq!(app.doc.routes[0].points, vec![[0., 0.], [50., 10.], [100., 0.]]);
        assert!(app.doc.routes[0].parts.is_empty());
        assert_eq!(app.doc.routes[0].replaces, Some(42));
        assert!(app.dirty);
        app.undo(false);
        assert_eq!(app.doc.routes[0].points.len(), 2);
        assert_eq!(app.doc.routes[0].parts.len(), 1);
        app.undo(true);
        assert_eq!(app.doc.routes[0].points.len(), 3);
        app.remove_route_point(0, 1);
        assert_eq!(app.doc.routes[0].points, vec![[0., 0.], [100., 0.]]);
        let undo_count = app.undo.len();
        app.remove_route_point(0, 0);
        assert_eq!(app.undo.len(), undo_count);
        let saved = serde_json::to_vec(&app.doc).unwrap();
        let restored: dayz_road_tool::document::Document = serde_json::from_slice(&saved).unwrap();
        assert_eq!(restored.routes[0].points, app.doc.routes[0].points);
        assert_eq!(restored.routes[0].replaces, Some(42));
    }
    use super::*;

    #[test]
    fn png_scope_tracks_transformed_deleted_and_replaced_existing_roads() {
        let mut app = App::new();
        let start = [200000., 0.];
        app.base = Some(tv4p::Project {
            path: "base.tv4p".into(),
            roads: vec![tv4p::Road {
                index: 0,
                id: 9,
                model: "asf2_25.p3d".into(),
                models: vec![],
                start,
                rotation_degrees: 0.,
                parts: 1,
                is_new: false,
                shape: Shape {
                    triangles: vec![[start, [200010., 0.], [200000., 10.]]],
                    ..Default::default()
                },
            }],
        });
        app.doc.transforms.push((9, [25., 50.], 90.));
        app.rebuild();
        assert_eq!(app.png_previews().len(), 1);
        assert_eq!(app.png_previews()[0].shape.triangles[0][0], [200025., 50.]);
        assert_eq!(app.png_previews()[0].shape.triangles[0][1], [200025., 40.]);
        app.doc.deleted.push(9);
        app.rebuild();
        assert!(app.png_previews().is_empty());
        app.doc.deleted.clear();
        app.doc.routes.push(Route {
            name: "replacement".into(),
            family: "asf2".into(),
            points: vec![start],
            parts: vec![],
            replaces: Some(9),
        });
        app.rebuild();
        assert!(app.png_previews().is_empty());
        let protected = std::env::temp_dir().join("builder-protected-source.png");
        app.file = Some(protected.clone());
        assert!(
            app.start_png(protected)
                .unwrap_err()
                .to_string()
                .contains("plik źródłowy")
        );
    }

    #[test]
    fn png_exports_selected_and_all_roads_with_map_coordinates_and_alpha() {
        let dir = std::env::temp_dir().join(format!(
            "builder-png-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new();
        app.doc.map.width = 128.;
        app.doc.map.height = 128.;
        app.png_settings.width = 128;
        app.png_settings.height = 128;
        for (id, x, y) in [
            (Selection::Existing(7), 200016., 96.),
            (Selection::Route(0), 200080., 16.),
        ] {
            app.push_preview(
                id,
                Shape {
                    triangles: vec![[[x, y], [x + 16., y], [x, y + 16.]]],
                    ..Default::default()
                },
                "asf2".into(),
            );
        }
        app.selected = Some(Selection::Existing(7));
        app.png_settings.selected_only = true;
        let selected_path = dir.join("selected.png");
        app.start_png(selected_path.clone()).unwrap();
        let wait = |app: &mut App| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while app.png_job.is_some() {
                assert!(Instant::now() < deadline, "PNG task did not finish");
                app.tick(&egui::Context::default());
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(app.status.starts_with("PNG:"), "{}", app.status);
        };
        wait(&mut app);
        let image = image::open(selected_path).unwrap().into_rgba8();
        assert_eq!(image.dimensions(), (128, 128));
        assert_eq!(image.get_pixel(20, 24).0, [255, 216, 70, 255]);
        assert_eq!(image.get_pixel(84, 104).0, [0, 0, 0, 0]);
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 0]);
        app.doc.road_colors.insert("asf2".into(), [17, 123, 222]);
        let custom_path = dir.join("custom-selected.png");
        app.start_png(custom_path.clone()).unwrap();
        // A running export must keep its own snapshot of the palette.
        app.doc.road_colors.insert("asf2".into(), [0, 0, 0]);
        wait(&mut app);
        let custom_image = image::open(custom_path).unwrap().into_rgba8();
        assert_eq!(custom_image.get_pixel(20, 24).0, [17, 123, 222, 255]);
        assert_eq!(custom_image.get_pixel(84, 104).0, [0, 0, 0, 0]);
        app.doc.road_colors.clear();
        app.selected = None;
        app.png_settings.selected_only = false;
        app.png_settings.transparent = false;
        let all_path = dir.join("all.png");
        app.start_png(all_path.clone()).unwrap();
        wait(&mut app);
        let image = image::open(all_path).unwrap().into_rgba8();
        assert_eq!(image.get_pixel(20, 24).0, [92, 182, 236, 255]);
        assert_eq!(image.get_pixel(84, 104).0, [92, 182, 236, 255]);
        assert_eq!(image.get_pixel(0, 0).0, [15, 23, 37, 255]);
        app.png_settings.selected_only = true;
        assert!(app.start_png(dir.join("empty.png")).is_err());
        app.png_settings.selected_only = false;
        app.png_settings.width = 20481;
        app.png_settings.height = 20480;
        assert!(app.start_png(dir.join("too-large.png")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn inactive_tick_applies_background_import_and_preserves_undo() {
        let mut app = App::new();
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Loaded::Shp(
            vec![dayz_road_tool::shapefile::Line {
                record: 1,
                part: 0,
                points: vec![[200000., 0.], [200025., 0.]],
            }],
            "asf2".into(),
            "roads".into(),
        )))
        .unwrap();
        app.jobs.push(Job {
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            label: "Import tras SHP".into(),
        });
        app.tick(&egui::Context::default());
        assert!(app.jobs.is_empty());
        assert_eq!(app.doc.routes.len(), 1);
        assert!(app.has_unsaved_changes());
        app.undo(false);
        assert!(app.doc.routes.is_empty());
        app.undo(true);
        assert_eq!(app.doc.routes.len(), 1);
    }

    #[test]
    fn close_guard_protects_drafts_and_can_cancel_or_accept() {
        let mut app = App::new();
        assert!(!app.has_unsaved_changes());
        app.draft.push([200000., 0.]);
        assert!(app.has_unsaved_changes());
        let ctx = egui::Context::default();
        for confirm in [false, true] {
            let mut input = egui::RawInput::default();
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .events
                .push(egui::ViewportEvent::Close);
            let output = ctx.run(input, |ctx| app.guard_close(ctx, || confirm));
            let cancelled = output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::CancelClose));
            assert_eq!(cancelled, !confirm);
            assert_eq!(app.draft.len(), 1);
        }
    }
    fn live_app() -> App {
        let mut app = App::initial();
        app.tool = Tool::Live;
        app.family = "asf2".into();
        app.draft = vec![[200000., 0.]];
        for (path, category) in [("asf2_25.p3d", 3), ("asf2_6konec.p3d", 6)] {
            app.catalog.push(CatalogPart {
                path: path.into(),
                family: "asf2".into(),
                category,
                index: 0,
                road_type_index: 0,
                model: geometry::filename_model(path).unwrap(),
            });
        }
        app
    }
    fn deliver(app: &mut App, points: Vec<Point>) {
        let parts = roads::fit(
            &points,
            &app.family,
            &app.catalog,
            &app.doc.routing,
            |_, _| true,
        )
        .unwrap();
        let mut shape = Shape::default();
        for p in &parts {
            shape.place(
                &app.catalog
                    .iter()
                    .find(|c| c.path == p.model)
                    .unwrap()
                    .model,
                -p.rotation.to_radians(),
                p.position,
            );
        }
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Generated { parts, shape })).unwrap();
        app.live_job = Some(LiveJob {
            key: app.live_key(&points),
            points,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
        });
    }
    #[test]
    fn live_click_commits_generated_caps_and_finish_is_undoable() {
        let mut app = live_app();
        let points = vec![[200000., 0.], [200000., 112.5]];
        app.queue_live(points.clone(), false);
        deliver(&mut app, points.clone());
        app.poll_live();
        assert_eq!(app.draft, points);
        assert!(app.doc.routes.is_empty());
        assert!(
            app.live_confirmed
                .as_ref()
                .unwrap()
                .result
                .as_ref()
                .unwrap()
                .shape
                .lines
                .len()
                >= 2
        );
        app.request_live_finish();
        assert_eq!(app.doc.routes.len(), 1);
        assert_eq!(
            app.doc.routes[0].parts.first().unwrap().model,
            "asf2_6konec.p3d"
        );
        assert_eq!(
            app.doc.routes[0].parts.last().unwrap().model,
            "asf2_6konec.p3d"
        );
        assert!(app.draft.is_empty());
        app.undo(false);
        assert!(app.doc.routes.is_empty());
        app.undo(true);
        assert_eq!(app.doc.routes.len(), 1);
    }
    #[test]
    fn live_pending_finish_and_stale_settings_cannot_commit_wrong_preview() {
        let mut app = live_app();
        let points = vec![[200000., 0.], [200000., 112.5]];
        app.queue_live(points.clone(), false);
        app.request_live_finish();
        deliver(&mut app, points.clone());
        app.doc.routing.tolerance += 1.;
        app.poll_live();
        assert!(app.doc.routes.is_empty());
        assert_eq!(app.draft.len(), 1);
        assert!(app.live_action.as_ref().unwrap().1);
        deliver(&mut app, points);
        app.poll_live();
        assert_eq!(app.doc.routes.len(), 1);
        assert!(app.live_action.is_none());
    }

    #[test]
    fn shp_import_adds_parts_as_routes_and_undoes_as_one_operation() {
        let mut app = live_app();
        app.tool = Tool::Select;
        app.draft.clear();
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Loaded::Shp(
            vec![
                dayz_road_tool::shapefile::Line {
                    record: 1,
                    part: 0,
                    points: vec![[200000., 0.], [200100., 0.]],
                },
                dayz_road_tool::shapefile::Line {
                    record: 1,
                    part: 1,
                    points: vec![[200010., 20.], [200010., 60.]],
                },
            ],
            "asf2".into(),
            "roads".into(),
        )))
        .unwrap();
        app.jobs.push(Job {
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            label: "SHP".into(),
        });
        app.poll();
        assert_eq!(app.doc.routes.len(), 2);
        assert_eq!(app.doc.routes[1].name, "roads #1.2");
        assert_eq!(app.doc.routes[1].family, "asf2");
        assert!(app.doc.routes.iter().all(|r| r.parts.is_empty()));
        assert_eq!(app.center, [200050., 30.]);
        app.undo(false);
        assert!(app.doc.routes.is_empty());
        app.undo(true);
        assert_eq!(app.doc.routes.len(), 2);
    }

    #[test]
    fn graded_terrain_undo_redo_restores_cache_and_saved_project() {
        let root = std::env::temp_dir().join(format!("road-grade-ui-{}", std::process::id()));
        let old = root.join("old");
        let new = root.join("new");
        for (dir, z) in [(&old, 10f32), (&new, 20f32)] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(
                dir.join("terrain.json"),
                r#"{"cols":2,"rows":2,"east":100.0,"north":200.0,"cell":1.0}"#,
            )
            .unwrap();
            std::fs::write(dir.join("height.f32"), z.to_le_bytes().repeat(4)).unwrap();
        }
        let mut app = App::initial();
        app.tool = Tool::Select;
        app.doc.terrain_cache = Some(old.clone());
        app.doc.terrain_origin = Some([100., 200.]);
        app.terrain_dir = Some(old.clone());
        app.terrain = Some(Terrain::open(&old).unwrap());
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Loaded::Graded(
            new.clone(),
            Terrain::open(&new).unwrap(),
            4,
        )))
        .unwrap();
        app.jobs.push(Job {
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            label: "ASC".into(),
        });
        app.poll();
        assert_eq!(app.terrain.as_ref().unwrap().value(0, 0), Some(20.));
        app.undo(false);
        assert_eq!(app.doc.terrain_cache, Some(old));
        assert_eq!(app.terrain.as_ref().unwrap().value(0, 0), Some(10.));
        assert_eq!(app.terrain.as_ref().unwrap().point(0, 0), [100.5, 201.5]);
        app.undo(true);
        assert_eq!(app.doc.terrain_cache, Some(new.clone()));
        assert_eq!(app.terrain.as_ref().unwrap().value(0, 0), Some(20.));
        let saved = root.join("test.dzroad");
        app.doc.save(&saved).unwrap();
        let loaded: Document = serde_json::from_slice(&std::fs::read(saved).unwrap()).unwrap();
        assert_eq!(loaded.terrain_cache, Some(new));
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }
}
