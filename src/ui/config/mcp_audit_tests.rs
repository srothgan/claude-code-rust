// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::agent::{events::ClientEvent, model, types};
use crate::app::config::{ConfigTab, present_mcp_elicitation_request};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;
use std::collections::BTreeMap;

fn key(app: &mut App, code: KeyCode) {
    crate::app::handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn form_app(schema: serde_json::Value, order: &[&str]) -> App {
    let mut app = App::test_default();
    present_mcp_elicitation_request(
        &mut app,
        types::ElicitationRequest {
            request_id: "visible-form".into(),
            server_name: "form-server".into(),
            message: "Review these answers".into(),
            mode: types::ElicitationMode::Form,
            url: None,
            elicitation_id: None,
            field_order: order.iter().map(|s| (*s).into()).collect(),
            requested_schema: Some(schema),
        },
    );
    app
}

fn draw(app: &mut App, width: u16, height: u16, details: bool) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            if app.mcp.elicitations.active().is_some() {
                render_elicitation_overlay(frame, frame.area(), app);
            } else if details {
                render_details_overlay(frame, frame.area(), app);
            } else {
                render(frame, frame.area(), app);
            }
        })
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .chunks(usize::from(width))
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect()
}

fn row<'a>(rows: &'a [String], text: &str) -> &'a str {
    rows.iter()
        .find(|r| r.contains(text))
        .unwrap_or_else(|| panic!("missing {text}:\n{}", rows.join("\n")))
}

fn answer_row<'a>(rows: &'a [String], title: &str) -> &'a str {
    let index = rows
        .iter()
        .position(|r| r.contains(title))
        .unwrap_or_else(|| panic!("missing field {title}:\n{}", rows.join("\n")));
    assert!(rows[index].contains('›'), "field must be selected: {}", rows[index]);
    let answer = rows[index + 1..]
        .iter()
        .take_while(|row| {
            !row.contains(" (")
                && !row.trim_matches(|c: char| c.is_whitespace() || c == '│').is_empty()
        })
        .find(|row| row.contains('['))
        .unwrap_or_else(|| panic!("missing answer inside field {title}:\n{}", rows.join("\n")));
    assert!(
        answer.contains('[') && answer.contains(']'),
        "answer must fit in its own row: {answer}"
    );
    answer
}

fn action(rows: &[String], name: &str, selected: bool) {
    // Exact standalone action labels exclude help text and request messages.
    let action = rows
        .iter()
        .find(|r| r.trim_matches(|c: char| c.is_whitespace() || "│›".contains(c)) == name)
        .unwrap_or_else(|| panic!("missing action {name}:\n{}", rows.join("\n")));
    assert_eq!(action.contains('›'), selected, "{action}");
}

#[test]
fn long_forms_keep_selected_answers_actions_and_validation_reachable_after_resize() {
    let properties = (0..20)
        .map(|i| {
            (format!("field-{i:02}"), json!({"type":"string","default":format!("answer-{i:02}")}))
        })
        .collect::<serde_json::Map<_, _>>();
    let mut app = form_app(json!({"type":"object","properties":properties}), &[]);
    for _ in 0..19 {
        key(&mut app, KeyCode::Tab);
    }
    app.mcp.elicitations.active_mut().unwrap().error = Some("Correct this answer.".into());
    for (width, height) in [(100, 32), (61, 25), (35, 18)] {
        let rows = draw(&mut app, width, height, false);
        assert!(answer_row(&rows, "field-19 (").contains("answer-19"));
        let error = row(&rows, "Correct this answer.");
        assert!(!error.contains("answer-19"));
        let footer = rows.iter().rev().take(6).cloned().collect::<Vec<_>>().join(" ");
        assert!(footer.contains("Esc") && footer.contains("cancel"));
        key(&mut app, KeyCode::Tab);
        action(&draw(&mut app, width, height, false), "Accept", true);
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Tab);
        action(&draw(&mut app, width, height, false), "Cancel", true);
        for _ in 0..3 {
            key(&mut app, KeyCode::Up);
        }
    }
    for _ in 0..19 {
        key(&mut app, KeyCode::Up);
    }
    assert!(answer_row(&draw(&mut app, 61, 25, false), "field-00 (").contains("answer-00"));
    let mut long = form_app(
        json!({"type":"object","properties":{
            "first":{"type":"string","title":"Editable answer","default":"visible-value","description":"Long explanatory help. ".repeat(200)},
            "later":{"type":"string","default":"later-value"}
        }}),
        &["first", "later"],
    );
    for (width, height) in [(61, 25), (35, 18)] {
        assert!(
            answer_row(&draw(&mut long, width, height, false), "Editable answer (")
                .contains("visible-value")
        );
        key(&mut long, KeyCode::Tab);
        assert!(
            answer_row(&draw(&mut long, width, height, false), "later (").contains("later-value")
        );
        key(&mut long, KeyCode::Up);
    }
}

