// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn values(candidates: &[SlashCandidate]) -> BTreeSet<&str> {
    let values = candidates.iter().map(|c| c.insert_value.as_str()).collect::<BTreeSet<_>>();
    assert_eq!(values.len(), candidates.len(), "choices must not have duplicate identities");
    values
}

fn rows(candidates: &[SlashCandidate]) -> BTreeMap<&str, (&str, Option<&str>)> {
    let _ = values(candidates);
    candidates
        .iter()
        .map(|c| (c.insert_value.as_str(), (c.primary.as_str(), c.secondary.as_deref())))
        .collect()
}

fn draft(app: &mut App, text: &str, row: usize, col: usize) {
    app.input.set_text(text);
    let _ = app.input.set_cursor(row, col);
    assert_eq!((app.input.cursor_row(), app.input.cursor_col()), (row, col));
    sync_with_cursor(app);
}

#[allow(clippy::expect_used)]
fn choose(app: &mut App, value: &str) {
    let count = app.slash.visible().expect("real menu").candidates.len();
    assert!(
        app.slash.visible().expect("real menu").candidates.iter().any(|c| c.insert_value == value),
        "missing choice {value}"
    );
    for _ in 0..count {
        let menu = app.slash.visible().expect("real menu");
        if menu.candidates[menu.dialog.selected].insert_value == value {
            return;
        }
        move_down(app);
    }
    let menu = app.slash.visible().expect("real menu after navigation");
    assert_eq!(
        menu.candidates[menu.dialog.selected].insert_value, value,
        "navigation must reach the requested choice"
    );
}

#[allow(clippy::expect_used)]
fn menu_region(app: &App) -> Vec<String> {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 14)).expect("test terminal");
    let area = ratatui::layout::Rect::new(3, 2, 100, 10);
    terminal
        .draw(|frame| {
            frame.render_widget(
                ratatui::widgets::Paragraph::new(crate::ui::input_rows::build_composer_hint_rows(
                    app,
                )),
                area,
            );
        })
        .expect("draw actual autocomplete rows");
    (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .filter(|row| !row.is_empty())
        .collect()
}

fn target(id: &str, text: &str) -> model::RewindTarget {
    model::RewindTarget {
        uuid: id.into(),
        first_text: text.into(),
        input_text: text.into(),
        index: 0,
        previous_assistant_uuid: None,
        resume_anchor_uuid: None,
    }
}

#[test]
fn resolved_submissions_preserve_text_and_parse_only_a_leading_command() {
    for input in ["", " \t\n", "hello world", "review /mode plan", "界 /docs commands"] {
        let resolved = ResolvedSubmission::resolve(input.into());
        assert!(matches!(&resolved, ResolvedSubmission::Prompt { text } if text == input));
        assert_eq!(resolved.into_text(), input);
    }
    for (input, name, args, command, class) in [
        (
            "/mode plan",
            "/mode",
            vec!["plan"],
            Some(AppSlashCommand::Mode),
            SubmissionClass::TurnExclusive,
        ),
        (
            " \t/docs  commands\n",
            "/docs",
            vec!["commands"],
            Some(AppSlashCommand::Docs),
            SubmissionClass::Informational,
        ),
        (
            "/model 模型-界",
            "/model",
            vec!["模型-界"],
            Some(AppSlashCommand::Model),
            SubmissionClass::TurnExclusive,
        ),
        (
            "/remote\tfirst\nsecond",
            "/remote",
            vec!["first", "second"],
            None,
            SubmissionClass::TurnExclusive,
        ),
        ("/modeled plan", "/modeled", vec!["plan"], None, SubmissionClass::TurnExclusive),
        (
            "/docs commands extra",
            "/docs",
            vec!["commands", "extra"],
            Some(AppSlashCommand::Docs),
            SubmissionClass::Invalid,
        ),
    ] {
        let resolved = ResolvedSubmission::resolve(input.into());
        let ResolvedSubmission::Slash {
            text,
            name: actual_name,
            args: actual_args,
            command: actual_command,
            class: actual_class,
        } = &resolved
        else {
            panic!("expected a slash submission")
        };
        assert_eq!(text, input);
        assert_eq!(actual_name, name);
        assert_eq!(actual_args, &args);
        assert_eq!(*actual_command, command);
        assert_eq!(*actual_class, class);
        assert_eq!(resolved.into_text(), input);
    }
}

