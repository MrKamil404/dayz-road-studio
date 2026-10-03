mod geometry;
mod gui;
mod i18n;
mod render;
mod shell;
mod tv4p;
use anyhow::{Result, bail};
use i18n::Language;
use std::path::{Path, PathBuf};
pub const DEFAULT_MODELS: &str = "P:\\dz\\structures\\roads\\parts";
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
        .strip_suffix(".0")
        .unwrap_or(env!("CARGO_PKG_VERSION"))
}
fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let language = if args.first().is_some_and(|s| s == "--lang") {
        let language = match args.get(1).map(String::as_str) {
            Some("pl") => Language::Polish,
            Some("en") => Language::English,
            _ => {
                eprintln!("{}", Language::Polish.tr("Język musi być pl lub en"));
                std::process::exit(1);
            }
        };
        args.drain(..2);
        language
    } else {
        Language::Polish
    };
    if let Err(err) = run(args, language) {
        eprintln!("{}", language.tr(&format!("{err:#}")));
        std::process::exit(1)
    }
}
fn run(a: Vec<String>, language: Language) -> Result<()> {
    if a.is_empty() {
        return gui::run(language);
    };
    let root = |index| PathBuf::from(a.get(index).map(String::as_str).unwrap_or(DEFAULT_MODELS));
    match a[0].as_str() {
        "--version" | "-V" => println!(
            "{} {}",
            language.tr("Scalanie dróg Terrain Builder"),
            version()
        ),
        "merge" if a.len() == 4 => println!(
            "{}",
            language.tr(&tv4p::merge(
                Path::new(&a[1]),
                Path::new(&a[2]),
                Path::new(&a[3])
            )?)
        ),
        "roundtrip" if a.len() == 2 => tv4p::roundtrip(Path::new(&a[1]), language)?,
        "types" if a.len() == 2 => tv4p::types(Path::new(&a[1]), language)?,
        "export" | "png" if a.len() == 3 || a.len() == 4 => {
            if tv4p::same_path(Path::new(&a[1]), Path::new(&a[2])) {
                bail!("Eksport musi trafić do innego pliku niż projekt źródłowy");
            }
            let mut lib = geometry::Library::new(root(3));
            let mut p = tv4p::load(Path::new(&a[1]), &mut lib)?;
            if a[0] == "export" {
                for road in &mut p.roads {
                    for warning in &mut road.shape.warnings {
                        *warning = language.tr(warning);
                    }
                }
                std::fs::write(&a[2], serde_json::to_vec_pretty(&p)?)?
            } else {
                let roads: Vec<_> = p.roads.iter().collect();
                render::png(
                    Path::new(&a[2]),
                    &roads,
                    &Default::default(),
                    2048,
                    1536,
                    false,
                )?;
            }
            println!(
                "{}",
                language.tr(&format!(
                    "{} roads, {} parts, {} geometry warnings",
                    p.roads.len(),
                    p.roads.iter().map(|r| r.parts).sum::<usize>(),
                    p.roads
                        .iter()
                        .map(|r| r.shape.warnings.len())
                        .sum::<usize>()
                ))
            );
        }
        "inspect-p3d" if a.len() == 2 => {
            let m = geometry::read_mlod(Path::new(&a[1]))?;
            println!(
                "{}",
                language.tr(&format!(
                    "Length {:.6} m; {} triangles; ports {:?}",
                    m.length,
                    m.triangles.len(),
                    m.ports
                ))
            );
        }
        "--help" | "help" => {
            println!(
                "{}",
                language.tr("Opcjonalny język: --lang pl lub --lang en (przed poleceniem)")
            );
            println!(
                "GUI: tv4p_merge_roads.exe\nmerge A.tv4p B.tv4p out.tv4p\nexport input.tv4p out.json [models-folder]\npng input.tv4p out.png [models-folder]\nroundtrip input.tv4p\ntypes input.tv4p\ninspect-p3d model.p3d"
            );
        }
        _ => bail!("Nieznane polecenie. Użyj --help."),
    }
    Ok(())
}
