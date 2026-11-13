// SPDX-License-Identifier: Apache-2.0
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

async fn in_child(name: &str) -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("CLAUDE_RS_AUDIT_ROOT") {
        return Some(root.into());
    }
    let root = tempfile::tempdir().unwrap();
    let mut command = crate::diagnostics_test_support::command(
        &format!("claude_paths::audit_tests::{name}"),
        root.path(),
        "paths",
    );
    command
        .current_dir(root.path())
        .env("CLAUDE_CONFIG_DIR", root.path().join("ambient"))
        .env("CLAUDE_CODE_PROJECT_DIR_NAME", "ambient-project");
    let output = crate::diagnostics_test_support::run(command).await;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    None
}

#[tokio::test]
async fn explicit_home_wins_over_environment_while_ordinary_resolution_uses_it() {
    let Some(root) =
        in_child("explicit_home_wins_over_environment_while_ordinary_resolution_uses_it").await
    else {
        return;
    };
    let home = root.join("explicit-home");
    let profile = root.join("ambient");
    for directory in [&home, &profile] {
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(directory.join("sentinel"), b"preserve").unwrap();
    }
    assert!(!root.join("project-a").exists());
    let explicit = ClaudePaths::resolve(Some(&home)).unwrap();
    assert_eq!(explicit.config_dir, home.join(".claude"));
    assert_eq!(explicit.preferences, home.join(".claude.json"));
    assert_eq!(explicit.credentials(), home.join(".claude/.credentials.json"));
    assert_eq!(
        explicit.default_memory_file("project-a"),
        home.join(".claude/projects/project-a/memory/MEMORY.md")
    );
    let ambient = ClaudePaths::resolve(None).unwrap();
    assert_eq!(ambient.config_dir, profile);
    assert_eq!(ambient.preferences, profile.join(".claude.json"));
    assert_eq!(ambient.credentials(), profile.join(".credentials.json"));
    assert_eq!(
        ambient.default_memory_file("project-a"),
        profile.join("projects/ambient-project/memory/MEMORY.md")
    );
    for directory in [&home, &profile] {
        assert_eq!(std::fs::read(directory.join("sentinel")).unwrap(), b"preserve");
        assert_eq!(std::fs::read_dir(directory).unwrap().count(), 1, "resolution is read-only");
    }
}

#[test]
fn resolved_project_names_preserve_ascii_and_replace_each_non_ascii_utf16_unit() {
    for (input, expected) in [
        ("/home/user/project", "-home-user-project"),
        (r"C:\Users\User\project", "C--Users-User-project"),
        ("/srv/my.app_v2 (copy)", "-srv-my-app-v2--copy-"),
        ("/a/\u{e9}", "-a--"),
        ("/a/\u{1f980}", "-a---"),
        ("Az19-_./", "Az19----"),
    ] {
        assert_eq!(encode_resolved_path(input), expected);
    }
}

#[test]
fn resolved_paths_normalize_windows_prefixes_and_keep_unresolved_input() {
    for (input, expected) in [
        (r"\\?\C:\work\project", r"C:\work\project"),
        (r"\\?\UNC\server\share\project", r"\\server\share\project"),
        ("/work/project", "/work/project"),
    ] {
        assert_eq!(normalize_resolved_path(input), expected);
    }
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing/child");
    assert_eq!(resolve_cwd(missing.to_str().unwrap()), missing.to_str().unwrap());
}

#[cfg(unix)]
#[test]
fn filesystem_resolution_follows_the_actual_symlink_target() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    let link = root.path().join("link");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert_eq!(
        resolve_cwd(link.to_str().unwrap()),
        std::fs::canonicalize(&target).unwrap().to_str().unwrap()
    );
}

