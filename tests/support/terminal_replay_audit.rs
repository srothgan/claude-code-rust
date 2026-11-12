// SPDX-License-Identifier: Apache-2.0

use super::*;

pub(super) fn native_tokens(rows: &[String]) -> Vec<String> {
    rows.iter()
        .flat_map(|row| row.split_whitespace())
        .filter(|word| {
            ["NATIVE_ROW_", "NATIVE_copper_", "NATIVE_river_", "NATIVE_END_"]
                .iter()
                .any(|prefix| word.starts_with(prefix))
        })
        .map(str::to_owned)
        .collect()
}

#[allow(clippy::expect_used)]
impl TerminalTest {
    pub(super) fn committed_rows(&self) -> Option<Vec<String>> {
        let (top, _) = self.captured_region(0, "live")?;
        let rows = self.transcript_rows();
        let height =
            usize::from(self.output.lock().expect("output lock").parser.screen().size().0.get());
        let end = rows.len().saturating_sub(height).saturating_add(top);
        Some(rows[..end].to_vec())
    }

    pub(super) fn committed_source(&self, marker: &str) -> Option<Vec<usize>> {
        Some(
            self.committed_rows()?
                .iter()
                .filter_map(|row| row.split_once(marker)?.1.trim().parse().ok())
                .collect(),
        )
    }

    pub(super) fn wait_committed_reply(&mut self, lines: usize) {
        let expected = (1..=lines).collect::<Vec<_>>();
        self.wait_until("complete ordered committed source exactly once", |test| {
            test.committed_source("streamed line ").as_ref() == Some(&expected)
        });
    }

    pub(super) fn committed_native_words(&self) -> Option<Vec<String>> {
        Some(native_tokens(&self.committed_rows()?))
    }

    pub(super) fn assert_native_reading_slice(&self) {
        let (_, rows) = self.captured_region(0, "live").expect("reading region");
        let actual = native_tokens(&rows);
        assert!(actual.len() >= 8, "fixture must expose multiple source rows in reading mode");
        let expected = native_history_words();
        let start =
            expected.iter().position(|word| word == &actual[0]).expect("known first visible word");
        assert_eq!(
            actual.as_slice(),
            &expected[start..start + actual.len()],
            "reading view contains one ordered contiguous source slice"
        );
    }

    pub(super) fn ordered_source(&self, marker: &str) -> Vec<usize> {
        self.transcript_rows()
            .iter()
            .filter_map(|row| row.split_once(marker)?.1.trim().parse().ok())
            .collect()
    }

    pub(super) fn wait_ordered_reply(&mut self, lines: usize) {
        let expected = (1..=lines).collect::<Vec<_>>();
        self.wait_until("complete ordered source exactly once", |test| {
            test.ordered_source("streamed line ") == expected
        });
    }

    pub(super) fn assert_ordered_reply(&self, lines: usize) {
        assert_eq!(self.ordered_source("streamed line "), (1..=lines).collect::<Vec<_>>());
    }

    pub(super) fn assert_turn_source(&self, user: &str, next: Option<&str>, lines: usize) {
        let rows = self.transcript_rows();
        let position = |source: &str| {
            let hits = rows
                .iter()
                .enumerate()
                .filter(|(_, row)| row.contains(source))
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            assert_eq!(hits.len(), 1, "one user message for {source}");
            hits[0]
        };
        let start = position(user);
        let end = next.map_or(rows.len(), position);
        assert!(start < end);
        let actual = rows[start..end]
            .iter()
            .filter_map(|row| row.split_once("streamed line ")?.1.trim().parse::<usize>().ok())
            .collect::<Vec<_>>();
        assert_eq!(actual, (1..=lines).collect::<Vec<_>>(), "source belongs to {user}");
    }

    pub(super) fn finish_replay(&mut self) {
        let offset = self.raw_offset();
        self.shutdown();
        // This command is emitted after restoration and follows all preceding
        // writes. Successful process exit alone does not drain ConPTY output.
        self.wait_final_rendered_output_since(offset, "claude-rs resume fake-session");
        let output = self.output.lock().expect("output lock");
        assert!(!output.parser.screen().alternate_screen());
        assert!(!output.parser.screen().hide_cursor());
    }

