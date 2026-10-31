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
        any_saved(cwds.iter().map(|cwd| self.session_transcript_saved_for(cwd, session_id)))
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
        transcript_in_candidates(
            entries.map(|entry| entry.map(|entry| entry.path())),
            &prefix,
            &file_name,
        )
    }

    fn projects_dir(&self) -> PathBuf {
        self.config_dir.join("projects")
    }

    fn project_dir_name(&self, cwd: &str) -> OsString {
        self.project_dir_name.clone().unwrap_or_else(|| encode_project_path(cwd).into())
    }
}

fn any_saved(candidates: impl IntoIterator<Item = std::io::Result<bool>>) -> std::io::Result<bool> {
    let mut first_error = None;
    for candidate in candidates {
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

fn transcript_in_candidates(
    entries: impl IntoIterator<Item = std::io::Result<PathBuf>>,
    prefix: &str,
    file_name: &str,
) -> std::io::Result<bool> {
    any_saved(entries.into_iter().map(|entry| {
        let path = entry?;
        if path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| has_project_prefix(name, prefix))
        {
            is_regular_file(&path.join(file_name))
        } else {
            Ok(false)
        }
    }))
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
    encode_resolved_path(&resolve_cwd(cwd))
}

fn encode_resolved_path(path: &str) -> String {
    path.chars()
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
    normalize_resolved_path(&resolved.to_string_lossy())
}

fn normalize_resolved_path(resolved: &str) -> String {
    // Windows canonicalizes to a verbatim path; Node's realpath does not.
    if let Some(share) = resolved.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{share}")
    } else if let Some(local) = resolved.strip_prefix(r"\\?\") {
        local.to_owned()
    } else {
        resolved.to_owned()
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
    fn default_layout_requires_home_for_unset_or_empty_profile() {
        for override_dir in [None, Some(OsStr::new(""))] {
            let paths =
                ClaudePaths::from_sources(Some(Path::new("profile")), override_dir, None).unwrap();
            assert_eq!(paths.config_dir, Path::new("profile/.claude"));
            assert_eq!(paths.preferences, Path::new("profile/.claude.json"));
            assert_eq!(paths.credentials(), Path::new("profile/.claude/.credentials.json"));
            assert!(ClaudePaths::from_sources(None, override_dir, None).is_none());
        }
    }

    #[test]
    fn override_is_self_contained_and_does_not_require_a_home() {
        let paths =
            ClaudePaths::from_sources(None, Some(OsStr::new("isolated profile")), None).unwrap();
        assert_eq!(paths.config_dir, Path::new("isolated profile"));
        assert_eq!(paths.preferences, Path::new("isolated profile/.claude.json"));
        assert_eq!(paths.credentials(), Path::new("isolated profile/.credentials.json"));
    }
}

#[cfg(test)]
#[path = "claude_paths_audit_tests.rs"]
mod audit_tests;
