// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::collections::BTreeMap;
use std::io::Read;
use std::time::{Duration, UNIX_EPOCH};

struct Fixture {
    dir: tempfile::TempDir,
    paths: crate::logging::DiagnosticsPaths,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root_dir = dir.path().join("diagnostics");
        let runtime_dir = root_dir.join("runtime");
        fs::create_dir_all(&runtime_dir).unwrap();
        let legacy_log_path = root_dir.join("claude-rs.log");
        Self {
            dir,
            paths: crate::logging::DiagnosticsPaths { root_dir, runtime_dir, legacy_log_path },
        }
    }
    fn log(&self, name: &str, text: &str, seconds: u64) -> PathBuf {
        let path = self.paths.runtime_dir.join(name);
        fs::write(&path, text).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)))
            .unwrap();
        path
    }
    fn plan(&self) -> BundlePlan {
        BundlePlan::build(&self.paths, Some(self.dir.path().join("out/bundle.zip"))).unwrap()
    }
}

fn observations() -> Value {
    serde_json::json!({"version":"1.2.3","checks":[{"id":"fixture_check","status":"pass","message":"selected fixture","details":{"Authorization":"doctor-secret","runtime":"controlled runtime"}}]})
}

fn entries(path: &Path) -> BTreeMap<String, String> {
    let mut zip = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
    (0..zip.len())
        .map(|index| {
            let mut entry = zip.by_index(index).unwrap();
            let name = entry.name().unwrap().into_owned();
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            (name, text)
        })
        .collect()
}

#[test]
fn latest_output_selects_metadata_recency_then_legacy_without_false_paths() {
    for mode in ["runtime", "legacy", "empty", "unrelated", "absent"] {
        let f = Fixture::new();
        let expected = if mode == "runtime" {
            f.log("claude-rs-20260101T000000Z-p1-raaa.log", "older", 100);
            let newest = f.log("claude-rs-20250101T000000Z-p1-rbbb.log", "newest", 200);
            fs::write(&f.paths.legacy_log_path, "legacy competitor").unwrap();
            Some(newest)
        } else if mode != "absent" {
            if mode == "unrelated" {
                f.log("unrelated.log", "not managed", 300);
            } else if mode == "legacy" {
                fs::remove_dir(&f.paths.runtime_dir).unwrap();
            }
            fs::write(&f.paths.legacy_log_path, "legacy").unwrap();
            Some(f.paths.legacy_log_path.clone())
        } else {
            None
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code =
            write_latest_path(&mut out, &mut err, &f.paths.runtime_dir, &f.paths.legacy_log_path)
                .unwrap();
        if let Some(path) = expected {
            assert_eq!(code, 0);
            assert!(err.is_empty());
            assert_eq!(out, format!("{}\n", path.display()).as_bytes());
        } else {
            assert_eq!(code, 1);
            assert!(out.is_empty());
            assert!(String::from_utf8(err).unwrap().contains("No claude-rs log"));
        }
    }
}

#[test]
fn summary_associates_each_location_and_latest_status_with_its_path() {
    for populated in [false, true] {
        let f = Fixture::new();
        let selected = f.paths.runtime_dir.join("claude-rs-20250101T000000Z-p1-rabc.log");
        if populated {
            f.log(selected.file_name().unwrap().to_str().unwrap(), "selected", 100);
            fs::write(&f.paths.legacy_log_path, "legacy").unwrap();
        }
        let mut out = Vec::new();
        assert_eq!(write_summary(&mut out, &f.paths, HumanStyle::new(false)).unwrap(), 0);
        let text = String::from_utf8(out).unwrap();
        let lines = text.lines().collect::<Vec<_>>();
        for (label, path, status) in [
            ("Runtime logs", &f.paths.runtime_dir, "[DIR]"),
            ("Legacy log", &f.paths.legacy_log_path, if populated { "[FILE]" } else { "[MISS]" }),
        ] {
            let at = lines.iter().position(|line| line.contains(label)).unwrap();
            assert!(lines[at].contains(status));
            assert_eq!(lines[at + 1].trim(), path.display().to_string());
        }
        let at = lines.iter().position(|line| line.contains("Latest log")).unwrap();
        assert!(lines[at].contains(if populated { "[FOUND]" } else { "[MISS]" }));
        if populated {
            assert_eq!(lines[at + 1].trim(), selected.display().to_string());
        } else {
            assert!(!text.contains("[FOUND]"));
        }
        for command in [
            "claude-rs logs --path",
            "claude-rs logs --latest",
            "claude-rs logs --tail 200",
            "claude-rs logs --bundle --yes",
        ] {
            assert!(text.contains(command));
        }
    }
}

#[test]
fn tail_preserves_exact_requested_suffix_and_never_falls_back_from_an_empty_runtime() {
    for (contents, count, expected) in [
        (
            "first\ntoken=tail-secret safe\nlast \u{96ea}\n",
            2,
            "token=[redacted] safe\nlast \u{96ea}\n",
        ),
        ("short", 9, "short\n"),
        ("unrequested", 0, ""),
        ("", 3, ""),
    ] {
        let f = Fixture::new();
        let older = f.log("claude-rs-20260101T000000Z-p1-raaa.log", "wrong older", 100);
        let selected = f.log("claude-rs-20250101T000000Z-p1-rbbb.log", contents, 200);
        fs::write(&f.paths.legacy_log_path, "wrong legacy").unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            write_tail(&mut out, &mut err, &f.paths.runtime_dir, &f.paths.legacy_log_path, count)
                .unwrap(),
            0
        );
        assert_eq!(out, expected.as_bytes());
        assert!(err.is_empty());
        assert_eq!(fs::read_to_string(selected).unwrap(), contents);
        assert_eq!(fs::read_to_string(older).unwrap(), "wrong older");
        assert_eq!(fs::read_to_string(&f.paths.legacy_log_path).unwrap(), "wrong legacy");
    }
    let f = Fixture::new();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(
        write_tail(&mut out, &mut err, &f.paths.runtime_dir, &f.paths.legacy_log_path, 2).unwrap(),
        1
    );
    assert!(out.is_empty());
    assert!(String::from_utf8(err).unwrap().contains("No claude-rs log"));
}