#[test]
fn ultracode_validation_preserves_existing_content_and_never_dispatches_invalid_forms() {
    for input in
        ["/ultracode", "/ultracode toggle", "/ultracode on extra", "/ultracode status extra"]
    {
        let (mut app, mut rx) = crate::app::test_support::app_with_connection();
        app.push_message_tracked(ChatMessage::new(
            MessageRole::User,
            vec![MessageBlock::Text(TextBlock::from_complete("KEEP_INVALID_SOURCE"))],
            None,
        ));
        app.input.set_text(input);
        crate::app::input_submit::submit_input(&mut app);
        assert_eq!(ResolvedSubmission::resolve(input.into()).class(), SubmissionClass::Invalid);
        assert!(app.transcript.messages.iter().any(|m| {
            matches!(m.role, MessageRole::User)
                && m.blocks
                    .iter()
                    .any(|b| matches!(b, MessageBlock::Text(t) if t.text == "KEEP_INVALID_SOURCE"))
        }));
        let feedback = app.transcript.messages.last().expect("validation feedback");
        let text = feedback
            .blocks
            .iter()
            .filter_map(
                |b| if let MessageBlock::Text(t) = b { Some(t.text.as_str()) } else { None },
            )
            .collect::<String>();
        assert!(
            text.contains("/ultracode")
                && text.contains("on")
                && text.contains("off")
                && text.contains("status"),
            "{text}"
        );
        assert!(matches!(rx.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)));
        assert!(matches!(app.surface_mode, crate::app::SurfaceMode::Chat));
        assert_eq!(app.status, AppStatus::Ready);
    }
    let app = App::test_default();
    assert!(values(&supported_command_candidates(&app)).contains("/ultracode"));
    assert_eq!(
        values(&argument_candidates(&app, "/ultracode", 0)),
        BTreeSet::from(["on", "off", "status"])
    );
    for (input, expected) in [
        ("/ultracode status", SubmissionClass::Informational),
        ("/ultracode on", SubmissionClass::TurnExclusive),
        ("/ultracode off", SubmissionClass::TurnExclusive),
    ] {
        assert_eq!(ResolvedSubmission::resolve(input.into()).class(), expected);
    }
}

#[test]
fn manual_app_command_inventory_is_complete_and_has_no_duplicates() {
    let markdown =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/commands.md"))
            .expect("manual");
    let documented = documented_app_slash_commands(&markdown);
    let names = documented.iter().map(String::as_str).collect::<BTreeSet<_>>();
    assert_eq!(names.len(), documented.len(), "each documented command occurs once");
    let catalog = APP_SLASH_COMMANDS.iter().map(|s| s.name).collect::<BTreeSet<_>>();
    assert_eq!(catalog.len(), APP_SLASH_COMMANDS.len(), "catalog identities are unique");
    assert_eq!(names, catalog, "manual and app command membership must agree");
}

#[test]
fn app_command_menu_offers_each_required_builtin_once() {
    let app = App::test_default();
    let candidates = supported_command_candidates(&app);
    let actual = rows(&candidates);
    for name in [
        "/agent", "/config", "/docs", "/login", "/logout", "/mcp", "/plugins", "/rewind", "/usage",
        "/status",
    ] {
        let (label, help) = actual.get(name).expect("required built-in");
        assert_eq!(*label, name);
        assert!(help.is_some_and(|text| !text.trim().is_empty()));
    }
}

