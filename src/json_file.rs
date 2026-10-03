// SPDX-License-Identifier: Apache-2.0

//! Atomic JSON replacement and cooperative locking; callers own documents and mutation rules.

use std::io::Write;
use std::path::Path;

/// Cooperative document lock shared with the bridge's targeted JSON writer.
pub(crate) struct DocumentLock {
    path: std::path::PathBuf,
    file: Option<std::fs::File>,
}

impl Drop for DocumentLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(crate) fn lock(path: &Path) -> std::io::Result<DocumentLock> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut name = path.as_os_str().to_os_string();
    name.push(".claude-rs.lock");
    let path = std::path::PathBuf::from(name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path)?;
    Ok(DocumentLock { path, file: Some(file) })
}

pub(crate) fn replace(path: &Path, document: &serde_json::Value) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| std::io::Error::other("JSON path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let temp_path = parent.join(format!(".claude-rs.{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp_path)?;
    let result: std::io::Result<()> = (|| {
        serde_json::to_writer_pretty(&mut file, document).map_err(std::io::Error::other)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        if let Ok(metadata) = std::fs::metadata(path) {
            std::fs::set_permissions(&temp_path, metadata.permissions())?;
        }
        Ok(())
    })();
    drop(file);
    let result = result.and_then(|()| std::fs::rename(&temp_path, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}
