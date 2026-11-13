// SPDX-License-Identifier: Apache-2.0

use super::*;

const SHIFT_ENTER: &[u8] = b"\x1b[13;2u";

#[cfg_attr(windows, allow(clippy::expect_used))]
fn send_keys(test: &mut TerminalTest, bytes: &[u8]) {
    #[cfg(windows)]
    {
        test.mark_action();
        let path = test.temp.path().join("native-keys.txt");
        std::fs::write(&path, bytes).expect("native keyboard source");
        test.deliver_native_event(Some(&path), "native-key-delivery.json");
    }
    #[cfg(not(windows))]
    test.send(bytes);
}

fn paste_source(test: &mut TerminalTest, source: &str) {
    // Windows' console API delivers logical Return key records; Unix passes
    // raw CR/LF bytes through bracketed-paste decoding and app normalization.
    #[cfg(windows)]
    send_keys(test, source.as_bytes());
    #[cfg(not(windows))]
    test.paste(source);
}

fn type_lines(test: &mut TerminalTest, source: &str) {
    for (index, line) in source.split('\n').enumerate() {
        let offset = test.log_offset();
        if index > 0 {
            send_keys(test, SHIFT_ENTER);
        }
        send_keys(test, line.as_bytes());
        test.wait_composer_text(offset, line);
    }
}

fn submit_and_observe(test: &mut TerminalTest, expected: &[&str]) {
    let offset = test.journal().len();
    test.submit_draft();
    test.wait_until("correlated prompt and completed reply", |test| {
        test.prompts().len() == expected.len()
            && test.journal().iter().skip(offset).any(|event| {
                event["name"] == "input-acknowledged"
                    && test
                        .prompts()
                        .last()
                        .is_some_and(|prompt| event["message_uuid"] == prompt["message_uuid"])
            })
            && test.screen().contains("[READY]")
    });
    test.assert_prompts(expected);
}

#[test]
fn keyboard_edits_across_lines_and_unicode_reach_the_bridge_exactly() {
    let mut test = TerminalTest::start("input-audit", 0);
    let cases: &[(&str, &[u8], &str, &str)] = &[
        ("ac", b"\x1b[Db", "abc", "abc"),
        ("leftRIGHT", b"\x1b[D\x1b[D\x1b[D\x1b[D\x1b[D\x1b[13;2umid", "left\nmidRIGHT", "midRIGHT"),
        ("LEFT\nRIGHT", b"\x1b[H\x7f!", "LEFT!RIGHT", "LEFT!RIGHT"),
        ("\u{1f980}x", b"\x7f!", "\u{1f980}!", "\u{1f980}!"),
        ("LEFT\nRIGHT", b"\x1b[H\x1b[D\x1b[3~!", "LEFT!RIGHT", "LEFT!RIGHT"),
        ("abc\nXYZ", b"\x1b[H\x1b[D!", "abc!\nXYZ", "abc!"),
        ("abc\nXYZ", b"\x1b[A\x1b[C!", "abc\n!XYZ", "!XYZ"),
        ("ab\nLONGROW", b"\x1b[A!", "ab!\nLONGROW", "ab!"),
        ("LONGROW\nxy", b"\x1b[H\x1b[A\x1b[F\x1b[B!", "LONGROW\nxy!", "xy!"),
        ("\u{1f600}\u{1f601}", b"\x1b[DX", "\u{1f600}X\u{1f601}", "\u{1f600}X\u{1f601}"),
        (
            "\u{1f600}\u{1f601}\u{1f602}",
            b"\x1b[H\x1b[C\x1b[3~",
            "\u{1f600}\u{1f602}",
            "\u{1f600}\u{1f602}",
        ),
        ("alpha\nbeta", b"\x1b[A\x1b[DZ\x1b[B\x1b[F!", "alpZha\nbeta!", "beta!"),
    ];
    let mut expected = Vec::new();
    for (source, keys, result, visible) in cases {
        type_lines(&mut test, source);
        let offset = test.log_offset();
        send_keys(&mut test, keys);
        test.wait_composer_text(offset, visible);
        expected.push(*result);
        submit_and_observe(&mut test, &expected);
    }
    test.shutdown();
    test.assert_prompts(&expected);
}

#[test]
fn bracketed_paste_in_the_middle_preserves_suffix_and_normalizes_line_endings() {
    let mut test = TerminalTest::start("input-audit", 0);
    type_lines(&mut test, "LEFT\u{754c}RIGHT");
    send_keys(&mut test, b"\x1b[H\x1b[C\x1b[C\x1b[C\x1b[C\x1b[C");
    let offset = test.log_offset();
    paste_source(&mut test, "alpha\r\n\u{3b2}eta\rthird\n");
    test.wait_until("complete multiline paste in the actual composer", |test| {
        let Some(rows) = test.composer_rows_since(offset) else {
            return false;
        };
        ["LEFT\u{754c}alpha", "\u{3b2}eta", "third", "RIGHT"]
            .iter()
            .all(|expected| rows.iter().any(|row| row.contains(expected)))
            && rows.iter().all(|row| !row.contains("[Pasted Text"))
    });
    let mut expected = vec!["LEFT\u{754c}alpha\n\u{3b2}eta\nthird\nRIGHT"];
    submit_and_observe(&mut test, &expected);
    type_lines(&mut test, "hd");
    send_keys(&mut test, b"\x1b[D");
    let offset = test.log_offset();
    paste_source(&mut test, "ello worl");
    test.wait_composer_text(offset, "hello world");
    expected.push("hello world");
    submit_and_observe(&mut test, &expected);
    test.shutdown();
    test.assert_prompts(&expected);
}

#[test]
fn two_bracketed_paste_chunks_share_one_placeholder_and_one_complete_payload() {
    let mut test = TerminalTest::start("input-audit", 0);
    type_lines(&mut test, "BEGIN|END");
    send_keys(&mut test, b"\x1b[D\x1b[D\x1b[D");
    let first = format!("FIRST\u{754c}\n{}\nEND1", "A".repeat(4990));
    let second = format!("SECOND\u{1f980}\r\n{}\nEND2", "B".repeat(5290));
    let offset = test.log_offset();
    paste_source(&mut test, &first);
    test.wait_composer_text(offset, "BEGIN|[Pasted Text 1 - 5002 chars]END");
    let offset = test.log_offset();
    paste_source(&mut test, &second);
    test.wait_until("one completed continuation placeholder in the composer", |test| {
        test.composer_rows_since(offset).is_some_and(|rows| {
            rows.iter().any(|row| row.contains("BEGIN|[Pasted Text 1 - 10305 chars]END"))
                && rows.iter().map(|row| row.matches("[Pasted Text").count()).sum::<usize>() == 1
        })
    });
    let payload = format!("{first}SECOND\u{1f980}\n{}\nEND2", "B".repeat(5290));
    let full = format!("BEGIN|{payload}END");
    submit_and_observe(&mut test, &[&full]);
    let prompts = test.prompts();
    assert_eq!(prompts[0]["inline_pastes"], serde_json::json!([payload]));
    test.shutdown();
    test.assert_prompts(&[&full]);
}
