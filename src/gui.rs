use crate::{
    geometry::Library,
    i18n::Language,
    render::{self, Bounds, View},
    tv4p::{self, Project, Road},
};
use anyhow::Result;
use eframe::egui::{self, Color32, Pos2, Sense, Stroke, Vec2};
use rfd::FileDialog;
use std::{collections::BTreeSet, path::PathBuf, sync::mpsc, time::Duration};
#[derive(Default)]
struct Dataset {
    project: Option<Project>,
    selected: BTreeSet<usize>,
    search: String,
    kind: String,
    min_length: String,
    max_length: String,
    only_selected: bool,
    only_new: bool,
    merged_result: bool,
    zoom: f64,
    pan: Vec2,
    focus: Option<Bounds>,
}
impl Dataset {
    fn accepts(&self, r: &Road) -> bool {
        let text = self.search.trim().to_lowercase();
        (text.is_empty()
            || r.id.to_string().contains(&text)
            || r.models.iter().any(|m| m.to_lowercase().contains(&text)))
            && (self.kind.is_empty() || road_kind(r) == self.kind)
            && number(&self.min_length)
                .map(|m| r.shape.length >= m)
                .unwrap_or(true)
            && number(&self.max_length)
                .map(|m| r.shape.length <= m)
                .unwrap_or(true)
            && (!self.only_selected || self.selected.contains(&r.index))
            && (!self.only_new || r.is_new)
    }
    fn reset_view(&mut self) {
        self.zoom = 1.;
        self.pan = Vec2::ZERO;
        self.focus = None;
    }
}
fn number(text: &str) -> Option<f64> {
    text.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
}
fn road_kind(r: &Road) -> String {
    r.model
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(&r.model)
        .split('_')
        .next()
        .unwrap_or("")
        .to_owned()
}
struct App {
    language: Language,
    datasets: [Dataset; 3],
    active: usize,
    output: String,
    model_root: String,
    library: Library,
    message: String,
    error: bool,
    png_selected: bool,
    transparent: bool,
    png_width: u32,
    png_height: u32,
    full_map: bool,
    map_east: f64,
    map_north: f64,
    map_width: f64,
    map_height: f64,
    png_job: Option<mpsc::Receiver<Result<String>>>,
}
impl Default for App {
    fn default() -> Self {
        Self {
            language: Language::Polish,
            datasets: std::array::from_fn(|_| Dataset {
                zoom: 1.,
                ..Default::default()
            }),
            active: 0,
            output: "output.tv4p".into(),
            model_root: crate::DEFAULT_MODELS.into(),
            library: Library::new(PathBuf::from(crate::DEFAULT_MODELS)),
            message: "Wczytaj projekt A lub B.".into(),
            error: false,
            png_selected: true,
            transparent: true,
            png_width: 15360,
            png_height: 15360,
            full_map: true,
            map_east: 200000.,
            map_north: 0.,
            map_width: 15360.,
            map_height: 15360.,
            png_job: None,
        }
    }
}
impl App {
    fn status(&mut self, r: Result<String>) {
        match r {
            Ok(m) => {
                self.message = m;
                self.error = false
            }
            Err(e) => {
                self.message = format!("{e:#}");
                self.error = true
            }
        }
    }
    fn load(&mut self, index: usize, path: PathBuf) {
        match tv4p::load(&path, &mut self.library) {
            Ok(mut p) => {
                // Re-reading models must preserve provenance from the last merge.
                let previous = &self.datasets[index];
                let same_result = index == 2
                    && previous.merged_result
                    && previous
                        .project
                        .as_ref()
                        .is_some_and(|old| tv4p::same_path(&old.path, &p.path));
                if same_result {
                    let ids: BTreeSet<_> = previous
                        .project
                        .as_ref()
                        .unwrap()
                        .roads
                        .iter()
                        .filter(|r| r.is_new)
                        .map(|r| r.id)
                        .collect();
                    for road in &mut p.roads {
                        road.is_new = ids.contains(&road.id);
                    }
                }
                let n = p.roads.len();
                let missing = p
                    .roads
                    .iter()
                    .filter(|r| !r.shape.warnings.is_empty())
                    .count();
                self.datasets[index] = Dataset {
                    project: Some(p),
                    merged_result: same_result,
                    zoom: 1.,
                    ..Default::default()
                };
                self.active = index;
                self.status(Ok(format!(
                    "Wczytano {n} dróg · problemy geometrii: {missing}"
                )))
            }
            Err(e) => self.status(Err(e)),
        }
    }
    fn merge(&mut self) {
        let a = self.datasets[0].project.as_ref().map(|p| p.path.clone());
        let b = self.datasets[1].project.as_ref().map(|p| p.path.clone());
        if let (Some(a), Some(b)) = (a, b) {
            let out = PathBuf::from(&self.output);
            match tv4p::merge_report(&a, &b, &out) {
                Ok(report) => {
                    self.load(2, out);
                    if !self.error {
                        let ds = &mut self.datasets[2];
                        ds.merged_result = true;
                        for road in &mut ds.project.as_mut().unwrap().roads {
                            road.is_new = report.new_ids.contains(&road.id);
                        }
                        self.status(Ok(report.message))
                    }
                }
                Err(e) => self.status(Err(e)),
            }
        }
    }
    fn export_png(&mut self) {
        if self.png_job.is_some() {
            return;
        }
        if let Err(e) = render::validate_dimensions(self.png_width, self.png_height) {
            self.status(Err(e));
            return;
        }
        let map_bounds = self.full_map.then_some([
            self.map_east,
            self.map_north,
            self.map_east + self.map_width,
            self.map_north + self.map_height,
        ]);
        if map_bounds.is_some_and(|b| {
            !b.iter().all(|n| n.is_finite()) || self.map_width <= 0. || self.map_height <= 0.
        }) {
            self.status(Err(anyhow::anyhow!(
                "Wymiary mapy muszą być dodatnie, a współrzędne skończone"
            )));
            return;
        }
        let ds = &self.datasets[self.active];
        let Some(p) = &ds.project else { return };
        let roads: Vec<_> = p
            .roads
            .iter()
            .filter(|r| {
                if self.png_selected {
                    ds.selected.contains(&r.index)
                } else {
                    ds.accepts(r)
                }
            })
            .collect();
        if roads.is_empty() {
            self.status(Err(anyhow::anyhow!(
                "Brak dróg w wybranym zakresie eksportu"
            )));
            return;
        }
        if let Some(path) = FileDialog::new()
            .add_filter("PNG", &["png"])
            .set_file_name(self.language.tr("drogi.png"))
            .set_title(self.language.tr("Eksportuj PNG…"))
            .save_file()
        {
            if self
                .datasets
                .iter()
                .filter_map(|d| d.project.as_ref())
                .any(|p| tv4p::same_path(&p.path, &path))
            {
                self.status(Err(anyhow::anyhow!(
                    "Eksport PNG musi mieć inną ścieżkę niż wczytany projekt"
                )));
                return;
            }
            let count = roads.len();
            let roads: Vec<Road> = roads.into_iter().cloned().collect();
            let selection = ds.selected.clone();
            let (width, height, transparent) = (self.png_width, self.png_height, self.transparent);
            let (sender, receiver) = mpsc::channel();
            match std::thread::Builder::new()
                .name("png-export".into())
                .spawn(move || {
                    let refs = roads.iter().collect::<Vec<_>>();
                    let result = render::png_in_bounds(
                        &path,
                        &refs,
                        &selection,
                        width,
                        height,
                        transparent,
                        map_bounds,
                    )
                    .map(|_| {
                        format!(
                            "PNG: {count} dróg · {width} × {height} px · {}",
                            path.display()
                        )
                    });
                    let _ = sender.send(result);
                }) {
                Ok(_) => {
                    self.png_job = Some(receiver);
                    self.status(Ok(format!(
                        "Eksportowanie {count} dróg do PNG {width} × {height}…"
                    )));
                }
                Err(e) => self.status(Err(e.into())),
            }
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let lang = self.language;
        if let Some(job) = &self.png_job {
            match job.try_recv() {
                Ok(result) => {
                    self.png_job = None;
                    self.status(result);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.png_job = None;
                    self.status(Err(anyhow::anyhow!(
                        lang.tr("Eksport PNG został przerwany")
                    )));
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(250))
                }
            }
        }
        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(lang.tr("Język"));
                let previous = self.language;
                egui::ComboBox::from_id_salt("language")
                    .selected_text(if self.language == Language::Polish {
                        "Polski"
                    } else {
                        "English"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.language, Language::Polish, "Polski");
                        ui.selectable_value(&mut self.language, Language::English, "English");
                    });
                if previous != self.language {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(
                        format!("{} {}", self.language.tr("Scalanie dróg Terrain Builder"), crate::version()),
                    ));
                    ctx.request_repaint();
                }
            });
            ui.horizontal(|ui| {
                for i in 0..2 {
                    if ui
                        .button(if i == 0 {
                            lang.tr("Wczytaj A…")
                        } else {
                            lang.tr("Wczytaj B…")
                        })
                        .clicked()
                    {
                        if let Some(p) = FileDialog::new()
                            .set_title(lang.tr(if i == 0 {
                                "Wczytaj A…"
                            } else {
                                "Wczytaj B…"
                            }))
                            .add_filter("Terrain Builder", &["tv4p"])
                            .pick_file()
                        {
                            self.load(i, p)
                        }
                    }
                    let label = self.datasets[i]
                        .project
                        .as_ref()
                        .and_then(|p| p.path.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| lang.tr("brak pliku").into());
                    ui.label(label);
                    ui.separator();
                }
            });
            ui.horizontal(|ui| {
                ui.label(lang.tr("Wynik:"));
                ui.add(egui::TextEdit::singleline(&mut self.output).desired_width(400.));
                if ui.button(lang.tr("Zapisz jako…")).clicked() {
                    if let Some(p) = FileDialog::new()
                        .set_title(lang.tr("Zapisz jako…"))
                        .add_filter("TV4P", &["tv4p"])
                        .set_file_name("output.tv4p")
                        .save_file()
                    {
                        self.output = p.to_string_lossy().into()
                    }
                }
                if ui
                    .add_enabled(
                        self.datasets[0].project.is_some() && self.datasets[1].project.is_some(),
                        egui::Button::new(lang.tr("Scal drogi")),
                    )
                    .clicked()
                {
                    self.merge()
                }
            });
            ui.horizontal(|ui| {
                ui.label(lang.tr("Modele MLOD:"));
                ui.add(egui::TextEdit::singleline(&mut self.model_root).desired_width(400.));
                let mut reload = ui.button(lang.tr("Zastosuj")).clicked();
                if ui.button(lang.tr("Folder…")).clicked() {
                    if let Some(p) = FileDialog::new()
                        .set_title(lang.tr("Wybierz folder modeli MLOD"))
                        .pick_folder()
                    {
                        self.model_root = p.to_string_lossy().into();
                        reload = true
                    }
                }
                if reload {
                    self.library = Library::new(PathBuf::from(&self.model_root));
                    let active = self.active;
                    for i in 0..3 {
                        if let Some(p) = self.datasets[i].project.as_ref().map(|p| p.path.clone()) {
                            self.load(i, p)
                        }
                    }
                    self.active = active;
                }
            });
            ui.horizontal(|ui| {
                for (i, label) in [lang.tr("Plik A"), lang.tr("Plik B"), lang.tr("Wynik")]
                    .iter()
                    .enumerate()
                {
                    if ui
                        .add_enabled(
                            self.datasets[i].project.is_some(),
                            egui::Button::new(lang.tr(label)).selected(self.active == i),
                        )
                        .clicked()
                    {
                        self.active = i
                    }
                }
                ui.separator();
                ui.colored_label(
                    if self.error {
                        Color32::LIGHT_RED
                    } else {
                        Color32::LIGHT_GRAY
                    },
                    lang.tr(&self.message),
                );
            });
            if !PathBuf::from(&self.model_root).is_dir() {
                ui.colored_label(Color32::YELLOW, lang.tr(&format!(
                    "Folder modeli nie istnieje: {}. Wybierz folder zawierający modele MLOD.", self.model_root
                )));
            }
            if let Some(project) = &self.datasets[self.active].project {
                let invalid: BTreeSet<_> = project.roads.iter()
                    .flat_map(|r| r.shape.non_mlod_models.iter()).collect();
                if !invalid.is_empty() {
                    ui.colored_label(Color32::LIGHT_RED, lang.tr(&format!(
                        "Modele nie są MLOD: {}. Podgląd tych części jest niedostępny. Wymagane są niezbinaryzowane pliki MLOD P3D.", invalid.len()
                    ))).on_hover_text(invalid.into_iter().cloned().collect::<Vec<_>>().join("\n"));
                }
            }
        });
        egui::TopBottomPanel::bottom("export").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("PNG:");
                egui::ComboBox::from_id_salt("scope")
                    .selected_text(if self.png_selected {
                        lang.tr("Tylko zaznaczone drogi")
                    } else {
                        lang.tr("Widoczne po filtrach")
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.png_selected, false, lang.tr("Widoczne po filtrach"));
                        ui.selectable_value(&mut self.png_selected, true, lang.tr("Tylko zaznaczone drogi"));
                    });
                ui.checkbox(&mut self.transparent, lang.tr("Przezroczyste tło"));
                let count = self.datasets[self.active].project.as_ref().map(|p| p.roads.iter().filter(|r| {
                    if self.png_selected { self.datasets[self.active].selected.contains(&r.index) }
                    else { self.datasets[self.active].accepts(r) }
                }).count()).unwrap_or(0);
                ui.label(lang.tr(&format!("Do eksportu: {count} dróg")));
                if ui
                    .add_enabled(
                        count > 0 && self.png_job.is_none(),
                        egui::Button::new(if self.png_job.is_some() { lang.tr("Eksportowanie…") } else { lang.tr("Eksportuj PNG…") }),
                    )
                    .clicked()
                {
                    self.export_png()
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(lang.tr("PNG [px]: szerokość"));
                ui.add(egui::DragValue::new(&mut self.png_width).range(128..=32768));
                ui.label(lang.tr("wysokość"));
                ui.add(egui::DragValue::new(&mut self.png_height).range(128..=32768));
                if ui.small_button("15360 × 15360").clicked() {
                    self.png_width = 15360; self.png_height = 15360;
                }
                ui.label(lang.tr(&format!("Bufor obrazu: {:.0} MiB", self.png_width as f64 * self.png_height as f64 * 4. / 1048576.)));
            });
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.full_map, lang.tr("Pełny obszar mapy"));
                ui.add_enabled_ui(self.full_map, |ui| {
                    ui.label(lang.tr("Lewy dolny róg [m]: E"));
                    ui.add(egui::DragValue::new(&mut self.map_east).speed(1.));
                    ui.label("N");
                    ui.add(egui::DragValue::new(&mut self.map_north).speed(1.));
                    ui.label(lang.tr("Rozmiar mapy [m]:"));
                    ui.add(egui::DragValue::new(&mut self.map_width).range(1.0..=1_000_000.0).speed(1.));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut self.map_height).range(1.0..=1_000_000.0).speed(1.));
                });
            });
            ui.small(if self.full_map {
                lang.tr("Eksport zachowuje współrzędne mapy, bez marginesów. Północ jest u góry; części poza mapą są przycinane.")
            } else {
                lang.tr("Kadr PNG jest dopasowany do eksportowanych dróg. Włącz pełny obszar mapy, aby zachować ich położenie na mapie.")
            });
        });
        let ds = &mut self.datasets[self.active];
        egui::SidePanel::left("roads")
            .default_width(345.)
            .min_width(260.)
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading(lang.tr("Lista dróg"));
                let Some(p) = ds.project.as_ref() else {
                    ui.label(lang.tr("Wczytaj plik."));
                    return;
                };
                let total = p.roads.len();
                let mut changed = ui
                    .add(
                        egui::TextEdit::singleline(&mut ds.search)
                            .hint_text(lang.tr("ID lub nazwa modelu…"))
                            .desired_width(f32::INFINITY),
                    )
                    .changed();
                let kinds: BTreeSet<_> = p.roads.iter().map(road_kind).collect();
                let old_kind = ds.kind.clone();
                egui::ComboBox::from_id_salt("road_kind")
                    .selected_text(if ds.kind.is_empty() {
                        lang.tr("Wszystkie typy")
                    } else {
                        ds.kind.clone()
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut ds.kind, String::new(), lang.tr("Wszystkie typy"));
                        for k in kinds {
                            ui.selectable_value(&mut ds.kind, k.clone(), k);
                        }
                    });
                changed |= old_kind != ds.kind;
                ui.horizontal(|ui| {
                    ui.label(lang.tr("Długość [m]:"));
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut ds.min_length)
                                .hint_text(lang.tr("od"))
                                .desired_width(65.),
                        )
                        .changed();
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut ds.max_length)
                                .hint_text(lang.tr("do"))
                                .desired_width(65.),
                        )
                        .changed();
                });
                changed |= ui
                    .checkbox(&mut ds.only_selected, lang.tr("Tylko zaznaczone"))
                    .changed();
                if ds.merged_result {
                    changed |= ui
                        .checkbox(&mut ds.only_new, lang.tr("Tylko nowe drogi z B"))
                        .changed();
                    let added = p.roads.iter().filter(|r| r.is_new).count();
                    ui.label(lang.tr(&format!("Nowe drogi z B: {added}")));
                    ui.small(lang.tr("Zielone: nowe z B · niebieskie: z A · żółte: zaznaczone"));
                }
                if changed {
                    ds.reset_view()
                }
                let visible: Vec<_> = ds
                    .project
                    .as_ref()
                    .unwrap()
                    .roads
                    .iter()
                    .filter(|r| ds.accepts(r))
                    .map(|r| r.index)
                    .collect();
                ui.label(lang.tr(&format!(
                    "Widoczne: {} / {total} · zaznaczone: {}",
                    visible.len(),
                    ds.selected.len()
                )));
                ui.horizontal(|ui| {
                    if ui.small_button(lang.tr("Zaznacz widoczne")).clicked() {
                        ds.selected.extend(visible.iter().copied());
                    }
                    if ui.small_button(lang.tr("Wyczyść zaznaczenie")).clicked() {
                        ds.selected.clear();
                    }
                });
                if ui.small_button(lang.tr("Reset filtrów")).clicked() {
                    ds.search.clear();
                    ds.kind.clear();
                    ds.min_length.clear();
                    ds.max_length.clear();
                    ds.only_selected = false;
                    ds.only_new = false;
                    ds.reset_view()
                }
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for index in visible {
                        let r = &ds.project.as_ref().unwrap().roads[index];
                        let marked = ds.selected.contains(&index);
                        let warning = if r.shape.warnings.is_empty() {
                            ""
                        } else {
                            " ⚠"
                        };
                        let label = lang.tr(&format!(
                            "#{} · {} · {:.1} m · {} części{}",
                            r.id,
                            road_kind(r),
                            r.shape.length,
                            r.parts,
                            warning
                        ));
                        let label = if r.is_new {
                            egui::RichText::new(format!("{} · {label}", lang.tr("NOWA")))
                                .color(Color32::from_rgb(80, 225, 154))
                        } else {
                            egui::RichText::new(label)
                        };
                        let response = ui.selectable_label(marked, label);
                        let warnings = r.shape.warnings.join("\n");
                        let tip = lang.tr(&format!(
                            "{}\nObrót bazowy: {:.4}° (TV4P 0x8C)\nMLOD: {} · nazwy: {}\n{}",
                            r.model,
                            r.rotation_degrees.rem_euclid(360.),
                            r.shape.mlod_parts,
                            r.shape.filename_parts,
                            warnings
                        ));
                        if response.on_hover_text(tip).clicked() {
                            if marked {
                                ds.selected.remove(&index);
                            } else {
                                ds.selected.insert(index);
                            }
                        }
                    }
                });
            });
        egui::CentralPanel::default().show(ctx, |ui| map(ui, ds, lang));
    }
}
fn map(ui: &mut egui::Ui, ds: &mut Dataset, lang: Language) {
    ui.horizontal(|ui| {
        ui.heading(lang.tr("Podgląd dróg"));
        if ui.button(lang.tr("Dopasuj widoczne")).clicked() {
            ds.reset_view()
        }
        if ui.button(lang.tr("Dopasuj zaznaczone")).clicked() {
            if let Some(p) = &ds.project {
                ds.focus =
                    render::bounds(p.roads.iter().filter(|r| ds.selected.contains(&r.index)));
                ds.zoom = 1.;
                ds.pan = Vec2::ZERO;
            }
        }
        if ui.button("−").clicked() {
            ds.zoom = (ds.zoom / 1.3).clamp(0.05, 200.)
        }
        if ui.button("+").clicked() {
            ds.zoom = (ds.zoom * 1.3).clamp(0.05, 200.)
        }
    });
    ui.small(lang.tr("Kształt i połączenia z MLOD · kółko: zoom · przeciągnij: przesuwanie · kliknij drogę: zaznaczenie"));
    let size = ui.available_size().max(Vec2::new(100., 100.));
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4., Color32::from_rgb(15, 23, 37));
    let Some(p) = &ds.project else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            lang.tr("Wczytaj projekt, aby wyświetlić drogi"),
            egui::FontId::proportional(18.),
            Color32::LIGHT_GRAY,
        );
        return;
    };
    let roads: Vec<_> = p.roads.iter().filter(|r| ds.accepts(r)).collect();
    let Some(b) = ds.focus.or_else(|| render::bounds(roads.iter().copied())) else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            lang.tr("Brak geometrii w bieżącym filtrze"),
            egui::FontId::proportional(18.),
            Color32::LIGHT_GRAY,
        );
        return;
    };
    if response.dragged() {
        ds.pan += response.drag_delta()
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0. {
            let old = ds.zoom;
            ds.zoom = (ds.zoom * (scroll as f64 * 0.002).exp()).clamp(0.05, 200.);
            if let Some(cursor) = response.hover_pos() {
                let rel = cursor - rect.center();
                ds.pan = rel - (rel - ds.pan) * (ds.zoom / old) as f32;
            }
        }
    }
    let view = View::new(
        b,
        rect.width() as f64,
        rect.height() as f64,
        ds.zoom,
        [ds.pan.x as f64, ds.pan.y as f64],
    );
    let screen = |p| {
        let p = view.point(p);
        Pos2::new(rect.left() + p[0] as f32, rect.top() + p[1] as f32)
    };
    let any = roads.iter().any(|r| ds.selected.contains(&r.index));
    for marked in [false, true] {
        for r in &roads {
            let selected = ds.selected.contains(&r.index);
            if marked != selected {
                continue;
            }
            let c = render::color(selected, any, r.is_new);
            let color = Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
            let mut mesh = egui::Mesh::default();
            for t in &r.shape.triangles {
                let n = mesh.vertices.len() as u32;
                for v in t {
                    mesh.colored_vertex(screen(*v), color)
                }
                mesh.add_triangle(n, n + 1, n + 2);
            }
            painter.add(egui::Shape::mesh(mesh));
            for line in &r.shape.lines {
                if line.len() > 1 {
                    painter.add(egui::Shape::line(
                        line.iter().copied().map(screen).collect(),
                        Stroke::new(if selected { 2.2_f32 } else { 1.1_f32 }, color),
                    ));
                }
            }
        }
    }
    if let Some(pos) = response.hover_pos() {
        let local = [(pos.x - rect.left()) as f64, (pos.y - rect.top()) as f64];
        let mut closest = None;
        let mut best = 9.;
        for r in &roads {
            for line in &r.shape.lines {
                for w in line.windows(2) {
                    let d = render::distance(local, view.point(w[0]), view.point(w[1]));
                    if d < best {
                        best = d;
                        closest = Some(r.index);
                    }
                }
            }
        }
        if let Some(index) = closest {
            let r = &p.roads[index];
            response.clone().on_hover_text(lang.tr(&format!(
                "#{} · {:.1} m · {} części",
                r.id, r.shape.length, r.parts
            )));
            if response.clicked() {
                if !ds.selected.remove(&index) {
                    ds.selected.insert(index);
                }
            }
        }
    }
    let unit_pixels = 100. * view.scale;
    if unit_pixels > 30. && unit_pixels < rect.width() as f64 / 2. {
        let a = rect.left_bottom() + Vec2::new(15., -25.);
        painter.line_segment(
            [a, a + Vec2::new(unit_pixels as f32, 0.)],
            Stroke::new(2.0_f32, Color32::WHITE),
        );
        painter.text(
            a + Vec2::new(0., -5.),
            egui::Align2::LEFT_BOTTOM,
            "100 m",
            egui::FontId::proportional(12.),
            Color32::WHITE,
        );
    }
}
pub fn run(language: Language) -> Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1380., 900.])
        .with_min_inner_size([1050., 700.]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../app_icon.png")) {
        viewport = viewport.with_icon(icon)
    }
    eframe::run_native(
        &format!(
            "{} {}",
            language.tr("Scalanie dróg Terrain Builder"),
            crate::version()
        ),
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(move |ctx| {
            ctx.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(App {
                language,
                ..App::default()
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
