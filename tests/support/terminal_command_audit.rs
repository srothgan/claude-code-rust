// SPDX-License-Identifier: Apache-2.0

use super::*;

pub(super) fn activity_matches(test: &TerminalTest, active: bool, tips: bool) -> bool {
    let Some((_, activity)) = test.captured_region(0, "activity") else { return false };
    let Some((_, live)) = test.captured_region(0, "live") else { return false };
    let Some((_, editor)) = test.captured_region(0, "editor") else { return false };
    let has_activity =
        activity.iter().any(|row| !row.contains("Tip:") && row.chars().any(char::is_alphanumeric));
    has_activity == active
        && activity.iter().any(|row| row.contains("Tip:")) == tips
        && !live.iter().any(|row| assistant_marker(row))
        && !editor.iter().any(|row| row.contains("NO_OUTPUT_YET"))
        && test.transcript_rows().iter().any(|row| row.contains("NO_OUTPUT_YET"))
}

pub(super) fn message_clocks_match(test: &TerminalTest) -> bool {
    let rows = test.transcript_rows();
    let Some(user) = rows.iter().position(|row| row.contains("Clock test")) else { return false };
    let Some(answer) = rows.iter().position(|row| row.contains("reply 1 started")) else {
        return false;
    };
    if user >= answer {
        return false;
    }
    // SDK metadata is rendered in the padding row immediately above its
    // message, not in the message body. Associate each distinct clock with
    // its own body before checking the assistant-only duration below it.
    let previous_text = |at: usize| rows[..at].iter().rev().find(|row| !row.trim().is_empty());
    previous_text(user).is_some_and(|row| row.trim() == "15:01Z")
        && previous_text(answer).is_some_and(|row| row.trim() == "15:02Z")
        && !rows[user..answer.saturating_sub(1)].iter().any(|row| row.contains("Elapsed"))
        && rows[answer..].iter().any(|row| row.contains("Elapsed 2.0s") && row.contains("Done "))
}

pub(super) fn code_choice_selected(test: &TerminalTest) -> bool {
    test.screen_rows().iter().any(|row| row.contains("› Code block 1 (rust)"))
        && !test.screen_rows().iter().any(|row| row.contains("› Full response"))
}

#[allow(clippy::expect_used)]
pub(super) fn saved_title(test: &TerminalTest, session: &str, title: &str) -> bool {
    let resolved = test.project_dir().canonicalize().expect("fixture project");
    let resolved = resolved.to_string_lossy();
    let resolved = resolved.strip_prefix(r"\\?\").unwrap_or(&resolved);
    let key: String =
        resolved.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let path = test.temp.path().join("profile/projects").join(key).join(format!("{session}.jsonl"));
    std::fs::read_to_string(path).is_ok_and(|text| text.lines().any(|line| {
        serde_json::from_str::<Value>(line).is_ok_and(|value| value == serde_json::json!({"type":"custom-title", "customTitle":title, "sessionId":session}))
    }))
}

#[allow(clippy::expect_used)]
pub(super) fn complete_export(path: &Path, project: &Path) -> bool {
    let Ok(file) = std::fs::read_to_string(path) else { return false };
    let Some((header, body)) = file.split_once("\n\n## User\n\n") else { return false };
    let expected = project.canonicalize().expect("fixture project exists");
    let directory = header.lines().find_map(|line| line.strip_prefix("- Directory: "));
    header.starts_with("# Conversation\n\n- Session: fake-session\n")
        && directory
            .is_some_and(|value| Path::new(value).canonicalize().ok().as_ref() == Some(&expected))
        && header.contains("\n- Model: Fake model\n- Exported: ")
        && format!("## User\n\n{body}") == EXPORTED_BODY
        && !file.contains('\u{1b}')
}

/// Probe only the PID written by this test's isolated native CLI.
#[cfg(windows)]
#[allow(unsafe_code)]
pub(super) fn process_alive(pid: u32) -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn WaitForSingleObject(handle: *mut std::ffi::c_void, milliseconds: u32) -> u32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    // SAFETY: The fixture supplies the PID. The owned handle is queried without
    // waiting or mutating the process, then closed exactly once.
    unsafe {
        let handle = OpenProcess(0x0010_0000, 0, pid); // SYNCHRONIZE
        if handle.is_null() {
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(87),
                "only a nonexistent PID establishes exit"
            );
            return false;
        }
        let result = WaitForSingleObject(handle, 0);
        CloseHandle(handle);
        assert_ne!(result, u32::MAX, "owned process handle must be queryable");
        result == 258 // WAIT_TIMEOUT means the process has not exited.
    }
}

