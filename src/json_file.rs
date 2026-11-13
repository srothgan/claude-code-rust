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
        #[cfg(not(test))]
        let mut writer = &mut file;
        #[cfg(test)]
        let mut writer = test_io::Writer::new(path, &mut file);
        serde_json::to_writer_pretty(&mut writer, document).map_err(std::io::Error::other)?;
        writer.write_all(b"\n")?;
        #[cfg(test)]
        test_io::fail_at(path, test_io::Failure::Sync)?;
        file.sync_all()?;
        if let Ok(metadata) = std::fs::metadata(path) {
            std::fs::set_permissions(&temp_path, metadata.permissions())?;
        }
        Ok(())
    })();
    drop(file);
    let result = result.and_then(|()| {
        #[cfg(test)]
        if let Some(checkpoint) = BEFORE_REPLACEMENT.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_some_and(|(target, _)| target == path) {
                slot.take().map(|(_, checkpoint)| checkpoint)
            } else {
                None
            }
        }) {
            checkpoint();
        }
        if !check()? {
            return Ok(false);
        }
        #[cfg(test)]
        test_io::fail_at(path, test_io::Failure::Rename)?;
        std::fs::rename(&temp_path, path)?;
        Ok(true)
    });
    if !matches!(result, Ok(true)) {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

#[cfg(test)]
type ReplacementCheckpoint = (std::path::PathBuf, Box<dyn FnOnce()>);

#[cfg(test)]
thread_local! {
    static BEFORE_REPLACEMENT: std::cell::RefCell<Option<ReplacementCheckpoint>> = const { std::cell::RefCell::new(None) };
}

/// Run one path-scoped checkpoint after serialization and before the real comparison.
#[cfg(test)]
pub(crate) fn with_before_replacement<T>(
    path: &Path,
    checkpoint: impl FnOnce() + 'static,
    operation: impl FnOnce() -> T,
) -> T {
    struct Reset(Option<ReplacementCheckpoint>);
    impl Drop for Reset {
        fn drop(&mut self) {
            BEFORE_REPLACEMENT.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _reset = Reset(
        BEFORE_REPLACEMENT.with(|slot| slot.replace(Some((path.to_owned(), Box::new(checkpoint))))),
    );
    operation()
}

#[cfg(test)]
pub(crate) fn with_replacement_failure(path: &Path, operation: impl FnOnce()) {
    test_io::with_failure(path, test_io::Failure::Rename, operation);
}

#[cfg(test)]
mod test_io {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(super) enum Failure {
        PartialWrite,
        Sync,
        Rename,
    }

    thread_local! {
        static FAILURE: std::cell::RefCell<Option<(std::path::PathBuf, Failure)>> = const { std::cell::RefCell::new(None) };
    }

    fn enabled(path: &Path, point: Failure) -> bool {
        FAILURE.with(|slot| {
            slot.borrow().as_ref().is_some_and(|(target, stage)| target == path && *stage == point)
        })
    }

    pub(super) fn fail_at(path: &Path, point: Failure) -> std::io::Result<()> {
        if enabled(path, point) {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                match point {
                    Failure::PartialWrite => "injected partial staged write denial",
                    Failure::Sync => "injected staged sync denial",
                    Failure::Rename => "injected replacement denial",
                },
            ))
        } else {
            Ok(())
        }
    }

    pub(super) fn with_failure<T>(path: &Path, point: Failure, operation: impl FnOnce() -> T) -> T {
        struct Reset(Option<(std::path::PathBuf, Failure)>);
        impl Drop for Reset {
            fn drop(&mut self) {
                FAILURE.with(|slot| *slot.borrow_mut() = self.0.take());
            }
        }
        let _reset = Reset(FAILURE.with(|slot| slot.replace(Some((path.to_owned(), point)))));
        operation()
    }

    // All bytes still pass through the real file. The fault returns a short write before
    // refusing the next write, so serialization fails with a partially written staging file.
    pub(super) struct Writer<'a> {
        file: &'a mut std::fs::File,
        target: &'a Path,
        remaining: Option<usize>,
    }

    impl<'a> Writer<'a> {
        pub(super) fn new(target: &'a Path, file: &'a mut std::fs::File) -> Self {
            Self { file, target, remaining: enabled(target, Failure::PartialWrite).then_some(8) }
        }
    }

    impl Write for Writer<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if let Some(remaining) = &mut self.remaining {
                if *remaining == 0 {
                    fail_at(self.target, Failure::PartialWrite)?;
                }
                let written = self.file.write(&bytes[..bytes.len().min(*remaining)])?;
                *remaining -= written;
                Ok(written)
            } else {
                self.file.write(bytes)
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.file.flush()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SIBLING_BYTES: &[u8] = b"unrelated\0binary\xff\n";
    const SIBLING_JSON: &[u8] = b"{ \"independent\" : [3, 7] }\n";

    struct Fixture {
        directory: tempfile::TempDir,
        path: std::path::PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().expect("isolated directory");
            let path = directory.path().join("settings.json");
            std::fs::write(directory.path().join("neighbor.bin"), SIBLING_BYTES).unwrap();
            std::fs::write(directory.path().join("unrelated.json"), SIBLING_JSON).unwrap();
            Self { directory, path }
        }

        fn assert_neighbors_and_no_staging_files(&self, target_exists: bool) {
            use std::collections::BTreeSet;
            assert_eq!(
                std::fs::read(self.directory.path().join("neighbor.bin")).unwrap(),
                SIBLING_BYTES
            );
            assert_eq!(
                std::fs::read(self.directory.path().join("unrelated.json")).unwrap(),
                SIBLING_JSON
            );
            let mut expected = BTreeSet::from([
                std::ffi::OsString::from("neighbor.bin"),
                std::ffi::OsString::from("unrelated.json"),
            ]);
            if target_exists {
                expected.insert("settings.json".into());
            }
            let actual = std::fs::read_dir(self.directory.path())
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<BTreeSet<_>>();
            assert_eq!(actual, expected, "no staging artifacts may remain");
        }
    }

    fn assert_document(path: &Path, expected: &serde_json::Value) -> Vec<u8> {
        let bytes = std::fs::read(path).expect("saved document");
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(), *expected);
        assert_eq!(bytes.last(), Some(&b'\n'), "complete JSON ends with a newline");
        bytes
    }

    #[test]
    fn creation_and_replacement_each_commit_the_intended_document_and_preserve_neighbors() {
        let fixture = Fixture::new();
        let first = json!({"obsolete":{"title":"初回 🦀","values":[true,7,null]},"first":1});
        assert!(replace_if_unchanged(&fixture.path, &first, None).expect("create"));
        let revision = assert_document(&fixture.path, &first);
        fixture.assert_neighbors_and_no_staging_files(true);

        let second = json!({"replacement":{"title":"Änderung 🌿","nested":[{"enabled":false}]}});
        assert!(replace_if_unchanged(&fixture.path, &second, Some(&revision)).expect("replace"));
        assert_document(&fixture.path, &second);
        fixture.assert_neighbors_and_no_staging_files(true);
    }

    #[test]
    fn failed_staging_comparison_and_rename_preserve_documents_and_neighbors_then_retry() {
        use test_io::Failure;
        for existing in [false, true] {
            for failure in
                [Some(Failure::PartialWrite), Some(Failure::Sync), None, Some(Failure::Rename)]
            {
                let fixture = Fixture::new();
                let original = b"{ \"original\" : {\"unchanged\":true} }\n";
                let revision = existing.then_some(original.as_slice());
                if existing {
                    std::fs::write(&fixture.path, original).unwrap();
                }
                let intended = json!({"updated":{"question":"保存しますか?","answers":[1,2,3]}});
                let result = if let Some(point) = failure {
                    test_io::with_failure(&fixture.path, point, || {
                        replace_if_unchanged(&fixture.path, &intended, revision)
                    })
                } else {
                    // Exercise the actual staging/check/cleanup owner with a read-check error.
                    replace_with_check(&fixture.path, &intended, || {
                        Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "comparison read failed",
                        ))
                    })
                };
                let error = result.expect_err("injected I/O failure must be returned");
                let expected_cause = match failure {
                    Some(Failure::PartialWrite) => "partial staged write denial",
                    Some(Failure::Sync) => "staged sync denial",
                    Some(Failure::Rename) => "replacement denial",
                    None => "comparison read failed",
                };
                assert!(error.to_string().contains(expected_cause), "{error}");
                if existing {
                    assert_eq!(std::fs::read(&fixture.path).unwrap(), original);
                } else {
                    assert_eq!(
                        std::fs::read(&fixture.path).unwrap_err().kind(),
                        std::io::ErrorKind::NotFound
                    );
                }
                fixture.assert_neighbors_and_no_staging_files(existing);
                assert!(replace_if_unchanged(&fixture.path, &intended, revision).expect("retry"));
                assert_document(&fixture.path, &intended);
                fixture.assert_neighbors_and_no_staging_files(true);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn new_json_staging_and_committed_file_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        const CHILD: &str = "CLAUDE_RS_JSON_MODE_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            // Set umask only in an isolated child; other tests retain their process settings.
            let output = std::process::Command::new("sh")
                .args(["-c", "umask 000; exec \"$@\"", "json-permission-test"])
                .arg(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "json_file::tests::new_json_staging_and_committed_file_are_owner_only",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .expect("isolated permission test");
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
                "child must execute its test"
            );
            return;
        }
        let fixture = Fixture::new();
        let intended = json!({"private":{"token":"fixture secret","enabled":true}});
        let directory = fixture.directory.path().to_owned();
        let expected = intended.clone();
        with_before_replacement(
            &fixture.path,
            move || {
                let staged = std::fs::read_dir(&directory)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .filter(|path| {
                        path.file_name()
                            .is_some_and(|name| name != "neighbor.bin" && name != "unrelated.json")
                    })
                    .collect::<Vec<_>>();
                let [staged] = staged.as_slice() else {
                    panic!("exactly one staging file must exist")
                };
                assert_eq!(std::fs::metadata(staged).unwrap().permissions().mode() & 0o777, 0o600);
                assert_document(staged, &expected);
            },
            || replace(&fixture.path, &intended),
        )
        .expect("create private document");
        assert_eq!(std::fs::metadata(&fixture.path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_document(&fixture.path, &intended);
        fixture.assert_neighbors_and_no_staging_files(true);
    }

    #[cfg(unix)]
    #[test]
    fn replacing_json_preserves_the_existing_access_mode_and_complete_new_content() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let original = b"{\"old\":true}\n";
        std::fs::write(&fixture.path, original).unwrap();
        std::fs::set_permissions(&fixture.path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let intended = json!({"replacement":{"label":"新的設定","items":[false,9]}});
        assert!(replace_if_unchanged(&fixture.path, &intended, Some(original)).expect("replace"));
        assert_eq!(std::fs::metadata(&fixture.path).unwrap().permissions().mode() & 0o777, 0o640);
        assert_document(&fixture.path, &intended);
        fixture.assert_neighbors_and_no_staging_files(true);
    }

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
}
