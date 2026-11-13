// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::cell::Cell;
use std::fs;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

pub(super) struct ResolverFixture {
    pub(super) dir: tempfile::TempDir,
    pub(super) installed_exe: PathBuf,
    pub(super) npm_platform_exe: PathBuf,
    pub(super) unbundled_exe: PathBuf,
    pub(super) cargo_target_exe: PathBuf,
    pub(super) packaged_bridge: PathBuf,
    pub(super) npm_sibling_bridge: PathBuf,
    pub(super) packaged_runtime: PathBuf,
    pub(super) cargo_target_bridge: PathBuf,
    pub(super) cwd_script: PathBuf,
    pub(super) manifest_script: PathBuf,
    pub(super) env_script: PathBuf,
    pub(super) env_runtime: PathBuf,
    pub(super) path_bun_runtime: PathBuf,
}

fn write_file(root: &Path, relative: &str) -> PathBuf {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"fixture").unwrap();
    path.canonicalize().unwrap()
}

pub(super) fn resolver_fixture() -> ResolverFixture {
    let dir = tempfile::tempdir().unwrap();
    // Keep every bounded ancestor search inside this fixture.
    let isolated = dir.path().join("0/1/2/3/4/5/6/7/8");
    let root = isolated.as_path();
    let runtime = if cfg!(windows) { "claude-rs-bridge-bun.exe" } else { "claude-rs-bridge-bun" };
    ResolverFixture {
        installed_exe: write_file(root, "package/vendor/x86_64/claude-rs"),
        npm_platform_exe: write_file(
            root,
            "npm-prefix/node_modules/@srothgan/claude-code-rust-win32-x64-msvc/bin/claude-rs",
        ),
        unbundled_exe: write_file(root, "other/vendor/x86_64/claude-rs"),
        cargo_target_exe: write_file(root, "manifest/target/debug/claude-rs"),
        packaged_bridge: write_file(root, "package/agent-sdk/dist/bridge.js"),
        npm_sibling_bridge: write_file(
            root,
            "npm-prefix/node_modules/claude-code-rust/agent-sdk/dist/bridge.js",
        ),
        packaged_runtime: write_file(root, &format!("package/vendor/x86_64/{runtime}")),
        cargo_target_bridge: write_file(root, "manifest/target/agent-sdk/dist/bridge.js"),
        cwd_script: write_file(root, "repo/agent-sdk/dist/bridge.js"),
        manifest_script: write_file(root, "manifest/agent-sdk/dist/bridge.js"),
        env_script: write_file(root, "env/bridge.js"),
        env_runtime: write_file(root, &format!("env/{runtime}")),
        path_bun_runtime: write_file(root, "path/bun"),
        dir,
    }
}

impl ResolverFixture {
    fn resolver(&self, executable: Option<PathBuf>, development: bool) -> BridgeScriptResolver<'_> {
        BridgeScriptResolver {
            explicit_script: None,
            env_script: None,
            current_exe: executable,
            allow_dev_fallbacks: development,
            cwd_script: self.cwd_script.clone(),
            manifest_script: self.manifest_script.clone(),
        }
    }
}

#[test]
fn script_overrides_reject_invalid_targets_without_using_healthy_fallbacks() {
    for explicit in [true, false] {
        for directory in [false, true] {
            let fixture = resolver_fixture();
            let invalid = fixture.dir.path().join("invalid override.js");
            if directory {
                fs::create_dir(&invalid).unwrap();
            }
            let mut resolver = fixture.resolver(Some(fixture.installed_exe.clone()), true);
            if explicit {
                resolver.explicit_script = Some(&invalid);
                resolver.env_script = Some(fixture.env_script.clone());
            } else {
                resolver.env_script = Some(invalid.clone());
            }
            let error = resolver.resolve().unwrap_err();
            assert!(matches!(error.downcast_ref::<AppError>(), Some(AppError::BridgeSpawnFailed)));
            let inspection = resolver.inspect();
            assert_eq!(inspection.resolved_path, None);
            for message in [format!("{error:#}"), inspection.error.unwrap()] {
                assert!(message.contains(&invalid.display().to_string()), "{message}");
                assert!(
                    message.contains(if directory { "not a file" } else { "does not exist" }),
                    "{message}"
                );
                assert!(
                    message.contains(if explicit {
                        "bridge script"
                    } else {
                        "CLAUDE_RS_AGENT_BRIDGE"
                    }),
                    "{message}"
                );
            }
            assert_eq!(inspection.explicit_path.as_deref(), explicit.then_some(invalid.as_path()));
            assert_eq!(
                inspection.env_path.as_deref(),
                Some(if explicit { fixture.env_script.as_path() } else { invalid.as_path() })
            );
        }
    }
    let fixture = resolver_fixture();
    let explicit = write_file(fixture.dir.path(), "explicit/selected.js");
    let mut resolver = fixture.resolver(Some(fixture.installed_exe.clone()), true);
    resolver.explicit_script = Some(&explicit);
    resolver.env_script = Some(fixture.env_script.clone());
    assert_eq!(resolver.resolve().unwrap(), explicit);
    assert_eq!(resolver.inspect().resolved_path.as_ref(), Some(&explicit));
    assert_eq!(resolver.inspect().error, None);
}