#[tokio::test]
async fn transcript_lookup_requires_the_selected_nonempty_regular_session_file() {
    let Some(root) =
        in_child("transcript_lookup_requires_the_selected_nonempty_regular_session_file").await
    else {
        return;
    };
    let profile = root.join("profile");
    let paths = ClaudePaths::from_sources(None, Some(profile.as_os_str()), None).unwrap();
    assert!(!root.join("project-a").exists());
    assert!(!root.join("project-b").exists());
    assert!(!paths.session_transcript_saved(&["project-a"], "chosen").unwrap());
    let project = profile.join("projects/project-a");
    std::fs::create_dir_all(project.join("chosen.jsonl")).unwrap();
    assert!(!paths.session_transcript_saved(&["project-a"], "chosen").unwrap());
    std::fs::remove_dir(project.join("chosen.jsonl")).unwrap();
    std::fs::write(project.join("other.jsonl"), b"wrong session").unwrap();
    std::fs::write(project.join("chosen.jsonl"), b"").unwrap();
    assert!(!paths.session_transcript_saved(&["project-a"], "chosen").unwrap());
    std::fs::write(project.join("chosen.jsonl"), b"saved bytes\n").unwrap();
    assert!(!paths.session_transcript_saved(&["project-b"], "chosen").unwrap());
    assert!(paths.session_transcript_saved(&["project-b", "project-a"], "chosen").unwrap());
    assert_eq!(std::fs::read(project.join("chosen.jsonl")).unwrap(), b"saved bytes\n");
    assert_eq!(std::fs::read(project.join("other.jsonl")).unwrap(), b"wrong session");
    std::fs::write(profile.join("projects/project-b"), b"not a directory").unwrap();
    assert!(!paths.session_transcript_saved(&["project-b"], "chosen").unwrap());
    let blocked = root.join("blocked-profile");
    std::fs::write(&blocked, b"preserve").unwrap();
    let paths = ClaudePaths::from_sources(None, Some(blocked.as_os_str()), None).unwrap();
    assert!(!paths.session_transcript_saved(&["project-a"], "chosen").unwrap());
    assert_eq!(std::fs::read(blocked).unwrap(), b"preserve");
}

#[tokio::test]
async fn exact_and_shortened_project_names_select_only_the_requested_session() {
    let Some(root) =
        in_child("exact_and_shortened_project_names_select_only_the_requested_session").await
    else {
        return;
    };
    let profile = root.join("profile");
    let paths = ClaudePaths::from_sources(None, Some(profile.as_os_str()), None).unwrap();
    let exact = "A".repeat(200);
    let long = "A".repeat(201);
    assert!(!paths.session_transcript_saved(&[&long], "chosen").unwrap());
    let projects = profile.join("projects");
    for (name, session, bytes) in [
        (format!("{}-unrelated", "B".repeat(200)), "chosen", "wrong prefix"),
        (format!("{exact}-other"), "other", "wrong session"),
        (format!("{exact}-empty"), "chosen", ""),
    ] {
        let path = projects.join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join(format!("{session}.jsonl")), bytes).unwrap();
    }
    std::fs::create_dir_all(projects.join(format!("{exact}-directory/chosen.jsonl"))).unwrap();
    assert!(!paths.session_transcript_saved(&[&long], "chosen").unwrap());
    let saved = projects.join(format!("{exact}-saved"));
    std::fs::create_dir(&saved).unwrap();
    std::fs::write(saved.join("chosen.jsonl"), b"long-name transcript").unwrap();
    assert!(paths.session_transcript_saved(&[&long], "chosen").unwrap());
    assert!(
        !paths.session_transcript_saved(&[&exact], "chosen").unwrap(),
        "200-byte names require an exact directory"
    );
    std::fs::create_dir(projects.join(&exact)).unwrap();
    std::fs::write(projects.join(&exact).join("chosen.jsonl"), b"exact transcript").unwrap();
    assert!(paths.session_transcript_saved(&[&exact], "chosen").unwrap());
    assert_eq!(
        paths.session_transcript_saved(&[&long.to_ascii_lowercase()], "chosen").unwrap(),
        cfg!(windows)
    );
    assert_eq!(std::fs::read(saved.join("chosen.jsonl")).unwrap(), b"long-name transcript");
}

