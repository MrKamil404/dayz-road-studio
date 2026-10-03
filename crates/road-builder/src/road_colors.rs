use crate::i18n::Language;
use eframe::egui;
use std::collections::{BTreeMap, BTreeSet};

pub type Palette = BTreeMap<String, [u8; 3]>;
pub const DEFAULT: [u8; 3] = [92, 182, 236];

pub fn family(model: &str) -> String {
    model
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(model)
        .split('_')
        .next()
        .unwrap_or("")
        .to_owned()
}

pub fn editor(
    ui: &mut egui::Ui,
    types: &BTreeSet<String>,
    palette: &mut Palette,
    lang: Language,
) -> bool {
    let mut changed = false;
    ui.collapsing(lang.tr("Kolory typów dróg"), |ui| {
        ui.small(lang.tr(
            "Wybrane kolory są używane także w PNG. Zaznaczenie w podglądzie pozostaje żółte.",
        ));
        if types.is_empty() {
            ui.label(lang.tr("Wczytaj drogi, aby zmienić kolory typów."));
        }
        egui::ScrollArea::vertical()
            .max_height(180.)
            .id_salt("road-type-colors")
            .show(ui, |ui| {
                for name in types {
                    ui.push_id(name, |ui| {
                        ui.horizontal(|ui| {
                            let mut rgb = palette.get(name).copied().unwrap_or(DEFAULT);
                            if ui.color_edit_button_srgb(&mut rgb).changed() {
                                palette.insert(name.clone(), rgb);
                                changed = true;
                            }
                            ui.label(name);
                            if ui
                                .add_enabled(
                                    palette.contains_key(name),
                                    egui::Button::new(lang.tr("Reset")),
                                )
                                .on_hover_text(lang.tr("Przywróć domyślny kolor tego typu"))
                                .clicked()
                            {
                                palette.remove(name);
                                changed = true;
                            }
                        });
                    });
                }
            });
    });
    changed
}
