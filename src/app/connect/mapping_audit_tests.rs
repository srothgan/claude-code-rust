// SPDX-License-Identifier: Apache-2.0

use super::*;
use serde_json::{Value, json};

fn mapped(payload: Value) -> model::SessionUpdate {
    map_session_update(serde_json::from_value(payload).expect("literal wire update"))
        .expect("supported update")
}

#[test]
fn lifecycle_wire_payloads_retain_their_domain_meaning() {
    assert_eq!(
        mapped(
            json!({"type":"api_retry_update","attempt":2,"max_retries":5,"retry_delay_ms":1250.5,"error_status":529,"error":"server_error"})
        ),
        model::SessionUpdate::ApiRetryUpdate {
            attempt: 2,
            max_retries: 5,
            retry_delay_ms: 1250.5,
            error_status: Some(529),
            error: model::ApiRetryError::ServerError
        }
    );
    for (payload, expected) in [
        (
            json!({"type":"runtime_session_state_update","state":"requires_action"}),
            model::SessionUpdate::RuntimeSessionStateUpdate(
                model::RuntimeSessionState::RequiresAction,
            ),
        ),
        (
            json!({"type":"session_status_update","status":"requesting"}),
            model::SessionUpdate::SessionStatusUpdate(model::SessionStatus::Requesting),
        ),
        (
            json!({"type":"session_status_update","status":"idle"}),
            model::SessionUpdate::SessionStatusUpdate(model::SessionStatus::Idle),
        ),
        (
            json!({"type":"system_notice_update","severity":"warning","message":"Plugin install failed."}),
            model::SessionUpdate::SystemNoticeUpdate {
                severity: model::SystemNoticeSeverity::Warning,
                message: "Plugin install failed.".into(),
            },
        ),
        (
            json!({"type":"prompt_suggestion_update","suggestion":"Add tests"}),
            model::SessionUpdate::PromptSuggestionUpdate("Add tests".into()),
        ),
    ] {
        assert_eq!(mapped(payload), expected);
    }
}

#[test]
fn compaction_mapping_preserves_lifecycle_measurements_and_unknown_failures() {
    for (payload, expected) in [
        (json!({"type":"compaction_update","phase":"started"}), model::CompactionUpdate::Started),
        (
            json!({"type":"compaction_update","phase":"boundary","trigger":"manual","pre_tokens":123_456,"post_tokens":20000,"duration_ms":1500}),
            model::CompactionUpdate::Boundary(model::CompactionBoundary {
                trigger: model::CompactionTrigger::Manual,
                pre_tokens: 123_456,
                post_tokens: Some(20000),
                duration_ms: Some(1500),
            }),
        ),
        (
            json!({"type":"compaction_update","phase":"boundary","trigger":"auto","pre_tokens":9876}),
            model::CompactionUpdate::Boundary(model::CompactionBoundary {
                trigger: model::CompactionTrigger::Auto,
                pre_tokens: 9876,
                post_tokens: None,
                duration_ms: None,
            }),
        ),
        (
            json!({"type":"compaction_update","phase":"finished","result":"success"}),
            model::CompactionUpdate::Finished {
                result: model::CompactionResult::Success,
                error_code: None,
                error: None,
            },
        ),
    ] {
        assert_eq!(mapped(payload), model::SessionUpdate::CompactionUpdate(expected));
    }
    for (token, expected) in [
        ("too_few_groups", model::CompactionFailureCode::TooFewGroups),
        ("future_failure", model::CompactionFailureCode::Unknown),
    ] {
        assert_eq!(
            mapped(
                json!({"type":"compaction_update","phase":"finished","result":"failed","error_code":token,"error":"Prompt is too long"})
            ),
            model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Finished {
                result: model::CompactionResult::Failed,
                error_code: Some(expected),
                error: Some("Prompt is too long".into())
            })
        );
    }
}