#[test]
fn automatic_scripts_follow_installed_cargo_and_release_selection_policy() {
    for (label, executable, development, remove_bundle, remove_cwd, remove_manifest, winner) in [
        ("installed package", "installed", true, false, false, false, "package"),
        ("installed cwd", "unbundled", true, false, false, false, "cwd"),
        ("installed manifest", "unbundled", true, false, true, false, "manifest"),
        ("cargo manifest", "cargo", true, false, false, false, "manifest"),
        ("cargo cwd", "cargo", true, false, false, true, "cwd"),
        ("cargo bundle", "cargo", true, false, true, true, "target"),
        ("release package", "installed", false, false, false, false, "package"),
        ("release refuses source", "unbundled", false, false, false, false, "error"),
        ("no executable development", "none", true, false, false, false, "cwd"),
        ("no executable release", "none", false, false, false, false, "error"),
        ("no allowed candidates", "installed", true, true, true, true, "error"),
    ] {
        let fixture = resolver_fixture();
        for (remove, path) in [
            (remove_bundle, &fixture.packaged_bridge),
            (remove_cwd, &fixture.cwd_script),
            (remove_manifest, &fixture.manifest_script),
        ] {
            if remove {
                fs::remove_file(path).unwrap();
            }
        }
        let executable = match executable {
            "installed" => Some(fixture.installed_exe.clone()),
            "unbundled" => Some(fixture.unbundled_exe.clone()),
            "cargo" => Some(fixture.cargo_target_exe.clone()),
            "none" => None,
            _ => unreachable!(),
        };
        let resolver = fixture.resolver(executable, development);
        let inspection = resolver.inspect();
        let expected = match winner {
            "package" => Some(&fixture.packaged_bridge),
            "cwd" => Some(&fixture.cwd_script),
            "manifest" => Some(&fixture.manifest_script),
            "target" => Some(&fixture.cargo_target_bridge),
            "error" => None,
            _ => unreachable!(),
        };
        if let Some(expected) = expected {
            assert_eq!(&resolver.resolve().unwrap(), expected, "{label}");
            assert_eq!(inspection.resolved_path.as_ref(), Some(expected), "{label}");
            assert_eq!(inspection.error, None);
        } else {
            let error = resolver.resolve().unwrap_err().to_string();
            assert!(error.contains("bridge script not found"), "{label}: {error}");
            assert_eq!(inspection.resolved_path, None);
            assert!(inspection.error.is_some());
            if label == "no allowed candidates" {
                assert!(error.contains(&fixture.cwd_script.display().to_string()));
                assert!(error.contains("working-directory"));
                assert!(inspection.candidates.iter().any(|c| c.path == fixture.cwd_script
                    && c.source == "working-directory"
                    && !c.is_file));
            }
        }
    }
}

#[test]
fn automatic_script_search_skips_a_directory_before_the_real_package() {
    let fixture = resolver_fixture();
    let directory = fixture.installed_exe.parent().unwrap().join("agent-sdk/dist/bridge.js");
    fs::create_dir_all(&directory).unwrap();
    let resolver = fixture.resolver(Some(fixture.installed_exe.clone()), false);
    assert_eq!(resolver.resolve().unwrap(), fixture.packaged_bridge);
    let inspection = resolver.inspect();
    assert_eq!(inspection.resolved_path, Some(fixture.packaged_bridge));
    assert!(inspection.candidates.iter().any(|c| c.path == directory && !c.is_file));
}

#[cfg(unix)]
#[test]
fn raw_executable_symlink_resolves_the_target_package_at_both_consumers() {
    let fixture = resolver_fixture();
    let shim = fixture.dir.path().join("global-bin/claude-rs");
    fs::create_dir_all(shim.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&fixture.npm_platform_exe, &shim).unwrap();
    let resolver = fixture.resolver(Some(shim), false);
    assert_eq!(resolver.resolve().unwrap(), fixture.npm_sibling_bridge);
    assert_eq!(resolver.inspect().resolved_path, Some(fixture.npm_sibling_bridge));
}