    pub(super) fn captured_region(
        &self,
        offset: usize,
        name: &str,
    ) -> Option<(usize, Vec<String>)> {
        let draw = self
            .runtime_records_since(offset)
            .into_iter()
            .rev()
            .find(|record| record["event_name"] == "inline_chat_viewport_draw")?;
        let top = usize::try_from(draw[format!("{name}_top")].as_u64()?).ok()?;
        let height = usize::try_from(draw[format!("{name}_height")].as_u64()?).ok()?;
        let output = self.output.lock().expect("output lock");
        let screen = output.parser.screen();
        if draw["terminal_height"] != screen.size().0.get()
            || draw["terminal_width"] != screen.size().1.get()
        {
            return None;
        }
        Some((top, screen.rows(0, screen.size().1.get()).skip(top).take(height).collect()))
    }

    pub(super) fn wait_region_text(&mut self, offset: usize, name: &str, needle: &str) {
        self.wait_until(&format!("{needle:?} in physical {name} region"), |test| {
            test.captured_region(offset, name)
                .is_some_and(|(_, rows)| rows.iter().any(|r| r.contains(needle)))
        });
    }

    pub(super) fn wait_held_reply(&mut self, lines: usize) {
        self.wait_until(
            "held reply associated with the actual prompt and rendered source",
            |test| {
                test.prompts().last().is_some_and(|prompt| {
                    test.journal().iter().any(|event| {
                        event["name"] == "reply-held"
                            && event["message_uuid"] == prompt["message_uuid"]
                            && event["session_id"] == prompt["session_id"]
                    })
                }) && test.ordered_source("streamed line ") == (1..=lines).collect::<Vec<_>>()
            },
        );
    }

    pub(super) fn reading_anchor(&self) -> Option<(usize, usize)> {
        let (_, rows) = self.captured_region(0, "live")?;
        rows.iter().enumerate().find_map(|(offset, row)| {
            Some((row.split_once("streamed line ")?.1.trim().parse().ok()?, offset))
        })
    }

    pub(super) fn wait_reading_burst(&mut self, lines: usize) {
        // The model update follows the burst on the same NDJSON stream. Its
        // actual footer rendering fences application consumption of the hidden
        // source without moving the reading view to inspect that source.
        self.wait_until("correlated burst consumed by the application", |test| {
            test.prompts().first().is_some_and(|prompt| {
                test.journal().iter().any(|event| {
                    event["name"] == format!("reading-barrier-{lines}")
                        && event["message_uuid"] == prompt["message_uuid"]
                        && event["session_id"] == prompt["session_id"]
                })
            }) && test.captured_region(0, "footer").is_some_and(|(_, rows)| {
                rows.iter().any(|row| row.contains(&format!("Burst{lines}")))
            })
        });
    }

    pub(super) fn assert_activity_regions(&self, thinking: bool) -> Vec<String> {
        let (live_top, live) = self.captured_region(0, "live").expect("live region");
        let (activity_top, activity) =
            self.captured_region(0, "activity").expect("activity region");
        let (hint_top, hint) = self.captured_region(0, "hint").expect("queue region");
        let (editor_top, editor) = self.captured_region(0, "editor").expect("editor region");
        assert!(live_top <= activity_top && activity_top < hint_top && hint_top < editor_top);
        assert!(activity.iter().any(|row| row.chars().any(char::is_alphanumeric)));
        assert!(hint.iter().any(|row| row.contains("QUEUED_ACTIVITY")));
        assert!(!live.iter().any(|row| row.contains("QUEUED_ACTIVITY")));
        assert!(editor.iter().any(|row| row.contains("DRAFT_ACTIVITY")));
        assert_eq!(
            live.iter().any(|row| assistant_marker(row) && row.contains("Thinking")),
            thinking
        );
        activity
            .into_iter()
            .chain(live.into_iter().filter(|row| assistant_marker(row) && row.contains("Thinking")))
            .filter(|row| row.chars().any(char::is_alphanumeric))
            .map(|row| row.trim().to_owned())
            .collect()
    }

    pub(super) fn assert_no_transient_history(&self, transient: &[String]) {
        let history = self.committed_rows().expect("committed history at current dimensions");
        assert!(
            history.iter().any(|row| row.contains("START_ACTIVITY")),
            "positive control: real user source must commit"
        );
        for row in history {
            assert!(
                !transient.iter().any(|text| row.trim() == text),
                "transient source committed: {row}"
            );
        }
    }
}