#[test]
fn model_catalog_normalizes_blank_descriptions_without_conflating_capabilities() {
    for (resolved, description, expected_resolved, expected_description) in [
        (None, None, None, None),
        (Some(""), Some(" \t"), None, None),
        (Some(" \t"), Some(" visible description "), None, Some(" visible description ")),
        (Some(" resolved-id "), Some(""), Some(" resolved-id "), None),
        (
            Some(" resolved-id "),
            Some(" visible description "),
            Some(" resolved-id "),
            Some(" visible description "),
        ),
    ] {
        let input: types::AvailableModel = serde_json::from_value(json!({
            "id":"advertised","display_name":"Visible model","resolved_model":resolved,"description":description,
            "supports_effort":true,"supported_effort_levels":["xhigh","max"],
            "supports_adaptive_thinking":true,"supports_fast_mode":false,"supports_auto_mode":null
        })).expect("model");
        let models = map_available_models(vec![input]);
        let row = models.iter().find(|m| m.id == "advertised").expect("advertised model");
        assert_eq!(row.display_name, "Visible model");
        assert_eq!(row.resolved_model.as_deref(), expected_resolved);
        assert_eq!(row.description.as_deref(), expected_description);
        assert!(row.supports_effort);
        assert_eq!(
            row.supported_effort_levels,
            [model::EffortLevel::XHigh, model::EffortLevel::Max]
        );
        assert_eq!(
            (row.supports_adaptive_thinking, row.supports_fast_mode, row.supports_auto_mode),
            (Some(true), Some(false), None)
        );
    }
    let minimal = map_available_models(vec![
        serde_json::from_value(
            json!({"id":"minimal","display_name":"Minimal","supports_effort":false}),
        )
        .unwrap(),
    ]);
    let row = minimal.iter().find(|m| m.id == "minimal").unwrap();
    assert!(row.supported_effort_levels.is_empty());
    assert!(!row.supports_effort);
    assert_eq!(
        (row.supports_adaptive_thinking, row.supports_fast_mode, row.supports_auto_mode),
        (None, None, None)
    );
}

#[test]
fn account_fields_preserve_identity_and_normalize_only_blank_values() {
    for (provider, expected) in [
        ("gateway", model::AccountApiProvider::Gateway),
        ("customGateway", model::AccountApiProvider::Other("customGateway".into())),
    ] {
        let account = convert_account_info(serde_json::from_value(json!({"email":"person@example.test","organization":"org-17","subscription_type":"Max","token_source":"oauth","api_key_source":"user","api_provider":provider})).expect("account"));
        assert_eq!(
            account,
            model::AccountInfo {
                email: Some("person@example.test".into()),
                organization: Some("org-17".into()),
                subscription_type: Some("Max".into()),
                token_source: Some("oauth".into()),
                api_key_source: Some("user".into()),
                api_provider: Some(expected)
            }
        );
    }
    for value in [Value::Null, json!(""), json!(" \t")] {
        let account = convert_account_info(serde_json::from_value(json!({"email":value,"organization":value,"subscription_type":value,"token_source":value,"api_key_source":value,"api_provider":value})).expect("blank account"));
        assert_eq!(account, model::AccountInfo::default());
    }
}

#[test]
fn command_and_agent_hints_preserve_nonblank_values_and_wire_defaults() {
    for (hint, expected) in [
        (Value::Null, None),
        (json!(""), None),
        (json!(" \n"), None),
        (json!(" <file> "), Some(" <file> ")),
    ] {
        let model::SessionUpdate::AvailableCommandsUpdate(update) = mapped(
            json!({"type":"available_commands_update","source":"commands_changed","generation":7,"commands":[
                {"name":"project-command","description":"Project command","input_hint":hint,"aliases":["project-alias"],"builtin":true},
                {"name":"minimal","description":"Minimal command"}
            ]}),
        ) else {
            panic!("commands")
        };
        assert_eq!(update.source.as_deref(), Some("commands_changed"));
        assert_eq!(update.generation, Some(7));
        let row = update
            .available_commands
            .iter()
            .find(|c| c.name == "project-command")
            .expect("command");
        assert_eq!(row.description, "Project command");
        assert_eq!(row.aliases, ["project-alias"]);
        assert!(row.builtin);
        assert_eq!(row.input_hint.as_deref(), expected);
        let minimal =
            update.available_commands.iter().find(|c| c.name == "minimal").expect("minimal");
        assert_eq!(minimal.description, "Minimal command");
        assert!(!minimal.builtin);
        assert!(minimal.aliases.is_empty());
        assert!(minimal.input_hint.is_none());
        let model::SessionUpdate::AvailableAgentsUpdate(agents) = mapped(
            json!({"type":"available_agents_update","agents":[{"name":"worker","description":"Build worker","model":hint}]}),
        ) else {
            panic!("agents")
        };
        let agent = agents.available_agents.iter().find(|a| a.name == "worker").expect("worker");
        assert_eq!(agent.description, "Build worker");
        assert_eq!(agent.model.as_deref(), expected);
    }
}

