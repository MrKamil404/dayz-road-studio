use crate::{gui, i18n::Language, update};
use dayz_road_tool::app::App as Builder;
use eframe::{App, egui};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, TryRecvError, channel},
    },
};

enum UpdateState {
    Idle,
    Checking(Receiver<anyhow::Result<update::Release>>),
    UpToDate,
    Failed(String),
    Available(update::Release),
    Downloading(
        update::Release,
        u64,
        Arc<AtomicU64>,
        Receiver<anyhow::Result<()>>,
    ),
    Installed(update::Release),
}

/// New-release check against GitHub, the release notes window and the in-place update.
struct Updater {
    state: UpdateState,
    /// The last check was started from the button, so its outcome is shown.
    manual: bool,
    window: bool,
    error: Option<String>,
    restart: bool,
}

impl Updater {
    fn new() -> Self {
        Self {
            state: UpdateState::Idle,
            manual: false,
            window: false,
            error: None,
            restart: false,
        }
    }

    fn exe() -> Option<PathBuf> {
        std::env::current_exe().ok()
    }

    fn exe_name() -> String {
        Self::exe()
            .and_then(|e| e.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default()
    }

    fn check(&mut self, manual: bool) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(update::latest());
        });
        self.manual = manual;
        self.state = UpdateState::Checking(rx);
    }

    fn found(&mut self, release: update::Release) {
        let next = if update::is_newer(&release.tag_name, crate::version()) {
            // A found release opens its notes once; the top bar button reopens them later.
            self.window = true;
            UpdateState::Available(release)
        } else {
            UpdateState::UpToDate
        };
        self.state = next;
    }

    fn tick(&mut self, ctx: &egui::Context) {
        match &self.state {
            UpdateState::Checking(rx) => match rx.try_recv() {
                Ok(Ok(release)) => self.found(release),
                Ok(Err(e)) => self.state = UpdateState::Failed(format!("{e:#}")),
                Err(TryRecvError::Empty) => {}
                Err(_) => {
                    self.state = UpdateState::Failed("Przerwano sprawdzanie aktualizacji".into())
                }
            },
            UpdateState::Downloading(release, _, _, rx) => match rx.try_recv() {
                Ok(Ok(())) => self.state = UpdateState::Installed(release.clone()),
                Ok(Err(e)) => {
                    self.error = Some(format!("{e:#}"));
                    self.state = UpdateState::Available(release.clone());
                }
                Err(TryRecvError::Empty) => {}
                Err(_) => self.state = UpdateState::Available(release.clone()),
            },
            _ => {}
        }
        if matches!(
            self.state,
            UpdateState::Checking(_) | UpdateState::Downloading(..)
        ) {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    fn install(&mut self) {
        let UpdateState::Available(release) = &self.state else {
            return;
        };
        let Some(exe) = Self::exe() else {
            self.error = Some("Nie można ustalić ścieżki programu".into());
            return;
        };
        let Some(asset) = update::asset(release, &Self::exe_name()).cloned() else {
            return;
        };
        let progress = Arc::new(AtomicU64::new(0));
        let (tx, rx) = channel();
        let shared = progress.clone();
        let size = asset.size;
        std::thread::spawn(move || {
            let _ = tx.send(update::install(&asset, &exe, &shared));
        });
        self.error = None;
        self.state = UpdateState::Downloading(release.clone(), size, progress, rx);
    }

    /// Top bar entry, laid out right to left.
    fn bar(&mut self, ui: &mut egui::Ui, lang: Language) {
        let mut check = false;
        match &self.state {
            UpdateState::Checking(_) => {
                ui.spinner();
                if self.manual {
                    ui.label(lang.tr("Sprawdzanie aktualizacji…"));
                }
            }
            UpdateState::Available(release) => {
                let text =
                    egui::RichText::new(lang.tr(&format!("Nowa wersja {}", release.tag_name)))
                        .color(egui::Color32::BLACK);
                if ui
                    .add(egui::Button::new(text).fill(egui::Color32::from_rgb(232, 196, 92)))
                    .clicked()
                {
                    self.window = true;
                }
            }
            UpdateState::Downloading(release, size, progress, _) => {
                let done = progress.load(Ordering::Relaxed) as f32 / (*size).max(1) as f32;
                if ui
                    .add(
                        egui::ProgressBar::new(done.min(1.))
                            .desired_width(160.)
                            .text(lang.tr(&format!("Pobieranie {}", release.tag_name))),
                    )
                    .clicked()
                {
                    self.window = true;
                }
            }
            UpdateState::Installed(release) => {
                let text = egui::RichText::new(
                    lang.tr(&format!("Uruchom ponownie, aby użyć {}", release.tag_name)),
                )
                .color(egui::Color32::BLACK);
                if ui
                    .add(egui::Button::new(text).fill(egui::Color32::from_rgb(120, 200, 140)))
                    .clicked()
                {
                    self.window = true;
                }
            }
            UpdateState::Idle | UpdateState::UpToDate | UpdateState::Failed(_) => {
                let mut button = ui.small_button(lang.tr("Sprawdź aktualizacje"));
                match &self.state {
                    UpdateState::Failed(e) => {
                        button = button.on_hover_text(lang.tr(e));
                        if self.manual {
                            ui.colored_label(
                                egui::Color32::from_rgb(230, 120, 110),
                                lang.tr("Nie udało się sprawdzić aktualizacji"),
                            )
                            .on_hover_text(lang.tr(e));
                        }
                    }
                    UpdateState::UpToDate if self.manual => {
                        ui.weak(lang.tr("Masz najnowszą wersję"));
                    }
                    _ => {}
                }
                check = button.clicked();
            }
        }
        if check {
            self.check(true);
        }
    }

    fn window(&mut self, ctx: &egui::Context, lang: Language) {
        let release = match &self.state {
            UpdateState::Available(r)
            | UpdateState::Downloading(r, ..)
            | UpdateState::Installed(r) => r.clone(),
            _ => return,
        };
        if !self.window {
            return;
        }
        let mut open = true;
        let mut install = false;
        let mut later = false;
        let mut restart = false;
        let asset = update::asset(&release, &Self::exe_name()).cloned();
        egui::Window::new(lang.tr("Aktualizacja programu"))
            .id(egui::Id::new("update-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(560.)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.screen_rect().center())
            .show(ctx, |ui| {
                ui.heading(lang.tr(&format!("Dostępna wersja {}", release.tag_name)));
                ui.label(lang.tr(&format!("Twoja wersja: {}", crate::version())));
                if let Some(name) = release
                    .name
                    .as_deref()
                    .filter(|n| !n.trim().is_empty() && *n != release.tag_name)
                {
                    ui.strong(name);
                }
                ui.separator();
                egui::ScrollArea::vertical().max_height(340.).show(ui, |ui| {
                    let notes = release
                        .body
                        .as_deref()
                        .unwrap_or("")
                        .replace("**", "")
                        .replace('\r', "");
                    if notes.trim().is_empty() {
                        ui.weak(lang.tr("Brak opisu zmian"));
                    } else {
                        ui.label(notes);
                    }
                });
                ui.separator();
                if let Some(e) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(230, 120, 110), lang.tr(e));
                }
                ui.horizontal(|ui| match &self.state {
                    UpdateState::Available(_) => {
                        if let Some(asset) = &asset {
                            if ui
                                .button(lang.tr(&format!(
                                    "Pobierz i zainstaluj ({:.1} MB)",
                                    asset.size as f64 / 1_048_576.
                                )))
                                .on_hover_text(lang.tr("Pobrany program zastąpi obecny plik; poprzedni zostaje jako .old do następnego uruchomienia."))
                                .clicked()
                            {
                                install = true;
                            }
                        } else {
                            ui.weak(lang.tr("Wydanie nie zawiera pliku .exe do pobrania"));
                        }
                        if ui.button(lang.tr("Strona wydania")).clicked() {
                            ctx.open_url(egui::OpenUrl::new_tab(&release.html_url));
                        }
                        if ui.button(lang.tr("Później")).clicked() {
                            later = true;
                        }
                    }
                    UpdateState::Downloading(_, size, progress, _) => {
                        let done = progress.load(Ordering::Relaxed);
                        ui.add(
                            egui::ProgressBar::new((done as f32 / (*size).max(1) as f32).min(1.))
                                .show_percentage()
                                .desired_width(300.),
                        );
                        ui.label(format!(
                            "{:.1} / {:.1} MB",
                            done as f64 / 1_048_576.,
                            *size as f64 / 1_048_576.
                        ));
                    }
                    UpdateState::Installed(_) => {
                        ui.label(lang.tr(
                            "Zainstalowano. Nowa wersja zadziała po ponownym uruchomieniu.",
                        ));
                        if ui.button(lang.tr("Uruchom ponownie")).clicked() {
                            restart = true;
                        }
                    }
                    _ => {}
                });
            });
        if !open || later {
            self.window = false;
        }
        if install {
            self.install();
        }
        if restart {
            // Closing goes through the unsaved-changes prompt; the new program starts on exit.
            self.restart = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Starts the new executable once the window has really closed.
    fn exit(&self) {
        if self.restart
            && matches!(self.state, UpdateState::Installed(_))
            && let Some(exe) = Self::exe()
        {
            let _ = std::process::Command::new(exe).spawn();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Toolset {
    #[default]
    Launcher,
    Merge,
    Builder,
}

struct Shell {
    active: Toolset,
    styled: Option<Toolset>,
    language: Language,
    merge: gui::App,
    builder: Builder,
    updater: Updater,
}

impl Shell {
    fn new(language: Language) -> Self {
        #[cfg(feature = "ui-screenshot")]
        let active = if std::env::var_os("EFRAME_SCREENSHOT_TO").is_some() {
            match std::env::var("DAYZ_SCREENSHOT_TOOLSET").as_deref() {
                Ok("builder") => Toolset::Builder,
                Ok("merge") => Toolset::Merge,
                _ => Toolset::Launcher,
            }
        } else {
            Toolset::Launcher
        };
        #[cfg(not(feature = "ui-screenshot"))]
        let active = Toolset::Launcher;
        #[allow(unused_mut)]
        let mut builder = Builder::new();
        #[cfg(feature = "ui-screenshot")]
        if std::env::var_os("DAYZ_SCREENSHOT_3D").is_some() {
            builder.screenshot_scene();
        }
        Self {
            active,
            styled: None,
            language,
            merge: gui::App::new(language),
            builder,
            updater: Updater::new(),
        }
    }

    fn apply_style(&mut self, ctx: &egui::Context) {
        if self.styled == Some(self.active) {
            return;
        }
        let style = if self.active == Toolset::Builder {
            Builder::style()
        } else {
            egui::Style {
                visuals: egui::Visuals::dark(),
                ..Default::default()
            }
        };
        ctx.set_style(style);
        self.styled = Some(self.active);
    }

    fn launcher(&mut self, ctx: &egui::Context) {
        let lang = self.language;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space((ui.available_height() * 0.18).min(140.));
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("TERRAIN BUILDER / DAYZ")
                        .size(14.)
                        .color(egui::Color32::from_rgb(131, 171, 184)),
                );
                ui.add_space(16.);
                ui.heading(egui::RichText::new(lang.tr("Wybierz zestaw narzędzi")).size(34.));
                ui.add_space(12.);
                ui.label(lang.tr("Scal projekty lub zaprojektuj nowe drogi."));
                ui.add_space(40.);
            });
            let width = ((ui.available_width() - 48.) / 2.).min(390.);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 210.),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.add_space(((ui.available_width() - width * 2. - 24.) / 2.).max(0.));
                    for (tool, name, description) in [
                        (
                            Toolset::Merge,
                            "Merge",
                            "Scalanie TV4P\nPodgląd, filtrowanie i eksport PNG",
                        ),
                        (
                            Toolset::Builder,
                            "Road Builder",
                            "Budowanie dróg\nSatelita, SHP, teren ASC i eksport TV4P",
                        ),
                    ] {
                        let text =
                            egui::RichText::new(format!("{name}\n\n{}", lang.tr(description)))
                                .size(20.);
                        if ui
                            .add_sized([width, 190.], egui::Button::new(text))
                            .clicked()
                        {
                            self.active = tool;
                            ctx.request_repaint();
                        }
                        ui.add_space(16.);
                    }
                },
            );
        });
    }
}

