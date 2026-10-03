use anyhow::{Context, Result, bail};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// Write next to the destination, flush, then rename. Never truncate the destination first.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_stream(path, |f| {
        f.write_all(bytes)?;
        Ok(())
    })
}

pub fn atomic_stream(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> Result<()>,
) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .context("Brak nazwy pliku")?
        .to_string_lossy();
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let tmp = parent.join(format!(".{name}.{stamp}.tmp"));
    let result = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        write(&mut f)?;
        f.sync_all()?;
        drop(f);
        // Windows rename does not replace existing files. Keep a recovery copy until success.
        let backup = parent.join(format!(".{name}.{stamp}.bak"));
        let exists = path.exists();
        if exists {
            fs::rename(path, &backup)?;
        }
        if let Err(e) = fs::rename(&tmp, path) {
            if exists && let Err(recovery) = fs::rename(&backup, path) {
                bail!(
                    "Zapis: {e}; przywracanie: {recovery}. Kopia: {}",
                    backup.display()
                );
            }
            return Err(e.into());
        }
        if exists {
            let _ = fs::remove_file(backup);
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}
