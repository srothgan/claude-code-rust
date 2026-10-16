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
    replace_with_check(path, document, || Ok(true)).map(|_| ())
}

/// Detect edits by non-cooperating writers immediately before atomic replacement.
pub(crate) fn replace_if_unchanged(
    path: &Path,
    document: &serde_json::Value,
    expected: Option<&[u8]>,
) -> std::io::Result<bool> {
    replace_with_check(path, document, || {
        let current = match std::fs::read(path) {
            Ok(raw) => Some(raw),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        Ok(current.as_deref() == expected)
    })
}

fn replace_with_check(
    path: &Path,
    document: &serde_json::Value,
    check: impl FnOnce() -> std::io::Result<bool>,
) -> std::io::Result<bool> {
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
    let result = result.and_then(|()| {
        if !check()? {
            return Ok(false);
        }
        std::fs::rename(&temp_path, path)?;
        Ok(true)
    });
    if !matches!(result, Ok(true)) {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_changed_or_newly_created_document_is_never_replaced_by_a_stale_write() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("settings.json");
        let original = b"{\"original\":true}";
        let external = b"{\"external\":\"preserve byte for byte\"}\n";
        std::fs::write(&path, external).expect("external editor");
        for expected in [Some(original.as_slice()), None] {
            assert!(
                !replace_if_unchanged(&path, &json!({"overwrite":true}), expected).expect("check")
            );
            assert_eq!(std::fs::read(&path).expect("read"), external);
        }
        assert_eq!(
            std::fs::read_dir(directory.path()).expect("list").count(),
            1,
            "no temporary files remain"
        );
    }

    #[test]
    fn matching_replacements_and_new_files_are_committed_as_complete_json() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("settings.json");
        assert!(replace_if_unchanged(&path, &json!({"first":1}), None).expect("create"));
        let raw = std::fs::read(&path).expect("first");
        assert!(replace_if_unchanged(&path, &json!({"second":2}), Some(&raw)).expect("replace"));
        let saved = std::fs::read_to_string(&path).expect("read");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&saved).expect("JSON"),
            json!({"second":2})
        );
        assert!(saved.ends_with('\n'));
        assert_eq!(std::fs::read_dir(directory.path()).expect("list").count(), 1);
    }
}