#[test]
fn task_mapping_preserves_dependency_and_execution_ancestry_by_task_id() {
    let model::SessionUpdate::TaskStateUpdate(empty) = mapped(
        json!({"type":"task_state_update","source":"background_tasks","tasks":[],"removed_task_ids":["bg-old"],"is_complete_snapshot":true}),
    ) else {
        panic!("background snapshot")
    };
    assert_eq!(empty.source, model::TaskUpdateSource::BackgroundTasks);
    assert!(empty.tasks.is_empty());
    assert_eq!(empty.removed_task_ids, ["bg-old"]);
    assert!(empty.is_complete_snapshot);

    for (complete, source, expected_source) in [
        (false, "task_update", model::TaskUpdateSource::Update),
        (true, "task_lifecycle", model::TaskUpdateSource::Lifecycle),
    ] {
        let model::SessionUpdate::TaskStateUpdate(update) = mapped(
            json!({"type":"task_state_update","source":source,"removed_task_ids":["obsolete"],"is_complete_snapshot":complete,"tasks":[
                {"task_id":"neighbor","subject":"Unrelated","status":"pending","blocks":[],"blocked_by":[]},
                {"task_id":"worker","subject":"Run checks","description":"Validate branch","active_form":"Running checks","status":"in_progress","owner":"agent","blocks":["downstream"],"blocked_by":["upstream"],"metadata":{"phase":"6A"},"run_id":"worker-run","parent_task_id":"parent-task","source_tool_call_id":"launch-tool","source_tool_call_run_id":"launch-run"}
            ]}),
        ) else {
            panic!("tasks")
        };
        assert_eq!(update.source, expected_source);
        assert_eq!(update.is_complete_snapshot, complete);
        assert_eq!(update.removed_task_ids, ["obsolete"]);
        let task = update.tasks.iter().find(|t| t.task_id == "worker").expect("worker");
        assert_eq!(task.subject, "Run checks");
        assert_eq!(task.description.as_deref(), Some("Validate branch"));
        assert_eq!(task.active_form.as_deref(), Some("Running checks"));
        assert_eq!(task.status, model::TaskStatus::InProgress);
        assert_eq!(task.owner.as_deref(), Some("agent"));
        assert_eq!(task.blocks, ["downstream"]);
        assert_eq!(task.blocked_by, ["upstream"]);
        assert_eq!(task.metadata, Some(json!({"phase":"6A"})));
        assert_eq!(
            (
                task.run_id.as_deref(),
                task.parent_task_id.as_deref(),
                task.source_tool_call_id.as_deref(),
                task.source_tool_call_run_id.as_deref()
            ),
            (Some("worker-run"), Some("parent-task"), Some("launch-tool"), Some("launch-run"))
        );
    }
}

