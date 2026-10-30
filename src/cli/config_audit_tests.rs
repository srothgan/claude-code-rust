// SPDX-License-Identifier: Apache-2.0

use super::*;
use clap::Parser;
use serde_json::json;
use std::collections::BTreeMap;

struct Fixture {
    root: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
}

#[allow(clippy::unwrap_used)]
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let project = root.path().join("project");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(project.join(".claude")).unwrap();
        Self { root, home, project }
    }

    fn cli(&self) -> Cli {
        let mut cli = Cli::try_parse_from(["claude-rs"]).unwrap();
        cli.dir = Some(self.project.clone());
        cli
    }

    fn paths(&self) -> [(ConfigFileSelector, &'static str, &'static str, PathBuf); 4] {
        [
            (
                ConfigFileSelector::Settings,
                "settings",
                "user",
                self.home.join(".claude").join("settings.json"),
            ),
            (
                ConfigFileSelector::ProjectSettings,
                "project_settings",
                "project",
                self.project.join(".claude").join("settings.json"),
            ),
            (
                ConfigFileSelector::LocalSettings,
                "local_settings",
                "local",
                self.project.join(".claude").join("settings.local.json"),
            ),
            (
                ConfigFileSelector::Preferences,
                "preferences",
                "user",
                self.home.join(".claude.json"),
            ),
        ]
    }

    fn seed(&self) {
        for (_, kind, _, path) in self.paths() {
            fs::write(path, serde_json::to_vec(&json!({"owner":kind,"enabled":false,"nested":{"apiKey":"private-config-token","safe":42}})).unwrap()).unwrap();
        }
    }

    fn show(&self, json: bool) -> (i32, String) {
        let mut out = Vec::new();
        let code = write_show_with_home_override(
            &self.cli(),
            &ConfigShowArgs { json },
            Some(&self.home),
            &mut out,
            HumanStyle::new(false),
        )
        .unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    fn snapshot(&self) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        snapshot(self.root.path())
    }
}

#[allow(clippy::unwrap_used)]
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut entries = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            entries.insert(path.clone(), None);
            entries.extend(snapshot(&path));
        } else {
            entries.insert(path.clone(), Some(fs::read(path).unwrap()));
        }
    }
    entries
}

#[allow(clippy::unwrap_used)]
fn record<'a>(report: &'a Value, kind: &str) -> &'a Value {
    report["files"].as_array().unwrap().iter().find(|file| file["kind"] == kind).unwrap()
}

#[allow(clippy::unwrap_used)]
fn block<'a>(output: &'a str, label: &str) -> &'a str {
    let start =
        output.lines().find(|line| line.starts_with("  [") && line.ends_with(label)).unwrap();
    let tail = output.split_once(start).unwrap().1;
    tail.split("\n  [").next().unwrap()
}

#[test]
fn locations_associate_each_scope_with_its_path_and_state() {
    let f = Fixture::new();
    fs::write(f.home.join(".claude").join("settings.json"), "{}").unwrap();
    fs::write(f.project.join(".claude").join("settings.json"), "{ broken").unwrap();
    fs::create_dir(f.home.join(".claude.json")).unwrap();
    let before = f.snapshot();
    let mut out = Vec::new();
    assert_eq!(
        write_path_with_home_override(
            &f.cli(),
            &ConfigPathArgs { json: false, which: None },
            Some(&f.home),
            &mut out,
            HumanStyle::new(false)
        )
        .unwrap(),
        0
    );
    let output = String::from_utf8(out).unwrap();
    for (label, scope, status, path) in [
        ("Global settings", "user", "FILE", f.home.join(".claude").join("settings.json")),
        ("Project settings", "project", "INVALID", f.project.join(".claude").join("settings.json")),
        ("Local settings", "local", "MISS", f.project.join(".claude").join("settings.local.json")),
        ("Preferences", "user", "NOTFILE", f.home.join(".claude.json")),
    ] {
        assert!(output.contains(&format!("[{status}] {label}")));
        let region = block(&output, label);
        assert!(region.contains(&format!("Scope: {scope}")));
        assert!(region.contains(&format!("Path: {}", path.display())));
    }
    assert!(output.contains("claude-rs config show"));
    assert!(output.contains("claude-rs config export --output"));
    assert_eq!(f.snapshot(), before);
}