#[test]
fn app_menu_metadata_owns_canonical_and_alias_collisions_with_sdk_commands() {
    for name in ["/config", "/fast"] {
        let mut app = App::test_default();
        app.sdk_inventory.available_commands = vec![
            model::AvailableCommand::new(name, "SDK_CANONICAL_POISON")
                .input_hint("SDK_ARGUMENT_POISON"),
            model::AvailableCommand::new("remote-probe", "REMOTE_ALLOWED")
                .aliases(vec![name.into()]),
        ];
        draft(&mut app, name, 0, name.chars().count());
        let menu = app.slash.visible().expect("actual command menu");
        let actual = rows(&menu.candidates);
        let (label, help) = actual.get(name).expect("app entry wins collision");
        assert_eq!(*label, name);
        let help = help.expect("app help remains available");
        assert!(
            !help.trim().is_empty() && !help.contains("POISON") && !help.contains("REMOTE_ALLOWED")
        );
        let all = supported_command_candidates(&app);
        assert_eq!(
            rows(&all)["/remote-probe"],
            ("/remote-probe", Some("REMOTE_ALLOWED")),
            "SDK-owned metadata still survives on its own entry"
        );
    }
}

#[test]
fn mode_choices_associate_each_session_id_with_its_own_label() {
    let mut app = App::test_default();
    app.session_runtime.mode = Some(crate::app::ModeState {
        current_mode_id: "plan".into(),
        current_mode_name: "Blueprint".into(),
        available_modes: vec![
            crate::app::ModeInfo { id: "plan".into(), name: "Blueprint".into() },
            crate::app::ModeInfo { id: "code".into(), name: "Implement".into() },
        ],
    });
    let candidates = argument_candidates(&app, "/mode", 0);
    assert_eq!(
        rows(&candidates),
        BTreeMap::from([
            ("plan", ("Blueprint", Some("plan"))),
            ("code", ("Implement", Some("code")))
        ])
    );
}

#[test]
fn model_choices_preserve_complete_sdk_metadata_by_inserted_identity() {
    let mut app = App::test_default();
    app.sdk_inventory.available_models = vec![
        model::AvailableModel::new("default", "Default").description("Default (recommended)"),
        model::AvailableModel::new("sonnet", "Claude Sonnet").description("Balanced coding model"),
        model::AvailableModel::new("opus", "Opus")
            .description("Opus 4.7 · Most capable for complex work"),
        model::AvailableModel::new("bare", "Unannotated"),
    ];
    let candidates = argument_candidates(&app, "/model", 0);
    assert_eq!(
        rows(&candidates),
        BTreeMap::from([
            ("default", ("Default", Some("Default (recommended)"))),
            ("sonnet", ("Claude Sonnet", Some("Balanced coding model"))),
            ("opus", ("Opus", Some("Opus 4.7 · Most capable for complex work"))),
            ("bare", ("Unannotated", Some("bare")))
        ])
    );
    let mut single = App::test_default();
    single.sdk_inventory.available_models = vec![
        model::AvailableModel::new("opus", "Opus")
            .description("Opus 4.7 · Most capable for complex work"),
    ];
    let single_choices = argument_candidates(&single, "/model", 0);
    assert_eq!(
        rows(&single_choices),
        BTreeMap::from([("opus", ("Opus", Some("Opus 4.7 · Most capable for complex work")))]),
        "do not manufacture an unadvertised default choice"
    );
}

#[test]
fn agent_choices_keep_reset_and_associate_description_and_model_with_the_right_agent() {
    let mut app = App::test_default();
    app.sdk_inventory.available_agents = vec![
        model::AvailableAgent::new("reviewer", "REVIEW_ONLY").model("model-review"),
        model::AvailableAgent::new("planner", "PLAN_ONLY"),
        model::AvailableAgent::new("model-only", "").model("model-bare"),
    ];
    let candidates = argument_candidates(&app, "/agent", 0);
    assert_eq!(values(&candidates), BTreeSet::from(["reset", "reviewer", "planner", "model-only"]));
    let actual = rows(&candidates);
    assert!(!actual["reset"].0.trim().is_empty());
    assert!(actual["reset"].1.is_some_and(|s| !s.trim().is_empty()));
    for (id, own, forbidden) in [
        ("reviewer", vec!["REVIEW_ONLY", "model-review"], vec!["PLAN_ONLY", "model-bare"]),
        ("planner", vec!["PLAN_ONLY"], vec!["REVIEW_ONLY", "model-review", "model-bare"]),
        ("model-only", vec!["model-bare"], vec!["REVIEW_ONLY", "PLAN_ONLY", "model-review"]),
    ] {
        let (label, description) = actual[id];
        assert_eq!(label, id);
        let description = description.expect("agent-specific help");
        assert!(own.iter().all(|part| description.contains(part)), "{id}: {description}");
        assert!(forbidden.iter().all(|part| !description.contains(part)), "{id}: {description}");
    }
}