#[test]
fn message_tool_and_retraction_ids_stay_with_their_payloads() {
    for (kind, id, text) in [
        ("user_message_chunk", "user-source", "user body"),
        ("agent_message_chunk", "assistant-source", "assistant body"),
    ] {
        for source in [Some(id), None] {
            let update = mapped(
                json!({"type":kind,"source_message_uuid":source,"content":{"type":"text","text":text}}),
            );
            let chunk = match update {
                model::SessionUpdate::UserMessageChunk(c) if kind == "user_message_chunk" => c,
                model::SessionUpdate::AgentMessageChunk(c) if kind == "agent_message_chunk" => c,
                _ => panic!("role must survive"),
            };
            assert_eq!(chunk.source_message_uuid.as_deref(), source);
            let model::ContentBlock::Text(body) = chunk.content else { panic!("text") };
            assert_eq!(body.text, text);
        }
    }
    let model::SessionUpdate::ExternalMessageUpdate(message) = mapped(
        json!({"type":"external_message_update","content":"Peer message","source_message_uuid":"peer-message","origin":{"kind":"peer","subkind":"peer-send-message","from":"reported-route","name":"Build session","from_session":"peer-session","sender_task_id":"peer-task","verified_peer_pid":4242,"from_mode":"bypass"}}),
    ) else {
        panic!("external")
    };
    assert_eq!(message.content, "Peer message");
    assert_eq!(message.source_message_uuid.as_deref(), Some("peer-message"));
    assert_eq!(
        message.origin,
        model::MessageOrigin {
            kind: "peer".into(),
            subkind: Some("peer-send-message".into()),
            from: Some("reported-route".into()),
            name: Some("Build session".into()),
            from_session: Some("peer-session".into()),
            sender_task_id: Some("peer-task".into()),
            verified_peer_pid: Some(4242),
            from_mode: Some("bypass".into())
        }
    );
    for source in [Some("result-source"), None] {
        let model::SessionUpdate::ToolCallUpdate(update) = mapped(
            json!({"type":"tool_call_update","tool_call_update":{"tool_call_id":"target-tool","source_message_uuid":source,"fields":{"status":"completed","raw_output":"literal result"}}}),
        ) else {
            panic!("tool patch")
        };
        assert_eq!(update.tool_call_id, "target-tool");
        assert_eq!(update.source_message_uuid.as_deref(), source);
        assert_eq!(update.fields.status, Some(model::ToolCallStatus::Completed));
        assert_eq!(update.fields.raw_output, Some(json!("literal result")));
    }
    let model::SessionUpdate::TranscriptRetraction(retraction) = mapped(
        json!({"type":"transcript_retraction","message_uuids":["assistant-source","result-source"],"reason":"assistant_supersedes","request_id":"retry-request","scope":"local"}),
    ) else {
        panic!("retraction")
    };
    assert_eq!(retraction.message_uuids, ["assistant-source", "result-source"]);
    assert_eq!(retraction.reason, model::TranscriptRetractionReason::AssistantSupersedes);
    assert_eq!(retraction.request_id.as_deref(), Some("retry-request"));
    assert_eq!(retraction.scope.as_deref(), Some("local"));
    let model::SessionUpdate::TranscriptRetraction(fallback) = mapped(
        json!({"type":"transcript_retraction","message_uuids":["assistant-old","tool-result-old"],"reason":"model_refusal_fallback","request_id":"req1","trigger":"refusal","direction":"retry","original_model":"opus","fallback_model":"sonnet","scope":"local","api_refusal_category":"cyber","api_refusal_explanation":"policy text","content":"retry body"}),
    ) else {
        panic!("fallback retraction")
    };
    assert_eq!(
        fallback,
        model::TranscriptRetraction {
            message_uuids: vec!["assistant-old".into(), "tool-result-old".into()],
            reason: model::TranscriptRetractionReason::ModelRefusalFallback,
            request_id: Some("req1".into()),
            trigger: Some("refusal".into()),
            direction: Some("retry".into()),
            original_model: Some("opus".into()),
            fallback_model: Some("sonnet".into()),
            scope: Some("local".into()),
            api_refusal_category: Some("cyber".into()),
            api_refusal_explanation: Some("policy text".into()),
            content: Some("retry body".into())
        }
    );
}

#[test]
fn rate_limit_representations_preserve_optional_enablement_and_credit_flags() {
    for (field, expected) in [
        (None, None),
        (Some(Value::Null), None),
        (Some(json!(false)), Some(false)),
        (Some(json!(true)), Some(true)),
    ] {
        let mut raw = json!({"status":"rejected","error_code":"credits_required","is_using_overage":true,"can_user_purchase_credits":true,"has_chargeable_saved_payment_method":false,"resets_at":1_700_000_100.5,"utilization":0.87,"rate_limit_type":"seven_day","limit_scope":"organization","overage_status":"allowed_warning","overage_resets_at":1_700_000_200.5,"overage_disabled_reason":"policy","surpassed_threshold":0.75});
        if let Some(field) = field {
            raw["overage_enabled"] = field;
        }
        let direct: model::RateLimitUpdate = serde_json::from_value(raw.clone()).expect("direct");
        raw["type"] = json!("rate_limit_update");
        let model::SessionUpdate::RateLimitUpdate(flattened) = mapped(raw) else {
            panic!("rate limit")
        };
        for update in [direct, flattened] {
            assert_eq!(update.status, model::RateLimitStatus::Rejected);
            assert_eq!(update.error_code.as_deref(), Some("credits_required"));
            assert_eq!(update.overage_enabled, expected);
            assert_eq!(update.is_using_overage, Some(true));
            assert_eq!(update.can_user_purchase_credits, Some(true));
            assert_eq!(update.has_chargeable_saved_payment_method, Some(false));
            assert_eq!(update.overage_status, Some(model::RateLimitStatus::AllowedWarning));
            assert_eq!(
                (
                    update.resets_at,
                    update.utilization,
                    update.overage_resets_at,
                    update.surpassed_threshold
                ),
                (Some(1_700_000_100.5), Some(0.87), Some(1_700_000_200.5), Some(0.75))
            );
            assert_eq!(
                (
                    update.rate_limit_type.as_deref(),
                    update.limit_scope.as_deref(),
                    update.overage_disabled_reason.as_deref()
                ),
                (Some("seven_day"), Some("organization"), Some("policy"))
            );
        }
    }
}

