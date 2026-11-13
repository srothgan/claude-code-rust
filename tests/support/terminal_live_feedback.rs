// SPDX-License-Identifier: Apache-2.0
use super::*;

pub(super) fn configure_home(command: &mut CommandBuilder, scenario: &str, project: &Path) {
    if scenario == "hold-title-home" {
        command.env("HOME", project);
        command.env("USERPROFILE", project);
    }
}

// ConPTY forwards window title OSC as OSC 0; Unix PTYs retain OSC 2.
#[allow(clippy::expect_used)]
fn titles(bytes: &[u8]) -> Vec<String> {
    let mut result = Vec::new();
    let mut at = 0;
    while at + 4 < bytes.len() {
        if bytes[at..].starts_with(b"\x1b]2;") || bytes[at..].starts_with(b"\x1b]0;") {
            let start = at + 4;
            if let Some(end) = bytes[start..].iter().position(|b| *b == 7 || *b == 27) {
                result.push(
                    String::from_utf8(bytes[start..start + end].to_vec()).expect("UTF-8 title"),
                );
                at = start + end;
            }
        }
        at += 1;
    }
    result
}
fn emitted(test: &TerminalTest, offset: usize) -> Vec<String> {
    titles(&test.raw_since(offset))
}
fn last(test: &TerminalTest, offset: usize) -> Option<String> {
    emitted(test, offset).pop()
}

