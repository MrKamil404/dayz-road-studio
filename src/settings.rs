//! User settings kept between runs in `%LOCALAPPDATA%\DayZRoadStudio\settings.json`.
use crate::i18n::Language;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Default)]
struct Settings {
    #[serde(default)]
    language: Option<String>,
}

pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("APPDATA"))?;
    Some(PathBuf::from(base).join("DayZRoadStudio").join("settings.json"))
}

fn read(file: &Path) -> Settings {
    std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The language chosen last time, if any.
pub fn language(file: &Path) -> Option<Language> {
    match read(file).language.as_deref() {
        Some("pl") => Some(Language::Polish),
        Some("en") => Some(Language::English),
        _ => None,
    }
}

/// Remembers the chosen language; failures are ignored, the setting is a convenience.
pub fn save_language(file: &Path, language: Language) {
    let mut s = read(file);
    s.language = Some(if language == Language::English { "en" } else { "pl" }.into());
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&s) {
        let tmp = file.with_extension("json.tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_round_trips_and_bad_files_fall_back() {
        let dir = std::env::temp_dir().join(format!("road-settings-{}", std::process::id()));
        let file = dir.join("DayZRoadStudio").join("settings.json");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(language(&file).is_none());
        save_language(&file, Language::English);
        assert!(language(&file) == Some(Language::English));
        save_language(&file, Language::Polish);
        assert!(language(&file) == Some(Language::Polish));
        std::fs::write(&file, b"not json").unwrap();
        assert!(language(&file).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
