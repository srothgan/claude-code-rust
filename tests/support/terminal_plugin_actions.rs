// SPDX-License-Identifier: Apache-2.0
use super::*;

#[allow(clippy::expect_used)]
pub(super) fn fixture_project(project: &Path) -> PathBuf {
    // Unix temp roots may be aliases (for example /var on macOS). Give the app
    // the same physical cwd that its CLI child reports in registration metadata.
    #[cfg(unix)]
    {
        project.canonicalize().expect("physical fixture project")
    }
    #[cfg(not(unix))]
    {
        project.to_path_buf()
    }
}

#[allow(clippy::expect_used)]
pub(super) fn setup(profile: &Path, project: &Path) {
    let state = serde_json::json!({
        "installed": [
            {"id":"alpha@other", "scope":"user", "enabled":true, "version":"1.0"},
            {"id":"target@target-market", "scope":"project", "enabled":true, "version":"1.0", "projectPath":project},
            {"id":"target@target-market", "scope":"user", "enabled":true, "version":"1.0"}
        ],
        "available": [
            {"pluginId":"alpha@other", "name":"Alpha", "marketplaceName":"other"},
            {"pluginId":"new-plugin@target-market", "name":"New Plugin", "marketplaceName":"target-market"}
        ],
        "marketplaces": [
            {"name":"alpha-market", "source":"github", "repo":"other/repository"},
            {"name":"target-market", "source":"github", "repo":"team/repository"}
        ],
        "unrelated":"preserve"
    });
    std::fs::write(profile.join("persistent-plugins.json"), state.to_string())
        .expect("registry fixture");
}