#[test]
fn selectors_emit_exact_paths_and_json_associates_all_missing_scopes() {
    let f = Fixture::new();
    let before = f.snapshot();
    for (selector, _, _, path) in f.paths() {
        let mut out = Vec::new();
        assert_eq!(
            write_path_with_home_override(
                &f.cli(),
                &ConfigPathArgs { json: false, which: Some(selector) },
                Some(&f.home),
                &mut out,
                HumanStyle::new(false)
            )
            .unwrap(),
            0
        );
        assert_eq!(String::from_utf8(out).unwrap(), format!("{}\n", path.display()));
    }
    let mut out = Vec::new();
    assert_eq!(
        write_path_with_home_override(
            &f.cli(),
            &ConfigPathArgs { json: true, which: None },
            Some(&f.home),
            &mut out,
            HumanStyle::new(false)
        )
        .unwrap(),
        0
    );
    let report: Value = serde_json::from_slice(&out).unwrap();
    for (_, kind, scope, path) in f.paths() {
        let file = record(&report, kind);
        assert_eq!(file["path"], path.to_str().unwrap());
        assert_eq!(file["scope"], scope);
        assert_eq!(file["status"], "missing");
        assert!(file.get("document").is_none());
    }
    assert_eq!(f.snapshot(), before);
}

#[test]
fn show_preserves_safe_scoped_documents_and_source_bytes() {
    let f = Fixture::new();
    f.seed();
    let before = f.snapshot();
    let (code, text) = f.show(true);
    assert_eq!(code, 0);
    assert!(!text.contains("private-config-token"));
    let report: Value = serde_json::from_str(&text).unwrap();
    for (_, kind, scope, path) in f.paths() {
        let file = record(&report, kind);
        assert_eq!(file["path"], path.to_str().unwrap());
        assert_eq!(file["scope"], scope);
        assert_eq!(file["status"], "valid");
        assert_eq!(
            file["document"],
            json!({"owner":kind,"enabled":false,"nested":{"apiKey":"[redacted]","safe":42}})
        );
    }
    assert_eq!(f.snapshot(), before);
}

#[test]
fn human_show_keeps_small_content_with_its_file_and_large_content_in_json_only() {
    let f = Fixture::new();
    fs::write(
        f.home.join(".claude").join("settings.json"),
        r#"{"model":"small-global-model","apiKey":"small-private-token"}"#,
    )
    .unwrap();
    let values: Vec<String> = (0..40).map(|i| format!("large-item-{i:02}-end")).collect();
    fs::write(
        f.project.join(".claude").join("settings.local.json"),
        serde_json::to_vec(&json!({"items":values,"apiKey":"large-private-token"})).unwrap(),
    )
    .unwrap();
    let before = f.snapshot();
    let (code, human) = f.show(false);
    assert_eq!(code, 0);
    let global = block(&human, "Global settings");
    assert!(global.contains("small-global-model") && global.contains("[redacted]"));
    let local = block(&human, "Local settings");
    assert!(local.contains("items") && local.contains("claude-rs config show --json"));
    assert!(!local.contains("small-global-model"));
    for value in &values {
        assert!(!human.contains(value));
    }
    for secret in ["small-private-token", "large-private-token"] {
        assert!(!human.contains(secret));
    }
    let (code, text) = f.show(true);
    assert_eq!(code, 0);
    let report: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        record(&report, "local_settings")["document"],
        json!({"items":values,"apiKey":"[redacted]"})
    );
    assert_eq!(record(&report, "settings")["document"]["model"], "small-global-model");
    assert_eq!(f.snapshot(), before);
}

