use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    Polish,
    English,
}

#[derive(Deserialize)]
struct Translation {
    pl: String,
    en: String,
}

fn catalog() -> &'static [Translation] {
    static CATALOG: OnceLock<Vec<Translation>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut entries: Vec<Translation> = serde_json::from_str(include_str!("translations.json"))
            .expect("Invalid translation catalog");
        // Specific messages must win over generic contexts such as "Saved {0}".
        entries.sort_by_key(|entry| {
            std::cmp::Reverse(
                entry
                    .pl
                    .split('{')
                    .map(|part| part.split('}').next_back().unwrap_or("").len())
                    .sum::<usize>(),
            )
        });
        entries
    })
}

impl Language {
    /// Translate at display time so saved statuses and model warnings also
    /// follow a language change without reloading or changing the source data.
    pub fn tr(self, text: &str) -> String {
        self.translate(text, 0)
    }

    fn translate(self, text: &str, depth: usize) -> String {
        if depth > 8 {
            return text.to_owned();
        }
        for entry in catalog() {
            let target = match self {
                Self::Polish => &entry.pl,
                Self::English => &entry.en,
            };
            if text == entry.pl || text == entry.en {
                return target.clone();
            }
        }
        for entry in catalog() {
            let target = match self {
                Self::Polish => &entry.pl,
                Self::English => &entry.en,
            };
            for source in [&entry.pl, &entry.en] {
                if !source.contains('{') {
                    continue;
                }
                if let Some(args) = capture(source, text) {
                    let args: Vec<_> = args.iter().map(|s| self.translate(s, depth + 1)).collect();
                    return substitute(target, &args);
                }
            }
        }
        // Anyhow context chains and per-model warnings contain nested messages.
        // Paths, IDs, model names and operating-system diagnostics stay intact.
        if text.contains('\n') {
            return text
                .split('\n')
                .map(|s| self.translate(s, depth + 1))
                .collect::<Vec<_>>()
                .join("\n");
        }
        if text.contains(": ") {
            return text
                .split(": ")
                .map(|s| self.translate(s, depth + 1))
                .collect::<Vec<_>>()
                .join(": ");
        }
        text.to_owned()
    }
}

// Catalog placeholders are positional ({0}, {1}, ...). Only their surrounding
// literal text is matched; captured filenames and numeric values are preserved.
fn capture<'a>(pattern: &str, text: &'a str) -> Option<Vec<&'a str>> {
    let mut rest = text;
    let mut pattern = pattern;
    let mut args = Vec::new();
    loop {
        let Some(open) = pattern.find('{') else {
            return (rest == pattern).then_some(args);
        };
        rest = rest.strip_prefix(&pattern[..open])?;
        let close = pattern[open..].find('}')? + open;
        let index: usize = pattern[open + 1..close].parse().ok()?;
        if index != args.len() {
            return None;
        }
        pattern = &pattern[close + 1..];
        let next = pattern.find('{').unwrap_or(pattern.len());
        let separator = &pattern[..next];
        let end = if separator.is_empty() {
            if next != pattern.len() {
                return None;
            }
            rest.len()
        } else if next == pattern.len() {
            rest.len()
                .checked_sub(separator.len())
                .filter(|_| rest.ends_with(separator))?
        } else {
            rest.find(separator)?
        };
        args.push(&rest[..end]);
        rest = &rest[end..];
    }
}

fn substitute(mut pattern: &str, args: &[String]) -> String {
    let mut result = String::new();
    while let Some(open) = pattern.find('{') {
        result.push_str(&pattern[..open]);
        let close = pattern[open..]
            .find('}')
            .expect("Invalid translation placeholder")
            + open;
        let index: usize = pattern[open + 1..close]
            .parse()
            .expect("Invalid translation index");
        result.push_str(&args[index]);
        pattern = &pattern[close + 1..];
    }
    result.push_str(pattern);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_and_terrain_statuses_translate_at_display_time() {
        assert_eq!(
            Language::English.tr("Podgląd: 12 segmentów · klik: zatwierdź · Enter: zakończ"),
            "Preview: 12 segments · click: confirm · Enter: finish"
        );
        let status = "Zapisano D:\\maps\\road.tv4p. Sprawdź wynik w Terrain Builder.";
        assert_eq!(
            Language::English.tr(status),
            "Saved D:\\maps\\road.tv4p. Check the result in Terrain Builder."
        );
        assert_eq!(Language::Polish.tr(status), status);
        assert_eq!(
            Language::English
                .tr("Droga #17: model asf2_25.p3d nie należy do typu Road Tool asf3 / kategorii 3"),
            "Road #17: model asf2_25.p3d does not belong to Road Tool type asf3 / category 3"
        );
    }
}