fn shutdown_title(test: &mut TerminalTest) {
    let offset = test.raw_offset();
    test.shutdown();
    test.wait_shutdown_screen();
    let started = Instant::now();
    loop {
        if last(test, offset).as_deref() == Some("Orbital Lab") {
            break;
        }
        assert!(
            started.elapsed() < WAIT_LIMIT,
            "missing final restored title: {:?}",
            emitted(test, offset)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(last(test, 0).as_deref(), Some("Orbital Lab"));
}

fn save_title_setting(
    test: &mut TerminalTest,
    label: &str,
    id: &str,
    pointer: &str,
    key: &[u8],
    expected: Option<bool>,
) -> usize {
    test.submit("/config", "/config");
    test.wait_settings_ready(label);
    test.select_setting(label);
    let count = test.commands("mutate_setting").len();
    let offset = test.raw_offset();
    test.send(key);
    test.wait_setting(
        "profile/settings.json",
        pointer,
        expected.as_ref().map(|b| if *b { &Value::Bool(true) } else { &Value::Bool(false) }),
    );
    test.wait_until("correlated setting acknowledgement", |test| {
        let commands = test.commands("mutate_setting");
        commands.get(count).is_some_and(|command| {
            command["mutation"]["id"] == id
                && test.journal().iter().any(|e| {
                    e["event"] == "settings_result"
                        && e["request_id"] == command["request_id"]
                        && e["result"]["persistence"] == "saved"
                })
        })
    });
    test.send(b"\x1b");
    offset
}

#[test]
fn live_title_tracks_acknowledged_preferences_and_restores_the_raw_project_on_exit() {
    let mut test = TerminalTest::start("hold-title-home", 2);
    #[cfg(unix)]
    assert!(
        test.screen_rows().iter().any(|row| row.contains("Loc: ~")),
        "selected HOME must actually abbreviate the displayed directory"
    );
    test.wait_until("idle title names the isolated home project", |test| {
        last(test, 0).is_some_and(|title| title.ends_with(" Orbital Lab"))
    });
    let idle = last(&test, 0).unwrap();
    let offset = test.raw_offset();
    test.submit("TITLE_WORK", "TITLE_WORK");
    test.wait_journal("reply-held");
    test.wait_until("active title changes while preserving project identity", |test| {
        let active: std::collections::BTreeSet<_> =
            emitted(test, offset).into_iter().filter(|t| t != &idle).collect();
        active.len() >= 2 && active.iter().all(|title| title.ends_with(" Orbital Lab"))
    });
    for (label, id, pointer, key, expected) in [
        (
            "Reduce motion",
            "prefersReducedMotion",
            "/prefersReducedMotion",
            b" ".as_slice(),
            Some(true),
        ),
        (
            "Show terminal status",
            "presentation.showStatusInTerminalTab",
            "/presentation/showStatusInTerminalTab",
            b" ".as_slice(),
            Some(false),
        ),
        (
            "Show terminal status",
            "presentation.showStatusInTerminalTab",
            "/presentation/showStatusInTerminalTab",
            b"\x1b[3~".as_slice(),
            None,
        ),
        (
            "Reduce motion",
            "prefersReducedMotion",
            "/prefersReducedMotion",
            b"\x1b[3~".as_slice(),
            None,
        ),
    ] {
        let offset = save_title_setting(&mut test, label, id, pointer, key, expected);
        if expected == Some(false) {
            test.wait_until("disabled status leaves only the project", |test| {
                last(test, offset).as_deref() == Some("Orbital Lab")
            });
        } else if id == "prefersReducedMotion" && expected.is_none() {
            test.wait_until("reset restores title animation", |test| {
                emitted(test, offset)
                    .into_iter()
                    .filter(|t| t != &idle)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    >= 2
            });
        } else {
            test.wait_until("acknowledged static active title", |test| {
                last(test, 0).is_some_and(|t| t.ends_with(" Orbital Lab") && t != idle)
            });
            let stable = last(&test, 0).unwrap();
            // Real render barriers across terminal resizes exercise additional frame ticks.
            let offset = test.raw_offset();
            for rows in [TALL_ROWS, SHORT_ROWS] {
                test.resize_and_wait_for_draw(rows, COLS);
            }
            assert!(emitted(&test, offset).iter().all(|t| t == &stable));
        }
    }
    let offset = test.raw_offset();
    test.signal("done");
    test.wait_until("completion returns the idle title", |test| {
        last(test, offset).as_ref() == Some(&idle)
    });
    shutdown_title(&mut test);
    assert!(emitted(&test, 0).iter().all(|t| !t.contains('~')));
}

#[test]
fn failed_turn_restores_idle_title_and_shutdown_removes_activity_status() {
    let mut test = TerminalTest::start("hold-title-error", 2);
    test.wait_until("initial idle title", |test| {
        last(test, 0).is_some_and(|t| t.ends_with(" Orbital Lab"))
    });
    let idle = last(&test, 0).unwrap();
    test.submit("TITLE_ERROR", "TITLE_ERROR");
    test.wait_journal("reply-held");
    let offset = test.raw_offset();
    test.signal("done");
    test.wait_until("failure returns idle title", |test| {
        last(test, offset).as_ref() == Some(&idle)
    });
    test.wait_until("failure retains cause", |test| {
        test.screen().contains("Fixture service unavailable")
    });
    shutdown_title(&mut test);
}

#[test]
fn quiet_event_loop_expires_only_failed_btw_rows_and_preserves_pending_work_and_draft() {
    let mut test = TerminalTest::start("btw-quiet-expiry", 1);
    for question in ["FAIL_QUESTION", "ACTIVE_QUESTION", "WAITING_QUESTION"] {
        test.submit_command(&format!("/btw {question}"));
        test.wait_until("accepted question appears in the BTW status region", |test| {
            test.screen_rows().iter().any(|row| row.contains("BTW") && row.contains(question))
        });
    }
    assert_eq!(
        test.commands("side_question").len(),
        1,
        "waiters must remain local while the first request is active"
    );
    test.paste("UNSENT_EXPIRY_DRAFT");
    test.wait_until("unsent draft is visible", |test| {
        test.screen().contains("UNSENT_EXPIRY_DRAFT")
    });
    test.signal("fail");
    test.wait_until("failed question and cause coexist with active work and waiter", |test| {
        let rows = test.screen_rows();
        rows.iter().any(|row| {
            row.contains("BTW") && row.contains("FAIL_QUESTION") && row.contains("EXPIRING_CAUSE")
        }) && rows.iter().any(|row| row.contains("BTW") && row.contains("ACTIVE_QUESTION"))
            && rows.iter().any(|row| row.contains("BTW") && row.contains("WAITING_QUESTION"))
            && test.commands("side_question").len() == 2
    });
    let journal = test.journal();
    test.wait_until("quiet loop removes expired failure while retaining named work", |test| {
        let rows = test.screen_rows();
        let screen = rows.join("\n");
        !rows.iter().any(|row| row.contains("BTW") && row.contains("FAIL_QUESTION"))
            && !screen.contains("EXPIRING_CAUSE")
            && screen.contains("ACTIVE_QUESTION")
            && screen.contains("WAITING_QUESTION")
            && screen.contains("UNSENT_EXPIRY_DRAFT")
    });
    assert_eq!(test.journal(), journal, "expiry requires neither input nor bridge events");
    let before = test.commands("side_question");
    assert_eq!(before.len(), 2);
    assert_eq!(before[0]["question"], "FAIL_QUESTION");
    assert_eq!(before[1]["question"], "ACTIVE_QUESTION");
    assert_ne!(before[0]["btw_id"], before[1]["btw_id"]);
    test.signal("answer");
    test.wait_until("held answer dispatches waiter and retains draft", |test| {
        let s = test.screen();
        s.contains("ANSWER_ACTIVE_QUESTION")
            && s.contains("ANSWER_WAITING_QUESTION")
            && s.contains("UNSENT_EXPIRY_DRAFT")
    });
    let commands = test.commands("side_question");
    assert_eq!(commands.len(), 3);
    assert_eq!(commands[2]["question"], "WAITING_QUESTION");
    for command in &commands[1..] {
        assert!(test.journal().iter().any(|e| e["event"] == "btw_result"
            && e["btw_id"] == command["btw_id"]
            && e["question"] == command["question"]));
    }
    test.shutdown();
}

#[cfg(unix)]
#[test]
fn suspend_and_resume_restore_plain_then_active_project_titles() {
    let mut test = TerminalTest::start("hold-title-home", 2);
    #[cfg(unix)]
    assert!(
        test.screen_rows().iter().any(|row| row.contains("Loc: ~")),
        "selected HOME must actually abbreviate the displayed directory"
    );
    test.submit("SUSPEND_WORK", "SUSPEND_WORK");
    test.wait_journal("reply-held");
    let offset = test.raw_offset();
    test.send(b"\x1a"); // Ctrl+Z follows the app's Unix suspend path.
    test.wait_until("suspend restores the plain project title", |test| {
        emitted(test, offset).iter().any(|title| title == "Orbital Lab")
    });
    let pid = test.child.process.process_id().expect("owned app process");
    // Only resume the process created by this fixture. SIGCONT is harmless if
    // the PTY's orphaned job group causes POSIX to ignore SIGTSTP.
    #[allow(unsafe_code)]
    let result = unsafe { libc::kill(i32::try_from(pid).expect("PID"), libc::SIGCONT) };
    assert_eq!(result, 0);
    test.wait_until("return from suspend updates the active title", |test| {
        let titles = emitted(test, offset);
        titles.iter().position(|title| title == "Orbital Lab").is_some_and(|plain| {
            titles[plain + 1..].iter().any(|title| title.ends_with(" Orbital Lab"))
        })
    });
    test.signal("done");
    shutdown_title(&mut test);
}