impl Shell {
    fn show(&mut self, ctx: &egui::Context) {
        self.apply_style(ctx);
        let lang = self.language;
        egui::TopBottomPanel::top("toolsets").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.strong("DAYZ ROAD STUDIO");
                ui.separator();
                if ui
                    .selectable_label(self.active == Toolset::Launcher, lang.tr("Narzędzia"))
                    .clicked()
                {
                    self.active = Toolset::Launcher;
                }
                if self.active != Toolset::Launcher {
                    ui.selectable_value(&mut self.active, Toolset::Merge, "Merge");
                    ui.selectable_value(&mut self.active, Toolset::Builder, "Road Builder");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::ComboBox::from_id_salt("shell-language")
                        .selected_text(if self.language == Language::Polish {
                            "Polski"
                        } else {
                            "English"
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.language, Language::Polish, "Polski");
                            ui.selectable_value(&mut self.language, Language::English, "English");
                        });
                    ui.label(lang.tr("Język"));
                    ui.separator();
                    self.updater.bar(ui, lang);
                });
            });
        });
        self.apply_style(ctx);
        self.merge.set_language(self.language);
        self.builder.set_language(match self.language {
            Language::Polish => dayz_road_tool::i18n::Language::Polish,
            Language::English => dayz_road_tool::i18n::Language::English,
        });
        self.updater.tick(ctx);
        self.builder.tick(ctx);
        self.builder.confirm_close(ctx);
        if self.active != Toolset::Merge {
            self.merge.tick(ctx);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "{} {}",
            crate::APP_NAME,
            crate::version()
        )));
        match self.active {
            Toolset::Launcher => self.launcher(ctx),
            Toolset::Merge => self.merge.show(ctx),
            Toolset::Builder => self.builder.show(ctx),
        }
        self.updater.window(ctx, lang);
    }
}