fn tool_payload(content: &Value) -> Value {
    json!({"tool_call_id":"target-tool","title":"Target tool","kind":"read","status":"killed","source_message_uuid":"tool-source","content":content,"locations":[]})
}
fn deliver(app: &mut crate::app::App, payload: Value) {
    crate::app::events::handle_client_event(
        app,
        crate::agent::events::ClientEvent::SessionUpdate {
            session_id: "mapping-session".into(),
            update: mapped(payload),
        },
    );
}
fn tool<'a>(app: &'a crate::app::App, id: &str) -> &'a crate::app::ToolCallInfo {
    let (mi, bi) = app.lookup_tool_call(id).expect("identified tool");
    let crate::app::MessageBlock::ToolCall(tool) = &app.transcript.messages[mi].blocks[bi] else {
        panic!("tool block")
    };
    tool
}
fn metadata_fixture() -> Value {
    json!({"structured_content_omitted":true,
        "bash":{"assistant_auto_backgrounded":true,"timed_out_after_ms":10000,"background_cwd_hint":"session cwd unchanged","background_ends_with_final_response":true},
        "agent":{"resolved_model":"resolved-sonnet","models_used":["used-opus","used-sonnet"]},
        "web_fetch":{"artifact_read":{"slug":"dashboard","ver":"v2","seeded":false}},
        "skill":{"background":true},"read":{"file_path":"src/lib.rs","start_line":42},
        "non_execution":{"kind":"interrupted","user_feedback":"Stopped intentionally."}})
}
fn task_fixture(flag: Option<bool>) -> Value {
    json!({"end_time":77,"total_paused_ms":11,"error":"Task stopped","is_backgrounded":flag,"spawn_depth":2,"request_id":"request-17","subagent_type":"tester","task_description":"Validate changes","task_type":"remote_agent","workflow_name":"release-check","prompt":"Run validation","output_file":"output.md","summary":"Validation complete","terminal_status":"killed","terminal_reason":"worker_restart","blocked":flag,"parent_agent_id":"agent-parent","ambient":flag})
}
fn assert_metadata(
    output: Option<&model::ToolOutputMetadata>,
    task: Option<&model::TaskMetadata>,
    flag: Option<bool>,
) {
    let output = output.expect("output metadata");
    let actual = serde_json::to_value(output).unwrap();
    for (key, value) in metadata_fixture().as_object().unwrap() {
        assert_eq!(&actual[key], value, "output field {key}");
    }
    assert!(!output.staged);
    let actual = serde_json::to_value(task.expect("task metadata")).unwrap();
    for (key, value) in task_fixture(flag).as_object().unwrap() {
        assert_eq!(&actual[key], value, "task field {key}");
    }
}
#[test]
fn tool_metadata_survives_initial_patch_and_interaction_snapshots() {
    for flag in [Some(true), Some(false), None] {
        let mut payload = tool_payload(&json!([]));
        payload["raw_output"] = json!("{\"answer\":42}\r\n");
        payload["output_metadata"] = metadata_fixture();
        payload["task_metadata"] = task_fixture(flag);
        payload["locations"] = json!([{"path":"valid.rs","line":17},{"path":"boundary.rs","line":4_294_967_295_u64},{"path":"overflow.rs","line":4_294_967_296_u64}]);
        let initial = convert_tool_call(serde_json::from_value(payload.clone()).unwrap());
        assert_eq!(initial.tool_call_id, "target-tool");
        assert_eq!(initial.status, model::ToolCallStatus::Killed);
        assert_eq!(initial.source_message_uuid.as_deref(), Some("tool-source"));
        assert_metadata(initial.output_metadata.as_ref(), initial.task_metadata.as_ref(), flag);
        assert_eq!(initial.raw_output, Some(json!("{\"answer\":42}\r\n")));
        let permission = map_permission_request(
            "session",
            serde_json::from_value(json!({"tool_call":payload,"options":[]})).unwrap(),
        )
        .0;
        let question=map_question_request("session",serde_json::from_value(json!({"tool_call":payload,"prompt":{"question":"Proceed?","header":"Gate","multi_select":false,"options":[]},"question_index":1,"total_questions":2})).unwrap()).0;
        let model::SessionUpdate::ToolCallUpdate(patch) = mapped(
            json!({"type":"tool_call_update","tool_call_update":{"tool_call_id":"target-tool","fields":payload}}),
        ) else {
            panic!("metadata patch")
        };
        assert_eq!(patch.tool_call_id, "target-tool");
        let patch = patch.fields;
        for fields in [patch, permission.tool_call.fields, question.tool_call.fields] {
            assert_metadata(fields.output_metadata.as_ref(), fields.task_metadata.as_ref(), flag);
            assert_eq!(fields.raw_output, Some(json!("{\"answer\":42}\r\n")));
            let locations = fields.locations.expect("present locations");
            for (path, line) in
                [("valid.rs", Some(17)), ("boundary.rs", Some(u32::MAX)), ("overflow.rs", None)]
            {
                assert_eq!(
                    locations
                        .iter()
                        .find(|l| l.path == std::path::Path::new(path))
                        .expect("location retained")
                        .line,
                    line
                );
            }
        }
        assert_eq!(
            initial
                .locations
                .iter()
                .find(|l| l.path == std::path::Path::new("overflow.rs"))
                .unwrap()
                .line,
            None
        );
    }
    let minimal: types::ToolOutputMetadata =
        serde_json::from_value(json!({"read":{"file_path":"notes.md","start_line":4}})).unwrap();
    assert!(!minimal.staged);
    assert!(!minimal.structured_content_omitted);
    let read = minimal.read.unwrap();
    assert_eq!(read.file_path, std::path::Path::new("notes.md"));
    assert_eq!(read.start_line, 4);
    for seeded in [Some(false), None] {
        let model::SessionUpdate::ToolCallUpdate(update) = mapped(
            json!({"type":"tool_call_update","tool_call_update":{"tool_call_id":"artifact-target","fields":{"output_metadata":{"web_fetch":{"artifact_read":{"slug":"versionless-dashboard","seeded":seeded}}}}}}),
        ) else {
            panic!("artifact patch")
        };
        assert_eq!(update.tool_call_id, "artifact-target");
        let artifact =
            update.fields.output_metadata.unwrap().web_fetch.unwrap().artifact_read.unwrap();
        assert_eq!(artifact.slug, "versionless-dashboard");
        assert_eq!(artifact.ver, None);
        assert_eq!(artifact.seeded, seeded);
    }
}
#[test]
fn status_only_tool_patch_preserves_data_while_present_empty_collections_clear() {
    let mut app = crate::app::App::test_default();
    app.session_runtime.activate_session("mapping-session".into());
    let mut initial = tool_payload(
        &json!([{"type":"content","content":{"type":"text","text":"retained source"}}]),
    );
    initial["locations"] = json!([{"path":"kept.rs","line":31}]);
    initial["output_metadata"] = metadata_fixture();
    initial["task_metadata"] = task_fixture(Some(false));
    deliver(&mut app, json!({"type":"tool_call","tool_call":initial}));
    for clear in [false, true] {
        let fields = if clear {
            json!({"content":[],"locations":[]})
        } else {
            json!({"status":"completed"})
        };
        deliver(
            &mut app,
            json!({"type":"tool_call_update","tool_call_update":{"tool_call_id":"target-tool","fields":fields}}),
        );
        let actual = tool(&app, "target-tool");
        assert_eq!(actual.status, model::ToolCallStatus::Completed);
        assert_metadata(
            actual.output_metadata.as_ref(),
            actual.task_metadata.as_ref(),
            Some(false),
        );
        if clear {
            assert!(actual.content.is_empty());
            assert!(actual.locations.is_empty());
        } else {
            assert_eq!(
                actual.content,
                [model::ToolCallContent::Content(model::Content {
                    content: model::ContentBlock::Text(model::TextContent {
                        text: "retained source".into()
                    })
                })]
            );
            assert_eq!(actual.locations[0].path, std::path::Path::new("kept.rs"));
            assert_eq!(actual.locations[0].line, Some(31));
        }
    }
}
#[test]
fn structured_tool_content_preserves_destination_and_resource_identity_in_order() {
    for (populated, size) in [(false, None), (true, Some(0)), (true, Some(42))] {
        let content = json!([
            {"type":"diff","old_path":"before.rs","new_path":"after.rs","old":"old\r\n","new":"new\t  ","repository":if populated {Some("acme/project")}else{None}},
            {"type":"mcp_resource","uri":"file://remote/manual.pdf","mime_type":if populated {Some("application/pdf")}else{None},"text":if populated {Some("manual body")}else{None},"blob_saved_to":if populated {Some("cache/manual.bin")}else{None}},
            {"type":"resource_link","uri":"resource://report","name":"report-file","title":if populated {Some("Report title")}else{None},"description":if populated {Some("Report description")}else{None},"mime_type":if populated {Some("text/plain")}else{None},"size":size,"annotations":if populated {Some(json!({"audience":["user"]}))}else{None}}
        ]);
        let initial = convert_tool_call(serde_json::from_value(tool_payload(&content)).unwrap());
        let patch = convert_tool_call_update_fields(
            serde_json::from_value(json!({"content":content})).unwrap(),
        );
        for actual in [initial.content, patch.content.expect("present content")] {
            let [
                model::ToolCallContent::Diff(diff),
                model::ToolCallContent::McpResource(resource),
                model::ToolCallContent::ResourceLink(link),
            ] = actual.as_slice()
            else {
                panic!("delivered content sequence")
            };
            assert_eq!(diff.path, std::path::Path::new("after.rs"));
            assert_eq!(diff.old_text.as_deref(), Some("old\r\n"));
            assert_eq!(diff.new_text, "new\t  ");
            assert_eq!(diff.repository.as_deref(), populated.then_some("acme/project"));
            assert_eq!(resource.uri, "file://remote/manual.pdf");
            assert_eq!(resource.mime_type.as_deref(), populated.then_some("application/pdf"));
            assert_eq!(resource.text.as_deref(), populated.then_some("manual body"));
            assert_eq!(
                resource.blob_saved_to.as_deref(),
                populated.then_some(std::path::Path::new("cache/manual.bin"))
            );
            assert_eq!(link.uri, "resource://report");
            assert_eq!(link.name, "report-file");
            assert_eq!(link.title.as_deref(), populated.then_some("Report title"));
            assert_eq!(link.description.as_deref(), populated.then_some("Report description"));
            assert_eq!(link.mime_type.as_deref(), populated.then_some("text/plain"));
            assert_eq!(link.size, size);
            assert_eq!(
                serde_json::to_value(&link.annotations).unwrap(),
                if populated { json!({"audience":["user"]}) } else { Value::Null }
            );
        }
    }
}

