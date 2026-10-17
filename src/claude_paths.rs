// SPDX-License-Identifier: Apache-2.0

//! Claude-owned user paths, shared by settings, authentication, and status views.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

pub(crate) struct ClaudePaths {
    pub(crate) config_dir: PathBuf,
    pub(crate) preferences: PathBuf,
    project_dir_name: Option<OsString>,
}

impl ClaudePaths {
    pub(crate) fn resolve(home_override: Option<&Path>) -> Option<Self> {
        // Explicit homes are used by isolated app instances and tests. Do not
        // let the launching user's environment redirect those instances.
        if let Some(home) = home_override {
            return Self::from_sources(Some(home), None, None);
        }
        let home = dirs::home_dir();
        let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR");
        let project_dir_name = std::env::var_os("CLAUDE_CODE_PROJECT_DIR_NAME");
        Self::from_sources(home.as_deref(), config_dir.as_deref(), project_dir_name.as_deref())
    }

    fn from_sources(
        home: Option<&Path>,
        config_dir: Option<&OsStr>,
        project_dir_name: Option<&OsStr>,
    ) -> Option<Self> {
        if let Some(config_dir) = config_dir.filter(|value| !value.is_empty()) {
            let config_dir = PathBuf::from(config_dir);
            let preferences = config_dir.join(".claude.json");
            return Some(Self {
                config_dir,
                preferences,
                project_dir_name: project_dir_name
                    .filter(|value| is_valid_project_dir_name(value))
                    .map(OsStr::to_os_string),
            });
        }
        let home = home?;
        Some(Self {
            config_dir: home.join(".claude"),
            preferences: home.join(".claude.json"),
            project_dir_name: None,
        })
    }

    pub(crate) fn credentials(&self) -> PathBuf {
        self.config_dir.join(".credentials.json")
    }

    pub(crate) fn default_memory_file(&self, cwd: &str) -> PathBuf {
        self.projects_dir().join(self.project_dir_name(cwd)).join("memory").join("MEMORY.md")
    }

    /// Whether `<session_id>.jsonl`, the transcript the SDK's `renameSession()`
    /// appends to, exists as a non-empty regular file in the project directory
    /// of any of `cwds`. An error other than a missing path is returned rather
    /// than read as `false`, unless another directory has the transcript.
    pub(crate) fn session_transcript_saved(
        &self,
        cwds: &[&str],
        session_id: &str,
    ) -> std::io::Result<bool> {
        let mut first_error = None;
        for cwd in cwds {
            match self.session_transcript_saved_for(cwd, session_id) {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(false), Err)
    }

    fn session_transcript_saved_for(&self, cwd: &str, session_id: &str) -> std::io::Result<bool> {
        let file_name = format!("{session_id}.jsonl");
        let project_name = self.project_dir_name(cwd);
        let projects = self.projects_dir();
        let Some(prefix) = shortened_project_prefix(&project_name) else {
            return is_regular_file(&projects.join(project_name).join(file_name));
        };
        // Claude Code shortens a long name with a hash suffix that may differ from
        // the SDK's. This accepts any folder with the shortened prefix, a superset
        // of the folders the SDK's lookup accepts; if the transcript is in a folder
        // the SDK rejects, the SDK's rename error reports it. An unreadable folder
        // does not hide a match in another one.
        let entries = match std::fs::read_dir(&projects) {
            Err(error) if is_missing(&error) => return Ok(false),
            entries => entries?,
        };
        let mut first_error = None;
        for entry in entries {
            let candidate = entry.and_then(|entry| {
                let matches = entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| has_project_prefix(name, &prefix));
                if matches { is_regular_file(&entry.path().join(&file_name)) } else { Ok(false) }
            });
            match candidate {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(false), Err)
    }

    fn projects_dir(&self) -> PathBuf {
        self.config_dir.join("projects")
    }

    fn project_dir_name(&self, cwd: &str) -> OsString {
        self.project_dir_name.clone().unwrap_or_else(|| encode_project_path(cwd).into())
    }
}

/// Longest project directory name the SDK uses unshortened.
const MAX_PROJECT_DIR_NAME: usize = 200;

/// The prefix shared by every shortened form of an encoded name that is too long.
fn shortened_project_prefix(name: &OsStr) -> Option<String> {
    let name = name.to_str()?;
    // Encoded names are ASCII, so a byte index is a character index.
    (name.len() > MAX_PROJECT_DIR_NAME)
        .then(|| format!("{}-", name.get(..MAX_PROJECT_DIR_NAME).unwrap_or(name)))
}

/// Windows paths are case-insensitive, so the SDK compares the prefix in lower case there.
fn has_project_prefix(name: &str, prefix: &str) -> bool {
    if cfg!(windows) {
        name.get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    } else {
        name.starts_with(prefix)
    }
}

/// The SDK skips an empty transcript as if it were missing.
fn is_regular_file(path: &Path) -> std::io::Result<bool> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file() && metadata.len() > 0),
        Err(error) if is_missing(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

/// The SDK treats `ENOENT` and `ENOTDIR` alike: the session is not there.
fn is_missing(error: &std::io::Error) -> bool {
    matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory)
}