#[tokio::test]
async fn project_override_admission_selects_both_memory_and_transcript_owners() {
    let Some(root) =
        in_child("project_override_admission_selects_both_memory_and_transcript_owners").await
    else {
        return;
    };
    let max = "a".repeat(64);
    let too_long = "a".repeat(65);
    for (name, accepted) in [
        ("work_2", true),
        ("com10", true),
        (max.as_str(), true),
        (too_long.as_str(), false),
        ("", false),
        ("../escape", false),
        ("a/b", false),
        (r"a\b", false),
        ("a b", false),
        ("con", false),
        ("COM1", false),
        ("lPt9", false),
        ("nul", false),
        ("AUX", false),
    ] {
        for selected_profile in [None, Some(OsStr::new("")), Some(OsStr::new("profile"))] {
            let fixture = tempfile::tempdir_in(&root).unwrap();
            let home = fixture.path().join("home");
            let profile = fixture.path().join("profile");
            let override_dir = selected_profile
                .map(|p| if p.is_empty() { OsStr::new("") } else { profile.as_os_str() });
            let paths =
                ClaudePaths::from_sources(Some(&home), override_dir, Some(OsStr::new(name)))
                    .unwrap();
            let selected = if selected_profile.is_some_and(|p| !p.is_empty()) {
                &profile
            } else {
                &home.join(".claude")
            };
            let project = if accepted && selected_profile.is_some_and(|p| !p.is_empty()) {
                name
            } else {
                "project-a"
            };
            let expected = selected.join("projects").join(project);
            assert_eq!(paths.default_memory_file("project-a"), expected.join("memory/MEMORY.md"));
            std::fs::create_dir_all(&expected).unwrap();
            std::fs::write(expected.join("other.jsonl"), b"neighbor").unwrap();
            assert!(!paths.session_transcript_saved(&["project-a"], "chosen").unwrap());
            std::fs::write(expected.join("chosen.jsonl"), b"selected").unwrap();
            assert!(paths.session_transcript_saved(&["project-a"], "chosen").unwrap());
            assert_eq!(std::fs::read(expected.join("chosen.jsonl")).unwrap(), b"selected");
        }
    }
}

#[cfg(unix)]
struct PermissionGuard(PathBuf, std::fs::Permissions);
#[cfg(unix)]
impl PermissionGuard {
    fn lock(path: &Path) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let old = std::fs::metadata(path).unwrap().permissions();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
        Self(path.into(), old)
    }
}
#[cfg(unix)]
impl Drop for PermissionGuard {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.0, self.1.clone());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn traversal_errors_survive_missing_candidates_but_yield_to_a_later_saved_session() {
    let Some(root) =
        in_child("traversal_errors_survive_missing_candidates_but_yield_to_a_later_saved_session")
            .await
    else {
        return;
    };
    let profile = root.join("profile");
    let paths = ClaudePaths::from_sources(None, Some(profile.as_os_str()), None).unwrap();
    let locked = profile.join("projects/project-a");
    let saved = profile.join("projects/project-b");
    for path in [&locked, &saved] {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("chosen.jsonl"), b"saved").unwrap();
    }
    let _guard = PermissionGuard::lock(&locked);
    if std::fs::metadata(locked.join("chosen.jsonl")).is_ok() {
        eprintln!("skipped: user bypasses traversal permissions");
        return;
    }
    assert_eq!(
        std::fs::metadata(locked.join("chosen.jsonl")).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        paths.session_transcript_saved(&["project-a", "missing"], "chosen").unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(paths.session_transcript_saved(&["project-a", "project-b"], "chosen").unwrap());
    let prefix = "a".repeat(200) + "-";
    let denied = profile.join("projects").join(format!("{prefix}denied"));
    let found = profile.join("projects").join(format!("{prefix}found"));
    for path in [&denied, &found] {
        std::fs::create_dir(path).unwrap();
        std::fs::write(path.join("chosen.jsonl"), b"preserve").unwrap();
    }
    let _guard2 = PermissionGuard::lock(&denied);
    assert_eq!(
        std::fs::metadata(denied.join("chosen.jsonl")).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let missing = profile.join("projects").join(format!("{prefix}missing"));
    assert_eq!(
        transcript_in_candidates(
            [Ok(denied.clone()), Ok(missing.clone())],
            &prefix,
            "chosen.jsonl"
        )
        .unwrap_err()
        .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(
        transcript_in_candidates(
            [Ok(denied), Ok(missing), Ok(found.clone())],
            &prefix,
            "chosen.jsonl"
        )
        .unwrap()
    );
    assert_eq!(std::fs::read(found.join("chosen.jsonl")).unwrap(), b"preserve");
}
