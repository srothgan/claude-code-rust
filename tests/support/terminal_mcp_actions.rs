// SPDX-License-Identifier: Apache-2.0
use super::*;

#[allow(clippy::expect_used)]
fn documents(test: &TerminalTest) -> Vec<Value> {
    [
        test.temp.path().join("profile/user-mcp.json"),
        test.temp.path().join("project/.mcp.json"),
        test.temp.path().join("project/local-mcp.json"),
    ]
    .iter()
    .map(|file| {
        serde_json::from_slice(&std::fs::read(file).expect("read isolated MCP fixture"))
            .expect("parse isolated MCP fixture")
    })
    .collect()
}
fn cli_calls(test: &TerminalTest) -> Vec<String> {
    std::fs::read_to_string(test.temp.path().join("profile/mcp-cli-calls"))
        .unwrap_or_default()
        .lines()
        .map(Into::into)
        .collect()
}
fn modal_rows(test: &TerminalTest, title: &str) -> Vec<String> {
    let rows = test.screen_rows();
    let Some(top) = rows.iter().position(|r| r.contains('\u{250c}') && r.contains(title)) else {
        return vec![];
    };
    rows[top + 1..].iter().take_while(|r| !r.contains('\u{2514}')).cloned().collect()
}
fn wait_details(test: &mut TerminalTest) {
    test.wait_until("target details actions", |test| {
        modal_rows(test, "target").iter().any(|row| row.contains("Reconnect server"))
    });
}
fn remove_dialog(test: &mut TerminalTest) {
    test.send(b"\r");
    wait_details(test);
    for _ in 0..10 {
        let selected = test.wait_for("selected MCP action", |test| {
            modal_rows(test, "target").into_iter().find(|r| r.contains("\u{203a} "))
        });
        if selected.contains("\u{203a} Remove") {
            break;
        }
        test.send(b"\x1b[B");
        test.wait_until("next MCP action", |test| !modal_rows(test, "target").contains(&selected));
    }
    assert!(modal_rows(test, "target").iter().any(|r| r.contains("\u{203a} Remove")));
    test.send(b"\r");
    test.wait_screen_with("Remove MCP server target", &["Cancel"]);
}
fn confirm_remove(test: &mut TerminalTest) {
    test.send(b"\x1b[B");
    test.send(b"\r");
}
#[test]
fn persisted_mcp_removal_uses_exact_cli_scope_and_cwd_and_preserves_other_documents() {
    for (scope, index, failure) in [("user", 0, false), ("project", 1, false), ("local", 2, true)] {
        let scenario =
            format!("mcp-remove-{scope}-{}", if failure { "failure" } else { "success" });
        let mode = format!("mcp-{scope}");
        let mut test = TerminalTest::start_with_options(&scenario, 3, Some(&mode), &[], false);
        let before = documents(&test);
        test.submit("/mcp", "/mcp");
        test.wait_screen_with("target", &["sibling"]);
        remove_dialog(&mut test);
        test.send(b"\r");
        wait_details(&mut test);
        assert_eq!(documents(&test), before);
        assert!(cli_calls(&test).is_empty());
        test.send(b"\r");
        test.wait_screen("Remove MCP server target");
        test.send(b"\x1b");
        wait_details(&mut test);
        assert_eq!(documents(&test), before);
        assert!(cli_calls(&test).is_empty());
        test.send(b"\r");
        test.wait_screen("Remove MCP server target");
        let snapshots = test.commands("get_mcp_snapshot").len();
        let reloads = test.commands("reload_plugins").len();
        confirm_remove(&mut test);
        if failure {
            test.wait_screen("FIXTURE_MCP_WRITE_DENIED");
            assert!(
                modal_rows(&test, "target").iter().any(|r| r.contains("FIXTURE_MCP_WRITE_DENIED"))
            );
            assert_eq!(documents(&test), before);
            assert_eq!(test.commands("reload_plugins").len(), reloads);
            std::fs::remove_file(test.temp.path().join("profile/mcp-fail")).unwrap();
            test.send(b"\r");
            test.wait_screen("Remove MCP server target");
            confirm_remove(&mut test);
        }
        test.wait_until("CLI persistence and runtime reload", |test| {
            test.commands("reload_plugins").len() == reloads + 1
                && documents(test)[index]["mcpServers"].get("target").is_none()
        });
        test.wait_until("confirmed MCP list excludes target", |test| {
            let rows = test.screen_rows();
            rows.iter().any(|r| r.contains("sibling") && r.contains("connected"))
                && !rows.iter().any(|r| r.contains("target") && r.contains("connected"))
        });
        test.signal("mcp-confirm");
        test.wait_until("confirming snapshot updates the surviving row", |test| {
            test.screen_rows().iter().any(|r| r.contains("sibling") && r.contains("disabled"))
        });
        let mut expected = before;
        expected[index]["mcpServers"].as_object_mut().unwrap().remove("target");
        assert_eq!(documents(&test), expected);
        assert_eq!(
            test.commands("get_mcp_snapshot").len(),
            snapshots,
            "reload owns the confirming snapshot"
        );
        let calls = cli_calls(&test);
        let expected_calls = if failure { 2 } else { 1 };
        assert_eq!(calls.len(), expected_calls * 2);
        for call in calls.chunks_exact(2) {
            assert_eq!(call[0], format!("mcp\tremove\t--scope\t{scope}\ttarget"));
            assert_eq!(
                Path::new(&call[1]).canonicalize().unwrap(),
                test.temp.path().join("project").canonicalize().unwrap()
            );
        }
        test.shutdown();
    }
}