#[cfg(unix)]
#[allow(unsafe_code, clippy::expect_used)]
pub(super) fn process_alive(pid: u32) -> bool {
    // SAFETY: Signal zero only probes existence; it does not send a signal.
    let result = unsafe { libc::kill(i32::try_from(pid).expect("fixture PID"), 0) };
    if result == 0 {
        return true;
    }
    let error = std::io::Error::last_os_error();
    assert_eq!(error.raw_os_error(), Some(libc::ESRCH), "PID probe failed: {error}");
    false
}

#[test]
fn compact_preserves_history_and_restores_input_after_success_or_failure() {
    for (scenario, feedback) in [
        ("compact-audit-success", "compacted"),
        ("compact-audit-failure", "FIXTURE_COMPACT_DENIED"),
    ] {
        let mut test = TerminalTest::start(scenario, 3);
        test.submit("BEFORE_COMPACTION", "BEFORE_COMPACTION");
        test.wait_turn_finished("reply 1 started");
        test.submit_command("/compact");
        test.wait_turn_finished(feedback);
        test.assert_prompts(&["BEFORE_COMPACTION", "/compact "]);
        assert_eq!(test.prompts()[1]["session_id"], "fake-session");
        test.submit("AFTER_COMPACTION", "AFTER_COMPACTION");
        test.wait_turn_finished("reply 2 started");
        test.assert_prompts(&["BEFORE_COMPACTION", "/compact ", "AFTER_COMPACTION"]);
        test.finish_replay();
        let history = test.transcript_rows();
        let position =
            |needle: &str| history.iter().position(|row| row.contains(needle)).expect(needle);
        assert!(position("BEFORE_COMPACTION") < position(feedback));
        assert!(position(feedback) < position("AFTER_COMPACTION"));
        test.assert_turn_source("BEFORE_COMPACTION", Some("/compact"), 3);
    }
}

#[allow(clippy::expect_used)]
pub(super) fn logout_and_relogin(test: &mut TerminalTest) {
    let profile = test.temp.path().join("profile");
    let credentials = profile.join(".credentials.json");
    test.submit_command("/logout");
    test.wait_until("logout removes credentials and returns unauthenticated session", |test| {
        !credentials.exists()
            && test.journal().iter().any(|e| e["event"] == "auth_required")
            && test.screen().contains("Authentication required: fixture-login")
    });
    assert_eq!(
        std::fs::read_to_string(profile.join("auth-calls")).expect("auth argv"),
        "auth\tlogin\nauth\tlogout\n"
    );
    test.assert_prompts(&["AFTER_AUTH 界 + PASTE 🦀"]);
    test.submit_command("/login");
    test.wait_screen("AUTH_CHILD_READY");
    test.send(b"second_login\r");
    test.wait_screen("AUTH_INPUT_RECORDED");
    test.release_auth_child();
    test.wait_until("login after logout completed", |test| {
        credentials.exists()
            && test.commands("new_session").len() == 3
            && test.screen().contains("[READY]")
    });
    test.submit("AFTER_LOGOUT_RELOGIN", "AFTER_LOGOUT_RELOGIN");
    test.wait_turn_finished("reply 2 started");
    test.assert_prompts(&["AFTER_AUTH 界 + PASTE 🦀", "AFTER_LOGOUT_RELOGIN"]);
    assert_eq!(test.prompts()[1]["session_id"], "fake-session-after-login");
    assert_eq!(
        std::fs::read_to_string(profile.join("auth-calls")).expect("auth argv"),
        "auth\tlogin\nauth\tlogout\nauth\tlogin\n"
    );
}

pub(super) fn reload_hidden_footer(inherited: &Path, saved: &str) {
    let mut restarted = TerminalTest::start_with_profile(
        "agents",
        3,
        None,
        &[],
        false,
        &[],
        AppProfileFixture {
            inherited_directory: Some(inherited),
            contents: Some(saved),
            ..AppProfileFixture::default()
        },
    );
    restarted.wait_until("restarted app loads the hidden footer", |test| {
        test.screen_rows().iter().any(|row| row.contains("[READY]") && !row.contains("AGENTS:"))
    });
    restarted.send(b"\x1b[D");
    restarted.wait_screen_with("Review parser", &["Subagents", "running"]);
    restarted.send(b"\x1b[A");
    restarted.wait_until("reloaded preference keeps agent details accessible", |test| {
        test.screen().contains("Check the parser for edge cases")
    });
    restarted.send(b"\x1b");
    restarted.wait_screen("Type a message");
    assert!(
        restarted
            .screen_rows()
            .iter()
            .any(|row| row.contains("[READY]") && !row.contains("AGENTS:"))
    );
    restarted.assert_prompts(&[]);
    restarted.shutdown();
}
