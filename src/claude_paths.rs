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
                    .filter(|value| !value.is_empty())
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
        let project_name =
            self.project_dir_name.clone().unwrap_or_else(|| encode_project_path(cwd).into());
        self.config_dir.join("projects").join(project_name).join("memory").join("MEMORY.md")
    }
}

fn encode_project_path(cwd: &str) -> String {
    cwd.replace(['/', '\\'], "-").replace(':', "-").trim_start_matches('-').to_owned()
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
        assert_eq!(encode_project_path("/home/user/project"), "home-user-project");
        assert_eq!(encode_project_path(r"C:\Users\User\project"), "C--Users-User-project");
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
            Path::new("home/.claude/projects/any-path/memory/MEMORY.md")
        );
    }
}
