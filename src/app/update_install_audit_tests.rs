// SPDX-License-Identifier: Apache-2.0
use super::*;
#[path = "../../tests/support/update_fixture.rs"]
mod fixture;
use fixture::{Fixture, OwnedChild, wait};
use serde_json::Value;

fn paths(root: &Path) -> WorkerPaths {
    WorkerPaths {
        settings: root.join("settings.json"),
        log: root.join("logs/worker.log"),
        lock: root.join("logs/worker.lock"),
    }
}
fn saved(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
fn fresh() -> Fixture {
    let f = Fixture::new();
    std::fs::write(
        f.path("settings.json"),
        r#"{"updates":{"autoInstall":true,"future":42},"presentation":{"copyFullResponse":true}}"#,
    )
    .unwrap();
    f
}
#[test]
fn installer_child_entry() {
    let Some(root) = std::env::var_os("UPDATE_TEST_ROOT") else { return };
    let paths = paths(Path::new(&root));
    let method = match std::env::var("UPDATE_TEST_METHOD").unwrap().as_str() {
        "npm" => InstallMethod::Npm,
        "script" => InstallMethod::Script {
            install_dir: std::env::var_os("UPDATE_TEST_DIR").map(PathBuf::from),
        },
        _ => panic!("fixture method"),
    };
    let mode = std::env::var("UPDATE_TEST_MODE").unwrap();
    let code = if mode == "foreground" {
        let mut app = App::test_default();
        app.global_settings_path = Some(paths.settings.clone());
        run_update_install(&app, "9.9.9", &method)
    } else {
        i32::from(
            run_worker(&paths, "9.9.9", &method, Duration::from_secs(20), |io| {
                install("9.9.9", &method, io)
            }) == WorkerOutcome::Failed,
        )
    };
    std::fs::write(Path::new(&root).join("result"), code.to_string()).unwrap();
}
fn run(
    f: &Fixture,
    tools: &Path,
    method: &str,
    mode: &str,
    code: i32,
    dir: Option<&Path>,
    download: &str,
) -> u32 {
    let mut command = f.command(std::env::current_exe().unwrap());
    command
        .args(["--exact", "app::update_install::audit_tests::installer_child_entry", "--nocapture"])
        .env("UPDATE_TEST_ROOT", f.root.path())
        .env("UPDATE_TEST_METHOD", method)
        .env("UPDATE_TEST_MODE", mode)
        .env("UPDATE_EXIT", code.to_string())
        .env("UPDATE_DOWNLOAD_MODE", download)
        .env("PATH", tools)
        .env("CLAUDE_RS_RELEASE", "wrong-release")
        .env("CLAUDE_RS_INSTALL_DIR", "wrong-directory");
    for key in [
        "CLAUDE_RS_BIN_DIR",
        "CLAUDE_RS_NO_MODIFY_PATH",
        "CLAUDE_RS_REMOVE_NPM",
        "CLAUDE_RS_RUN",
        "CLAUDE_RS_UNINSTALL",
        "CLAUDE_RS_UPDATE",
        "CLAUDE_RS_UPDATE_PARENT_PID",
        "CLAUDE_RS_VERIFY",
    ] {
        command.env(key, "conflicting-inherited-value");
    }
    if let Some(dir) = dir {
        command.env("UPDATE_TEST_DIR", dir);
    } else {
        command.env_remove("UPDATE_TEST_DIR");
    }
    let output = File::create(f.path("harness-output")).unwrap();
    command.stdout(output.try_clone().unwrap()).stderr(output);
    let mut child = OwnedChild(command.spawn().unwrap());
    let pid = child.0.id();
    assert!(
        child.finish().success(),
        "{}",
        std::fs::read_to_string(f.path("harness-output")).unwrap()
    );
    pid
}
fn journal(f: &Fixture) -> String {
    std::fs::read_to_string(f.path("journal")).unwrap()
}
fn assert_preserved(f: &Fixture) {
    let v = saved(&f.path("settings.json"));
    assert_eq!(v["updates"]["autoInstall"], true);
    assert_eq!(v["updates"]["future"], 42);
    assert_eq!(v["presentation"]["copyFullResponse"], true);
}

#[test]
fn actual_npm_installs_propagate_foreground_exit_and_worker_failure_then_clear_it() {
    for mode in ["foreground", "worker"] {
        let f = fresh();
        let tools = f.install_tools();
        f.release();
        for code in [41, 0] {
            run(&f, &tools, "npm", mode, code, None, "");
            let j = journal(&f);
            let args: Vec<_> = j.lines().filter_map(|l| l.strip_prefix("arg=")).collect();
            assert_eq!(args, ["install", "-g", "claude-code-rust@9.9.9"]);
            assert_eq!(
                std::fs::read_to_string(f.path("result")).unwrap(),
                if mode == "foreground" {
                    code.to_string()
                } else {
                    i32::from(code != 0).to_string()
                }
            );
            let v = saved(&f.path("settings.json"));
            if code != 0 {
                let error = v["updates"]["last_install_error"].as_str().unwrap();
                assert!(error.contains("npm update install for v9.9.9"));
                let status =
                    error.split("exited with status ").nth(1).unwrap().split('.').next().unwrap();
                assert_eq!(status.split_whitespace().last(), Some("41"), "{error}");
                if mode == "worker" {
                    assert!(error.contains(&paths(f.root.path()).log.display().to_string()));
                }
            } else {
                assert!(v["updates"].get("last_install_error").is_none());
            }
            assert_preserved(&f);
            let log = std::fs::read_to_string(if mode == "worker" {
                paths(f.root.path()).log
            } else {
                f.path("harness-output")
            })
            .unwrap();
            assert!(log.contains("fixture-installer-output"));
        }
    }
}
#[test]
fn script_dispatch_preserves_selected_release_directory_and_noninteractive_intent() {
    for inplace in [false, true] {
        let f = fresh();
        let tools = f.install_tools();
        f.release();
        let directory = f.path("selected install dir");
        let pid =
            run(&f, &tools, "script", "worker", 0, inplace.then_some(directory.as_path()), "");
        assert_eq!(std::fs::read_to_string(f.path("result")).unwrap(), "0");
        let j = journal(&f);
        let args: Vec<_> = j.lines().filter_map(|l| l.strip_prefix("arg=")).collect();
        assert!(j.lines().any(|l| l == "CLAUDE_RS_NON_INTERACTIVE=1"));
        for key in [
            "CLAUDE_RS_NO_MODIFY_PATH",
            "CLAUDE_RS_REMOVE_NPM",
            "CLAUDE_RS_RUN",
            "CLAUDE_RS_UNINSTALL",
            "CLAUDE_RS_VERIFY",
        ] {
            assert!(!j.lines().any(|l| l.starts_with(&format!("{key}="))), "{key}: {j}");
        }
        #[cfg(windows)]
        {
            assert_eq!(
                &args[..5],
                ["-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"]
            );
            assert!(args[5].contains("https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.ps1"));
            for value in ["CLAUDE_RS_RELEASE=9.9.9", "CLAUDE_RS_KEEP_NPM=1"] {
                assert!(j.lines().any(|l| l == value));
            }
            for (key, expected) in [
                ("CLAUDE_RS_INSTALL_DIR", directory.display().to_string()),
                ("CLAUDE_RS_UPDATE", "1".into()),
                ("CLAUDE_RS_UPDATE_PARENT_PID", pid.to_string()),
            ] {
                let value = j.lines().find_map(|l| l.strip_prefix(&format!("{key}=")));
                assert_eq!(value, inplace.then_some(expected.as_str()));
            }
        }
        #[cfg(unix)]
        {
            let _ = pid;
            let mut expected = vec!["--release", "9.9.9", "--yes", "--keep-npm"];
            let dir = directory.to_str().unwrap();
            if inplace {
                expected.extend(["--update", "--install-dir", dir]);
            }
            assert_eq!(args, expected);
            for key in [
                "CLAUDE_RS_RELEASE",
                "CLAUDE_RS_INSTALL_DIR",
                "CLAUDE_RS_BIN_DIR",
                "CLAUDE_RS_UPDATE",
            ] {
                assert!(!j.lines().any(|l| l.starts_with(&format!("{key}="))));
            }
            let download = std::fs::read_to_string(f.path("download-journal")).unwrap();
            assert!(download.contains("arg=https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.sh"));
            let script = PathBuf::from(std::fs::read_to_string(f.path("download-path")).unwrap());
            assert!(!script.parent().unwrap().exists(), "temporary download cleaned up");
        }
        assert_preserved(&f);
    }
}
#[test]
fn missing_executable_and_failed_spawn_are_reported_without_running_an_installer() {
    for invalid_executable in [false, true] {
        let f = fresh();
        let tools = f.path("tools");
        std::fs::create_dir(&tools).unwrap();
        f.release();
        if invalid_executable {
            #[cfg(windows)]
            let name = "npm.exe";
            #[cfg(unix)]
            let name = "npm";
            let path = tools.join(name);
            #[cfg(windows)]
            std::fs::write(&path, "not an executable").unwrap();
            #[cfg(unix)]
            std::fs::write(&path, "#!/nonexistent-audit-interpreter\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        run(&f, &tools, "npm", "foreground", 0, None, "");
        assert_eq!(std::fs::read_to_string(f.path("result")).unwrap(), "1");
        assert!(!f.path("ready").exists());
        assert!(!f.path("journal").exists());
        let v = saved(&f.path("settings.json"));
        let error = v["updates"]["last_install_error"].as_str().unwrap();
        assert!(error.contains("npm update install for v9.9.9"));
        assert!(error.contains(if invalid_executable {
            "failed to start"
        } else {
            "failed to resolve"
        }));
        assert_preserved(&f);
    }
}
#[test]
#[cfg(unix)]
fn download_failures_never_launch_the_script_and_remove_temporary_files() {
    for mode in ["fail", "missing"] {
        let f = fresh();
        let tools = f.install_tools();
        f.release();
        run(&f, &tools, "script", "worker", 0, None, mode);
        assert_eq!(std::fs::read_to_string(f.path("result")).unwrap(), "1");
        assert!(!f.path("ready").exists());
        let script = PathBuf::from(std::fs::read_to_string(f.path("download-path")).unwrap());
        assert!(!script.parent().unwrap().exists());
        let v = saved(&f.path("settings.json"));
        let error = v["updates"]["last_install_error"].as_str().unwrap();
        assert!(error.contains(if mode == "fail" {
            "installer download exited with status"
        } else {
            "did not create install.sh"
        }));
        assert_preserved(&f);
    }
}
#[test]
fn worker_preparation_refuses_installation_and_repaired_retry_releases_the_lock() {
    for obstruction in ["lock-parent", "log"] {
        let f = fresh();
        let p = paths(f.root.path());
        if obstruction == "lock-parent" {
            std::fs::write(f.path("logs"), "obstruction").unwrap();
        } else {
            std::fs::create_dir_all(&p.log).unwrap();
        }
        let result = run_worker(&p, "9.9.9", &InstallMethod::Npm, Duration::from_secs(10), |_| {
            panic!("installation before lock/log preparation")
        });
        assert_eq!(result, WorkerOutcome::Failed);
        let v = saved(&p.settings);
        let error = v["updates"]["last_install_error"].as_str().unwrap();
        assert!(error.contains(if obstruction == "lock-parent" {
            "cannot lock"
        } else {
            "cannot create"
        }));
        assert!(error.contains(
            &if obstruction == "lock-parent" { &p.lock } else { &p.log }.display().to_string()
        ));
        assert_preserved(&f);
        if obstruction == "lock-parent" {
            std::fs::remove_file(f.path("logs")).unwrap();
        } else {
            std::fs::remove_dir(&p.log).unwrap();
        }
        f.release();
        let result = run_worker(&p, "9.9.9", &InstallMethod::Npm, Duration::from_secs(10), |io| {
            io.run(&mut f.command(fixture::binary()))
        });
        assert_eq!(result, WorkerOutcome::Installed);
        assert!(f.path("marker").exists());
        assert!(saved(&p.settings)["updates"].get("last_install_error").is_none());
        assert_preserved(&f);
    }
    let f = fresh();
    let p = paths(f.root.path());
    std::fs::write(&p.settings, "{broken-selected-document").unwrap();
    f.release();
    assert_eq!(
        run_worker(&p, "9.9.9", &InstallMethod::Npm, Duration::from_secs(10), |io| io
            .run(&mut f.command(fixture::binary()))),
        WorkerOutcome::Installed
    );
    assert_eq!(std::fs::read_to_string(&p.settings).unwrap(), "{broken-selected-document");
    assert!(
        std::fs::read_to_string(&p.log)
            .unwrap()
            .contains("Failed to update app settings after install")
    );
}
#[test]
fn timeout_stops_the_announced_installer_and_its_live_descendant() {
    let mut f = fresh();
    let p = paths(f.root.path());
    let mut command = f.command(fixture::binary());
    command.arg("tree-parent");
    let ready = f.path("ready");
    let child_ready = f.path("child-ready");
    let started = Instant::now();
    let (finished, result) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let outcome = run_worker(&p, "9.9.9", &InstallMethod::Npm, Duration::from_secs(5), |io| {
            io.run(&mut command)
        });
        let _ = finished.send(outcome);
    });
    // Startup must be observed while both processes are alive, before enforcement.
    wait("installer and descendant readiness", || ready.exists() && child_ready.exists());
    f.collect_pids();
    assert_eq!(f.pids.len(), 2);
    assert!(f.pids.iter().all(|pid| !fixture::stopped(*pid)), "fixture must start before deadline");
    assert_eq!(
        result.recv_timeout(Duration::from_secs(20)).expect("worker enforcement is bounded"),
        WorkerOutcome::Failed
    );
    worker.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(30));
    for pid in &f.pids {
        wait("terminated owned process", || fixture::stopped(*pid));
    }
    f.release();
    assert!(!f.path("marker").exists(), "terminated processes cannot complete after release");
    let v = saved(&f.path("settings.json"));
    assert!(v["updates"]["last_install_error"].as_str().unwrap().contains("time limit"));
}
#[test]
fn detached_producer_returns_before_release_and_child_completes_with_a_bound() {
    let mut f = fresh();
    let mut command = f.command(fixture::binary());
    command.arg("detached");
    let (tx, rx) = std::sync::mpsc::channel();
    let producer = std::thread::spawn(move || {
        let child = spawn_detached(&mut command).unwrap();
        let _ = tx.send(OwnedChild(child));
    });
    wait("detached child readiness", || f.path("ready").exists());
    f.collect_pids();
    let mut child =
        rx.recv_timeout(Duration::from_secs(5)).expect("producer must return while child is gated");
    assert!(child.0.try_wait().unwrap().is_none());
    assert!(!f.path("marker").exists());
    #[cfg(unix)]
    {
        super::tests::assert_leads_its_own_session(&child.0);
    }
    f.release();
    assert!(child.finish().success());
    assert_eq!(std::fs::read_to_string(f.path("marker")).unwrap(), "completed");
    producer.join().unwrap();
}
