// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Type conversion functions: bridge wire types -> app model types.

use crate::agent::model;
use crate::agent::types;
use crate::app::{ModeInfo, ModeState};

pub(super) fn map_rate_limit_status(status: types::RateLimitStatus) -> model::RateLimitStatus {
    match status {
        types::RateLimitStatus::Allowed => model::RateLimitStatus::Allowed,
        types::RateLimitStatus::AllowedWarning => model::RateLimitStatus::AllowedWarning,
        types::RateLimitStatus::Rejected => model::RateLimitStatus::Rejected,
    }
}

pub(super) fn map_rate_limit_update(update: types::RateLimitUpdate) -> model::RateLimitUpdate {
    model::RateLimitUpdate {
        status: map_rate_limit_status(update.status),
        resets_at: update.resets_at,
        utilization: update.utilization,
        rate_limit_type: update.rate_limit_type,
        overage_status: update.overage_status.map(map_rate_limit_status),
        overage_resets_at: update.overage_resets_at,
        overage_disabled_reason: update.overage_disabled_reason,
        is_using_overage: update.is_using_overage,
        surpassed_threshold: update.surpassed_threshold,
    }
}

pub(super) fn map_api_retry_error(error: types::ApiRetryError) -> model::ApiRetryError {
    match error {
        types::ApiRetryError::AuthenticationFailed => model::ApiRetryError::AuthenticationFailed,
        types::ApiRetryError::OauthOrgNotAllowed => model::ApiRetryError::OauthOrgNotAllowed,
        types::ApiRetryError::BillingError => model::ApiRetryError::BillingError,
        types::ApiRetryError::RateLimit => model::ApiRetryError::RateLimit,
        types::ApiRetryError::Overloaded => model::ApiRetryError::Overloaded,
        types::ApiRetryError::InvalidRequest => model::ApiRetryError::InvalidRequest,
        types::ApiRetryError::ModelNotFound => model::ApiRetryError::ModelNotFound,
        types::ApiRetryError::ServerError => model::ApiRetryError::ServerError,
        types::ApiRetryError::MaxOutputTokens => model::ApiRetryError::MaxOutputTokens,
        types::ApiRetryError::Unknown => model::ApiRetryError::Unknown,
    }
}

pub(super) fn map_mcp_server_connection_status(
    status: types::McpServerConnectionStatus,
) -> model::McpServerConnectionStatus {
    match status {
        types::McpServerConnectionStatus::Connected => model::McpServerConnectionStatus::Connected,
        types::McpServerConnectionStatus::Failed => model::McpServerConnectionStatus::Failed,
        types::McpServerConnectionStatus::NeedsAuth => model::McpServerConnectionStatus::NeedsAuth,
        types::McpServerConnectionStatus::Pending => model::McpServerConnectionStatus::Pending,
        types::McpServerConnectionStatus::Disabled => model::McpServerConnectionStatus::Disabled,
    }
}

fn map_mcp_tool_permission_policy(
    policy: types::McpServerToolPermissionPolicy,
) -> model::McpServerToolPermissionPolicy {
    match policy {
        types::McpServerToolPermissionPolicy::Allow => model::McpServerToolPermissionPolicy::Allow,
        types::McpServerToolPermissionPolicy::Ask => model::McpServerToolPermissionPolicy::Ask,
        types::McpServerToolPermissionPolicy::Deny => model::McpServerToolPermissionPolicy::Deny,
    }
}

fn map_mcp_server_org_max_permission(
    permission: types::McpServerOrgMaxPermission,
) -> model::McpServerOrgMaxPermission {
    match permission {
        types::McpServerOrgMaxPermission::Allow => model::McpServerOrgMaxPermission::Allow,
        types::McpServerOrgMaxPermission::Ask => model::McpServerOrgMaxPermission::Ask,
        types::McpServerOrgMaxPermission::Blocked => model::McpServerOrgMaxPermission::Blocked,
    }
}

fn map_mcp_tool_policy(policy: types::McpServerToolPolicy) -> model::McpServerToolPolicy {
    model::McpServerToolPolicy {
        name: policy.name,
        permission_policy: policy.permission_policy.map(map_mcp_tool_permission_policy),
        org_max_permission: policy.org_max_permission.map(map_mcp_server_org_max_permission),
    }
}