impl App for Shell {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.show(ctx);
    }

    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
        self.updater.exit();
    }
}

pub(crate) fn run(language: Language) -> anyhow::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1440., 920.])
        .with_min_inner_size([1050., 700.]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../app_icon.png")) {
        viewport = viewport.with_icon(icon);
    }
    eframe::run_native(
        crate::APP_NAME,
        eframe::NativeOptions {
            viewport,
            // The Road Builder 3D view draws terrain with depth testing.
            depth_buffer: 24,
            ..Default::default()
        },
        Box::new(move |_| {
            let mut shell = Shell::new(language);
            if let Some(exe) = Updater::exe() {
                update::cleanup(&exe);
            }
            // DAYZ_NO_UPDATE_CHECK=1 skips the automatic check at start.
            if std::env::var_os("DAYZ_NO_UPDATE_CHECK").is_none()
                && std::env::var_os("EFRAME_SCREENSHOT_TO").is_none()
            {
                shell.updater.check(false);
            }
            Ok(Box::new(shell))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(
        ctx: &egui::Context,
        shell: &mut Shell,
        events: Vec<egui::Event>,
        size: egui::Vec2,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ctx| shell.show(ctx),
        )
    }

    fn texts(shape: &egui::epaint::Shape, found: &mut Vec<(String, egui::Pos2)>) {
        match shape {
            egui::epaint::Shape::Text(text) => found.push((
                text.galley.job.text.clone(),
                text.pos + text.galley.size() / 2.,
            )),
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    texts(shape, found);
                }
            }
            _ => {}
        }
    }

    fn labels(output: &egui::FullOutput) -> Vec<(String, egui::Pos2)> {
        let mut found = Vec::new();
        for shape in &output.shapes {
            texts(&shape.shape, &mut found);
        }
        found
    }

    fn click(ctx: &egui::Context, shell: &mut Shell, pos: egui::Pos2) -> egui::FullOutput {
        let size = egui::vec2(1440., 920.);
        render(
            ctx,
            shell,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
            size,
        );
        render(
            ctx,
            shell,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            size,
        )
    }

    #[test]
    fn launcher_tabs_preserve_drawing_and_route_keyboard_to_active_tool() {
        let ctx = egui::Context::default();
        let mut shell = Shell::new(Language::English);
        let output = render(&ctx, &mut shell, vec![], egui::vec2(1440., 920.));
        let builder_tile = labels(&output)
            .into_iter()
            .find(|(text, _)| text.starts_with("Road Builder\n"))
            .unwrap()
            .1;
        click(&ctx, &mut shell, builder_tile);
        assert!(shell.active == Toolset::Builder);
        let output = render(&ctx, &mut shell, vec![], egui::vec2(1440., 920.));
        assert!(
            labels(&output)
                .iter()
                .any(|(text, _)| text == "Road workshop")
        );
        assert!(ctx.style().visuals.dark_mode);
        assert!(!shell.builder.has_unsaved_changes());
        let output = click(&ctx, &mut shell, egui::pos2(800., 400.));
        assert!(shell.builder.has_unsaved_changes());
        let merge_tab = labels(&output)
            .into_iter()
            .find(|(text, _)| text == "Merge")
            .unwrap()
            .1;
        click(&ctx, &mut shell, merge_tab);
        assert!(shell.active == Toolset::Merge);
        let output = render(
            &ctx,
            &mut shell,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            egui::vec2(1440., 920.),
        );
        assert!(ctx.style().visuals.dark_mode);
        assert!(shell.builder.has_unsaved_changes());
        let builder_tab = labels(&output)
            .into_iter()
            .find(|(text, _)| text == "Road Builder")
            .unwrap()
            .1;
        click(&ctx, &mut shell, builder_tab);
        shell.language = Language::Polish;
        let output = render(&ctx, &mut shell, vec![], egui::vec2(1440., 920.));
        assert!(
            labels(&output)
                .iter()
                .any(|(text, _)| text == "Warsztat dróg")
        );
        assert!(shell.builder.has_unsaved_changes());
    }

    #[test]
    fn found_release_shows_its_notes_and_the_top_bar_button() {
        let ctx = egui::Context::default();
        let mut shell = Shell::new(Language::English);
        shell.updater.found(update::Release {
            tag_name: "2.4.0".into(),
            name: None,
            body: None,
            html_url: String::new(),
            assets: vec![],
        });
        assert!(matches!(shell.updater.state, UpdateState::UpToDate));
        assert!(!shell.updater.window);
        shell.updater.found(update::Release {
            tag_name: "99.0.0".into(),
            name: Some("99.0.0 - rails".into()),
            body: Some("## 99.0.0\r\n- **Tracks** added".into()),
            html_url: String::new(),
            assets: vec![update::Asset {
                name: "tv4p_merge_roads.exe".into(),
                size: 8_250_880,
                browser_download_url: String::new(),
            }],
        });
        // Windows measure themselves in their first frame.
        render(&ctx, &mut shell, vec![], egui::vec2(1440., 920.));
        let output = render(&ctx, &mut shell, vec![], egui::vec2(1440., 920.));
        let texts: Vec<_> = labels(&output).into_iter().map(|(t, _)| t).collect();
        for expected in [
            "New version 99.0.0",
            "Version 99.0.0 is available",
            "Later",
            "Release page",
        ] {
            assert!(
                texts.iter().any(|t| t == expected),
                "{expected} in {texts:?}"
            );
        }
        assert!(texts.iter().any(|t| t.contains("- Tracks added")));
        let later = labels(&output)
            .into_iter()
            .find(|(t, _)| t == "Later")
            .unwrap()
            .1;
        click(&ctx, &mut shell, later);
        let output = render(&ctx, &mut shell, vec![], egui::vec2(1440., 920.));
        let texts: Vec<_> = labels(&output).into_iter().map(|(t, _)| t).collect();
        assert!(!texts.iter().any(|t| t == "Later"));
        assert!(texts.iter().any(|t| t == "New version 99.0.0"));
    }

    #[test]
    fn all_toolsets_render_in_both_languages_at_minimum_window_size() {
        for language in [Language::Polish, Language::English] {
            let ctx = egui::Context::default();
            let mut shell = Shell::new(language);
            for tool in [
                Toolset::Launcher,
                Toolset::Merge,
                Toolset::Builder,
                Toolset::Merge,
            ] {
                shell.active = tool;
                let output = render(&ctx, &mut shell, vec![], egui::vec2(1050., 700.));
                assert!(!output.shapes.is_empty());
                assert!(ctx.style().visuals.dark_mode);
                assert!(
                    labels(&output)
                        .iter()
                        .any(|(text, _)| text == "DAYZ ROAD STUDIO")
                );
            }
        }
    }
}