#[test]
fn unsupported_form_explains_the_field_and_only_selects_exit_actions() {
    let mut app =
        form_app(json!({"type":"object","properties":{"nested-record":{"type":"object"}}}), &[]);
    let rows = draw(&mut app, 100, 30, false);
    assert!(row(&rows, "Unsupported field type").contains("nested-record"));
    action(&rows, "Decline", true);
    action(&rows, "Cancel", false);
    assert!(
        !rows
            .iter()
            .any(|r| r.trim_matches(|c: char| c.is_whitespace() || "│›".contains(c)) == "Accept")
    );
    key(&mut app, KeyCode::Down);
    let rows = draw(&mut app, 100, 30, false);
    action(&rows, "Decline", false);
    action(&rows, "Cancel", true);
    let mut supported =
        form_app(json!({"type":"object","properties":{"flag":{"type":"boolean"}}}), &[]);
    key(&mut supported, KeyCode::Tab);
    action(&draw(&mut supported, 100, 30, false), "Accept", true);
}

fn session_app() -> App {
    let mut app = App::test_default();
    app.session_runtime.session_id = Some(model::SessionId::new("mcp-session"));
    app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
    app.config.active_tab = ConfigTab::Mcp;
    app
}

fn snapshot(app: &mut App, servers: Vec<McpServerStatus>, error: Option<&str>) {
    crate::app::handle_client_event(
        app,
        ClientEvent::McpSnapshotReceived {
            session_id: "mcp-session".into(),
            servers,
            auth_capabilities: model::McpAuthCapabilities::default(),
            source: None,
            error: error.map(str::to_owned),
        },
    );
}

fn server(name: &str, config: McpServerStatusConfig) -> McpServerStatus {
    McpServerStatus {
        name: name.into(),
        status: McpServerConnectionStatus::Connected,
        scope: Some("project".into()),
        source: None,
        error: None,
        config: Some(config),
        server_info: Some(model::McpServerInfo {
            name: format!("{name}-implementation"),
            version: "2.7".into(),
        }),
        tools: vec![model::McpTool { name: "read".into(), description: None, annotations: None }],
    }
}

fn stdio(env: BTreeMap<String, String>) -> McpServerStatusConfig {
    McpServerStatusConfig::Stdio {
        command: "fixture-runner".into(),
        args: vec!["--public-arg".into()],
        env,
        timeout: None,
        request_timeout_ms: None,
        always_load: None,
    }
}