#[allow(clippy::expect_used)]
fn registry(test: &TerminalTest) -> Value {
    serde_json::from_slice(
        &std::fs::read(test.temp.path().join("profile/persistent-plugins.json")).expect("registry"),
    )
    .expect("registry JSON")
}
#[allow(clippy::expect_used)]
fn calls(test: &TerminalTest) -> Vec<Value> {
    std::fs::read_to_string(test.temp.path().join("profile/plugin-cli.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("CLI journal JSON"))
        .collect()
}
fn mutations(test: &TerminalTest) -> Vec<Value> {
    calls(test)
        .into_iter()
        .filter(|call| {
            !call["argv"].as_array().is_some_and(|args| args.iter().any(|arg| arg == "--json"))
        })
        .collect()
}
fn modal(test: &TerminalTest, title: &str) -> Vec<String> {
    let rows = test.screen_rows();
    let Some(top) = rows.iter().position(|row| row.contains('\u{250c}') && row.contains(title))
    else {
        return vec![];
    };
    let border: Vec<_> = rows[top].chars().collect();
    let Some(left) = border.iter().position(|ch| *ch == '\u{250c}') else {
        return vec![];
    };
    let Some(right) = border.iter().position(|ch| *ch == '\u{2510}') else {
        return vec![];
    };
    rows[top + 1..]
        .iter()
        .take_while(|row| row.chars().nth(left) != Some('\u{2514}'))
        .map(|row| row.chars().skip(left + 1).take(right - left - 1).collect())
        .collect()
}
fn selected(test: &TerminalTest, title: &str) -> Option<String> {
    modal(test, title).into_iter().find(|row| row.contains("\u{203a} "))
}
#[allow(clippy::panic)]
fn action(test: &mut TerminalTest, title: &str, label: &str) {
    for _ in 0..5 {
        let current = test.wait_for("selected plugin action", |test| selected(test, title));
        if current.contains(&format!("\u{203a} {label}")) {
            return;
        }
        test.send(b"\x1b[B");
        test.wait_until("next plugin action", |test| {
            selected(test, title).as_ref() != Some(&current)
        });
    }
    panic!("missing action {label}: {}", test.screen());
}
fn close(test: &mut TerminalTest, title: &str) {
    test.send(b"\x1b");
    test.wait_until("overlay closed", |test| modal(test, title).is_empty());
}
fn open(test: &mut TerminalTest, title: &str) {
    test.send(b"\r");
    test.wait_until("plugin overlay", |test| !modal(test, title).is_empty());
}
fn next_row(test: &mut TerminalTest) {
    let before = test.screen();
    test.send(b"\x1b[B");
    test.wait_until("next inventory selection", |test| test.screen() != before);
}
fn start() -> TerminalTest {
    let mut test = TerminalTest::start_with_options(
        "persistent-plugins",
        3,
        Some("persistent-plugins"),
        &[],
        false,
    );
    test.submit("/plugins", "/plugins");
    test.wait_until("persistent inventory", |test| {
        test.screen_rows().iter().any(|row| row.contains("Plugin: alpha@other"))
    });
    test
}
#[allow(clippy::panic)]
fn installed_target(test: &mut TerminalTest) {
    for _ in 0..5 {
        open(test, "Installed plugin");
        let rows = modal(test, "Installed plugin");
        if rows.iter().any(|row| row.contains("Target"))
            && rows.iter().any(|row| row.contains("project scope"))
        {
            return;
        }
        close(test, "Installed plugin");
        next_row(test);
    }
    panic!("missing project target: {}", test.screen());
}
#[allow(clippy::expect_used)]
fn completed(test: &mut TerminalTest, expected: &Value, reloads: usize, argv: &[&str]) {
    test.wait_until("persisted operation and runtime reload", |test| {
        registry(test) == *expected && test.commands("reload_plugins").len() == reloads + 1
    });
    let writes = mutations(test);
    let last = writes.last().expect("mutation recorded");
    assert_eq!(last["argv"], serde_json::json!(argv));
    assert_eq!(
        Path::new(last["cwd"].as_str().expect("cwd")).canonicalize().expect("recorded cwd"),
        test.temp.path().join("project").canonicalize().expect("project cwd")
    );
    // Completion is correlated to the action and its three inventory reads.
    let reads = calls(test);
    assert_eq!(
        reads[reads.len() - 3..].iter().map(|call| &call["argv"]).collect::<Vec<_>>(),
        vec![
            &serde_json::json!(["plugin", "list", "--json"]),
            &serde_json::json!(["plugin", "list", "--available", "--json"]),
            &serde_json::json!(["plugin", "marketplace", "list", "--json"])
        ]
    );
}

#[test]
fn installed_actions_preserve_other_scopes_and_uninstall_requires_explicit_confirmation() {
    let mut test = start();
    let mut expected = registry(&test);
    installed_target(&mut test);
    action(&mut test, "Installed plugin", "Update");
    test.send(b"\x1b[A");
    test.wait_until("up selects disable", |test| {
        selected(test, "Installed plugin").is_some_and(|row| row.contains("Disable"))
    });
    close(&mut test, "Installed plugin");
    assert!(mutations(&test).is_empty());
    assert_eq!(registry(&test), expected);
    installed_target(&mut test);
    std::fs::write(test.temp.path().join("profile/plugin-failure"), "disable").unwrap();
    let reloads = test.commands("reload_plugins").len();
    test.send(b"\r");
    test.wait_screen("FIXTURE_PLUGIN_WRITE_DENIED");
    assert_eq!(registry(&test), expected);
    assert_eq!(test.commands("reload_plugins").len(), reloads);
    std::fs::remove_file(test.temp.path().join("profile/plugin-failure")).unwrap();
    installed_target(&mut test);
    test.send(b"\r");
    expected["installed"][1]["enabled"] = Value::Bool(false);
    completed(
        &mut test,
        &expected,
        reloads,
        &["plugin", "disable", "target@target-market", "--scope", "project"],
    );
    installed_target(&mut test);
    assert!(selected(&test, "Installed plugin").unwrap().contains("Enable"));
    action(&mut test, "Installed plugin", "Update");
    let reloads = test.commands("reload_plugins").len();
    test.send(b"\r");
    expected["installed"][1]["version"] = "2.0".into();
    completed(
        &mut test,
        &expected,
        reloads,
        &["plugin", "update", "target@target-market", "--scope", "project"],
    );
    installed_target(&mut test);
    let reloads = test.commands("reload_plugins").len();
    test.send(b"\r");
    expected["installed"][1]["enabled"] = Value::Bool(true);
    completed(
        &mut test,
        &expected,
        reloads,
        &["plugin", "enable", "target@target-market", "--scope", "project"],
    );
    installed_target(&mut test);
    action(&mut test, "Installed plugin", "Uninstall");
    let writes = mutations(&test);
    for cancel in [b"\r".as_slice(), b"\x1b".as_slice()] {
        open(&mut test, "Uninstall Target");
        test.send(cancel);
        test.wait_until("cancel restores installed actions", |test| {
            selected(test, "Installed plugin").is_some_and(|row| row.contains("Uninstall"))
        });
        assert_eq!(mutations(&test), writes);
        assert_eq!(registry(&test), expected);
    }
    open(&mut test, "Uninstall Target");
    let reloads = test.commands("reload_plugins").len();
    test.send(b"\x1b[B\r");
    expected["installed"].as_array_mut().unwrap().remove(1);
    completed(
        &mut test,
        &expected,
        reloads,
        &["plugin", "uninstall", "target@target-market", "--scope", "project"],
    );
    let expected_calls = ["disable", "disable", "update", "enable", "uninstall"].map(|action| {
        serde_json::json!(["plugin", action, "target@target-market", "--scope", "project"])
    });
    assert_eq!(
        mutations(&test).iter().map(|call| &call["argv"]).collect::<Vec<_>>(),
        expected_calls.iter().collect::<Vec<_>>()
    );
    test.shutdown();
}

#[test]
fn catalog_install_uses_selected_scope_after_navigation_and_cancel() {
    for (scope, label) in [
        ("user", "Install for user"),
        ("project", "Install for project"),
        ("local", "Install locally"),
    ] {
        let mut test = start();
        let mut expected = registry(&test);
        test.send(b"\x1b[C");
        test.wait_screen("New Plugin");
        next_row(&mut test);
        open(&mut test, "Install plugin");
        assert!(modal(&test, "Install plugin").iter().any(|row| row.contains("New Plugin")));
        action(&mut test, "Install plugin", "Install for project");
        test.send(b"\x1b[A");
        test.wait_until("up selects user installation", |test| {
            selected(test, "Install plugin").is_some_and(|row| row.contains("Install for user"))
        });
        close(&mut test, "Install plugin");
        assert!(mutations(&test).is_empty());
        assert_eq!(registry(&test), expected);
        open(&mut test, "Install plugin");
        action(&mut test, "Install plugin", label);
        let reloads = test.commands("reload_plugins").len();
        test.send(b"\r");
        let mut entry = serde_json::json!({"id":"new-plugin@target-market", "scope":scope, "enabled":true, "version":"1.0"});
        if scope != "user" {
            entry["projectPath"] =
                fixture_project(&test.project_dir()).to_string_lossy().into_owned().into();
        }
        expected["installed"].as_array_mut().unwrap().push(entry);
        completed(
            &mut test,
            &expected,
            reloads,
            &["plugin", "install", "new-plugin@target-market", "--scope", scope],
        );
        assert_eq!(mutations(&test).len(), 1, "only the confirmed install may execute");
        test.shutdown();
    }
}

#[test]
fn marketplace_editing_and_confirmed_removal_preserve_installed_plugins() {
    let mut test = start();
    let mut expected = registry(&test);
    test.send(b"\x1b[C\x1b[C");
    test.wait_screen("Marketplace: alpha-market");
    next_row(&mut test);
    next_row(&mut test);
    open(&mut test, "Add Marketplace");
    test.paste("discarded/source");
    test.wait_screen("discarded/source");
    close(&mut test, "Add Marketplace");
    assert!(mutations(&test).is_empty());
    open(&mut test, "Add Marketplace");
    assert!(!modal(&test, "Add Marketplace").iter().any(|row| row.contains("discarded/source")));
    test.paste("team/new-marketX");
    test.wait_screen("team/new-marketX");
    test.send(b"\x1b[D\x1b[3~");
    test.wait_until("edited source", |test| {
        modal(test, "Add Marketplace")
            .iter()
            .any(|row| row.contains("team/new-market") && !row.contains("team/new-marketX"))
    });
    let reloads = test.commands("reload_plugins").len();
    test.send(b"\r");
    expected["marketplaces"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"name":"new-market","source":"github","repo":"team/new-market"}));
    completed(
        &mut test,
        &expected,
        reloads,
        &["plugin", "marketplace", "add", "team/new-market", "--scope", "user"],
    );
    // Sorted list keeps the selected index on Target; prove the target in its modal.
    open(&mut test, "Marketplace");
    assert!(modal(&test, "Marketplace").iter().any(|row| row.contains("Target Market")));
    action(&mut test, "Marketplace", "Remove");
    test.send(b"\x1b[A");
    test.wait_until("up selects marketplace update", |test| {
        selected(test, "Marketplace").is_some_and(|row| row.contains("Update"))
    });
    let reloads = test.commands("reload_plugins").len();
    test.send(b"\r");
    expected["marketplaces"][1]["repo"] = "updated/repository".into();
    completed(&mut test, &expected, reloads, &["plugin", "marketplace", "update", "target-market"]);
    open(&mut test, "Marketplace");
    action(&mut test, "Marketplace", "Remove");
    let writes = mutations(&test);
    for cancel in [b"\r".as_slice(), b"\x1b".as_slice()] {
        open(&mut test, "Remove Target Market");
        test.send(cancel);
        test.wait_until("cancel restores marketplace actions", |test| {
            selected(test, "Marketplace").is_some_and(|row| row.contains("Remove"))
        });
        assert_eq!(mutations(&test), writes);
        assert_eq!(registry(&test), expected);
    }
    open(&mut test, "Remove Target Market");
    let reloads = test.commands("reload_plugins").len();
    test.send(b"\x1b[B\r");
    expected["marketplaces"].as_array_mut().unwrap().remove(1);
    completed(&mut test, &expected, reloads, &["plugin", "marketplace", "remove", "target-market"]);
    assert_eq!(mutations(&test).len(), 3, "only add, update and confirmed removal may execute");
    assert!(test.prompts().is_empty(), "marketplace source must never reach chat");
    test.send(b"\x1b");
    test.wait_screen("Type a message");
    test.submit("FOLLOWUP_MARKETPLACE", "FOLLOWUP_MARKETPLACE");
    test.wait_until("next chat submission", |test| !test.prompts().is_empty());
    test.assert_prompts(&["FOLLOWUP_MARKETPLACE"]);
    test.shutdown();
}