#[test]
fn bundle_selection_keeps_recent_families_and_bounds_orphans_by_metadata() {
    let f = Fixture::new();
    let mut expected = BTreeSet::new();
    for n in 1..=7 {
        let name = format!("claude-rs-20250101T000000Z-p{n}-rabc.log");
        let base = f.log(&name, "base", n * 100);
        let rotated =
            f.log(&format!("{name}.1"), "rotation", if n == 1 { 9000 } else { n * 100 + 1 });
        if n >= 3 {
            expected.insert(base);
            expected.insert(rotated);
        }
    }
    f.log("claude-rs-20250101T000000Z-p9-rabc.log.1", "orphan", 9999);
    f.log("other.log", "unrelated", 9999);
    fs::create_dir(f.paths.runtime_dir.join("directory")).unwrap();
    assert_eq!(
        select_bundle_runtime_logs(&f.paths.runtime_dir)
            .unwrap()
            .into_iter()
            .collect::<BTreeSet<_>>(),
        expected
    );
    let f = Fixture::new();
    let mut expected = BTreeSet::new();
    assert!(select_bundle_runtime_logs(&f.paths.runtime_dir).unwrap().is_empty());
    for n in 1..=7 {
        let path = f.log(&format!("claude-rs-20250101T000000Z-p{n}-rabc.log.1"), "orphan", n * 100);
        if n >= 3 {
            expected.insert(path);
        }
    }
    assert_eq!(
        select_bundle_runtime_logs(&f.paths.runtime_dir)
            .unwrap()
            .into_iter()
            .collect::<BTreeSet<_>>(),
        expected
    );
}

