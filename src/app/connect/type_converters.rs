// SPDX-License-Identifier: Apache-2.0

//! Type conversion functions: bridge wire types -> app model types.

use crate::agent::model;
use crate::agent::types;
use crate::app::{ModeInfo, ModeState};

pub(super) fn map_available_commands_update(
    commands: Vec<types::AvailableCommand>,
    source: Option<String>,
    generation: Option<u64>,
) -> model::AvailableCommandsUpdate {
    let mut update = model::AvailableCommandsUpdate::new(
        commands
            .into_iter()
            .map(|mut cmd| {
                cmd.input_hint = cmd.input_hint.filter(|value| !value.trim().is_empty());
                cmd
            })
            .collect(),
    );
    if let Some(source) = source {
        update = update.source(source);
    }
    if let Some(generation) = generation {
        update = update.generation(generation);
    }
    update
}

pub(super) fn map_available_agents_update(
    agents: Vec<types::AvailableAgent>,
) -> model::AvailableAgentsUpdate {
    model::AvailableAgentsUpdate::new(
        agents
            .into_iter()
            .map(|mut agent| {
                agent.model = agent.model.filter(|value| !value.trim().is_empty());
                agent
            })
            .collect(),
    )
}

pub(super) fn map_rewind_result(
    session_id: String,
    restore_mode: types::RewindRestoreMode,
    status: types::RewindResultStatus,
    file_result: Option<types::RewindFilesResult>,
    message: Option<String>,
) -> model::RewindResult {
    model::RewindResult { session_id, restore_mode, status, file_result, message }
}

pub(super) fn map_available_models(
    models: Vec<types::AvailableModel>,
) -> Vec<model::AvailableModel> {
    models
        .into_iter()
        .map(|mut model_info| {
            model_info.resolved_model =
                model_info.resolved_model.filter(|value| !value.trim().is_empty());
            model_info.description =
                model_info.description.filter(|value| !value.trim().is_empty());
            model_info
        })
        .collect()
}

