mod geometry;
mod gui;
mod render;
mod tv4p;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
pub const DEFAULT_MODELS: &str = "G:\\dz\\structures\\roads\\parts";
fn main() {
    if let Err(err) = run() {
        eprintln!("{err:#}");
        std::process::exit(1)
    }
}
fn run() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.is_empty() {
        return gui::run();
    };
    let root = |index| PathBuf::from(a.get(index).map(String::as_str).unwrap_or(DEFAULT_MODELS));
    match a[0].as_str() {
        "merge" if a.len() == 4 => println!(
            "{}",
            tv4p::merge(Path::new(&a[1]), Path::new(&a[2]), Path::new(&a[3]))?
        ),
        "roundtrip" if a.len() == 2 => tv4p::roundtrip(Path::new(&a[1]))?,
        "types" if a.len() == 2 => tv4p::types(Path::new(&a[1]))?,
        "export" | "png" if a.len() == 3 || a.len() == 4 => {
            if tv4p::same_path(Path::new(&a[1]), Path::new(&a[2])) {
                bail!("Eksport musi trafić do innego pliku niż projekt źródłowy");
            }
            let mut lib = geometry::Library::new(root(3));
            let p = tv4p::load(Path::new(&a[1]), &mut lib)?;
            if a[0] == "export" {
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
                "{} roads, {} parts, {} geometry warnings",
                p.roads.len(),
                p.roads.iter().map(|r| r.parts).sum::<usize>(),
                p.roads
                    .iter()
                    .map(|r| r.shape.warnings.len())
                    .sum::<usize>()
            );
        }
        "inspect-p3d" if a.len() == 2 => {
            let m = geometry::read_mlod(Path::new(&a[1]))?;
            println!(
                "Length {:.6} m; {} triangles; ports {:?}",
                m.length,
                m.triangles.len(),
                m.ports
            );
        }
        "--help" | "help" => println!(
            "GUI: tv4p_merge_roads.exe\nmerge A.tv4p B.tv4p out.tv4p\nexport input.tv4p out.json [models-folder]\npng input.tv4p out.png [models-folder]\nroundtrip input.tv4p\ntypes input.tv4p\ninspect-p3d model.p3d"
        ),
        _ => bail!("Nieznane polecenie. Użyj --help."),
    }
    Ok(())
}
