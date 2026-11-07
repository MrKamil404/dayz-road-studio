//! Checks GitHub for a newer release and replaces the running executable.
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

pub const REPOSITORY: &str = "MrKamil404/dayz-road-studio";
const USER_AGENT: &str = concat!("DayZ-Road-Studio-updater/", env!("CARGO_PKG_VERSION"));

#[derive(Deserialize, Clone, Debug)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
}

/// Numeric parts of a version such as `v2.4.0` or `2.5.0-beta`.
pub fn parse_version(text: &str) -> Option<Vec<u64>> {
    let text = text.trim().trim_start_matches(['v', 'V']);
    let core = text.split(['-', '+']).next()?;
    core.split('.').map(|p| p.parse().ok()).collect()
}

pub fn is_newer(tag: &str, current: &str) -> bool {
    let (Some(mut a), Some(mut b)) = (parse_version(tag), parse_version(current)) else {
        return false;
    };
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a > b
}

fn agent() -> Result<ureq::Agent> {
    let tls =
        native_tls::TlsConnector::new().context("Nie udało się przygotować połączenia HTTPS")?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .user_agent(USER_AGENT)
        .build())
}

/// The latest published (non-draft, non-prerelease) release.
pub fn latest() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/latest");
    agent()?
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .call()
        .context("Nie udało się połączyć z GitHub")?
        .into_json()
        .context("Nieprawidłowa odpowiedź GitHub")
}

/// The executable to download: the one named like the running program, or the only `.exe`.
pub fn asset<'a>(release: &'a Release, exe_name: &str) -> Option<&'a Asset> {
    let exes: Vec<_> = release
        .assets
        .iter()
        .filter(|a| a.name.to_ascii_lowercase().ends_with(".exe"))
        .collect();
    exes.iter()
        .find(|a| a.name.eq_ignore_ascii_case(exe_name))
        .or(if exes.len() == 1 { exes.first() } else { None })
        .copied()
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    exe.with_file_name(name)
}

/// The previous executable kept after an update.
pub fn previous(exe: &Path) -> PathBuf {
    sibling(exe, ".old")
}

/// Removes the executable left over from an earlier update.
pub fn cleanup(exe: &Path) {
    let _ = std::fs::remove_file(previous(exe));
    let _ = std::fs::remove_file(sibling(exe, ".new"));
}

/// Downloads the asset next to `exe` and swaps it in. The running program keeps
/// working from the renamed `.old` file until it is restarted.
pub fn install(asset: &Asset, exe: &Path, progress: &AtomicU64) -> Result<()> {
    let new = sibling(exe, ".new");
    let response = agent()?
        .get(&asset.browser_download_url)
        .call()
        .context("Nie udało się pobrać nowej wersji")?;
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&new)
        .with_context(|| format!("Nie można zapisać pliku {}", new.display()))?;
    let mut buf = vec![0; 64 * 1024];
    let mut total = 0u64;
    let mut head = Vec::new();
    loop {
        let n = reader
            .read(&mut buf)
            .context("Przerwano pobieranie nowej wersji")?;
        if n == 0 {
            break;
        }
        if head.len() < 2 {
            head.extend_from_slice(&buf[..n.min(2)]);
        }
        file.write_all(&buf[..n])?;
        total += n as u64;
        progress.store(total, Ordering::Relaxed);
    }
    file.sync_all()?;
    drop(file);
    let check = || -> Result<()> {
        ensure!(
            total == asset.size,
            "Pobrano {} z {} bajtów nowej wersji",
            total,
            asset.size
        );
        ensure!(
            head.starts_with(b"MZ"),
            "Pobrany plik nie jest programem Windows"
        );
        Ok(())
    };
    if let Err(e) = check() {
        let _ = std::fs::remove_file(&new);
        return Err(e);
    }
    replace(exe, &new)
}

/// Windows allows renaming a running executable, so the swap needs no helper process.
fn replace(exe: &Path, new: &Path) -> Result<()> {
    let old = previous(exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old)
        .with_context(|| format!("Nie można zastąpić pliku {}", exe.display()))?;
    if let Err(e) = std::fs::rename(new, exe) {
        let _ = std::fs::rename(&old, exe);
        bail!("Nie można zastąpić pliku {}: {e}", exe.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(names: &[&str]) -> Release {
        Release {
            tag_name: "2.5.0".into(),
            name: None,
            body: None,
            html_url: String::new(),
            assets: names
                .iter()
                .map(|n| Asset {
                    name: (*n).into(),
                    size: 1,
                    browser_download_url: String::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("2.10.0", "2.9.3"));
        assert!(is_newer("v2.5", "2.4.9"));
        assert!(is_newer("3", "2.99.99"));
        assert!(!is_newer("2.4.0", "2.4.0"));
        assert!(!is_newer("2.4", "2.4.0"));
        assert!(!is_newer("2.3.9", "2.4.0"));
        assert!(!is_newer("2.5.0-beta", "2.5.0"));
        assert!(!is_newer("latest", "2.4.0"));
    }

    #[test]
    fn picks_the_matching_executable() {
        let r = release(&["notes.md", "TV4P_merge_roads.exe", "other.exe"]);
        assert_eq!(
            asset(&r, "tv4p_merge_roads.exe").unwrap().name,
            "TV4P_merge_roads.exe"
        );
        let r = release(&["notes.md", "studio.exe"]);
        assert_eq!(asset(&r, "renamed.exe").unwrap().name, "studio.exe");
        assert!(asset(&release(&["a.exe", "b.exe"]), "renamed.exe").is_none());
        assert!(asset(&release(&["notes.md"]), "tv4p_merge_roads.exe").is_none());
    }

    #[test]
    fn replacing_keeps_the_previous_executable_and_cleanup_removes_it() {
        let dir = std::env::temp_dir().join(format!("update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("app.exe");
        std::fs::write(&exe, b"MZold").unwrap();
        let new = sibling(&exe, ".new");
        std::fs::write(&new, b"MZnew").unwrap();
        replace(&exe, &new).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"MZnew");
        assert_eq!(std::fs::read(previous(&exe)).unwrap(), b"MZold");
        assert!(!new.exists());
        cleanup(&exe);
        assert!(!previous(&exe).exists());
        assert!(exe.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod network {
    use super::*;

    /// Talks to GitHub: `cargo test -- --ignored latest_release_downloads_and_replaces`.
    #[test]
    #[ignore]
    fn latest_release_downloads_and_replaces() {
        let release = latest().unwrap();
        println!("latest {} {}", release.tag_name, release.html_url);
        assert!(parse_version(&release.tag_name).is_some());
        let asset = asset(&release, "tv4p_merge_roads.exe")
            .expect("release has an exe")
            .clone();
        let dir = std::env::temp_dir().join(format!("update-net-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("tv4p_merge_roads.exe");
        std::fs::write(&exe, b"MZold").unwrap();
        let progress = AtomicU64::new(0);
        install(&asset, &exe, &progress).unwrap();
        assert_eq!(progress.load(Ordering::Relaxed), asset.size);
        assert_eq!(std::fs::metadata(&exe).unwrap().len(), asset.size);
        assert_eq!(std::fs::read(previous(&exe)).unwrap(), b"MZold");
        let out = std::process::Command::new(&exe)
            .arg("--version")
            .output()
            .unwrap();
        println!("installed: {}", String::from_utf8_lossy(&out.stdout).trim());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