pub(super) fn convert_account_info(account: types::AccountInfo) -> model::AccountInfo {
    model::AccountInfo {
        email: account.email.filter(|value| !value.trim().is_empty()),
        organization: account.organization.filter(|value| !value.trim().is_empty()),
        subscription_type: account.subscription_type.filter(|value| !value.trim().is_empty()),
        token_source: account.token_source.filter(|value| !value.trim().is_empty()),
        api_key_source: account.api_key_source.filter(|value| !value.trim().is_empty()),
        api_provider: account
            .api_provider
            .filter(|value| !value.trim().is_empty())
            .map(model::AccountApiProvider::from_wire),
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn map_session_update(update: types::SessionUpdate) -> Option<model::SessionUpdate> {
    match update {
        types::SessionUpdate::ConversationReset {
            new_conversation_id,
            trigger,
            timestamp,
            user_message_uuid,
        } => Some(model::SessionUpdate::ConversationReset {
            new_conversation_id,
            trigger,
            timestamp,
            user_message_uuid,
        }),
        types::SessionUpdate::MessageMetadata { role, timestamp, source_message_uuid } => {
            Some(model::SessionUpdate::MessageMetadata { role, timestamp, source_message_uuid })
        }
        types::SessionUpdate::TurnTiming { duration_ms, api_duration_ms } => {
            Some(model::SessionUpdate::TurnTiming { duration_ms, api_duration_ms })
        }
        types::SessionUpdate::UserMessageChunk { content, source_message_uuid } => {
            let content = convert_content_block(content)?;
            Some(model::SessionUpdate::UserMessageChunk(
                model::ContentChunk::new(content).source_message_uuid(source_message_uuid),
            ))
        }
        types::SessionUpdate::ExternalMessageUpdate { content, source_message_uuid, origin } => {
            Some(model::SessionUpdate::ExternalMessageUpdate(model::ExternalMessageUpdate {
                content,
                source_message_uuid,
                origin,
            }))
        }
        types::SessionUpdate::AgentMessageChunk { content, source_message_uuid } => {
            let content = convert_content_block(content)?;
            Some(model::SessionUpdate::AgentMessageChunk(
                model::ContentChunk::new(content).source_message_uuid(source_message_uuid),
            ))
        }
        types::SessionUpdate::AgentResponseStarted => {
            Some(model::SessionUpdate::AgentResponseStarted)
        }
        types::SessionUpdate::AgentActivityUpdate { phase } => {
            Some(model::SessionUpdate::AgentActivityUpdate(phase))
        }
        types::SessionUpdate::ToolCall { tool_call } => {
            Some(model::SessionUpdate::ToolCall(convert_tool_call(tool_call)))
        }
        types::SessionUpdate::ToolCallUpdate { tool_call_update } => {
            Some(model::SessionUpdate::ToolCallUpdate(convert_tool_call_update(tool_call_update)))
        }
        types::SessionUpdate::TranscriptRetraction { retraction } => {
            Some(model::SessionUpdate::TranscriptRetraction(retraction))
        }
        types::SessionUpdate::TaskStateUpdate { update: task_update } => {
            Some(model::SessionUpdate::TaskStateUpdate(task_update))
        }
        types::SessionUpdate::AvailableCommandsUpdate { commands, source, generation } => {
            Some(model::SessionUpdate::AvailableCommandsUpdate(map_available_commands_update(
                commands, source, generation,
            )))
        }
        types::SessionUpdate::AvailableAgentsUpdate { agents } => {
            Some(model::SessionUpdate::AvailableAgentsUpdate(map_available_agents_update(agents)))
        }
        types::SessionUpdate::ModeStateUpdate { mode } => {
            Some(model::SessionUpdate::ModeStateUpdate(convert_mode_state(mode)))
        }
        types::SessionUpdate::CurrentModelUpdate { current_model } => Some(
            model::SessionUpdate::CurrentModelUpdate(model::CurrentModelUpdate::new(current_model)),
        ),
        types::SessionUpdate::ConfigOptionUpdate { option_id, value } => {
            Some(model::SessionUpdate::ConfigOptionUpdate(model::ConfigOptionUpdate {
                option_id,
                value,
            }))
        }
        types::SessionUpdate::UltracodeUpdate { ultracode } => {
            Some(model::SessionUpdate::UltracodeUpdate { ultracode })
        }
        types::SessionUpdate::FastModeUpdate { fast_mode_state, fast_mode_disabled_reason } => {
            Some(model::SessionUpdate::FastModeUpdate {
                state: fast_mode_state,
                disabled_reason: fast_mode_disabled_reason,
            })
        }
        types::SessionUpdate::RateLimitUpdate {
            status,
            error_code,
            resets_at,
            utilization,
            rate_limit_type,
            limit_scope,
            overage_enabled,
            overage_status,
            overage_resets_at,
            overage_disabled_reason,
            is_using_overage,
            surpassed_threshold,
            can_user_purchase_credits,
            has_chargeable_saved_payment_method,
        } => Some(model::SessionUpdate::RateLimitUpdate(types::RateLimitUpdate {
            status,
            error_code,
            resets_at,
            utilization,
            rate_limit_type,
            limit_scope,
            overage_enabled,
            overage_status,
            overage_resets_at,
            overage_disabled_reason,
            is_using_overage,
            surpassed_threshold,
            can_user_purchase_credits,
            has_chargeable_saved_payment_method,
        })),
        types::SessionUpdate::ApiRetryUpdate {
            attempt,
            max_retries,
            retry_delay_ms,
            error_status,
            error,
        } => Some(model::SessionUpdate::ApiRetryUpdate {
            attempt,
            max_retries,
            retry_delay_ms,
            error_status,
            error,
        }),
        types::SessionUpdate::PromptSuggestionUpdate { suggestion } => {
            Some(model::SessionUpdate::PromptSuggestionUpdate(suggestion))
        }
        types::SessionUpdate::SessionTitleUpdate { title } => {
            Some(model::SessionUpdate::SessionTitleUpdate(
                crate::session_config::sanitize_session_title(&title),
            ))
        }
        types::SessionUpdate::RuntimeSessionStateUpdate { state } => {
            Some(model::SessionUpdate::RuntimeSessionStateUpdate(state))
        }
        types::SessionUpdate::SettingsParseError { file, path, message } => {
            Some(model::SessionUpdate::SettingsParseError { file, path, message })
        }
        types::SessionUpdate::SessionStatusUpdate { status } => {
            Some(model::SessionUpdate::SessionStatusUpdate(status))
        }
        types::SessionUpdate::NotificationUpdate { notification, replay } => {
            Some(model::SessionUpdate::NotificationUpdate { notification, replay })
        }
        types::SessionUpdate::SystemNoticeUpdate { severity, message } => {
            Some(model::SessionUpdate::SystemNoticeUpdate { severity, message })
        }
        types::SessionUpdate::CompactionUpdate { update: compaction } => {
            Some(model::SessionUpdate::CompactionUpdate(match compaction {
                types::CompactionUpdate::Started => model::CompactionUpdate::Started,
                types::CompactionUpdate::Boundary {
                    trigger,
                    pre_tokens,
                    post_tokens,
                    duration_ms,
                } => model::CompactionUpdate::Boundary(model::CompactionBoundary {
                    trigger,
                    pre_tokens,
                    post_tokens,
                    duration_ms,
                }),
                types::CompactionUpdate::Finished { result, error_code, error } => {
                    model::CompactionUpdate::Finished { result, error_code, error }
                }
            }))
        }
    }
}

pub(super) fn map_permission_request(
    session_id: &str,
    request: types::PermissionRequest,
) -> (model::RequestPermissionRequest, String) {
    let tool_call_id = request.tool_call.tool_call_id.clone();
    let tool_call_meta = request.tool_call.meta.clone();
    let tool_call_fields = convert_tool_call_to_fields(request.tool_call);
    let mut tool_call_update = model::ToolCallUpdate::new(tool_call_id.clone(), tool_call_fields);
    if let Some(meta) = tool_call_meta {
        tool_call_update = tool_call_update.meta(meta);
    }
    let options = request
        .options
        .into_iter()
        .map(|opt| {
            let kind = match opt.kind.as_str() {
                "allow_once" => model::PermissionOptionKind::AllowOnce,
                "allow_session" => model::PermissionOptionKind::AllowSession,
                "allow_always" => model::PermissionOptionKind::AllowAlways,
                "reject_once" => model::PermissionOptionKind::RejectOnce,
                "question_choice" => model::PermissionOptionKind::QuestionChoice,
                "plan_approve" => model::PermissionOptionKind::PlanApprove,
                "plan_reject" => model::PermissionOptionKind::PlanReject,
                _ => {
                    tracing::warn!(
                        "unknown permission option kind from bridge; defaulting to reject_once: session_id={} tool_call_id={} option_id={} option_name={} option_kind={}",
                        session_id,
                        tool_call_id,
                        opt.option_id,
                        opt.name,
                        opt.kind
                    );
                    model::PermissionOptionKind::RejectOnce
                }
            };
            model::PermissionOption::new(opt.option_id, opt.name, kind).description(opt.description)
        })
        .collect();
    (
        model::RequestPermissionRequest::new(
            model::SessionId::new(session_id),
            tool_call_update,
            options,
            convert_permission_display(request.display),
        )
        .mcp_server(request.mcp_server),
        tool_call_id,
    )
}

pub(super) fn map_question_request(
    session_id: &str,
    request: types::QuestionRequest,
) -> (model::RequestQuestionRequest, String) {
    let tool_call_id = request.tool_call.tool_call_id.clone();
    let tool_call_meta = request.tool_call.meta.clone();
    let tool_call_fields = convert_tool_call_to_fields(request.tool_call);
    let mut tool_call_update = model::ToolCallUpdate::new(tool_call_id.clone(), tool_call_fields);
    if let Some(meta) = tool_call_meta {
        tool_call_update = tool_call_update.meta(meta);
    }

    let prompt = request.prompt;

    (
        model::RequestQuestionRequest::new(
            model::SessionId::new(session_id),
            tool_call_update,
            prompt,
            usize::try_from(request.question_index).unwrap_or(0),
            usize::try_from(request.total_questions).unwrap_or(0),
        )
        .idle_timeout_ms(request.idle_timeout_ms),
        tool_call_id,
    )
}

/// Convert a wire `user_dialog_request` into the app model. Returns the model
/// request and the synthetic `request_id` used as its focus-queue/index key.
pub(super) fn map_user_dialog_request(
    session_id: &str,
    request: types::UserDialogRequest,
) -> (model::RequestUserDialogRequest, String) {
    let request_id = request.request_id.clone();
    let payload = request.payload;
    let options = request.options;
    (
        model::RequestUserDialogRequest::new(
            model::SessionId::new(session_id),
            request_id.clone(),
            request.dialog_kind,
            payload,
            options,
        ),
        request_id,
    )
}

pub(super) fn convert_content_block(content: types::ContentBlock) -> Option<model::ContentBlock> {
    match content {
        types::ContentBlock::Text { text } => {
            Some(model::ContentBlock::Text(model::TextContent::new(text)))
        }
        types::ContentBlock::Image { mime_type, uri: _, data } => {
            let mime = mime_type.unwrap_or_else(|| "image/png".to_owned());
            let image_data = data.unwrap_or_default();
            if !crate::app::clipboard_image::is_supported_image_type(&mime) {
                tracing::warn!(mime_type = %mime, "convert_content_block: skipping unsupported image type");
                return None;
            }
            if image_data.is_empty() {
                tracing::warn!("convert_content_block: skipping image block with empty data");
                return None;
            }
            Some(model::ContentBlock::Image(model::ImageContent::new(image_data, mime)))
        }
    }
}

pub(super) fn convert_tool_call(tool_call: types::ToolCall) -> model::ToolCall {
    let types::ToolCall {
        tool_call_id,
        title,
        kind,
        status,
        source_message_uuid,
        content,
        raw_input,
        raw_output,
        output_metadata,
        task_metadata,
        locations,
        meta,
    } = tool_call;

    let mut tc = model::ToolCall::new(tool_call_id, title)
        .kind(convert_tool_kind(&kind))
        .status(convert_tool_status(&status))
        .source_message_uuid(source_message_uuid)
        .content(content.into_iter().filter_map(convert_tool_call_content).collect())
        .locations(
            locations
                .into_iter()
                .map(|loc| {
                    let mut location = model::ToolCallLocation::new(loc.path);
                    if let Some(line) = loc.line.and_then(|line| u32::try_from(line).ok()) {
                        location = location.line(line);
                    }
                    location
                })
                .collect(),
        );

    if let Some(raw_input) = raw_input {
        tc = tc.raw_input(raw_input);
    }

    if let Some(raw_output) = raw_output {
        tc = tc.raw_output(serde_json::Value::String(raw_output));
    }
    if let Some(output_metadata) = output_metadata {
        tc = tc.output_metadata(output_metadata);
    }
    if let Some(task_metadata) = task_metadata {
        tc = tc.task_metadata(task_metadata);
    }
    if let Some(meta) = meta {
        tc = tc.meta(meta);
    }

    tc
}

pub(super) fn convert_tool_call_update(update: types::ToolCallUpdate) -> model::ToolCallUpdate {
    let update_meta = update.fields.meta.clone();
    let mut out = model::ToolCallUpdate::new(
        update.tool_call_id,
        convert_tool_call_update_fields(update.fields),
    )
    .source_message_uuid(update.source_message_uuid);
    if let Some(meta) = update_meta {
        out = out.meta(meta);
    }
    out
}

pub(super) fn convert_tool_call_to_fields(
    tool_call: types::ToolCall,
) -> model::ToolCallUpdateFields {
    let mut fields = model::ToolCallUpdateFields::new()
        .title(tool_call.title)
        .kind(convert_tool_kind(&tool_call.kind))
        .status(convert_tool_status(&tool_call.status))
        .content(
            tool_call.content.into_iter().filter_map(convert_tool_call_content).collect::<Vec<_>>(),
        )
        .locations(
            tool_call
                .locations
                .into_iter()
                .map(|loc| {
                    let mut location = model::ToolCallLocation::new(loc.path);
                    if let Some(line) = loc.line.and_then(|line| u32::try_from(line).ok()) {
                        location = location.line(line);
                    }
                    location
                })
                .collect::<Vec<_>>(),
        );

    if let Some(raw_input) = tool_call.raw_input {
        fields = fields.raw_input(raw_input);
    }

    if let Some(raw_output) = tool_call.raw_output {
        fields = fields.raw_output(serde_json::Value::String(raw_output));
    }
    if let Some(output_metadata) = tool_call.output_metadata {
        fields = fields.output_metadata(output_metadata);
    }
    if let Some(task_metadata) = tool_call.task_metadata {
        fields = fields.task_metadata(task_metadata);
    }

    fields
}

pub(super) fn convert_tool_call_update_fields(
    fields: types::ToolCallUpdateFields,
) -> model::ToolCallUpdateFields {
    let mut out = model::ToolCallUpdateFields::new();

    if let Some(title) = fields.title {
        out = out.title(title);
    }
    if let Some(kind) = fields.kind {
        out = out.kind(convert_tool_kind(&kind));
    }
    if let Some(status) = fields.status {
        out = out.status(convert_tool_status(&status));
    }
    if let Some(content) = fields.content {
        out = out
            .content(content.into_iter().filter_map(convert_tool_call_content).collect::<Vec<_>>());
    }
    if let Some(raw_input) = fields.raw_input {
        out = out.raw_input(raw_input);
    }
    if let Some(raw_output) = fields.raw_output {
        out = out.raw_output(serde_json::Value::String(raw_output));
    }
    if let Some(output_metadata) = fields.output_metadata {
        out = out.output_metadata(output_metadata);
    }
    if let Some(task_metadata) = fields.task_metadata {
        out = out.task_metadata(task_metadata);
    }
    if let Some(locations) = fields.locations {
        out = out.locations(
            locations
                .into_iter()
                .map(|loc| {
                    let mut location = model::ToolCallLocation::new(loc.path);
                    if let Some(line) = loc.line.and_then(|line| u32::try_from(line).ok()) {
                        location = location.line(line);
                    }
                    location
                })
                .collect::<Vec<_>>(),
        );
    }

    out
}

fn convert_permission_display(
    display: Option<types::PermissionDisplay>,
) -> Option<model::PermissionDisplay> {
    display.filter(|display| !display.is_empty())
}

fn convert_tool_call_content(
    tool_content: types::ToolCallContent,
) -> Option<model::ToolCallContent> {
    match tool_content {
        types::ToolCallContent::Content { content } => {
            let block = convert_content_block(content)?;
            Some(model::ToolCallContent::Content(model::Content::new(block)))
        }
        types::ToolCallContent::Diff { old_path: _, new_path, old, new, repository } => {
            Some(model::ToolCallContent::Diff(
                model::Diff::new(new_path, new).old_text(Some(old)).repository(repository),
            ))
        }
        types::ToolCallContent::McpResource { uri, mime_type, text, blob_saved_to } => {
            Some(model::ToolCallContent::McpResource(
                model::McpResource::new(uri)
                    .mime_type(mime_type)
                    .text(text)
                    .blob_saved_to(blob_saved_to),
            ))
        }
        types::ToolCallContent::ResourceLink {
            uri,
            name,
            title,
            description,
            mime_type,
            size,
            annotations,
        } => Some(model::ToolCallContent::ResourceLink(
            model::McpResourceLink::new(uri, name)
                .title(title)
                .description(description)
                .mime_type(mime_type)
                .size(size)
                .annotations(annotations),
        )),
    }
}

pub(super) fn convert_tool_kind(kind: &str) -> model::ToolKind {
    match kind {
        "read" => model::ToolKind::Read,
        "edit" => model::ToolKind::Edit,
        "delete" => model::ToolKind::Delete,
        "move" => model::ToolKind::Move,
        "execute" => model::ToolKind::Execute,
        "search" => model::ToolKind::Search,
        "fetch" => model::ToolKind::Fetch,
        "switch_mode" => model::ToolKind::SwitchMode,
        "other" => model::ToolKind::Other,
        _ => model::ToolKind::Think,
    }
}

pub(super) fn convert_tool_status(status: &str) -> model::ToolCallStatus {
    match status {
        "in_progress" => model::ToolCallStatus::InProgress,
        "detached" => model::ToolCallStatus::Detached,
        "completed" => model::ToolCallStatus::Completed,
        "failed" => model::ToolCallStatus::Failed,
        "killed" => model::ToolCallStatus::Killed,
        _ => model::ToolCallStatus::Pending,
    }
}

pub(super) fn convert_mode_state(mode: types::ModeState) -> ModeState {
    let available_modes: Vec<ModeInfo> =
        mode.available_modes.into_iter().map(|m| ModeInfo { id: m.id, name: m.name }).collect();
    ModeState {
        current_mode_id: mode.current_mode_id,
        current_mode_name: mode.current_mode_name,
        available_modes,
    }
}

#[cfg(test)]
mod tests {
    use super::{map_permission_request, map_question_request, map_session_update};
    use crate::agent::{model, types};

    #[test]
    fn completed_result_cards_follow_acknowledged_clock_changes_through_cached_live_layout() {
        use crate::agent::events::ClientEvent;
        use crate::agent::settings::{
            SettingsApplication, SettingsPersistence, SettingsResult, SettingsSnapshot,
        };
        use crate::agent::wire::{BridgeEvent, EventEnvelope};
        use crate::app::{App, HistoryOutputId};
        use crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding;
        use serde_json::json;

        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/tool-result-times.json"))
                .expect("SDK result fixture");
        let fixture = &fixtures[0];
        let mut app = App::test_default();
        app.session_runtime.activate_session("times".into());
        let updates = [
            json!({"type":"tool_call", "tool_call": {
                "tool_call_id":"timed-job", "title":"CronCreate", "kind":"other", "status":"in_progress", "content":[],
                "raw_input": fixture["input"], "locations":[], "meta":{"claudeCode":{"toolName":"CronCreate"}}
            }}),
            json!({"type":"tool_call_update", "tool_call_update":{
                "tool_call_id":"timed-job", "fields":{
                    "status":"completed", "content":[], "output_metadata":{"presentation": fixture["presentation"]}
                }
            }}),
        ];
        for update in updates {
            let event: EventEnvelope = serde_json::from_value(json!({
                "event":"session_update", "session_id":"times", "update":update
            }))
            .expect("live result wire event");
            let BridgeEvent::SessionUpdate { session_id, update } = event.event else {
                panic!("session update")
            };
            crate::app::events::handle_client_event(
                &mut app,
                ClientEvent::SessionUpdate {
                    session_id,
                    update: map_session_update(update).expect("domain update"),
                },
            );
            // Seed the real body cache before the completed result and before each preference change.
            let _ = serialize_live_rows_with_boundaries_excluding(
                &mut app,
                120,
                &std::collections::BTreeSet::default(),
                None,
            );
        }
        for (format, zone, expected) in [
            ("%F %T.%3f %z", "Europe/Berlin", "Next run: 2026-03-29 03:30:07.123 +0200"),
            ("%F %T.%3f %z", "UTC", "Next run: 2026-03-29 01:30:07.123 +0000"),
            ("24-hour-utc", "Europe/Berlin", "Next run: 2026-03-29 01:30Z"),
            ("12-hour", "Europe/Berlin", "Next run: 2026-03-29 3:30 AM"),
        ] {
            let mut snapshot = SettingsSnapshot::test_value("timeFormat", json!(format));
            snapshot.time_zone = Some(zone.into());
            snapshot.cwd.clone_from(&app.cwd_raw);
            app.config.pending_settings_request =
                Some(crate::app::config::PendingSettingsRequest::Inspection("clock".into()));
            crate::app::events::handle_client_event(
                &mut app,
                ClientEvent::SettingsResultReceived {
                    session_id: "times".into(),
                    request_id: Some("clock".into()),
                    result: SettingsResult {
                        persistence: SettingsPersistence::NotRequested,
                        application: SettingsApplication::Host,
                        snapshot: Some(snapshot),
                        error: None,
                    },
                },
            );
            let rendered = serialize_live_rows_with_boundaries_excluding(
                &mut app,
                120,
                &std::collections::BTreeSet::default(),
                None,
            );
            let segment = rendered
                .segments()
                .iter()
                .find(|segment| {
                    segment.ids.contains(&HistoryOutputId::ToolCall("timed-job".into()))
                })
                .expect("completed job region");
            let rows: Vec<String> = rendered
                .segment_rows(segment)
                .expect("card rows")
                .iter()
                .map(ToString::to_string)
                .collect();
            assert!(rows.iter().any(|row| row.contains("Schedule ID: create-a")), "{rows:?}");
            assert!(rows.iter().any(|row| row.contains(expected)), "{rows:?}");
            assert!(rows.iter().any(|row| row.contains("Prompt: Inspect the build")), "{rows:?}");
            assert_eq!(rows.iter().filter(|row| row.contains("Next run:")).count(), 1);
        }
    }

    #[test]
    fn current_model_wire_update_preserves_sdk_effort_levels() {
        let update = serde_json::from_value(serde_json::json!({
            "type": "current_model_update", "current_model": {
                "requested_id": "sonnet", "resolved_id": "claude-sonnet",
                "display_name_short": "Sonnet", "display_name_long": "Claude Sonnet",
                "catalog_id": "claude-sonnet", "supports_effort": true,
                "supported_effort_levels": ["low", "xhigh", "max"],
                "supports_fast_mode": true, "supports_auto_mode": false,
                "supports_adaptive_thinking": true, "is_authoritative": true
            }
        }))
        .expect("current model wire update");
        let Some(model::SessionUpdate::CurrentModelUpdate(update)) = map_session_update(update)
        else {
            panic!("current model update");
        };
        assert_eq!(
            update.current_model.supported_effort_levels,
            vec![model::EffortLevel::Low, model::EffortLevel::XHigh, model::EffortLevel::Max]
        );
        assert_eq!(update.current_model.supports_fast_mode, Some(true));
        assert_eq!(update.current_model.supports_auto_mode, Some(false));
    }

    #[test]
    fn session_title_updates_decode_from_the_bridge_wire() {
        let decode = |json: &str| {
            map_session_update(serde_json::from_str::<types::SessionUpdate>(json).expect("wire"))
        };
        assert_eq!(
            decode(r#"{"type":"session_title_update","title":" probe-e2e "}"#),
            Some(model::SessionUpdate::SessionTitleUpdate(Some("probe-e2e".to_owned())))
        );
        assert_eq!(
            decode(r#"{"type":"session_title_update","title":"  "}"#),
            Some(model::SessionUpdate::SessionTitleUpdate(None))
        );
        assert_eq!(
            decode(r#"{"type":"session_title_update","title":"a\u001b]52;c;eA==\u0007b"}"#),
            Some(model::SessionUpdate::SessionTitleUpdate(Some("a]52;c;eA==b".to_owned())))
        );
        // C1 controls such as ST (U+009C) and DEL are stripped as well.
        assert_eq!(
            decode(r#"{"type":"session_title_update","title":"name\u009c\u007f\u001b[2J"}"#),
            Some(model::SessionUpdate::SessionTitleUpdate(Some("name[2J".to_owned())))
        );
    }

    #[test]
    fn map_question_request_preserves_preview_and_annotation_shape() {
        let (request, tool_call_id) = map_question_request(
            "session-1",
            types::QuestionRequest {
                idle_timeout_ms: Some(60_000),
                tool_call: types::ToolCall {
                    tool_call_id: "tool-1".to_owned(),
                    title: "Pick target".to_owned(),
                    kind: "other".to_owned(),
                    status: "in_progress".to_owned(),
                    source_message_uuid: None,
                    content: Vec::new(),
                    raw_input: Some(serde_json::json!({ "source": "ask_user_question" })),
                    raw_output: None,
                    output_metadata: None,
                    task_metadata: None,
                    locations: Vec::new(),
                    meta: Some(
                        serde_json::json!({ "claudeCode": { "toolName": "AskUserQuestion" } }),
                    ),
                },
                prompt: types::QuestionPrompt {
                    question: "Where should this roll out?".to_owned(),
                    header: "Target".to_owned(),
                    multi_select: true,
                    options: vec![
                        types::QuestionOption {
                            option_id: "question_0".to_owned(),
                            label: "Staging".to_owned(),
                            description: Some("Validate in staging first".to_owned()),
                            preview: Some("Deploy to staging first.".to_owned()),
                        },
                        types::QuestionOption {
                            option_id: "question_1".to_owned(),
                            label: "Production".to_owned(),
                            description: Some("Customer-facing rollout".to_owned()),
                            preview: None,
                        },
                    ],
                },
                question_index: 1,
                total_questions: 3,
            },
        );

        assert_eq!(tool_call_id, "tool-1");
        assert_eq!(
            request,
            model::RequestQuestionRequest::new(
                model::SessionId::new("session-1"),
                model::ToolCallUpdate::new(
                    "tool-1",
                    model::ToolCallUpdateFields::new()
                        .title("Pick target")
                        .kind(model::ToolKind::Other)
                        .status(model::ToolCallStatus::InProgress)
                        .content(Vec::new())
                        .raw_input(serde_json::json!({ "source": "ask_user_question" }))
                        .locations(Vec::new()),
                )
                .meta(serde_json::json!({ "claudeCode": { "toolName": "AskUserQuestion" } })),
                model::QuestionPrompt::new(
                    "Where should this roll out?",
                    "Target",
                    true,
                    vec![
                        model::QuestionOption::new("question_0", "Staging")
                            .description(Some("Validate in staging first".to_owned()))
                            .preview(Some("Deploy to staging first.".to_owned())),
                        model::QuestionOption::new("question_1", "Production")
                            .description(Some("Customer-facing rollout".to_owned()))
                            .preview(None),
                    ],
                ),
                1,
                3,
            )
            .idle_timeout_ms(Some(60_000))
        );
    }

    fn permission_wire_fixture() -> serde_json::Value {
        serde_json::json!({
            "tool_call": {"tool_call_id":"mapped-tool", "title":"Inspect build", "kind":"execute", "status":"in_progress",
                "content":[], "locations":[], "raw_input":{"command":"check-build"},
                "meta":{"claudeCode":{"toolName":"Bash"}}},
            "options":[
                {"option_id":"permit-build", "name":"Run build", "description":"Once only", "kind":"allow_once"},
                {"option_id":"refuse-build", "name":"Stop build", "description":"Leave files alone", "kind":"reject_once"}],
            "mcp_server":{"name":"build-server", "source":"project"}
        })
    }

    #[test]
    fn mapped_permission_metadata_drives_the_actual_default_response() {
        use crate::app::test_support::{
            admit_tool, app_with_connection, assert_permission_response, press, request_permission,
        };
        for (display, expected_display, expected_id) in [
            (serde_json::Value::Null, None, "permit-build"),
            (
                serde_json::json!({"title":"  ","display_name":"\t","description":"\n"}),
                None,
                "permit-build",
            ),
            (
                serde_json::json!({"default_to_no":true}),
                Some(model::PermissionDisplay::new().default_to_no(true)),
                "refuse-build",
            ),
            (
                serde_json::json!({"default_to_no":false,"suppress_always_allow_rule":true}),
                Some(model::PermissionDisplay::new().suppress_always_allow_rule(true)),
                "permit-build",
            ),
            (
                serde_json::json!({"title":"  Build approval  ","display_name":" Build action ","description":" Changes outputs ","default_to_no":true,"suppress_always_allow_rule":true}),
                Some(
                    model::PermissionDisplay::new()
                        .title(Some("  Build approval  ".into()))
                        .display_name(Some(" Build action ".into()))
                        .description(Some(" Changes outputs ".into()))
                        .default_to_no(true)
                        .suppress_always_allow_rule(true),
                ),
                "refuse-build",
            ),
        ] {
            let mut fixture = permission_wire_fixture();
            if !display.is_null() {
                fixture["display"] = display;
            }
            let (request, id) = map_permission_request(
                "session-1",
                serde_json::from_value(fixture).expect("literal request"),
            );
            assert_eq!(id, "mapped-tool");
            assert_eq!(request.session_id.to_string(), "session-1");
            assert_eq!(request.tool_call.tool_call_id, "mapped-tool");
            assert_eq!(request.tool_call.fields.title.as_deref(), Some("Inspect build"));
            assert_eq!(request.tool_call.fields.kind, Some(model::ToolKind::Execute));
            assert_eq!(request.tool_call.fields.status, Some(model::ToolCallStatus::InProgress));
            assert_eq!(
                request.tool_call.fields.raw_input,
                Some(serde_json::json!({"command":"check-build"}))
            );
            assert_eq!(
                request.tool_call.meta,
                Some(serde_json::json!({"claudeCode":{"toolName":"Bash"}}))
            );
            assert_eq!(
                request.options,
                vec![
                    model::PermissionOption::new(
                        "permit-build",
                        "Run build",
                        model::PermissionOptionKind::AllowOnce
                    )
                    .description(Some("Once only".into())),
                    model::PermissionOption::new(
                        "refuse-build",
                        "Stop build",
                        model::PermissionOptionKind::RejectOnce
                    )
                    .description(Some("Leave files alone".into())),
                ]
            );
            assert_eq!(request.display, expected_display);
            assert_eq!(
                request.mcp_server,
                Some(model::McpServerProvenance {
                    name: "build-server".into(),
                    source: "project".into()
                })
            );
            let (mut app, mut commands) = app_with_connection();
            app.status = crate::app::AppStatus::Running;
            admit_tool(
                &mut app,
                model::ToolCall::new("mapped-tool", "Inspect build")
                    .status(model::ToolCallStatus::InProgress),
            );
            let mut receiver = request_permission(&mut app, request);
            press(&mut app, crossterm::event::KeyCode::Enter);
            assert_permission_response(&mut receiver, expected_id);
            assert!(commands.try_recv().is_err());
        }
    }

    #[test]
    fn unknown_permission_kinds_are_reject_once_without_losing_option_identity() {
        for reverse in [false, true] {
            let mut fixture = permission_wire_fixture();
            fixture["options"].as_array_mut().expect("options").push(serde_json::json!({
                "option_id":"future-id", "name":"Future choice", "description":"Future explanation", "kind":"future_permission_kind"
            }));
            if reverse {
                fixture["options"].as_array_mut().expect("options").reverse();
            }
            let (request, id) = map_permission_request(
                "unknown-session",
                serde_json::from_value(fixture).expect("wire request"),
            );
            assert_eq!(id, "mapped-tool");
            assert_eq!(request.tool_call.tool_call_id, "mapped-tool");
            assert_eq!(request.session_id.to_string(), "unknown-session");
            let mut expected = vec![
                model::PermissionOption::new(
                    "permit-build",
                    "Run build",
                    model::PermissionOptionKind::AllowOnce,
                )
                .description(Some("Once only".into())),
                model::PermissionOption::new(
                    "refuse-build",
                    "Stop build",
                    model::PermissionOptionKind::RejectOnce,
                )
                .description(Some("Leave files alone".into())),
                model::PermissionOption::new(
                    "future-id",
                    "Future choice",
                    model::PermissionOptionKind::RejectOnce,
                )
                .description(Some("Future explanation".into())),
            ];
            if reverse {
                expected.reverse();
            }
            assert_eq!(request.options, expected);
        }
    }
}

#[cfg(test)]
#[path = "mapping_audit_tests.rs"]
mod audit_tests;