#[test]
fn runtime_selection_and_inspection_preserve_path_priority_and_lookup_policy() {
    for (label, override_runtime, bundle, development, lookup_succeeds, kind) in [
        ("explicit", true, true, true, true, Some(BridgeRuntimeKind::Explicit)),
        ("bundled", false, true, true, true, Some(BridgeRuntimeKind::BundledBun)),
        ("path", false, false, true, true, Some(BridgeRuntimeKind::PathBun)),
        ("path failure", false, false, true, false, None),
        ("release failure", false, false, false, true, None),
    ] {
        let fixture = resolver_fixture();
        let executable = if bundle { &fixture.installed_exe } else { &fixture.unbundled_exe };
        let override_path = override_runtime.then_some(fixture.env_runtime.as_path());
        let resolved_lookup = Cell::new(false);
        let result =
            resolve_bridge_runtime_path_with(override_path, Some(executable), development, || {
                resolved_lookup.set(true);
                if lookup_succeeds {
                    Ok(fixture.path_bun_runtime.clone())
                } else {
                    Err(which::Error::CannotFindBinaryPath)
                }
            });
        let inspected_lookup = Cell::new(false);
        let inspection =
            inspect_bridge_runtime_with(override_path, Some(executable), development, || {
                inspected_lookup.set(true);
                if lookup_succeeds {
                    Ok(fixture.path_bun_runtime.clone())
                } else {
                    Err(which::Error::CannotFindBinaryPath)
                }
            });
        let expected_lookup = matches!(label, "path" | "path failure");
        assert_eq!(resolved_lookup.get(), expected_lookup, "{label}");
        assert_eq!(inspected_lookup.get(), expected_lookup, "{label}");
        let expected = match kind {
            Some(BridgeRuntimeKind::Explicit) => Some(&fixture.env_runtime),
            Some(BridgeRuntimeKind::BundledBun) => Some(&fixture.packaged_runtime),
            Some(BridgeRuntimeKind::PathBun) => Some(&fixture.path_bun_runtime),
            None => None,
        };
        if let Some(expected) = expected {
            assert_eq!(&result.unwrap(), expected, "{label}");
            assert_eq!(inspection.resolved_path.as_ref(), Some(expected));
            assert_eq!(inspection.error, None);
        } else {
            assert!(matches!(
                result.unwrap_err().downcast_ref::<AppError>(),
                Some(AppError::BridgeRuntimeNotFound)
            ));
            assert_eq!(inspection.resolved_path, None);
            assert!(inspection.error.is_some());
        }
        assert_eq!(inspection.resolved_kind, kind);
        assert_eq!(
            inspection.path_bun.as_ref(),
            (label == "path").then_some(&fixture.path_bun_runtime)
        );
    }
}

#[test]
fn invalid_development_runtime_overrides_cannot_select_available_fallbacks() {
    for directory in [false, true] {
        let fixture = resolver_fixture();
        let invalid = fixture.dir.path().join("invalid runtime");
        if directory {
            fs::create_dir(&invalid).unwrap();
        }
        let lookup = Cell::new(false);
        let error = resolve_bridge_runtime_path_with(
            Some(&invalid),
            Some(&fixture.installed_exe),
            true,
            || {
                lookup.set(true);
                Ok(fixture.path_bun_runtime.clone())
            },
        )
        .unwrap_err();
        assert!(matches!(error.downcast_ref::<AppError>(), Some(AppError::BridgeRuntimeNotFound)));
        let inspection =
            inspect_bridge_runtime_with(Some(&invalid), Some(&fixture.installed_exe), true, || {
                lookup.set(true);
                Ok(fixture.path_bun_runtime.clone())
            });
        assert!(!lookup.get());
        assert_eq!(inspection.resolved_path, None);
        assert_eq!(inspection.resolved_kind, None);
        assert_eq!(inspection.path_bun, None);
        assert_eq!(inspection.env_path, Some(invalid.clone()));
        for message in [format!("{error:#}"), inspection.error.unwrap()] {
            assert!(message.contains("CLAUDE_RS_AGENT_BRIDGE_RUNTIME"), "{message}");
            assert!(message.contains(&invalid.display().to_string()), "{message}");
            assert!(
                message.contains(if directory { "not a file" } else { "does not exist" }),
                "{message}"
            );
        }
        for (executable, expected) in [
            (&fixture.installed_exe, Some(&fixture.packaged_runtime)),
            (&fixture.unbundled_exe, None),
        ] {
            let result =
                resolve_bridge_runtime_path_with(Some(&invalid), Some(executable), false, || {
                    lookup.set(true);
                    Ok(fixture.path_bun_runtime.clone())
                });
            let inspected =
                inspect_bridge_runtime_with(Some(&invalid), Some(executable), false, || {
                    lookup.set(true);
                    Ok(fixture.path_bun_runtime.clone())
                });
            assert!(!lookup.get());
            if let Some(expected) = expected {
                assert_eq!(&result.unwrap(), expected);
            } else {
                assert!(matches!(
                    result.unwrap_err().downcast_ref::<AppError>(),
                    Some(AppError::BridgeRuntimeNotFound)
                ));
            }
            assert_eq!(inspected.resolved_path.as_ref(), expected);
            assert_eq!(inspected.resolved_kind, expected.map(|_| BridgeRuntimeKind::BundledBun));
            assert_eq!(inspected.error.is_some(), expected.is_none());
        }
    }
}