fn map_mcp_status_config(config: types::McpServerStatusConfig) -> model::McpServerStatusConfig {
    match config {
        types::McpServerStatusConfig::Stdio { command, args, env, timeout, always_load } => {
            model::McpServerStatusConfig::Stdio { command, args, env, timeout, always_load }
        }
        types::McpServerStatusConfig::Sse { url, headers, tools, timeout, always_load } => {
            model::McpServerStatusConfig::Sse {
                url,
                headers,
                tools: tools.into_iter().map(map_mcp_tool_policy).collect(),
                timeout,
                always_load,
            }
        }
        types::McpServerStatusConfig::Http { url, headers, tools, timeout, always_load } => {
            model::McpServerStatusConfig::Http {
                url,
                headers,
                tools: tools.into_iter().map(map_mcp_tool_policy).collect(),
                timeout,
                always_load,
            }
        }
        types::McpServerStatusConfig::Sdk { name } => model::McpServerStatusConfig::Sdk { name },
        types::McpServerStatusConfig::ClaudeaiProxy { url, id, timeout } => {
            model::McpServerStatusConfig::ClaudeaiProxy { url, id, timeout }
        }
        types::McpServerStatusConfig::Unknown { raw_type } => {
            model::McpServerStatusConfig::Unknown { raw_type }
        }
    }
}

pub(super) fn map_mcp_server_status(status: types::McpServerStatus) -> model::McpServerStatus {
    model::McpServerStatus {
        name: status.name,
        status: map_mcp_server_connection_status(status.status),
        server_info: status
            .server_info
            .map(|info| model::McpServerInfo { name: info.name, version: info.version }),
        error: status.error,
        config: status.config.map(map_mcp_status_config),
        scope: status.scope,
        tools: status
            .tools
            .into_iter()
            .map(|tool| model::McpTool {
                name: tool.name,
                description: tool.description,
                annotations: tool.annotations.map(|annotations| model::McpToolAnnotations {
                    read_only: annotations.read_only,
                    destructive: annotations.destructive,
                    open_world: annotations.open_world,
                }),
            })
            .collect(),
    }
}

fn map_system_notice_severity(
    severity: types::SystemNoticeSeverity,
) -> model::SystemNoticeSeverity {
    match severity {
        types::SystemNoticeSeverity::Info => model::SystemNoticeSeverity::Info,
        types::SystemNoticeSeverity::Warning => model::SystemNoticeSeverity::Warning,
        types::SystemNoticeSeverity::Error => model::SystemNoticeSeverity::Error,
    }
}

fn map_effort_level(level: types::EffortLevel) -> model::EffortLevel {
    match level {
        types::EffortLevel::Low => model::EffortLevel::Low,
        types::EffortLevel::Medium => model::EffortLevel::Medium,
        types::EffortLevel::High => model::EffortLevel::High,
        types::EffortLevel::XHigh => model::EffortLevel::XHigh,
        types::EffortLevel::Max => model::EffortLevel::Max,
    }
}