#[test]
fn effort_choices_follow_supported_subsets_and_always_offer_reset() {
    use model::EffortLevel::{High, Low, Max, XHigh};
    for (supported, advertised, expected) in [
        (true, vec![Low, High], BTreeSet::from(["low", "high", "reset"])),
        (true, vec![XHigh, Max], BTreeSet::from(["xhigh", "max", "reset"])),
        (true, vec![], BTreeSet::from(["reset"])),
        (false, vec![Low, Max], BTreeSet::from(["reset"])),
    ] {
        let mut app = App::test_default();
        app.session_runtime.current_model = Some(
            model::CurrentModel::new("fixture", "Fixture", "Fixture")
                .supports_effort(supported)
                .supported_effort_levels(advertised),
        );
        let candidates = argument_candidates(&app, "/effort", 0);
        assert_eq!(values(&candidates), expected);
        for candidate in &candidates {
            assert!(!candidate.primary.trim().is_empty());
            assert!(candidate.secondary.as_ref().is_some_and(|help| !help.trim().is_empty()));
        }
    }
}

#[test]
fn rewind_menu_fetches_once_for_the_active_session_then_completes_the_matching_target() {
    use crate::agent::{events::ClientEvent, wire::BridgeCommand};
    let (mut app, mut rx) = crate::app::test_support::app_with_connection();
    app.sdk_inventory.rewind_targets_session_id = Some(model::SessionId::new("old-session"));
    app.sdk_inventory.rewind_targets = vec![target("stale-target", "STALE_ONLY")];
    draft(&mut app, "/rewind ", 0, 8);
    for _ in 0..3 {
        sync_with_cursor(&mut app);
    }
    assert!(
        matches!(rx.try_recv().expect("active-session request").command, BridgeCommand::GetRewindTargets { session_id } if session_id == "session-1")
    );
    assert!(rx.try_recv().is_err(), "repeated synchronization must not duplicate the request");
    assert!(app.slash.visible().expect("loading menu").candidates.is_empty());
    assert!(menu_region(&app).iter().any(|r| r.contains("Loading") && r.contains("messages")));
    crate::app::handle_client_event(
        &mut app,
        ClientEvent::RewindTargetsReceived {
            session_id: "old-session".into(),
            targets: vec![target("wrong-target", "WRONG_ONLY")],
            error: None,
        },
    );
    assert!(app.slash.visible().expect("still loading current session").candidates.is_empty());
    assert!(
        menu_region(&app).iter().all(|r| !r.contains("WRONG_ONLY") && !r.contains("STALE_ONLY"))
    );
    crate::app::handle_client_event(
        &mut app,
        ClientEvent::RewindTargetsReceived {
            session_id: "session-1".into(),
            targets: vec![target("current-target", "CURRENT_ONLY")],
            error: None,
        },
    );
    let candidates = &app.slash.visible().expect("received current menu").candidates;
    assert_eq!(
        rows(candidates),
        BTreeMap::from([("current-target", ("CURRENT_ONLY", Some("current-target")))])
    );
    assert!(
        menu_region(&app)
            .iter()
            .any(|r| r.contains("CURRENT_ONLY") && r.contains("current-target"))
    );
    sync_with_cursor(&mut app);
    choose(&mut app, "current-target");
    confirm_selection(&mut app);
    assert_eq!(app.input.text(), "/rewind current-target ");
    assert_eq!(
        values(&app.slash.visible().expect("restore-mode menu").candidates),
        BTreeSet::from(["both", "code", "conversation"])
    );
    choose(&mut app, "code");
    confirm_selection(&mut app);
    assert_eq!(app.input.text(), "/rewind current-target code ");
    assert!(!app.slash.is_visible());
    assert_eq!(app.focus_owner(), crate::app::FocusOwner::Input);
    assert!(rx.try_recv().is_err(), "completion neither refetches nor dispatches a rewind");
}