#[tokio::test]
async fn launcher_preserves_single_script_argument_diagnostics_and_separate_pipes() {
    for (enabled, expected_diagnostics) in [(false, "0"), (true, "1")] {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("bridge target \u{96ea}.js");
        fs::write(&script, "throw new Error('the argv fixture must not execute this script');")
            .unwrap();
        let probe = directory.path().join("runtime probe.cjs");
        fs::write(&probe,r"const fs = require('node:fs');
const input = fs.readFileSync(0, 'utf8');
fs.writeSync(1, JSON.stringify({argv: process.argv.slice(1), diagnostics: process.env.CLAUDE_RS_BRIDGE_DIAGNOSTICS, input}));
fs.writeSync(2, 'separate-stderr');
process.exit(0);
").unwrap();
        let launcher = BridgeLauncher {
            runtime_path: which::which("node").unwrap(),
            script_path: script.clone(),
        };
        let mut command = launcher.command(enabled);
        command.env(
            "NODE_OPTIONS",
            format!("--require \"{}\"", probe.to_string_lossy().replace('\\', "/")),
        );
        let mut child = command.spawn().unwrap();
        let mut input = child.stdin.take().expect("piped stdin");
        input.write_all(b"fixture-input\n").await.unwrap();
        drop(input);
        let output = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
            serde_json::json!({
            "argv":[script.to_str().unwrap()],"diagnostics":expected_diagnostics,"input":"fixture-input\n"})
        );
        assert_eq!(output.stderr, b"separate-stderr");
    }
}

struct OwnedProcessProbe {
    runtime: PathBuf,
    pid: Option<u32>,
}

impl OwnedProcessProbe {
    async fn alive(&self) -> bool {
        let mut command = Command::new(&self.runtime);
        command.env_remove("NODE_OPTIONS")
            .args(["-e", "try { process.kill(Number(process.argv[1]), 0); } catch(e) { process.exit(e.code === 'ESRCH' ? 3 : 2); }", "--"])
            .arg(self.pid.unwrap().to_string()).kill_on_drop(true);
        let output =
            tokio::time::timeout(Duration::from_secs(10), command.output()).await.unwrap().unwrap();
        match output.status.code() {
            Some(0) => true,
            Some(3) => false,
            other => panic!("process probe failed: {other:?} {output:?}"),
        }
    }
}

impl Drop for OwnedProcessProbe {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            // Failure cleanup targets only the direct child created by this test.
            let _ = std::process::Command::new(&self.runtime).env_remove("NODE_OPTIONS")
                .args(["-e", "try { process.kill(Number(process.argv[1]), 'SIGKILL'); } catch(e) { if(e.code !== 'ESRCH') throw e; }", "--"])
                .arg(pid.to_string()).status();
        }
    }
}

#[tokio::test]
async fn dropping_the_launched_owner_terminates_the_ready_child_after_stdin_eof() {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("owned-child.cjs");
    fs::write(&script,r"const fs = require('node:fs');
fs.writeSync(1, JSON.stringify({event:'ready',pid:process.pid})+'\n');
process.stdin.on('end', () => fs.writeSync(1, JSON.stringify({event:'stdin_closed',pid:process.pid})+'\n'));
process.stdin.resume();
setInterval(() => {}, 1000);
").unwrap();
    let runtime = which::which("node").unwrap();
    let launcher = BridgeLauncher { runtime_path: runtime.clone(), script_path: script };
    let mut child = launcher.command(false).env_remove("NODE_OPTIONS").spawn().unwrap();
    let pid = child.id().unwrap();
    let mut probe = OwnedProcessProbe { runtime, pid: Some(pid) };
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), output.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line).unwrap(),
        serde_json::json!({"event":"ready","pid":pid})
    );
    drop(child.stdin.take());
    line.clear();
    tokio::time::timeout(Duration::from_secs(10), output.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line).unwrap(),
        serde_json::json!({"event":"stdin_closed","pid":pid})
    );
    assert!(probe.alive().await, "fixture must survive stdin EOF before owner drop");
    drop(child);
    let mut tail = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), output.read_to_end(&mut tail))
        .await
        .unwrap()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while probe.alive().await {
        assert!(Instant::now() < deadline, "fixture process survived owner drop");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    probe.pid = None;
}
