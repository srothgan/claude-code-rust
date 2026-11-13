// SPDX-License-Identifier: Apache-2.0
// XDG data roots isolate the real default-log resolver on Linux. Windows Known Folders do not.
#![cfg(target_os = "linux")]

use std::io::Read;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

#[allow(clippy::unwrap_used, clippy::expect_used)]
async fn run(root: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_claude-rs"));
    command
        .current_dir(root.join("project"))
        .arg("--bridge-script")
        .arg(root.join("bridge.js"))
        .arg("logs")
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .env("CLAUDE_CONFIG_DIR", root.join("profile"))
        .env("CLAUDE_RS_CONFIG_DIR", root.join("app-profile"))
        .env("CLAUDE_RS_AGENT_BRIDGE_RUNTIME", root.join("runtime probe.sh"))
        .env_remove("CLAUDE_RS_AGENT_BRIDGE")
        .env("HOME", root.join("home"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("NO_COLOR", "1")
        .env("SUPPORT_PRIVATE_SENTINEL", "forbidden-environment-content");
    tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .expect("bounded logs command")
        .unwrap()
}

#[tokio::test]
async fn logs_dispatch_selects_redacted_tail_and_publishes_only_with_consent() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for directory in ["project", "profile", "app-profile", "data", "home"] {
        std::fs::create_dir(root.join(directory)).unwrap();
    }
    std::fs::write(
        root.join("bridge.js"),
        "throw new Error('support commands must not start the bridge');",
    )
    .unwrap();
    let runtime = root.join("runtime probe.sh");
    std::fs::write(&runtime, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.argv\"\nprintf '4.5.6\\n'\n")
        .unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
    let profile = root.join("profile/settings.json");
    let project = root.join("project/private.txt");
    std::fs::write(&profile, r#"{"env":{"PRIVATE_SENTINEL":"forbidden-profile-content"}}"#)
        .unwrap();
    std::fs::write(&project, "forbidden-project-content").unwrap();
    std::fs::write(
        root.join("profile/.credentials.json"),
        r#"{"accessToken":"forbidden-credential-content"}"#,
    )
    .unwrap();
    let runtime_dir = root.join("data/claude-code-rust/logs/runtime");
    let path = run(root, &["--path"]).await;
    assert_eq!(path.status.code(), Some(0));
    assert!(path.stderr.is_empty());
    assert_eq!(path.stdout, format!("{}\n", runtime_dir.display()).as_bytes());
    // Seed only after verifying that the process actually resolved the isolated data root.
    std::fs::create_dir_all(&runtime_dir).unwrap();
    let log = runtime_dir.join("claude-rs-20250101T000000Z-p1-rabc.log");
    let source = "excluded first\nselected token=cli-secret safe\nlast selected\n";
    std::fs::write(&log, source).unwrap();
    let summary = run(root, &[]).await;
    assert_eq!(summary.status.code(), Some(0));
    assert!(summary.stderr.is_empty());
    let summary = String::from_utf8(summary.stdout).unwrap();
    let lines = summary.lines().collect::<Vec<_>>();
    let latest = lines.iter().position(|line| line.contains("Latest log")).unwrap();
    assert!(lines[latest].contains("[FOUND]"));
    assert_eq!(lines[latest + 1].trim(), log.display().to_string());
    let latest = run(root, &["--latest"]).await;
    assert_eq!(latest.status.code(), Some(0));
    assert!(latest.stderr.is_empty());
    assert_eq!(latest.stdout, format!("{}\n", log.display()).as_bytes());
    let tail = run(root, &["--tail", "2"]).await;
    assert_eq!(tail.status.code(), Some(0));
    assert!(tail.stderr.is_empty());
    assert_eq!(tail.stdout, b"selected token=[redacted] safe\nlast selected\n");
    assert_eq!(std::fs::read_dir(runtime_dir.parent().unwrap()).unwrap().count(), 1);
    let output = root.join("exports/selected support.zip");
    let output_arg = output.to_str().unwrap();
    let refused = run(root, &["--bundle", "--output", output_arg]).await;
    assert_eq!(refused.status.code(), Some(1));
    assert!(refused.stdout.is_empty());
    assert!(String::from_utf8(refused.stderr).unwrap().contains("--yes"));
    assert!(!output.parent().unwrap().exists());
    let accepted = run(root, &["--bundle", "--yes", "--output", output_arg]).await;
    assert_eq!(accepted.status.code(), Some(0), "{}", String::from_utf8_lossy(&accepted.stderr));
    assert!(accepted.stderr.is_empty());
    let bytes = std::fs::read(&output).unwrap();
    assert_bundle(&bytes, root, &runtime);
    let occupied = run(root, &["--bundle", "--yes", "--output", output_arg]).await;
    assert!(!occupied.status.success());
    assert!(String::from_utf8(occupied.stderr).unwrap().contains("already exists"));
    assert_eq!(std::fs::read(&output).unwrap(), bytes);
    assert_eq!(
        std::fs::read_dir(output.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>(),
        vec![output]
    );
    assert_eq!(std::fs::read_to_string(log).unwrap(), source);
    assert_eq!(
        std::fs::read_to_string(profile).unwrap(),
        r#"{"env":{"PRIVATE_SENTINEL":"forbidden-profile-content"}}"#
    );
    assert_eq!(std::fs::read_to_string(project).unwrap(), "forbidden-project-content");
    assert_eq!(
        std::fs::read_to_string(root.join("profile/.credentials.json")).unwrap(),
        r#"{"accessToken":"forbidden-credential-content"}"#
    );
    assert!(std::fs::read_dir(root.join("app-profile")).unwrap().next().is_none());
}

#[allow(clippy::unwrap_used)]
fn assert_bundle(bytes: &[u8], root: &Path, runtime: &Path) {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut entries = std::collections::BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().unwrap().into_owned();
        let mut value = String::new();
        entry.read_to_string(&mut value).unwrap();
        entries.insert(name, value);
    }
    assert_eq!(
        entries["logs/runtime/claude-rs-20250101T000000Z-p1-rabc.log"],
        "excluded first\nselected token=[redacted] safe\nlast selected"
    );
    let doctor: serde_json::Value = serde_json::from_str(&entries["doctor.json"]).unwrap();
    let version = doctor["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "bridge_runtime_version")
        .unwrap();
    assert_eq!(version["status"], "pass");
    assert_eq!(version["details"]["version"], "4.5.6");
    assert_eq!(version["details"]["runtime"], runtime.display().to_string());
    assert_eq!(std::fs::read_to_string(root.join("runtime probe.sh.argv")).unwrap(), "--version\n");
    for text in entries.values() {
        for secret in [
            "cli-secret",
            "forbidden-profile-content",
            "forbidden-project-content",
            "forbidden-environment-content",
            "forbidden-credential-content",
        ] {
            assert!(!text.contains(secret));
        }
    }
}