pub(super) fn map_available_commands_update(
    commands: Vec<types::AvailableCommand>,
    source: Option<String>,
    generation: Option<u64>,
) -> model::AvailableCommandsUpdate {
    let mut update = model::AvailableCommandsUpdate::new(
        commands
            .into_iter()
            .map(|cmd| {
                let mut mapped = model::AvailableCommand::new(cmd.name, cmd.description);
                if let Some(input_hint) = cmd.input_hint
                    && !input_hint.trim().is_empty()
                {
                    mapped = mapped.input_hint(input_hint);
                }
                mapped
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
            .map(|agent| {
                let mut mapped = model::AvailableAgent::new(agent.name, agent.description);
                if let Some(model_name) = agent.model
                    && !model_name.trim().is_empty()
                {
                    mapped = mapped.model(model_name);
                }
                mapped
            })
            .collect(),
    )
}

pub(super) fn map_available_models(
    models: Vec<types::AvailableModel>,
) -> Vec<model::AvailableModel> {
    models
        .into_iter()
        .map(|model_info| {
            let mut mapped = model::AvailableModel::new(model_info.id, model_info.display_name);
            if let Some(description) = model_info.description
                && !description.trim().is_empty()
            {
                mapped = mapped.description(description);
            }
            mapped = mapped.supports_effort(model_info.supports_effort);
            mapped = mapped.supports_adaptive_thinking(model_info.supports_adaptive_thinking);
            mapped = mapped.supports_fast_mode(model_info.supports_fast_mode);
            mapped = mapped.supports_auto_mode(model_info.supports_auto_mode);
            if !model_info.supported_effort_levels.is_empty() {
                mapped = mapped.supported_effort_levels(
                    model_info.supported_effort_levels.into_iter().map(map_effort_level).collect(),
                );
            }
            mapped
        })
        .collect()
}

pub(super) fn convert_current_model(current_model: types::CurrentModel) -> model::CurrentModel {
    let mut mapped = model::CurrentModel::new(
        current_model.resolved_id,
        current_model.display_name_short,
        current_model.display_name_long,
    )
    .supports_effort(current_model.supports_effort)
    .supported_effort_levels(
        current_model.supported_effort_levels.into_iter().map(map_effort_level).collect(),
    )
    .supports_fast_mode(current_model.supports_fast_mode)
    .supports_auto_mode(current_model.supports_auto_mode)
    .supports_adaptive_thinking(current_model.supports_adaptive_thinking)
    .authoritative(current_model.is_authoritative);
    if let Some(requested_id) = current_model.requested_id {
        mapped = mapped.requested_id(requested_id);
    }
    if let Some(catalog_id) = current_model.catalog_id {
        mapped = mapped.catalog_id(catalog_id);
    }
    mapped
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
        types::SessionUpdate::UserMessageChunk { content, source_message_uuid } => {
            let content = convert_content_block(content)?;
            Some(model::SessionUpdate::UserMessageChunk(
                model::ContentChunk::new(content).source_message_uuid(source_message_uuid),
            ))
        }
        types::SessionUpdate::AgentMessageChunk { content, source_message_uuid } => {
            let content = convert_content_block(content)?;
            Some(model::SessionUpdate::AgentMessageChunk(
                model::ContentChunk::new(content).source_message_uuid(source_message_uuid),
            ))
        }
        types::SessionUpdate::AgentThoughtChunk { content, source_message_uuid } => {
            let content = convert_content_block(content)?;
            Some(model::SessionUpdate::AgentThoughtChunk(
                model::ContentChunk::new(content).source_message_uuid(source_message_uuid),
            ))
        }
        types::SessionUpdate::ToolCall { tool_call } => {
            Some(model::SessionUpdate::ToolCall(convert_tool_call(tool_call)))
        }
        types::SessionUpdate::ToolCallUpdate { tool_call_update } => {
            Some(model::SessionUpdate::ToolCallUpdate(convert_tool_call_update(tool_call_update)))
        }
        types::SessionUpdate::TranscriptRetraction { retraction } => Some(
            model::SessionUpdate::TranscriptRetraction(convert_transcript_retraction(retraction)),
        ),
        types::SessionUpdate::TaskStateUpdate { update: task_update } => {
            Some(model::SessionUpdate::TaskStateUpdate(convert_task_state_update(task_update)))
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
        types::SessionUpdate::CurrentModeUpdate { current_mode_id } => {
            Some(model::SessionUpdate::CurrentModeUpdate(model::CurrentModeUpdate::new(
                model::SessionModeId::new(current_mode_id),
            )))
        }
        types::SessionUpdate::CurrentModelUpdate { current_model } => {
            Some(model::SessionUpdate::CurrentModelUpdate(model::CurrentModelUpdate::new(
                convert_current_model(current_model),
            )))
        }
        types::SessionUpdate::ConfigOptionUpdate { option_id, value } => {
            Some(model::SessionUpdate::ConfigOptionUpdate(model::ConfigOptionUpdate {
                option_id,
                value,
            }))
        }
        types::SessionUpdate::FastModeUpdate { fast_mode_state } => {
            Some(model::SessionUpdate::FastModeUpdate(match fast_mode_state {
                types::FastModeState::Off => model::FastModeState::Off,
                types::FastModeState::Cooldown => model::FastModeState::Cooldown,
                types::FastModeState::On => model::FastModeState::On,
            }))
        }
        types::SessionUpdate::RateLimitUpdate {
            status,
            resets_at,
            utilization,
            rate_limit_type,
            overage_status,
            overage_resets_at,
            overage_disabled_reason,
            is_using_overage,
            surpassed_threshold,
        } => Some(model::SessionUpdate::RateLimitUpdate(map_rate_limit_update(
            types::RateLimitUpdate {
                status,
                resets_at,
                utilization,
                rate_limit_type,
                overage_status,
                overage_resets_at,
                overage_disabled_reason,
                is_using_overage,
                surpassed_threshold,
            },
        ))),
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
            error: map_api_retry_error(error),
        }),
        types::SessionUpdate::PromptSuggestionUpdate { suggestion } => {
            Some(model::SessionUpdate::PromptSuggestionUpdate(suggestion))
        }
        types::SessionUpdate::RuntimeSessionStateUpdate { state } => {
            Some(model::SessionUpdate::RuntimeSessionStateUpdate(match state {
                types::RuntimeSessionState::Idle => model::RuntimeSessionState::Idle,
                types::RuntimeSessionState::Running => model::RuntimeSessionState::Running,
                types::RuntimeSessionState::RequiresAction => {
                    model::RuntimeSessionState::RequiresAction
                }
            }))
        }
        types::SessionUpdate::SettingsParseError { file, path, message } => {
            Some(model::SessionUpdate::SettingsParseError { file, path, message })
        }
        types::SessionUpdate::SessionStatusUpdate { status } => {
            Some(model::SessionUpdate::SessionStatusUpdate(match status {
                types::SessionStatus::Compacting => model::SessionStatus::Compacting,
                types::SessionStatus::Requesting => model::SessionStatus::Requesting,
                types::SessionStatus::Idle => model::SessionStatus::Idle,
            }))
        }
        types::SessionUpdate::SystemNoticeUpdate { severity, message } => {
            Some(model::SessionUpdate::SystemNoticeUpdate {
                severity: map_system_notice_severity(severity),
                message,
            })
        }
        types::SessionUpdate::CompactionBoundary { trigger, pre_tokens } => {
            Some(model::SessionUpdate::CompactionBoundary(model::CompactionBoundary {
                trigger: match trigger {
                    types::CompactionTrigger::Manual => model::CompactionTrigger::Manual,
                    types::CompactionTrigger::Auto => model::CompactionTrigger::Auto,
                },
                pre_tokens,
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
        ),
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

    let prompt = model::QuestionPrompt::new(
        request.prompt.question,
        request.prompt.header,
        request.prompt.multi_select,
        request
            .prompt
            .options
            .into_iter()
            .map(|option| {
                model::QuestionOption::new(option.option_id, option.label)
                    .description(option.description)
                    .preview(option.preview)
            })
            .collect(),
    );

    (
        model::RequestQuestionRequest::new(
            model::SessionId::new(session_id),
            tool_call_update,
            prompt,
            usize::try_from(request.question_index).unwrap_or(0),
            usize::try_from(request.total_questions).unwrap_or(0),
        ),
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
    let payload = model::RefusalFallbackPayload {
        original_model: request.payload.original_model,
        fallback_model: request.payload.fallback_model,
        api_refusal_category: request.payload.api_refusal_category,
        guidance_text: request.payload.guidance_text,
        retracted_message_uuids: request.payload.retracted_message_uuids,
    };
    let options = request
        .options
        .into_iter()
        .map(|option| model::UserDialogOption::new(option.option_id, option.label))
        .collect();
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
        tc = tc.output_metadata(convert_tool_output_metadata(output_metadata));
    }
    if let Some(task_metadata) = task_metadata {
        tc = tc.task_metadata(convert_task_metadata(task_metadata));
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
        fields = fields.output_metadata(convert_tool_output_metadata(output_metadata));
    }
    if let Some(task_metadata) = tool_call.task_metadata {
        fields = fields.task_metadata(convert_task_metadata(task_metadata));
    }

    fields
}

fn convert_transcript_retraction(
    retraction: types::TranscriptRetraction,
) -> model::TranscriptRetraction {
    model::TranscriptRetraction {
        message_uuids: retraction.message_uuids,
        reason: match retraction.reason {
            types::TranscriptRetractionReason::ModelRefusalFallback => {
                model::TranscriptRetractionReason::ModelRefusalFallback
            }
            types::TranscriptRetractionReason::ModelFallback => {
                model::TranscriptRetractionReason::ModelFallback
            }
            types::TranscriptRetractionReason::AssistantSupersedes => {
                model::TranscriptRetractionReason::AssistantSupersedes
            }
        },
        request_id: retraction.request_id,
        trigger: retraction.trigger,
        direction: retraction.direction,
        original_model: retraction.original_model,
        fallback_model: retraction.fallback_model,
        api_refusal_category: retraction.api_refusal_category,
        api_refusal_explanation: retraction.api_refusal_explanation,
        content: retraction.content,
    }
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
        out = out.output_metadata(convert_tool_output_metadata(output_metadata));
    }
    if let Some(task_metadata) = fields.task_metadata {
        out = out.task_metadata(convert_task_metadata(task_metadata));
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

fn convert_tool_output_metadata(
    output_metadata: types::ToolOutputMetadata,
) -> model::ToolOutputMetadata {
    model::ToolOutputMetadata::new().bash(output_metadata.bash.map(|bash| {
        model::BashOutputMetadata::new()
            .assistant_auto_backgrounded(bash.assistant_auto_backgrounded)
    }))
}

fn convert_permission_display(
    display: Option<types::PermissionDisplay>,
) -> Option<model::PermissionDisplay> {
    let display = display?;
    let mapped = model::PermissionDisplay::new()
        .title(display.title)
        .display_name(display.display_name)
        .description(display.description);
    (!mapped.is_empty()).then_some(mapped)
}

fn convert_task_metadata(task_metadata: types::TaskMetadata) -> model::TaskMetadata {
    model::TaskMetadata::new()
        .end_time(task_metadata.end_time)
        .total_paused_ms(task_metadata.total_paused_ms)
        .error(task_metadata.error)
        .backgrounded(task_metadata.is_backgrounded)
        .request_id(task_metadata.request_id)
        .subagent_type(task_metadata.subagent_type)
        .task_description(task_metadata.task_description)
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
        "completed" => model::ToolCallStatus::Completed,
        "failed" => model::ToolCallStatus::Failed,
        "killed" => model::ToolCallStatus::Killed,
        _ => model::ToolCallStatus::Pending,
    }
}

fn convert_task_status(status: types::TaskStatus) -> model::TaskStatus {
    match status {
        types::TaskStatus::Pending => model::TaskStatus::Pending,
        types::TaskStatus::InProgress => model::TaskStatus::InProgress,
        types::TaskStatus::Completed => model::TaskStatus::Completed,
    }
}

fn convert_task_update_source(source: types::TaskUpdateSource) -> model::TaskUpdateSource {
    match source {
        types::TaskUpdateSource::Create => model::TaskUpdateSource::Create,
        types::TaskUpdateSource::Update => model::TaskUpdateSource::Update,
        types::TaskUpdateSource::Get => model::TaskUpdateSource::Get,
        types::TaskUpdateSource::List => model::TaskUpdateSource::List,
        types::TaskUpdateSource::Lifecycle => model::TaskUpdateSource::Lifecycle,
    }
}

fn convert_task_item(task: types::TaskItem) -> model::TaskItem {
    model::TaskItem {
        task_id: task.task_id,
        subject: task.subject,
        description: task.description,
        active_form: task.active_form,
        status: convert_task_status(task.status),
        owner: task.owner,
        blocks: task.blocks,
        blocked_by: task.blocked_by,
        metadata: task.metadata,
        source_tool_call_id: task.source_tool_call_id,
    }
}

fn convert_task_state_update(update: types::TaskStateUpdate) -> model::TaskStateUpdate {
    model::TaskStateUpdate {
        source: convert_task_update_source(update.source),
        tasks: update.tasks.into_iter().map(convert_task_item).collect(),
        removed_task_ids: update.removed_task_ids,
        is_complete_snapshot: update.is_complete_snapshot,
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
        convert_account_info, convert_current_model, convert_tool_call, convert_tool_call_update,
        convert_tool_call_update_fields, map_available_commands_update, map_available_models,
        map_permission_request, map_question_request, map_session_update,
    };
    use crate::agent::{model, types};

    #[test]
    fn map_available_models_preserves_optional_fast_and_auto_metadata() {
        let mapped = map_available_models(vec![
            types::AvailableModel {
                id: "sonnet".to_owned(),
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
    fn convert_current_model_preserves_new_effort_levels() {
        let mapped = convert_current_model(types::CurrentModel {
            requested_id: Some("sonnet".to_owned()),
            resolved_id: "claude-sonnet".to_owned(),
            display_name_short: "Sonnet".to_owned(),
            display_name_long: "Claude Sonnet".to_owned(),
            catalog_id: Some("claude-sonnet".to_owned()),
            supports_effort: true,
            supported_effort_levels: vec![
                types::EffortLevel::Low,
                types::EffortLevel::XHigh,
                types::EffortLevel::Max,
            ],
            supports_fast_mode: Some(true),
            supports_auto_mode: Some(false),
            supports_adaptive_thinking: Some(true),
            is_authoritative: true,
        });

        assert_eq!(
            mapped.supported_effort_levels,
            vec![model::EffortLevel::Low, model::EffortLevel::XHigh, model::EffortLevel::Max,]
        );
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
    fn map_permission_request_preserves_display_metadata() {
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
    }

    #[test]
    fn map_question_request_preserves_preview_and_annotation_shape() {
        let (request, tool_call_id) = map_question_request(
            "session-1",
            types::QuestionRequest {
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
        );
    }

    #[test]
    fn convert_tool_call_update_fields_preserves_output_metadata() {
        let fields = convert_tool_call_update_fields(types::ToolCallUpdateFields {
            status: Some("completed".to_owned()),
            output_metadata: Some(types::ToolOutputMetadata {
                bash: Some(types::BashOutputMetadata { assistant_auto_backgrounded: Some(true) }),
            }),
            ..types::ToolCallUpdateFields::default()
        });

        assert_eq!(
            fields.output_metadata,
            Some(model::ToolOutputMetadata::new().bash(Some(
                model::BashOutputMetadata::new().assistant_auto_backgrounded(Some(true)),
            )))
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
            }],
            Some("commands_changed".to_owned()),
            Some(3),
        );

        assert_eq!(
            update,
            model::AvailableCommandsUpdate::new(vec![
                model::AvailableCommand::new("project-command", "Project command")
                    .input_hint("<value>")
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
                request_id: Some("request-1".to_owned()),
                subagent_type: Some("tester".to_owned()),
                task_description: Some("Validate changes".to_owned()),
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
                    .request_id(Some("request-1".to_owned()))
                    .subagent_type(Some("tester".to_owned()))
                    .task_description(Some("Validate changes".to_owned())),
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
                request_id: Some("request-2".to_owned()),
                subagent_type: Some("reviewer".to_owned()),
                task_description: Some("Review changes".to_owned()),
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
                    .request_id(Some("request-2".to_owned()))
                    .subagent_type(Some("reviewer".to_owned()))
                    .task_description(Some("Review changes".to_owned())),
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
    fn map_mcp_server_status_converts_latest_config_fields() {
        let status = types::McpServerStatus {
            name: "notion".to_owned(),
            status: types::McpServerConnectionStatus::Connected,
            server_info: None,
            error: None,
            config: Some(types::McpServerStatusConfig::Http {
                url: "https://mcp.notion.com/mcp".to_owned(),
                headers: std::collections::BTreeMap::new(),
                tools: vec![
                    types::McpServerToolPolicy {
                        name: "search".to_owned(),
                        permission_policy: Some(types::McpServerToolPermissionPolicy::Deny),
                        org_max_permission: Some(types::McpServerOrgMaxPermission::Blocked),
                    },
                    types::McpServerToolPolicy {
                        name: "lookup".to_owned(),
                        permission_policy: None,
                        org_max_permission: Some(types::McpServerOrgMaxPermission::Ask),
                    },
                ],
                timeout: Some(5000),
                always_load: Some(true),
            }),
            scope: Some("project".to_owned()),
            tools: Vec::new(),
        };

        let mapped = super::map_mcp_server_status(status);

        let Some(model::McpServerStatusConfig::Http { tools, timeout, always_load, .. }) =
            mapped.config
        else {
            panic!("expected http MCP config");
        };
        assert_eq!(timeout, Some(5000));
        assert_eq!(always_load, Some(true));
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].permission_policy, Some(model::McpServerToolPermissionPolicy::Deny));
        assert_eq!(tools[0].org_max_permission, Some(model::McpServerOrgMaxPermission::Blocked));
        assert_eq!(tools[1].permission_policy, None);
        assert_eq!(tools[1].org_max_permission, Some(model::McpServerOrgMaxPermission::Ask));
    }
}
