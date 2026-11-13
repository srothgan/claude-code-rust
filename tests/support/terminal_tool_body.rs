// SPDX-License-Identifier: Apache-2.0

use super::*;

fn card_regions(rows: &[String]) -> Option<[&[String]; 3]> {
    let mut starts = Vec::new();
    for title in ["BASH_TITLE_ORCHID", "READ_TITLE_ORCHID", "EDIT_TITLE_IRIS"] {
        let matching = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.contains(title))
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            return None;
        }
        starts.push(matching[0]);
    }
    if !(starts[0] < starts[1] && starts[1] < starts[2]) {
        return None;
    }
    Some([&rows[starts[0]..starts[1]], &rows[starts[1]..starts[2]], &rows[starts[2]..]])
}

fn cards_delivered(rows: &[String]) -> bool {
    let Some([bash, read, edit]) = card_regions(rows) else { return false };
    let all = |rows: &[String], needles: &[&str]| {
        needles.iter().all(|s| rows.iter().any(|r| r.contains(s)))
    };
    all(bash, &["WIRE_DENIED_ORCHID"])
        && all(read, &["READ_CONTENT_ORCHID", "READ_CONTENT_JASMINE"])
        && all(edit, &["DIFF_ADDED_ORCHID", "DIFF_OLD_JASMINE"])
}

#[allow(clippy::expect_used)]
fn assert_card_association(test: &TerminalTest) {
    let rows = test.transcript_rows();
    let [bash, read, edit] = card_regions(&rows).expect("distinct ordered tool-card regions");
    for (region, wanted, forbidden) in [
        (
            bash,
            &["WIRE_DENIED_ORCHID"][..],
            &["<tool_use_error>", "</tool_use_error>", "READ_CONTENT_", "DIFF_"][..],
        ),
        (
            read,
            &["READ_CONTENT_ORCHID", "READ_CONTENT_JASMINE"][..],
            &["WIRE_DENIED_", "DIFF_"][..],
        ),
        (
            edit,
            &["DIFF_ADDED_ORCHID", "DIFF_OLD_JASMINE"][..],
            &["WIRE_DENIED_", "READ_CONTENT_"][..],
        ),
    ] {
        let rendered = region.join("\n");
        for needle in wanted {
            assert_eq!(rendered.matches(needle).count(), 1, "own card content: {rendered}");
        }
        for needle in forbidden {
            assert!(!rendered.contains(needle), "foreign or private content in card: {rendered}");
        }
    }
    let added =
        edit.iter().find(|r| r.contains("DIFF_ADDED_ORCHID")).expect("added diff source row");
    let removed =
        edit.iter().find(|r| r.contains("DIFF_OLD_JASMINE")).expect("removed diff source row");
    assert!(added[..added.find("DIFF_ADDED_ORCHID").expect("source")].contains('+'));
    assert!(removed[..removed.find("DIFF_OLD_JASMINE").expect("source")].contains('-'));
}

#[test]
fn wire_tool_payloads_reach_their_own_cards_without_exposing_error_wrappers() {
    let mut test = TerminalTest::start("tool-body-wire", 0);
    test.submit("WIRE_BODY_REQUEST", "WIRE_BODY_REQUEST");
    test.wait_until("correlated tool payloads rendered", |test| {
        test.journal().iter().any(|event| {
            event["name"] == "body-wire-delivered"
                && test.prompts().last().is_some_and(|p| p["message_uuid"] == event["message_uuid"])
        }) && cards_delivered(&test.transcript_rows())
    });
    assert_card_association(&test);
    test.release();
    test.wait_until("correlated completion applied with all tool output", |test| {
        test.journal().iter().any(|event| {
            event["name"] == "body-wire-completed"
                && test.prompts().last().is_some_and(|p| p["message_uuid"] == event["message_uuid"])
        }) && test.screen().contains("[READY]")
            && cards_delivered(&test.transcript_rows())
    });
    test.resize_and_wait_for_draw(55, 61);
    test.wait_until("tool rows restored after terminal replay", |test| {
        cards_delivered(&test.transcript_rows())
    });
    assert_card_association(&test);
    test.assert_prompts(&["WIRE_BODY_REQUEST"]);
    test.shutdown();
}
