use crate::{gui, i18n::Language};
use dayz_road_tool::app::App as Builder;
use eframe::{App, egui};

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
        Self {
            active,
            styled: None,
            language,
            merge: gui::App::new(language),
            builder: Builder::new(),
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
                ui.strong("DAYZ ROAD TOOLS");
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
                });
            });
        });
        self.apply_style(ctx);
        self.merge.set_language(self.language);
        self.builder.set_language(match self.language {
            Language::Polish => dayz_road_tool::i18n::Language::Polish,
            Language::English => dayz_road_tool::i18n::Language::English,
        });
        self.builder.tick(ctx);
        self.builder.confirm_close(ctx);
        if self.active != Toolset::Merge {
            self.merge.tick(ctx);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "DayZ Road Tools {}",
            crate::version()
        )));
        match self.active {
            Toolset::Launcher => self.launcher(ctx),
            Toolset::Merge => self.merge.show(ctx),
            Toolset::Builder => self.builder.show(ctx),
        }
    }
}

impl App for Shell {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.show(ctx);
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
        "DayZ Road Tools",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(move |_| Ok(Box::new(Shell::new(language)))),
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
                        .any(|(text, _)| text == "DAYZ ROAD TOOLS")
                );
            }
        }
    }
}