/// The Agent SDK names a project directory after the resolved working
/// directory, replacing every UTF-16 unit outside `[A-Za-z0-9]` with `-`.
fn encode_project_path(cwd: &str) -> String {
    resolve_cwd(cwd)
        .chars()
        .flat_map(|ch| {
            if ch.is_ascii_alphanumeric() {
                std::iter::repeat_n(ch, 1)
            } else {
                std::iter::repeat_n('-', ch.len_utf16())
            }
        })
        .collect()
}

/// The SDK resolves symlinks first (`/tmp` is `/private/tmp` on macOS) and
/// keeps the path as given when it cannot.
fn resolve_cwd(cwd: &str) -> String {
    let Ok(resolved) = std::fs::canonicalize(cwd) else {
        return cwd.to_owned();
    };
    let resolved = resolved.to_string_lossy();
    // Windows canonicalizes to a verbatim path; Node's realpath does not.
    if let Some(share) = resolved.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{share}")
    } else if let Some(local) = resolved.strip_prefix(r"\\?\") {
        local.to_owned()
    } else {
        resolved.into_owned()
    }
}

/// The SDK ignores a project directory override that is not a plain name or
/// that names a Windows device.
fn is_valid_project_dir_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    let device = matches!(lower.as_str(), "con" | "prn" | "aux" | "nul")
        || (lower.len() == 4
            && (lower.starts_with("com") || lower.starts_with("lpt"))
            && lower.as_bytes().get(3).is_some_and(u8::is_ascii_digit));
    (1..=64).contains(&name.len())
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        && !device
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_preferences_live_outside_config_directory() {
        let paths = ClaudePaths::from_sources(Some(Path::new("profile")), None, None).unwrap();
        assert_eq!(paths.config_dir, Path::new("profile/.claude"));
        assert_eq!(paths.preferences, Path::new("profile/.claude.json"));
        assert_eq!(paths.credentials(), Path::new("profile/.claude/.credentials.json"));
    }

    #[test]
    fn override_is_self_contained_and_does_not_require_a_home() {
        let paths =
            ClaudePaths::from_sources(None, Some(OsStr::new("isolated profile")), None).unwrap();
        assert_eq!(paths.config_dir, Path::new("isolated profile"));
        assert_eq!(paths.preferences, Path::new("isolated profile/.claude.json"));
        assert_eq!(paths.credentials(), Path::new("isolated profile/.credentials.json"));
    }

    #[test]
    fn empty_override_uses_the_default_layout() {
        let paths =
            ClaudePaths::from_sources(Some(Path::new("profile")), Some(OsStr::new("")), None)
                .unwrap();
        assert_eq!(paths.preferences, Path::new("profile/.claude.json"));
        assert!(ClaudePaths::from_sources(None, None, None).is_none());
    }

    #[test]
    fn project_names_are_encoded_for_both_platforms() {
        // Claude Code stores `/Users/me` under `-Users-me`: the leading separator stays.
        assert_eq!(encode_project_path("/home/user/project"), "-home-user-project");
        assert_eq!(encode_project_path(r"C:\Users\User\project"), "C--Users-User-project");
    }

    #[test]
    fn every_non_alphanumeric_utf16_unit_becomes_a_dash() {
        assert_eq!(encode_project_path("/srv/my.app_v2 (copy)"), "-srv-my-app-v2--copy-");
        assert_eq!(encode_project_path("/a/\u{e9}"), "-a--");
        // A character outside the BMP is two UTF-16 units in the SDK's regex.
        assert_eq!(encode_project_path("/a/\u{1f980}"), "-a---");
    }

    #[cfg(unix)]
    #[test]
    fn project_names_follow_symlinks_like_the_sdk() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let resolved = std::fs::canonicalize(&target).unwrap();
        let expected =
            resolved.to_str().unwrap().replace(|ch: char| !ch.is_ascii_alphanumeric(), "-");
        assert_eq!(encode_project_path(link.to_str().unwrap()), expected);
    }

    fn profile_paths(profile: &Path) -> ClaudePaths {
        ClaudePaths::from_sources(None, Some(profile.as_os_str()), None).unwrap()
    }

    #[test]
    fn a_session_is_saved_once_its_transcript_is_a_regular_file() {
        let profile = tempfile::tempdir().unwrap();
        let paths = profile_paths(profile.path());
        // `/work/app` does not exist, so it is encoded as given.
        let project = profile.path().join("projects/-work-app");
        assert!(!paths.session_transcript_saved(&["/work/app"], "s1").unwrap());

        std::fs::create_dir_all(project.join("s1.jsonl")).unwrap();
        assert!(!paths.session_transcript_saved(&["/work/app"], "s1").unwrap());

        std::fs::write(project.join("s2.jsonl"), "").unwrap();
        assert!(!paths.session_transcript_saved(&["/work/app"], "s2").unwrap(), "empty");
        std::fs::write(project.join("s2.jsonl"), "{}\n").unwrap();
        assert!(paths.session_transcript_saved(&["/work/app"], "s2").unwrap());
        assert!(!paths.session_transcript_saved(&["/work/other"], "s2").unwrap());
        // A session resumed from a sibling worktree keeps its own folder.
        assert!(paths.session_transcript_saved(&["/work/other", "/work/app"], "s2").unwrap());
    }

    #[test]
    fn a_long_project_name_matches_any_shortened_directory() {
        let profile = tempfile::tempdir().unwrap();
        let paths = profile_paths(profile.path());
        let cwd = format!("/{}", "a".repeat(250));
        assert!(!paths.session_transcript_saved(&[&cwd], "s1").unwrap());
        let shortened = format!("-{}-1x2y3z", "a".repeat(199));
        let project = profile.path().join("projects").join(shortened);
        std::fs::create_dir_all(&project).unwrap();
        assert!(!paths.session_transcript_saved(&[&cwd], "s1").unwrap());
        std::fs::write(project.join("s1.jsonl"), "{}\n").unwrap();
        assert!(paths.session_transcript_saved(&[&cwd], "s1").unwrap());
    }

    /// Makes `dir` unreadable. Returns `false`, with the folder restored, when
    /// the lock has no effect (root ignores it), so the caller can skip.
    #[cfg(unix)]
    fn lock_dir(dir: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(dir).is_ok() {
            unlock_dir(dir);
            return false;
        }
        true
    }

    #[cfg(unix)]
    fn unlock_dir(dir: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_project_directory_is_an_error_not_a_missing_session() {
        let profile = tempfile::tempdir().unwrap();
        let paths = profile_paths(profile.path());
        let project = profile.path().join("projects/-work-app");
        std::fs::create_dir_all(&project).unwrap();
        if !lock_dir(&project) {
            eprintln!("skipped: permissions do not restrict this user");
            return;
        }
        let result = paths.session_transcript_saved(&["/work/app"], "s1");
        let other = profile.path().join("projects/-work-other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("s1.jsonl"), "{}\n").unwrap();
        let found_elsewhere = paths.session_transcript_saved(&["/work/app", "/work/other"], "s1");
        unlock_dir(&project);
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
        assert!(found_elsewhere.unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_sibling_folder_does_not_hide_a_long_name_match() {
        let profile = tempfile::tempdir().unwrap();
        let paths = profile_paths(profile.path());
        let cwd = format!("/{}", "a".repeat(250));
        let prefix = format!("-{}-", "a".repeat(199));
        let locked = profile.path().join("projects").join(format!("{prefix}locked"));
        std::fs::create_dir_all(&locked).unwrap();
        if !lock_dir(&locked) {
            eprintln!("skipped: permissions do not restrict this user");
            return;
        }
        let only_locked = paths.session_transcript_saved(&[&cwd], "s1");
        let saved = profile.path().join("projects").join(format!("{prefix}saved"));
        std::fs::create_dir_all(&saved).unwrap();
        std::fs::write(saved.join("s1.jsonl"), "{}\n").unwrap();
        let with_match = paths.session_transcript_saved(&[&cwd], "s1");
        unlock_dir(&locked);
        assert_eq!(only_locked.unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
        assert!(with_match.unwrap());
    }

    #[test]
    fn project_override_must_be_a_plain_name() {
        for rejected in ["../escape", "con", "COM1", "lpt9", &"a".repeat(65)] {
            let paths = ClaudePaths::from_sources(
                None,
                Some(OsStr::new("profile")),
                Some(OsStr::new(rejected)),
            )
            .unwrap();
            assert_eq!(
                paths.default_memory_file("/any/path"),
                Path::new("profile/projects/-any-path/memory/MEMORY.md"),
                "{rejected}"
            );
        }
        let accepted =
            ClaudePaths::from_sources(None, Some(OsStr::new("profile")), Some(OsStr::new("com10")))
                .unwrap();
        assert_eq!(
            accepted.default_memory_file("/any/path"),
            Path::new("profile/projects/com10/memory/MEMORY.md")
        );
    }

    #[test]
    fn project_override_applies_only_to_an_overridden_config_root() {
        let paths =
            ClaudePaths::from_sources(None, Some(OsStr::new("profile")), Some(OsStr::new("work")))
                .unwrap();
        assert_eq!(
            paths.default_memory_file("/any/path"),
            Path::new("profile/projects/work/memory/MEMORY.md")
        );
        let default =
            ClaudePaths::from_sources(Some(Path::new("home")), None, Some(OsStr::new("work")))
                .unwrap();
        assert_eq!(
            default.default_memory_file("/any/path"),
            Path::new("home/.claude/projects/-any-path/memory/MEMORY.md")
        );
    }
}