#[test]
fn rewind_menu_distinguishes_loading_empty_history_and_filtered_empty_results() {
    use crate::agent::{events::ClientEvent, wire::BridgeCommand};
    for state in ["loading", "empty", "filtered"] {
        let (mut app, mut rx) = crate::app::test_support::app_with_connection();
        let input = if state == "filtered" { "/rewind missing" } else { "/rewind " };
        draft(&mut app, input, 0, input.chars().count());
        assert!(
            matches!(rx.try_recv().expect("request before response").command, BridgeCommand::GetRewindTargets { session_id } if session_id == "session-1")
        );
        if state != "loading" {
            crate::app::handle_client_event(
                &mut app,
                ClientEvent::RewindTargetsReceived {
                    session_id: "session-1".into(),
                    targets: if state == "filtered" {
                        vec![target("current-target", "CURRENT_ONLY")]
                    } else {
                        vec![]
                    },
                    error: None,
                },
            );
        }
        assert!(app.slash.visible().expect("menu remains visible").candidates.is_empty());
        let region = menu_region(&app).join("\n").to_lowercase();
        assert!(region.contains("messages"), "{state}: {region}");
        match state {
            "loading" => assert!(
                region.contains("loading")
                    && !region.contains("no matching")
                    && !region.contains("no previous")
            ),
            "empty" => assert!(
                region.contains("no previous")
                    && !region.contains("loading")
                    && !region.contains("no matching")
            ),
            "filtered" => assert!(
                region.contains("no matching")
                    && !region.contains("loading")
                    && !region.contains("no previous")
            ),
            _ => unreachable!(),
        }
        assert!(!region.contains("current_only"));
        let cursor = (app.input.cursor_row(), app.input.cursor_col());
        confirm_selection(&mut app);
        assert_eq!(app.input.text(), input, "a placeholder cannot be selected");
        assert_eq!((app.input.cursor_row(), app.input.cursor_col()), cursor);
        assert!(rx.try_recv().is_err());
    }
}

#[test]
fn empty_mode_matches_keep_visible_usage_in_the_menu_region() {
    let mut app = completion_app();
    draft(&mut app, "/mode xyz", 0, 9);
    assert!(app.slash.visible().expect("empty argument menu").candidates.is_empty());
    let region = menu_region(&app).join("\n");
    assert!(region.contains("/mode") && region.contains("<id>"), "{region}");
    confirm_selection(&mut app);
    assert_eq!(app.input.text(), "/mode xyz");
    assert!(app.slash.is_visible());
}

fn completion_app() -> App {
    let mut app = App::test_default();
    app.session_runtime.mode = Some(crate::app::ModeState {
        current_mode_id: "plan".into(),
        current_mode_name: "Blueprint".into(),
        available_modes: vec![crate::app::ModeInfo { id: "plan".into(), name: "Blueprint".into() }],
    });
    app.sdk_inventory.available_models = vec![
        model::AvailableModel::new("sonnet", "Sonnet"),
        model::AvailableModel::new("new-id", "New").description("old-id"),
        model::AvailableModel::new("new-界", "Unicode").description("ol界"),
    ];
    app.sdk_inventory.available_agents = vec![model::AvailableAgent::new("reviewer", "Review")];
    app.session_runtime.current_model = Some(
        model::CurrentModel::new("fixture", "Fixture", "Fixture")
            .supports_effort(true)
            .supported_effort_levels(vec![model::EffortLevel::XHigh]),
    );
    app
}