#[test]
fn read_results_replace_warmed_target_cards_and_preserve_exact_source() {
    use crate::agent::events::ClientEvent;
    use crate::agent::wire::{BridgeEvent, EventEnvelope};
    use crate::app::{App, MessageBlock};
    use crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding;

    let mut app = App::test_default();
    app.session_runtime.activate_session("mapping-session".into());
    deliver(
        &mut app,
        json!({"type":"tool_call","tool_call":{"tool_call_id":"neighbor","title":"Neighbor","kind":"read","status":"completed","content":[{"type":"content","content":{"type":"text","text":"neighbor preview"}}],"locations":[]}}),
    );
    let source = "# Notes\r\n\r\n\t**literal**  \r\n";
    let updates = [
        serde_json::json!({ "type": "tool_call", "tool_call": {
            "tool_call_id": "read-1", "title": "Read notes.md", "kind": "read",
            "status": "in_progress", "content": [{ "type": "content", "content": { "type": "text", "text": "old preview" } }],
            "raw_input": { "file_path": "notes.md" }, "locations": [], "meta": { "claudeCode": { "toolName": "Read" } }
        }}),
        serde_json::json!({ "type": "tool_call_update", "tool_call_update": {
            "tool_call_id": "read-1", "fields": {
                "status": "completed", "raw_output": source,
                "content": [{ "type": "content", "content": { "type": "text", "text": source } }],
                "output_metadata": { "read": { "file_path": "notes.md", "start_line": 60 } }
            }
        }}),
    ];
    for (index, update) in updates.into_iter().enumerate() {
        let event: EventEnvelope = serde_json::from_value(serde_json::json!({
            "event": "session_update", "session_id": "mapping-session", "update": update
        }))
        .expect("Read wire event");
        let BridgeEvent::SessionUpdate { session_id, update } = event.event else {
            panic!("expected session update")
        };
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate {
                session_id,
                update: map_session_update(update).expect("mapped Read update"),
            },
        );
        let (mi, bi) = app.lookup_tool_call("read-1").expect("Read card");
        for width in [24, 80] {
            let rendered = serialize_live_rows_with_boundaries_excluding(
                &mut app,
                width,
                &std::collections::BTreeSet::new(),
                None,
            );
            let segment = rendered
                .segments()
                .iter()
                .find(|segment| segment.msg_idx == mi && segment.block_idx == Some(bi))
                .expect("Read card segment");
            let rows = rendered.segment_rows(segment).expect("Read card rows");
            let rows: Vec<String> = rows
                .iter()
                .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect())
                .collect();
            assert!(!rows.iter().any(|row| row.contains("neighbor preview")));
            if index == 0 {
                assert!(rows.iter().any(|row| row.contains("old preview")));
            } else {
                assert!(!rows.iter().any(|row| row.contains("old preview")));
                let line =
                    rows.iter().find(|row| row.contains("# Notes")).expect("source in target card");
                assert!(
                    line.split_whitespace().collect::<Vec<_>>().ends_with(&["60", "#", "Notes"]),
                    "line number belongs to source: {line}"
                );
                let MessageBlock::ToolCall(tc) = &app.transcript.messages[mi].blocks[bi] else {
                    panic!("expected Read card")
                };
                let [
                    model::ToolCallContent::Content(model::Content {
                        content: model::ContentBlock::Text(text),
                    }),
                ] = tc.content.as_slice()
                else {
                    panic!("canonical Read source")
                };
                assert_eq!(text.text, source);
                assert_eq!(tc.read_output_metadata().expect("Read metadata").start_line, 60);
            }
        }
    }
}