#[allow(clippy::unwrap_used)]
fn failing_fixture(scenario: &str) -> (Fixture, &'static str, &'static str) {
    let f = Fixture::new();
    fs::write(f.home.join(".claude").join("settings.json"), r#"{"model":"surviving-global"}"#)
        .unwrap();
    let path = f.project.join(".claude").join("settings.local.json");
    let (status, cause) = match scenario {
        "malformed" => {
            fs::write(path, "{ broken").unwrap();
            ("invalid", "parse JSON")
        }
        "nonobject" => {
            fs::write(path, "[1,2]").unwrap();
            ("invalid", "top-level JSON object")
        }
        "unreadable" => {
            fs::write(path, [0xff, 0xfe]).unwrap();
            ("unreadable", "read file")
        }
        "directory" => {
            fs::create_dir(path).unwrap();
            ("not_file", "not a file")
        }
        _ => unreachable!(),
    };
    (f, status, cause)
}

#[test]
fn failing_documents_are_reported_without_repair_and_keep_valid_scopes() {
    for scenario in ["malformed", "nonobject", "unreadable", "directory"] {
        let (f, status, cause) = failing_fixture(scenario);
        let before = f.snapshot();
        let (code, text) = f.show(true);
        assert_eq!(code, 1, "{scenario}");
        let report: Value = serde_json::from_str(&text).unwrap();
        let local = record(&report, "local_settings");
        assert_eq!(
            local["path"],
            f.project.join(".claude").join("settings.local.json").to_str().unwrap()
        );
        assert_eq!(local["status"], status);
        assert!(local["error"].as_str().unwrap().contains(cause));
        assert!(local.get("document").is_none());
        assert_eq!(record(&report, "settings")["document"]["model"], "surviving-global");
        let (code, human) = f.show(false);
        assert_eq!(code, 1);
        assert!(block(&human, "Local settings").contains(cause));
        assert!(block(&human, "Global settings").contains("surviving-global"));
        assert_eq!(f.snapshot(), before, "{scenario}");
    }
}

#[test]
fn export_preflight_rejects_failures_before_creating_output_but_allows_missing_files() {
    for scenario in ["malformed", "nonobject", "unreadable", "directory"] {
        let (f, _, _) = failing_fixture(scenario);
        let before = f.snapshot();
        let target = f.root.path().join("new-parent/export.json");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            write_export_with_home_override(
                &f.cli(),
                &ConfigExportArgs { output: Some(target.clone()) },
                Some(&f.home),
                &mut out,
                &mut err
            )
            .unwrap(),
            1
        );
        assert!(out.is_empty() && !err.is_empty());
        assert!(!target.parent().unwrap().exists());
        assert_eq!(f.snapshot(), before);
    }
    let f = Fixture::new();
    let before = f.snapshot();
    assert_eq!(f.show(true).0, 0);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(
        write_export_with_home_override(
            &f.cli(),
            &ConfigExportArgs { output: None },
            Some(&f.home),
            &mut out,
            &mut err
        )
        .unwrap(),
        0
    );
    assert!(err.is_empty());
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["schema"], "claude-rs-config-export/v1");
    for (_, kind, _, path) in f.paths() {
        assert_eq!(record(&report, kind)["status"], "missing");
        assert_eq!(record(&report, kind)["path"], path.to_str().unwrap());
    }
    assert_eq!(f.snapshot(), before);
}

#[test]
fn partial_export_failure_cleans_owned_target_and_retry_writes_redacted_scopes() {
    let f = Fixture::new();
    f.seed();
    fs::write(f.root.path().join("neighbor"), b"untouched neighbor").unwrap();
    let before = f.snapshot();
    let target = f.root.path().join("export.json");
    let args = ConfigExportArgs { output: Some(target.clone()) };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let error = write_export_with_io(
        &f.cli(),
        &args,
        Some(&f.home),
        &mut out,
        &mut err,
        |file, path, text| {
            file.write_all(&text.as_bytes()[..20])?;
            file.flush()?;
            assert_eq!(fs::read(path).unwrap(), &text.as_bytes()[..20]);
            Err(std::io::Error::other("injected partial export failure"))
                .with_context(|| format!("failed to write config export {}", path.display()))
        },
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("injected partial export failure"));
    assert!(error.to_string().contains(target.to_str().unwrap()));
    assert!(out.is_empty() && err.is_empty());
    assert!(!target.exists());
    assert_eq!(f.snapshot(), before);
    assert_eq!(
        write_export_with_home_override(&f.cli(), &args, Some(&f.home), &mut out, &mut err)
            .unwrap(),
        0
    );
    assert!(err.is_empty());
    let bytes = fs::read(&target).unwrap();
    assert!(bytes.ends_with(b"\n"));
    assert!(!String::from_utf8_lossy(&bytes).contains("private-config-token"));
    let report: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(report["schema"], "claude-rs-config-export/v1");
    for (_, kind, scope, path) in f.paths() {
        let file = record(&report, kind);
        assert_eq!(file["path"], path.to_str().unwrap());
        assert_eq!(file["scope"], scope);
        assert_eq!(
            file["document"],
            json!({"owner":kind,"enabled":false,"nested":{"apiKey":"[redacted]","safe":42}})
        );
    }
    let mut after = f.snapshot();
    after.remove(&target);
    assert_eq!(after, before);
}