#[test]
fn mcp_content_distinguishes_missing_session_loading_empty_failure_and_data() {
    let mut app = App::test_default();
    let rows = draw(&mut app, 120, 28, false);
    row(&rows, "No active session");
    row(&rows, "Open or resume a session");
    app = session_app();
    app.mcp.in_flight = true;
    let rows = draw(&mut app, 120, 28, false);
    row(&rows, "Loading MCP status");
    row(&rows, "Waiting for the current session");
    snapshot(&mut app, vec![], None);
    let rows = draw(&mut app, 120, 28, false);
    row(&rows, "No MCP servers");
    row(&rows, "did not report any MCP servers");
    snapshot(&mut app, vec![], Some("fixture connection refused"));
    let rows = draw(&mut app, 120, 28, false);
    let empty = rows.iter().position(|r| r.contains("No MCP servers")).unwrap();
    assert!(
        rows[empty + 1..].iter().any(|r| r.contains("fixture connection refused")),
        "error must be in the content body, not only the summary"
    );
    snapshot(&mut app, vec![server("live-source", stdio(BTreeMap::new()))], None);
    let rows = draw(&mut app, 120, 28, false);
    row(&rows, "live-source-implementation 2.7");
    row(&rows, "fixture-runner");
}

#[test]
fn snapshot_rows_bind_server_metadata_and_navigation_opens_the_selected_identity() {
    let mut app = session_app();
    let first = server("filesystem", stdio(BTreeMap::new()));
    let mut second = server(
        "catalog",
        McpServerStatusConfig::Http {
            url: "https://catalog.example/mcp".into(),
            headers: BTreeMap::new(),
            tools: vec![],
            timeout: None,
            request_timeout_ms: None,
            always_load: None,
        },
    );
    second.status = McpServerConnectionStatus::NeedsAuth;
    second.scope = Some("user".into());
    second.server_info =
        Some(model::McpServerInfo { name: "Catalog Service".into(), version: "9.1".into() });
    second.tools.clear();
    snapshot(&mut app, vec![first.clone(), second.clone()], None);
    let rows = draw(&mut app, 150, 30, false);
    let first_index =
        rows.iter().position(|r| r.contains("filesystem") && r.contains("connected")).unwrap();
    assert!(
        rows[first_index].contains("project")
            && rows[first_index].contains("stdio")
            && rows[first_index].contains('›')
    );
    assert!(
        rows[first_index + 1].contains("filesystem-implementation 2.7")
            && rows[first_index + 1].contains("1 tool")
            && rows[first_index + 1].contains("fixture-runner")
    );
    let second_index =
        rows.iter().position(|r| r.contains("catalog") && r.contains("needs auth")).unwrap();
    assert!(
        rows[second_index].contains("user")
            && rows[second_index].contains("http")
            && !rows[second_index].contains('›')
    );
    assert!(
        rows[second_index + 1].contains("Catalog Service 9.1")
            && rows[second_index + 1].contains("no tools")
            && rows[second_index + 1].contains("https://catalog.example/mcp")
    );
    row(&rows, "total 2");
    row(&rows, "connected 1");
    row(&rows, "needs auth 1");
    key(&mut app, KeyCode::Down);
    let rows = draw(&mut app, 150, 30, false);
    assert!(row(&rows, "catalog  ").contains('›'));
    assert!(!row(&rows, "filesystem  ").contains('›'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.config.mcp_details_overlay().unwrap().server_name, "catalog");
    let rows = draw(&mut app, 140, 44, true);
    row(&rows, "Catalog Service");
    assert!(!rows.iter().any(|r| r.contains("filesystem-implementation")));
    key(&mut app, KeyCode::Esc);
    second.status = McpServerConnectionStatus::Connected;
    snapshot(&mut app, vec![first, second], None);
    let rows = draw(&mut app, 150, 30, false);
    row(&rows, "connected 2");
    row(&rows, "needs auth 0");
}

fn policies(always_load: Option<bool>) -> McpServerStatusConfig {
    McpServerStatusConfig::Http {
        url: "https://policies.example/mcp".into(),
        headers: BTreeMap::new(),
        timeout: Some(1200),
        request_timeout_ms: Some(8700),
        always_load,
        tools: vec![
            model::McpServerToolPolicy {
                name: "search".into(),
                permission_policy: Some(model::McpServerToolPermissionPolicy::Deny),
                org_max_permission: None,
            },
            model::McpServerToolPolicy {
                name: "lookup".into(),
                permission_policy: None,
                org_max_permission: Some(model::McpServerOrgMaxPermission::Ask),
            },
        ],
    }
}

fn kv(rows: &[String], label: &str, expected: &str) {
    let found = rows
        .iter()
        .find(|r| {
            r.trim_start_matches(|c: char| c.is_whitespace() || c == '│')
                .starts_with(&format!("{label}:"))
        })
        .unwrap_or_else(|| panic!("missing {label}:\n{}", rows.join("\n")));
    assert!(found.contains(expected), "{label} must own {expected}: {found}");
}

#[test]
fn runtime_options_and_tool_policies_remain_associated_and_reachable_in_details() {
    for (setting, label) in
        [(Some(true), Some("enabled")), (Some(false), Some("disabled")), (None, None)]
    {
        let rows = config_lines(&policies(setting)).iter().map(Line::to_string).collect::<Vec<_>>();
        kv(&rows, "Timeout", "1200 ms");
        kv(&rows, "Request timeout", "8700 ms");
        kv(&rows, "search", "always deny");
        kv(&rows, "lookup", "not specified, org max ask");
        if let Some(label) = label {
            kv(&rows, "Always load", label);
        } else {
            assert!(!rows.iter().any(|r| r.contains("Always load")));
        }
    }
    let absent =
        config_lines(&stdio(BTreeMap::new())).iter().map(Line::to_string).collect::<Vec<_>>();
    for key in ["Timeout:", "Request timeout:", "Always load:"] {
        assert!(!absent.iter().any(|r| r.contains(key)));
    }
    let mut app = session_app();
    snapshot(&mut app, vec![server("policy-target", policies(Some(false)))], None);
    key(&mut app, KeyCode::Enter);
    let mut observed = Vec::new();
    for _ in 0..5 {
        observed.extend(draw(&mut app, 100, 30, true));
        key(&mut app, KeyCode::PageDown);
    }
    kv(&observed, "Timeout", "1200 ms");
    kv(&observed, "Request timeout", "8700 ms");
    kv(&observed, "Always load", "disabled");
    kv(&observed, "search", "always deny");
    kv(&observed, "lookup", "not specified, org max ask");
    // Scroll must clamp to the actual content and recover when resized or reopened.
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::PageDown);
    let rows = draw(&mut app, 61, 25, true);
    kv(&rows, "lookup", "not specified");
    row(&rows, "Esc");
    key(&mut app, KeyCode::Home);
    kv(&draw(&mut app, 100, 30, true), "Status", "connected");
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Enter);
    kv(&draw(&mut app, 100, 30, true), "Status", "connected");
}