#[test]
fn retry_projection_presence_survives_wire_mapping_and_application() {
    let mut app = crate::app::App::test_default();
    app.session_runtime.activate_session("mapping-session".into());
    let mut initial = tool_payload(&json!([]));
    initial["status"] = json!("in_progress");
    initial["task_metadata"] = json!({"subagent_retry":{"state":"waiting","agent_id":"worker-17","attempt":2,"max_retries":4,"retry_delay_ms":1500,"error_status":429,"error_category":"rate_limit"}});
    deliver(&mut app, json!({"type":"tool_call","tool_call":initial}));
    let waiting = Some(model::SubagentRetryUpdate::Waiting {
        agent_id: Some("worker-17".into()),
        attempt: 2,
        max_retries: 4,
        retry_delay_ms: 1500,
        error_status: Some(429),
        error_category: Some("rate_limit".into()),
    });
    assert_eq!(tool(&app, "target-tool").task_metadata.as_ref().unwrap().subagent_retry, waiting);
    for (fields, expected) in [
        (json!({"status":"in_progress"}), waiting),
        (json!({"task_metadata":{}}), None),
        (
            json!({"task_metadata":{"subagent_retry":{"state":"clear"}}}),
            Some(model::SubagentRetryUpdate::Clear),
        ),
    ] {
        deliver(
            &mut app,
            json!({"type":"tool_call_update","tool_call_update":{"tool_call_id":"target-tool","fields":fields}}),
        );
        assert_eq!(
            tool(&app, "target-tool").task_metadata.as_ref().unwrap().subagent_retry,
            expected
        );
    }
}