#[test]
fn real_completion_menus_replace_only_the_active_token_and_return_focus_to_input() {
    for (input, cursor, value, expected, expected_cursor) in [
        ("/agent ", (0, 7), "reviewer", "/agent reviewer ", (0, 16)),
        ("/effort ", (0, 8), "xhigh", "/effort xhigh ", (0, 14)),
        ("/mode ", (0, 6), "plan", "/mode plan ", (0, 11)),
        ("/model ", (0, 7), "sonnet", "/model sonnet ", (0, 14)),
        ("/model old-id trailing", (0, 13), "new-id", "/model new-id trailing", (0, 14)),
        (
            "\n  /model ol界 trailing\nuntouched",
            (1, 12),
            "new-界",
            "\n  /model new-界 trailing\nuntouched",
            (1, 15),
        ),
    ] {
        let mut app = completion_app();
        draft(&mut app, input, cursor.0, cursor.1);
        assert_eq!(app.focus_owner(), crate::app::FocusOwner::Mention);
        choose(&mut app, value);
        confirm_selection(&mut app);
        assert_eq!(app.input.text(), expected);
        assert_eq!((app.input.cursor_row(), app.input.cursor_col()), expected_cursor);
        assert!(!app.slash.is_visible());
        assert_eq!(app.focus_owner(), crate::app::FocusOwner::Input);
        sync_with_cursor(&mut app);
        assert!(!app.slash.is_visible(), "completed argument stays dismissed");
    }
}

#[test]
fn docs_topics_are_selected_from_real_menus_and_route_to_their_own_feedback() {
    for (topic, label, detail) in [
        ("mode", "FIXTURE_MODE", "mode-fixture"),
        ("models", "FIXTURE_MODEL", "MODEL_PROBE"),
        ("shortcuts", "Enter", "Send message"),
        ("commands", "/topic-probe", "COMMAND_PROBE"),
        ("agents", "agent-fixture", "AGENT_PROBE"),
    ] {
        let (mut app, mut rx) = crate::app::test_support::app_with_connection();
        app.session_runtime.mode = Some(crate::app::ModeState {
            current_mode_id: "mode-fixture".into(),
            current_mode_name: "FIXTURE_MODE".into(),
            available_modes: vec![crate::app::ModeInfo {
                id: "mode-fixture".into(),
                name: "FIXTURE_MODE".into(),
            }],
        });
        app.sdk_inventory.available_models = vec![
            model::AvailableModel::new("model-fixture", "FIXTURE_MODEL").description("MODEL_PROBE"),
        ];
        app.sdk_inventory.available_agents =
            vec![model::AvailableAgent::new("agent-fixture", "AGENT_PROBE")];
        app.sdk_inventory.available_commands =
            vec![model::AvailableCommand::new("topic-probe", "COMMAND_PROBE")];
        draft(&mut app, "/do", 0, 3);
        choose(&mut app, "/docs");
        confirm_selection(&mut app);
        assert_eq!(app.input.text(), "/docs ");
        assert_eq!((app.input.cursor_row(), app.input.cursor_col()), (0, 6));
        assert_eq!(
            values(&app.slash.visible().expect("topic menu").candidates),
            BTreeSet::from(["mode", "models", "shortcuts", "commands", "agents"])
        );
        choose(&mut app, topic);
        confirm_selection(&mut app);
        assert_eq!(app.input.text(), format!("/docs {topic} "));
        assert_eq!(
            (app.input.cursor_row(), app.input.cursor_col()),
            (0, format!("/docs {topic} ").chars().count())
        );
        assert!(!app.slash.is_visible());
        assert_eq!(app.focus_owner(), crate::app::FocusOwner::Input);
        crate::app::input_submit::submit_input(&mut app);
        let message = app.transcript.messages.last().expect("topic feedback");
        let body = message
            .blocks
            .iter()
            .filter_map(
                |b| if let MessageBlock::Text(t) = b { Some(t.text.as_str()) } else { None },
            )
            .collect::<String>();
        let row = body
            .lines()
            .find_map(|line| {
                let cells = line.split('|').map(str::trim).collect::<Vec<_>>();
                (cells.len() > 3 && cells[1].contains(label) && cells[2].contains(detail))
                    .then_some(cells)
            })
            .unwrap_or_else(|| panic!("requested topic row for {topic}: {body}"));
        assert!(!row[1].is_empty() && !row[2].is_empty());
        assert!(row[2].contains(detail), "{topic}: {row:?}");
        for marker in ["MODEL_PROBE", "COMMAND_PROBE", "AGENT_PROBE"] {
            if marker != detail {
                assert!(!body.contains(marker), "wrong topic data in {topic}: {body}");
            }
        }
        assert!(rx.try_recv().is_err(), "app docs must not be forwarded");
        assert!(app.input.text().is_empty());
    }
}