#[test]
fn unicode_multiline_answers_keep_the_insertion_point_and_adjacent_text_in_their_row() {
    let original = "START-界界界界界界界界界界界界\nLEFT界RIGHT\n界界界界界界界界界界界界-END";
    let mut app = form_app(
        json!({"type":"object","properties":{
        "answer":{"type":"string","title":"Editable","default":original},
        "neighbor":{"type":"string","default":"neighbor-value"}
    },"required":["answer","neighbor"]}),
        &["answer", "neighbor"],
    );
    for (width, height) in [(120, 32), (35, 18)] {
        key(&mut app, KeyCode::Home);
        let rows = draw(&mut app, width, height, false);
        assert!(answer_row(&rows, "Editable (").contains("|START-"));
        for _ in 0..23 {
            key(&mut app, KeyCode::Right);
        }
        let rows = draw(&mut app, width, height, false);
        assert!(answer_row(&rows, "Editable (").contains("LEFT|界 RIGHT"), "{}", rows.join("\n"));
        let entry = app.mcp.elicitations.active().unwrap();
        assert_eq!(
            serde_json::Value::Object(entry.form.as_ref().ok().unwrap().content().unwrap()),
            json!({"answer":original,"neighbor":"neighbor-value"})
        );
        key(&mut app, KeyCode::End);
        let rows = draw(&mut app, width, height, false);
        assert!(answer_row(&rows, "Editable (").contains("-END|"));
        let neighbor = rows.iter().position(|r| r.contains("neighbor (")).unwrap();
        assert!(rows[neighbor + 1].contains("neighbor-value"));
    }
    key(&mut app, KeyCode::Home);
    for _ in 0..23 {
        key(&mut app, KeyCode::Right);
    }
    key(&mut app, KeyCode::Char('!'));
    let rows = draw(&mut app, 35, 18, false);
    assert!(answer_row(&rows, "Editable (").contains("LEFT!|界 RIGHT"));
    let expected = "START-界界界界界界界界界界界界\nLEFT!界RIGHT\n界界界界界界界界界界界界-END";
    assert_eq!(
        serde_json::Value::Object(
            app.mcp.elicitations.active().unwrap().form.as_ref().ok().unwrap().content().unwrap()
        ),
        json!({"answer":expected,"neighbor":"neighbor-value"})
    );
    // Cursor beside a newline exposes its escaped representation without editing the wire value.
    key(&mut app, KeyCode::Home);
    for _ in 0..18 {
        key(&mut app, KeyCode::Right);
    }
    assert!(answer_row(&draw(&mut app, 35, 18, false), "Editable (").contains("|\\nLEFT"));
}