#[test]
fn archive_preserves_selected_safe_records_and_excludes_private_sources() {
    for crash in [false, true] {
        let f = Fixture::new();
        let log = concat!(
            "{\"target\":\"bridge.sdk\",\"event_name\":\"selected-bridge\",\"message\":\"keep bridge\",\"fields\":{\"Authorization\":\"runtime-secret\"}}\n",
            "{\"target\":\"app.agent\",\"event_name\":\"captured_bridge_stderr\",\"message\":\"stderr Bearer captured-secret\"}\n",
            "{\"target\":\"app.session\",\"event_name\":\"unrelated-app\",\"message\":\"keep app\"}\nnot JSON\n42\n"
        );
        let base = f.log("claude-rs-20250101T000000Z-p1-rabc.log", log, 100);
        let rotation = f.log(
            "claude-rs-20250101T000000Z-p1-rabc.log.1",
            "rotated safe token=rotation-secret",
            99,
        );
        fs::write(&f.paths.legacy_log_path, "legacy safe token=legacy-secret").unwrap();
        let crash_path = f.paths.root_dir.join(crate::failure::LAST_CRASH_FILE_NAME);
        let crash_source = r#"{"category":"render","message":"failed token=crash-secret","location":"render.rs:8"}"#;
        if crash {
            fs::write(&crash_path, crash_source).unwrap();
        }
        for (name, secret) in [
            ("settings.json", "forbidden-config"),
            (".credentials.json", "forbidden-credentials"),
            ("environment.txt", "forbidden-environment"),
            ("project.txt", "forbidden-project"),
        ] {
            fs::write(f.dir.path().join(name), secret).unwrap();
        }
        let plan = f.plan();
        create_bundle_with(&f.paths, &plan, || Ok(observations()), publish_bundle).unwrap();
        let archive = entries(&plan.output_path);
        assert_archive_metadata(&archive, &f, &plan, crash);
        let doctor: Value = serde_json::from_str(&archive["doctor.json"]).unwrap();
        assert_eq!(doctor["checks"][0]["id"], "fixture_check");
        assert_eq!(doctor["checks"][0]["details"]["Authorization"], "[redacted]");
        assert_eq!(doctor["checks"][0]["details"]["runtime"], "controlled runtime");
        let records = archive["logs/bridge-diagnostics.jsonl"]
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            records,
            vec![
                serde_json::json!({"target":"bridge.sdk","event_name":"selected-bridge","message":"keep bridge","fields":{"Authorization":"[redacted]"}}),
                serde_json::json!({"target":"app.agent","event_name":"captured_bridge_stderr","message":"stderr Bearer [redacted]"})
            ]
        );
        let raw = &archive["logs/runtime/claude-rs-20250101T000000Z-p1-rabc.log"];
        assert!(raw.contains("keep app") && raw.contains("not JSON"));
        assert_eq!(
            archive["logs/runtime/claude-rs-20250101T000000Z-p1-rabc.log.1"],
            "rotated safe token=[redacted]"
        );
        assert_eq!(archive["logs/legacy/claude-rs.log"], "legacy safe token=[redacted]");
        if crash {
            let value: Value =
                serde_json::from_str(&archive[crate::failure::LAST_CRASH_FILE_NAME]).unwrap();
            assert_eq!(
                value,
                serde_json::json!({"category":"render","message":"failed token=[redacted]","location":"render.rs:8"})
            );
            assert_eq!(fs::read_to_string(crash_path).unwrap(), crash_source);
        }
        for text in archive.values() {
            for secret in [
                "runtime-secret",
                "captured-secret",
                "rotation-secret",
                "legacy-secret",
                "crash-secret",
                "doctor-secret",
                "forbidden-config",
                "forbidden-credentials",
                "forbidden-environment",
                "forbidden-project",
            ] {
                assert!(!text.contains(secret), "private fixture value appeared in exported logs");
            }
        }
        for (name, expected) in [
            ("settings.json", "forbidden-config"),
            (".credentials.json", "forbidden-credentials"),
            ("environment.txt", "forbidden-environment"),
            ("project.txt", "forbidden-project"),
        ] {
            assert_eq!(fs::read_to_string(f.dir.path().join(name)).unwrap(), expected);
        }
        assert_eq!(fs::read_to_string(base).unwrap(), log);
        assert_eq!(fs::read_to_string(rotation).unwrap(), "rotated safe token=rotation-secret");
        assert_eq!(
            fs::read_to_string(&f.paths.legacy_log_path).unwrap(),
            "legacy safe token=legacy-secret"
        );
    }
}

