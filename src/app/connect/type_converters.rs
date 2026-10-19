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
    use super::{
        convert_account_info, convert_tool_call, convert_tool_call_update,
        convert_tool_call_update_fields, map_available_commands_update, map_available_models,
        map_permission_request, map_question_request, map_session_update,
    };
    use crate::agent::{model, types};

    #[test]
    fn shared_session_types_keep_wire_tokens_defaults_and_unknown_values() {
        for (payload, expected) in [
            (
                serde_json::json!({"type": "runtime_session_state_update", "state": "requires_action"}),
                model::SessionUpdate::RuntimeSessionStateUpdate(
                    model::RuntimeSessionState::RequiresAction,
                ),
            ),
            (
                serde_json::json!({"type": "system_notice_update", "severity": "warning", "message": "notice"}),
                model::SessionUpdate::SystemNoticeUpdate {
                    severity: model::SystemNoticeSeverity::Warning,
                    message: "notice".into(),
                },
            ),
        ] {
            assert_eq!(
                map_session_update(serde_json::from_value(payload).expect("wire event")),
                Some(expected)
            );
        }
        assert_eq!(
            serde_json::from_str::<types::ApiRetryError>("\"future_error\"")
                .expect("open error vocabulary"),
            model::ApiRetryError::Unknown
        );
        assert_eq!(
            serde_json::from_str::<types::CompactionFailureCode>("\"future_failure\"")
                .expect("open compaction failure vocabulary"),
            model::CompactionFailureCode::Unknown
        );
        let command: types::AvailableCommand =
            serde_json::from_value(serde_json::json!({"name": "review", "description": "Review"}))
                .expect("minimal command");
        assert!(command.aliases.is_empty());
        assert!(!command.builtin);
        let metadata: types::ToolOutputMetadata = serde_json::from_value(
            serde_json::json!({"read": {"file_path": "notes.md", "start_line": 4}}),
        )
        .expect("minimal output metadata");
        assert!(!metadata.staged);
        assert!(!metadata.structured_content_omitted);
        assert_eq!(
            serde_json::to_value(metadata).expect("output metadata")["read"],
            serde_json::json!({"file_path": "notes.md", "start_line": 4})
        );
    }

    #[test]
    fn read_wire_updates_replace_cached_preview_without_changing_source() {
        use crate::agent::events::ClientEvent;
        use crate::agent::wire::{BridgeEvent, EventEnvelope};
        use crate::app::{App, MessageBlock};
        use crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding;

        let mut app = App::test_default();
        app.session_runtime.activate_session("read-session".into());
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
                "event": "session_update", "session_id": "read-session", "update": update
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
                let body = rows
                    .iter()
                    .map(|row| {
                        row.spans.iter().map(|span| span.content.as_ref()).collect::<String>()
                    })
                    .find(|text| text.trim_start().starts_with(['│', '└']))
                    .expect("Read body row");
                if index == 0 {
                    assert_eq!(body.trim(), "└─ old preview");
                } else {
                    assert_eq!(body.trim(), "│  60  # Notes");
                    let MessageBlock::ToolCall(tc) = &app.transcript.messages[mi].blocks[bi] else {
                        panic!("expected Read card")
                    };
                    assert_eq!(tc.content, vec![model::ToolCallContent::from(source)]);
                    assert_eq!(tc.read_output_metadata().expect("Read metadata").start_line, 60);
                }
            }
        }
    }

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
    fn map_available_models_preserves_optional_fast_and_auto_metadata() {
        let mapped = map_available_models(vec![
            types::AvailableModel {
                id: "sonnet".to_owned(),
                resolved_model: Some("claude-sonnet-5".to_owned()),
                display_name: "Claude Sonnet".to_owned(),
                description: Some("Balanced model".to_owned()),
                supports_effort: true,
                supported_effort_levels: vec![
                    types::EffortLevel::Low,
                    types::EffortLevel::Medium,
                    types::EffortLevel::High,
                    types::EffortLevel::XHigh,
                    types::EffortLevel::Max,
                ],
                supports_adaptive_thinking: Some(true),
                supports_fast_mode: Some(true),
                supports_auto_mode: Some(false),
            },
            types::AvailableModel {
                id: "haiku".to_owned(),
                resolved_model: None,
                display_name: "Claude Haiku".to_owned(),
                description: None,
                supports_effort: false,
                supported_effort_levels: Vec::new(),
                supports_adaptive_thinking: None,
                supports_fast_mode: None,
                supports_auto_mode: None,
            },
        ]);

        assert_eq!(
            mapped,
            vec![
                model::AvailableModel::new("sonnet", "Claude Sonnet")
                    .resolved_model("claude-sonnet-5")
                    .description("Balanced model")
                    .supports_effort(true)
                    .supported_effort_levels(vec![
                        model::EffortLevel::Low,
                        model::EffortLevel::Medium,
                        model::EffortLevel::High,
                        model::EffortLevel::XHigh,
                        model::EffortLevel::Max,
                    ])
                    .supports_adaptive_thinking(Some(true))
                    .supports_fast_mode(Some(true))
                    .supports_auto_mode(Some(false)),
                model::AvailableModel::new("haiku", "Claude Haiku")
                    .supports_adaptive_thinking(None)
                    .supports_fast_mode(None)
                    .supports_auto_mode(None),
            ]
        );
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
    fn convert_account_info_maps_gateway_and_unknown_providers_to_app_model() {
        let gateway = convert_account_info(types::AccountInfo {
            email: Some("user@example.com".to_owned()),
            organization: Some("org-1".to_owned()),
            subscription_type: Some("Claude Max".to_owned()),
            token_source: Some("oauth".to_owned()),
            api_key_source: Some("user".to_owned()),
            api_provider: Some("gateway".to_owned()),
        });

        assert_eq!(gateway.email.as_deref(), Some("user@example.com"));
        assert_eq!(gateway.api_provider, Some(model::AccountApiProvider::Gateway));
        assert_eq!(gateway.login_method_label(), "External provider");

        let unknown = convert_account_info(types::AccountInfo {
            api_provider: Some("customGateway".to_owned()),
            ..Default::default()
        });

        assert_eq!(
            unknown.api_provider,
            Some(model::AccountApiProvider::Other("customGateway".to_owned()))
        );
        assert_eq!(
            unknown.api_provider.as_ref().map(model::AccountApiProvider::label),
            Some("customGateway")
        );
    }

    #[test]
    fn map_lifecycle_updates_preserves_new_sdk_state() {
        assert_eq!(
            map_session_update(types::SessionUpdate::ApiRetryUpdate {
                attempt: 2,
                max_retries: 4,
                retry_delay_ms: 1500.0,
                error_status: Some(529),
                error: types::ApiRetryError::ServerError,
            }),
            Some(model::SessionUpdate::ApiRetryUpdate {
                attempt: 2,
                max_retries: 4,
                retry_delay_ms: 1500.0,
                error_status: Some(529),
                error: model::ApiRetryError::ServerError,
            })
        );
        for (wire_error, model_error) in [
            (types::ApiRetryError::ModelNotFound, model::ApiRetryError::ModelNotFound),
            (types::ApiRetryError::OauthOrgNotAllowed, model::ApiRetryError::OauthOrgNotAllowed),
            (types::ApiRetryError::Overloaded, model::ApiRetryError::Overloaded),
        ] {
            assert_eq!(
                map_session_update(types::SessionUpdate::ApiRetryUpdate {
                    attempt: 1,
                    max_retries: 4,
                    retry_delay_ms: 1000.0,
                    error_status: None,
                    error: wire_error,
                }),
                Some(model::SessionUpdate::ApiRetryUpdate {
                    attempt: 1,
                    max_retries: 4,
                    retry_delay_ms: 1000.0,
                    error_status: None,
                    error: model_error,
                })
            );
        }
        assert_eq!(
            map_session_update(types::SessionUpdate::RuntimeSessionStateUpdate {
                state: types::RuntimeSessionState::RequiresAction,
            }),
            Some(model::SessionUpdate::RuntimeSessionStateUpdate(
                model::RuntimeSessionState::RequiresAction,
            ))
        );
        assert_eq!(
            map_session_update(types::SessionUpdate::PromptSuggestionUpdate {
                suggestion: "Add tests".to_owned(),
            }),
            Some(model::SessionUpdate::PromptSuggestionUpdate("Add tests".to_owned()))
        );
        assert_eq!(
            map_session_update(types::SessionUpdate::SessionStatusUpdate {
                status: types::SessionStatus::Requesting,
            }),
            Some(model::SessionUpdate::SessionStatusUpdate(model::SessionStatus::Requesting))
        );
        assert_eq!(
            map_session_update(types::SessionUpdate::SystemNoticeUpdate {
                severity: types::SystemNoticeSeverity::Warning,
                message: "Plugin install failed.".to_owned(),
            }),
            Some(model::SessionUpdate::SystemNoticeUpdate {
                severity: model::SystemNoticeSeverity::Warning,
                message: "Plugin install failed.".to_owned(),
            })
        );
        assert_eq!(
            map_session_update(types::SessionUpdate::CompactionUpdate {
                update: types::CompactionUpdate::Boundary {
                    trigger: types::CompactionTrigger::Manual,
                    pre_tokens: 123_456,
                    post_tokens: Some(20_000),
                    duration_ms: Some(1_500),
                },
            }),
            Some(model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Boundary(
                model::CompactionBoundary {
                    trigger: model::CompactionTrigger::Manual,
                    pre_tokens: 123_456,
                    post_tokens: Some(20_000),
                    duration_ms: Some(1_500),
                }
            )))
        );
        assert_eq!(
            map_session_update(types::SessionUpdate::CompactionUpdate {
                update: types::CompactionUpdate::Finished {
                    result: types::CompactionResult::Failed,
                    error_code: Some(types::CompactionFailureCode::TooFewGroups),
                    error: Some("Prompt is too long".to_owned()),
                },
            }),
            Some(model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Finished {
                result: model::CompactionResult::Failed,
                error_code: Some(model::CompactionFailureCode::TooFewGroups),
                error: Some("Prompt is too long".to_owned()),
            }))
        );
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
    fn map_session_update_converts_task_state_update() {
        let update: types::SessionUpdate = serde_json::from_value(serde_json::json!({
            "type": "task_state_update",
            "source": "task_update",
            "tasks": [
                {
                    "task_id": "task-1",
                    "subject": "Run checks",
                    "description": "Validate the branch",
                    "active_form": "Running checks",
                    "status": "in_progress",
                    "owner": "agent",
                    "blocks": ["task-2"],
                    "blocked_by": ["task-0"],
                    "metadata": { "phase": "6A" },
                    "source_tool_call_id": "tool-1"
                }
            ],
            "removed_task_ids": ["task-old"],
            "is_complete_snapshot": false
        }))
        .expect("task state update should deserialize");

        let mapped = map_session_update(update).expect("task state update should map");
        let model::SessionUpdate::TaskStateUpdate(update) = mapped else {
            panic!("expected task state update");
        };

        assert_eq!(update.source, model::TaskUpdateSource::Update);
        assert_eq!(update.removed_task_ids, vec!["task-old"]);
        assert!(!update.is_complete_snapshot);
        assert_eq!(update.tasks.len(), 1);
        assert_eq!(update.tasks[0].task_id, "task-1");
        assert_eq!(update.tasks[0].status, model::TaskStatus::InProgress);
        assert_eq!(update.tasks[0].metadata, Some(serde_json::json!({ "phase": "6A" })));
    }

    #[test]
    fn map_session_update_preserves_source_message_uuid() {
        let mapped = map_session_update(types::SessionUpdate::AgentMessageChunk {
            content: types::ContentBlock::Text { text: "hello".to_owned() },
            source_message_uuid: Some("assistant-1".to_owned()),
        })
        .expect("message chunk should map");

        let model::SessionUpdate::AgentMessageChunk(chunk) = mapped else {
            panic!("expected agent message chunk");
        };
        assert_eq!(chunk.source_message_uuid.as_deref(), Some("assistant-1"));
    }

    #[test]
    fn map_session_update_preserves_external_message_provenance() {
        let mapped = map_session_update(types::SessionUpdate::ExternalMessageUpdate {
            content: "Peer message".to_owned(),
            source_message_uuid: Some("message-peer".to_owned()),
            origin: types::MessageOrigin {
                kind: "peer".to_owned(),
                subkind: Some("peer-send-message".to_owned()),
                from: Some("reported-route".to_owned()),
                name: Some("Build session".to_owned()),
                from_session: Some("session-peer".to_owned()),
                sender_task_id: Some("task-peer".to_owned()),
                verified_peer_pid: Some(4242),
                from_mode: Some("bypass".to_owned()),
            },
        })
        .expect("external update should map");

        let model::SessionUpdate::ExternalMessageUpdate(update) = mapped else {
            panic!("expected external message update");
        };
        assert_eq!(update.content, "Peer message");
        assert_eq!(update.source_message_uuid.as_deref(), Some("message-peer"));
        assert_eq!(update.origin.kind, "peer");
        assert_eq!(update.origin.name.as_deref(), Some("Build session"));
        assert_eq!(update.origin.from_session.as_deref(), Some("session-peer"));
        assert_eq!(update.origin.verified_peer_pid, Some(4242));
        assert_eq!(update.origin.from_mode.as_deref(), Some("bypass"));
    }

    #[test]
    fn rate_limit_enablement_survives_both_conversion_paths() {
        for enabled in [None, Some(false), Some(true)] {
            let raw = serde_json::json!({
                "status": "allowed_warning", "overage_enabled": enabled, "is_using_overage": false
            });
            let direct: model::RateLimitUpdate =
                serde_json::from_value(raw.clone()).expect("rate limit");
            let mut update = raw;
            update["type"] = serde_json::json!("rate_limit_update");
            let mapped =
                map_session_update(serde_json::from_value(update).expect("session update"))
                    .expect("mapped update");
            let model::SessionUpdate::RateLimitUpdate(flattened) = mapped else {
                panic!("rate limit update");
            };
            assert_eq!(direct.overage_enabled, enabled);
            assert_eq!(flattened.overage_enabled, enabled);
            assert_eq!(flattened.is_using_overage, Some(false));
        }
    }

    #[test]
    fn map_session_update_preserves_rate_limit_credits_metadata() {
        let mapped = map_session_update(types::SessionUpdate::RateLimitUpdate {
            status: types::RateLimitStatus::Rejected,
            error_code: Some("credits_required".to_owned()),
            resets_at: None,
            utilization: None,
            rate_limit_type: None,
            limit_scope: None,
            overage_enabled: None,
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: None,
            is_using_overage: None,
            surpassed_threshold: None,
            can_user_purchase_credits: Some(true),
            has_chargeable_saved_payment_method: Some(false),
        })
        .expect("rate limit update should map");

        let model::SessionUpdate::RateLimitUpdate(update) = mapped else {
            panic!("expected rate limit update");
        };
        assert_eq!(update.status, model::RateLimitStatus::Rejected);
        assert_eq!(update.error_code.as_deref(), Some("credits_required"));
        assert_eq!(update.can_user_purchase_credits, Some(true));
        assert_eq!(update.has_chargeable_saved_payment_method, Some(false));
    }

    #[test]
    fn map_session_update_converts_transcript_retraction() {
        let mapped = map_session_update(types::SessionUpdate::TranscriptRetraction {
            retraction: types::TranscriptRetraction {
                message_uuids: vec!["old-1".to_owned()],
                reason: types::TranscriptRetractionReason::AssistantSupersedes,
                request_id: Some("req-1".to_owned()),
                trigger: None,
                direction: None,
                original_model: None,
                fallback_model: None,
                scope: Some("local".to_owned()),
                api_refusal_category: None,
                api_refusal_explanation: None,
                content: None,
            },
        })
        .expect("transcript retraction should map");

        let model::SessionUpdate::TranscriptRetraction(retraction) = mapped else {
            panic!("expected transcript retraction");
        };
        assert_eq!(retraction.message_uuids, vec!["old-1"]);
        assert_eq!(retraction.reason, model::TranscriptRetractionReason::AssistantSupersedes);
        assert_eq!(retraction.request_id.as_deref(), Some("req-1"));
    }

    #[test]
    fn map_permission_request_preserves_display_and_mcp_provenance() {
        let (request, tool_call_id) = map_permission_request(
            "session-1",
            types::PermissionRequest {
                tool_call: types::ToolCall {
                    tool_call_id: "tool-1".to_owned(),
                    title: "Bash npm test".to_owned(),
                    kind: "execute".to_owned(),
                    status: "in_progress".to_owned(),
                    source_message_uuid: None,
                    content: Vec::new(),
                    raw_input: None,
                    raw_output: None,
                    output_metadata: None,
                    task_metadata: None,
                    locations: Vec::new(),
                    meta: None,
                },
                options: vec![types::PermissionOption {
                    option_id: "allow".to_owned(),
                    name: "Allow".to_owned(),
                    description: None,
                    kind: "allow_once".to_owned(),
                }],
                display: Some(types::PermissionDisplay {
                    title: Some("Claude wants to run tests".to_owned()),
                    display_name: Some("Run tests".to_owned()),
                    description: Some("This command reads project files".to_owned()),
                    default_to_no: false,
                    suppress_always_allow_rule: false,
                }),
                mcp_server: Some(types::McpServerProvenance {
                    name: "filesystem".to_owned(),
                    source: "project".to_owned(),
                }),
            },
        );

        assert_eq!(tool_call_id, "tool-1");
        assert_eq!(
            request.display,
            Some(
                model::PermissionDisplay::new()
                    .title(Some("Claude wants to run tests".to_owned()))
                    .display_name(Some("Run tests".to_owned()))
                    .description(Some("This command reads project files".to_owned())),
            )
        );
        assert_eq!(
            request.mcp_server,
            Some(model::McpServerProvenance {
                name: "filesystem".to_owned(),
                source: "project".to_owned(),
            })
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

    #[test]
    fn convert_tool_call_update_fields_preserves_output_metadata() {
        let fields = convert_tool_call_update_fields(types::ToolCallUpdateFields {
            status: Some("completed".to_owned()),
            output_metadata: Some(types::ToolOutputMetadata {
                presentation: None,
                timing: None,
                staged: false,
                structured_content_omitted: true,
                bash: Some(types::BashOutputMetadata {
                    assistant_auto_backgrounded: Some(true),
                    timed_out_after_ms: Some(10_000),
                    background_cwd_hint: Some("session cwd unchanged".to_owned()),
                    background_ends_with_final_response: Some(true),
                }),
                agent: Some(types::AgentOutputMetadata {
                    resolved_model: Some("claude-sonnet-4-7".to_owned()),
                    models_used: Some(vec![
                        "claude-opus-4-8".to_owned(),
                        "claude-sonnet-4-7".to_owned(),
                    ]),
                }),
                web_fetch: Some(types::WebFetchOutputMetadata {
                    artifact_read: Some(types::WebFetchArtifactReadMetadata {
                        slug: "dashboard".to_owned(),
                        ver: Some("v2".to_owned()),
                        seeded: Some(false),
                    }),
                }),
                skill: Some(types::SkillOutputMetadata { background: Some(true) }),
                read: Some(types::ReadOutputMetadata {
                    file_path: "src/lib.rs".into(),
                    start_line: 42,
                }),
                non_execution: Some(types::ToolNonExecutionMetadata {
                    kind: "interrupted".to_owned(),
                    user_feedback: Some("Stopped intentionally.".to_owned()),
                }),
            }),
            ..types::ToolCallUpdateFields::default()
        });

        assert_eq!(
            fields.output_metadata,
            Some(
                model::ToolOutputMetadata::new()
                    .structured_content_omitted(true)
                    .bash(Some(
                        model::BashOutputMetadata::new()
                            .assistant_auto_backgrounded(Some(true))
                            .timed_out_after_ms(Some(10_000))
                            .background_cwd_hint(Some("session cwd unchanged".to_owned()))
                            .background_ends_with_final_response(Some(true)),
                    ))
                    .agent(Some(
                        model::AgentOutputMetadata::new()
                            .resolved_model(Some("claude-sonnet-4-7".to_owned()))
                            .models_used(Some(vec![
                                "claude-opus-4-8".to_owned(),
                                "claude-sonnet-4-7".to_owned(),
                            ])),
                    ))
                    .web_fetch(Some(
                        model::WebFetchOutputMetadata::new().artifact_read(Some(
                            model::WebFetchArtifactReadMetadata::new("dashboard")
                                .ver(Some("v2".to_owned()))
                                .seeded(Some(false)),
                        ))
                    ))
                    .skill(Some(model::SkillOutputMetadata::new().background(Some(true))))
                    .read(Some(model::ReadOutputMetadata {
                        file_path: "src/lib.rs".into(),
                        start_line: 42,
                    }))
                    .non_execution(Some(
                        model::ToolNonExecutionMetadata::new("interrupted")
                            .user_feedback(Some("Stopped intentionally.".to_owned())),
                    ))
            )
        );
    }

    #[test]
    fn convert_tool_call_update_preserves_source_message_uuid() {
        let update = convert_tool_call_update(types::ToolCallUpdate {
            tool_call_id: "tool-1".to_owned(),
            source_message_uuid: Some("user-result".to_owned()),
            fields: types::ToolCallUpdateFields::default(),
        });

        assert_eq!(update.source_message_uuid.as_deref(), Some("user-result"));
    }

    #[test]
    fn map_available_commands_update_preserves_source_and_generation() {
        let update = map_available_commands_update(
            vec![types::AvailableCommand {
                name: "project-command".to_owned(),
                description: "Project command".to_owned(),
                input_hint: Some("<value>".to_owned()),
                aliases: vec!["project-alias".to_owned()],
                builtin: true,
            }],
            Some("commands_changed".to_owned()),
            Some(3),
        );

        assert_eq!(
            update,
            model::AvailableCommandsUpdate::new(vec![
                model::AvailableCommand::new("project-command", "Project command")
                    .input_hint("<value>")
                    .aliases(vec!["project-alias".to_owned()])
                    .builtin(true)
            ])
            .source("commands_changed")
            .generation(3)
        );
    }

    #[test]
    fn convert_tool_status_maps_killed() {
        assert_eq!(super::convert_tool_status("killed"), model::ToolCallStatus::Killed);
    }

    #[test]
    fn convert_tool_call_update_fields_preserves_task_metadata() {
        let fields = convert_tool_call_update_fields(types::ToolCallUpdateFields {
            task_metadata: Some(types::TaskMetadata {
                end_time: Some(123),
                total_paused_ms: Some(45),
                error: Some("Task stopped".to_owned()),
                is_backgrounded: Some(true),
                spawn_depth: Some(2),
                request_id: Some("request-1".to_owned()),
                subagent_type: Some("tester".to_owned()),
                task_description: Some("Validate changes".to_owned()),
                task_type: Some("remote_agent".to_owned()),
                workflow_name: Some("release-check".to_owned()),
                prompt: Some("Run validation".to_owned()),
                output_file: Some("C:/tmp/output.md".to_owned()),
                summary: Some("Validation complete".to_owned()),
                terminal_status: Some("completed".to_owned()),
                terminal_reason: Some("worker_restart".to_owned()),
                blocked: Some(true),
                parent_agent_id: Some("agent-parent".to_owned()),
                ambient: Some(true),
                subagent_retry: None,
            }),
            ..types::ToolCallUpdateFields::default()
        });

        assert_eq!(
            fields.task_metadata,
            Some(
                model::TaskMetadata::new()
                    .end_time(Some(123))
                    .total_paused_ms(Some(45))
                    .error(Some("Task stopped".to_owned()))
                    .backgrounded(Some(true))
                    .spawn_depth(Some(2))
                    .request_id(Some("request-1".to_owned()))
                    .subagent_type(Some("tester".to_owned()))
                    .task_description(Some("Validate changes".to_owned()))
                    .task_type(Some("remote_agent".to_owned()))
                    .workflow_name(Some("release-check".to_owned()))
                    .prompt(Some("Run validation".to_owned()))
                    .output_file(Some("C:/tmp/output.md".to_owned()))
                    .summary(Some("Validation complete".to_owned()))
                    .terminal_status(Some("completed".to_owned()))
                    .terminal_reason(Some("worker_restart".to_owned()))
                    .blocked(Some(true))
                    .parent_agent_id(Some("agent-parent".to_owned()))
                    .ambient(Some(true)),
            )
        );
    }

    #[test]
    fn convert_tool_call_preserves_task_metadata() {
        let tool_call = convert_tool_call(types::ToolCall {
            tool_call_id: "tool-task".to_owned(),
            title: "Agent task".to_owned(),
            kind: "think".to_owned(),
            status: "killed".to_owned(),
            source_message_uuid: Some("assistant-tool".to_owned()),
            content: Vec::new(),
            raw_input: None,
            raw_output: None,
            output_metadata: None,
            task_metadata: Some(types::TaskMetadata {
                end_time: Some(77),
                total_paused_ms: Some(11),
                error: Some("Task stopped".to_owned()),
                is_backgrounded: Some(false),
                spawn_depth: Some(1),
                request_id: Some("request-2".to_owned()),
                subagent_type: Some("reviewer".to_owned()),
                task_description: Some("Review changes".to_owned()),
                task_type: Some("local_workflow".to_owned()),
                workflow_name: Some("review-flow".to_owned()),
                prompt: Some("Review changes".to_owned()),
                output_file: Some("C:/tmp/review.md".to_owned()),
                summary: Some("Review stopped".to_owned()),
                terminal_status: Some("killed".to_owned()),
                terminal_reason: Some("worker_restart".to_owned()),
                blocked: Some(false),
                parent_agent_id: Some("agent-root".to_owned()),
                ambient: Some(false),
                subagent_retry: None,
            }),
            locations: Vec::new(),
            meta: None,
        });

        assert_eq!(tool_call.status, model::ToolCallStatus::Killed);
        assert_eq!(tool_call.source_message_uuid.as_deref(), Some("assistant-tool"));
        assert_eq!(
            tool_call.task_metadata,
            Some(
                model::TaskMetadata::new()
                    .end_time(Some(77))
                    .total_paused_ms(Some(11))
                    .error(Some("Task stopped".to_owned()))
                    .backgrounded(Some(false))
                    .spawn_depth(Some(1))
                    .request_id(Some("request-2".to_owned()))
                    .subagent_type(Some("reviewer".to_owned()))
                    .task_description(Some("Review changes".to_owned()))
                    .task_type(Some("local_workflow".to_owned()))
                    .workflow_name(Some("review-flow".to_owned()))
                    .prompt(Some("Review changes".to_owned()))
                    .output_file(Some("C:/tmp/review.md".to_owned()))
                    .summary(Some("Review stopped".to_owned()))
                    .terminal_status(Some("killed".to_owned()))
                    .terminal_reason(Some("worker_restart".to_owned()))
                    .blocked(Some(false))
                    .parent_agent_id(Some("agent-root".to_owned()))
                    .ambient(Some(false)),
            )
        );
    }

    #[test]
    fn convert_tool_call_preserves_diff_repository() {
        let tool_call = convert_tool_call(types::ToolCall {
            tool_call_id: "tool-1".to_owned(),
            title: "Write src/main.rs".to_owned(),
            kind: "edit".to_owned(),
            status: "completed".to_owned(),
            source_message_uuid: None,
            content: vec![types::ToolCallContent::Diff {
                old_path: "src/main.rs".to_owned(),
                new_path: "src/main.rs".to_owned(),
                old: "old".to_owned(),
                new: "new".to_owned(),
                repository: Some("acme/project".to_owned()),
            }],
            raw_input: None,
            raw_output: None,
            output_metadata: None,
            task_metadata: None,
            locations: Vec::new(),
            meta: None,
        });

        assert_eq!(
            tool_call.content,
            vec![model::ToolCallContent::Diff(
                model::Diff::new("src/main.rs", "new")
                    .old_text(Some("old"))
                    .repository(Some("acme/project".to_owned())),
            )]
        );
    }

    #[test]
    fn convert_tool_call_preserves_mcp_resource_blob_path() {
        let tool_call = convert_tool_call(types::ToolCall {
            tool_call_id: "tool-2".to_owned(),
            title: "ReadMcpResource docs file://manual.pdf".to_owned(),
            kind: "read".to_owned(),
            status: "completed".to_owned(),
            source_message_uuid: None,
            content: vec![types::ToolCallContent::McpResource {
                uri: "file://manual.pdf".to_owned(),
                mime_type: Some("application/pdf".to_owned()),
                text: Some(
                    "[Resource from docs at file://manual.pdf] Saved to C:\\tmp\\manual.pdf"
                        .to_owned(),
                ),
                blob_saved_to: Some("C:\\tmp\\manual.pdf".to_owned()),
            }],
            raw_input: None,
            raw_output: None,
            output_metadata: None,
            task_metadata: None,
            locations: Vec::new(),
            meta: None,
        });

        assert_eq!(
            tool_call.content,
            vec![model::ToolCallContent::McpResource(
                model::McpResource::new("file://manual.pdf")
                    .mime_type(Some("application/pdf".to_owned()))
                    .text(Some(
                        "[Resource from docs at file://manual.pdf] Saved to C:\\tmp\\manual.pdf"
                            .to_owned(),
                    ))
                    .blob_saved_to(Some("C:\\tmp\\manual.pdf".to_owned())),
            )]
        );
    }

    #[test]
    fn convert_tool_call_preserves_mcp_resource_link_metadata() {
        let annotations = std::collections::BTreeMap::from([(
            "audience".to_owned(),
            serde_json::json!(["user"]),
        )]);
        let tool_call = convert_tool_call(types::ToolCall {
            tool_call_id: "tool-link".to_owned(),
            title: "Export report".to_owned(),
            kind: "other".to_owned(),
            status: "completed".to_owned(),
            source_message_uuid: None,
            content: vec![types::ToolCallContent::ResourceLink {
                uri: "mcp://docs/report.csv".to_owned(),
                name: "report.csv".to_owned(),
                title: Some("Quarterly report".to_owned()),
                description: Some("Generated report".to_owned()),
                mime_type: Some("text/csv".to_owned()),
                size: Some(42),
                annotations: Some(annotations.clone()),
            }],
            raw_input: None,
            raw_output: None,
            output_metadata: None,
            task_metadata: None,
            locations: Vec::new(),
            meta: None,
        });

        assert_eq!(
            tool_call.content,
            vec![model::ToolCallContent::ResourceLink(
                model::McpResourceLink::new("mcp://docs/report.csv", "report.csv")
                    .title(Some("Quarterly report".to_owned()))
                    .description(Some("Generated report".to_owned()))
                    .mime_type(Some("text/csv".to_owned()))
                    .size(Some(42))
                    .annotations(Some(annotations)),
            )]
        );
    }

    #[test]
    fn mcp_wire_status_preserves_sdk_config_and_policy_fields() {
        let mapped: model::McpServerStatus = serde_json::from_value(serde_json::json!({
            "name": "notion", "status": "needs-auth", "config": {
                "type": "http", "url": "https://mcp.notion.com/mcp", "tools": [
                    {"name": "search", "permission_policy": "always_deny", "org_max_permission": "blocked"},
                    {"name": "lookup", "org_max_permission": "ask"}
                ], "timeout": 5000, "request_timeout_ms": 30000, "always_load": true
            }, "scope": "project", "source": "project"
        })).expect("MCP status wire payload");
        assert_eq!(mapped.status, model::McpServerConnectionStatus::NeedsAuth);
        assert!(mapped.tools.is_empty());
        assert_eq!(
            serde_json::to_value(&mapped).expect("serialize status")["status"],
            "needs-auth"
        );
        let Some(model::McpServerStatusConfig::Http {
            tools,
            timeout,
            request_timeout_ms,
            always_load,
            ..
        }) = mapped.config
        else {
            panic!("expected http MCP config");
        };
        assert_eq!(timeout, Some(5000));
        assert_eq!(request_timeout_ms, Some(30000));
        assert_eq!(always_load, Some(true));
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].permission_policy, Some(model::McpServerToolPermissionPolicy::Deny));
        assert_eq!(tools[0].org_max_permission, Some(model::McpServerOrgMaxPermission::Blocked));
        assert_eq!(tools[1].permission_policy, None);
        assert_eq!(tools[1].org_max_permission, Some(model::McpServerOrgMaxPermission::Ask));
    }
}