#[test]
fn environment_and_header_values_stay_private_before_clipping_and_in_live_views() {
    for populated in [false, true] {
        let env = if populated {
            BTreeMap::from([
                ("TOKEN".into(), "env-private-193".into()),
                ("KEY".into(), "env-private-847".into()),
            ])
        } else {
            BTreeMap::new()
        };
        let headers = if populated {
            BTreeMap::from([("Authorization".into(), "header-private-526".into())])
        } else {
            BTreeMap::new()
        };
        for (name, config, count_key, count) in [
            (
                "stdio-private",
                stdio(env),
                "Env",
                if populated { "2 variable(s)" } else { "0 variable(s)" },
            ),
            (
                "http-private",
                McpServerStatusConfig::Http {
                    url: "https://public.example/http".into(),
                    headers: headers.clone(),
                    tools: vec![],
                    timeout: None,
                    request_timeout_ms: None,
                    always_load: None,
                },
                "Headers",
                if populated { "1 configured" } else { "0 configured" },
            ),
            (
                "sse-private",
                McpServerStatusConfig::Sse {
                    url: "https://public.example/sse".into(),
                    headers,
                    tools: vec![],
                    timeout: None,
                    request_timeout_ms: None,
                    always_load: None,
                },
                "Headers",
                if populated { "1 configured" } else { "0 configured" },
            ),
        ] {
            let projected = config_lines(&config).iter().map(Line::to_string).collect::<Vec<_>>();
            kv(&projected, count_key, count);
            let mut app = session_app();
            snapshot(&mut app, vec![server(name, config)], None);
            let list = draw(&mut app, 120, 30, false);
            row(&list, name);
            let public = if name.starts_with("stdio") {
                "fixture-runner"
            } else if name.starts_with("http") {
                "https://public.example/http"
            } else {
                "https://public.example/sse"
            };
            row(&list, public);
            key(&mut app, KeyCode::Enter);
            let mut details = draw(&mut app, 120, 36, true);
            key(&mut app, KeyCode::End);
            details.extend(draw(&mut app, 120, 36, true));
            kv(&details, count_key, count);
            row(&details, public);
            for rows in [&projected, &list, &details] {
                let text = rows.join("\n");
                for private in ["env-private-193", "env-private-847", "header-private-526"] {
                    assert!(!text.contains(private), "secret disclosed: {text}");
                }
            }
        }
    }
}