#[test]
fn failed_source_read_cleans_partial_bundle_and_retry_preserves_source_data() {
    let f = Fixture::new();
    let input = f.log("claude-rs-20250101T000000Z-p1-rabc.log", "safe token=private", 100);
    let plan = f.plan();
    fs::remove_file(&input).unwrap();
    let sentinel = plan.output_path.parent().unwrap().join("unrelated");
    fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    fs::write(&sentinel, "keep").unwrap();
    let error =
        create_bundle_with(&f.paths, &plan, || Ok(observations()), publish_bundle).unwrap_err();
    assert!(format!("{error:#}").contains(&input.display().to_string()));
    assert!(!plan.output_path.exists());
    assert_eq!(
        fs::read_dir(sentinel.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>(),
        vec![sentinel.clone()]
    );
    assert_eq!(fs::read_to_string(&sentinel).unwrap(), "keep");
    fs::write(&input, "safe token=private").unwrap();
    create_bundle_with(&f.paths, &plan, || Ok(observations()), publish_bundle).unwrap();
    assert_eq!(
        entries(&plan.output_path)["logs/runtime/claude-rs-20250101T000000Z-p1-rabc.log"],
        "safe token=[redacted]"
    );
    assert_eq!(fs::read_to_string(input).unwrap(), "safe token=private");
}

#[test]
fn publication_failure_cleans_temporary_archive_and_does_not_overwrite_a_racing_target() {
    for racing_target in [false, true] {
        let f = Fixture::new();
        let source = f.log("claude-rs-20250101T000000Z-p1-rabc.log", "safe source", 100);
        let plan = f.plan();
        let result = create_bundle_with(
            &f.paths,
            &plan,
            || Ok(observations()),
            |temporary, destination| {
                assert!(entries(temporary).contains_key("manifest.json"));
                if racing_target {
                    fs::write(destination, "racing-owner").unwrap();
                    publish_bundle(temporary, destination)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "fixture publication denied",
                    ))
                }
            },
        );
        let error = result.unwrap_err();
        assert!(format!("{error:#}").contains(&plan.output_path.display().to_string()));
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            if racing_target {
                std::io::ErrorKind::AlreadyExists
            } else {
                std::io::ErrorKind::PermissionDenied
            },
        );
        if racing_target {
            assert_eq!(fs::read_to_string(&plan.output_path).unwrap(), "racing-owner");
        } else {
            assert!(!plan.output_path.exists());
        }
        let siblings = fs::read_dir(plan.output_path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(siblings, if racing_target { vec![plan.output_path.clone()] } else { vec![] });
        assert_eq!(fs::read_to_string(source).unwrap(), "safe source");
        if racing_target {
            fs::remove_file(&plan.output_path).unwrap();
        }
        create_bundle_with(&f.paths, &plan, || Ok(observations()), publish_bundle).unwrap();
        assert!(entries(&plan.output_path).contains_key("manifest.json"));
    }
}

fn assert_archive_metadata(
    archive: &BTreeMap<String, String>,
    f: &Fixture,
    plan: &BundlePlan,
    crash: bool,
) {
    let mut expected = BTreeSet::from([
        "manifest.json".to_owned(),
        "doctor.json".into(),
        "paths.json".into(),
        "logs/bridge-diagnostics.jsonl".into(),
        "logs/runtime/claude-rs-20250101T000000Z-p1-rabc.log".into(),
        "logs/runtime/claude-rs-20250101T000000Z-p1-rabc.log.1".into(),
        "logs/legacy/claude-rs.log".into(),
    ]);
    if crash {
        expected.insert(crate::failure::LAST_CRASH_FILE_NAME.into());
    }
    assert_eq!(archive.keys().cloned().collect::<BTreeSet<_>>(), expected);
    let manifest: Value = serde_json::from_str(&archive["manifest.json"]).unwrap();
    assert_eq!(manifest["schema"], "claude-rs-debug-bundle/v2");
    assert_eq!(manifest["output_path"], plan.output_path.display().to_string());
    assert_eq!(
        manifest["included_files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<BTreeSet<_>>(),
        expected
    );
    assert_eq!(manifest["last_crash_metadata"], if crash { "included" } else { "not_available" });
    for name in ["manifest.json", "paths.json"] {
        let value: Value = serde_json::from_str(&archive[name]).unwrap();
        for (key, path) in [
            ("diagnostics_root", &f.paths.root_dir),
            ("runtime_log_dir", &f.paths.runtime_dir),
            ("legacy_log_path", &f.paths.legacy_log_path),
        ] {
            assert_eq!(value[key], path.display().to_string());
        }
    }
}
