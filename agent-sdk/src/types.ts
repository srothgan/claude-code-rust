export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

export interface PromptChunk {
  kind: string;
  value: Json;
}

export interface ModeInfo {
  id: string;
  name: string;
  description?: string;
}

export interface ModeState {
  current_mode_id: string;
  current_mode_name: string;
  available_modes: ModeInfo[];
}

export interface AvailableCommand {
  name: string;
  description: string;
  input_hint?: string;
}

export type AvailableCommandsSource =
  | "session_result_commands"
  | "init_slash_commands"
  | "supportedCommands"
  | "commands_changed"
  | "reload_plugins";

export interface AvailableAgent {
  name: string;
  description: string;
  model?: string;
}

export type EffortLevel = "low" | "medium" | "high" | "xhigh" | "max";

export interface AvailableModel {
  id: string;
  resolved_model?: string;
  display_name: string;
  description?: string;
  supports_effort: boolean;
  supported_effort_levels: EffortLevel[];
  supports_adaptive_thinking?: boolean;
  supports_fast_mode?: boolean;
  supports_auto_mode?: boolean;
}

export interface CurrentModel {
  requested_id?: string;
  resolved_id: string;
  display_name_short: string;
  display_name_long: string;
  catalog_id?: string;
  supports_effort: boolean;
  supported_effort_levels: EffortLevel[];
  supports_fast_mode?: boolean;
  supports_auto_mode?: boolean;
  supports_adaptive_thinking?: boolean;
  is_authoritative: boolean;
}

export type FastModeState = "off" | "cooldown" | "on";
export interface FastModeSnapshot {
  state: FastModeState;
  disabled_reason?: string;
}
export type RateLimitStatus = "allowed" | "allowed_warning" | "rejected";
export type TerminalReason =
  | "blocking_limit"
  | "rapid_refill_breaker"
  | "prompt_too_long"
  | "image_error"
  | "model_error"
  | "api_error"
  | "malformed_tool_use_exhausted"
  | "aborted_streaming"
  | "aborted_tools"
  | "stop_hook_prevented"
  | "hook_stopped"
  | "tool_deferred"
  | "max_turns"
  | "background_requested"
  | "budget_exhausted"
  | "structured_output_retry_exhausted"
  | "tool_deferred_unavailable"
  | "turn_setup_failed"
  | "completed";

export interface RateLimitUpdate {
  status: RateLimitStatus;
  error_code?: "credits_required";
  resets_at?: number;
  utilization?: number;
  rate_limit_type?: string;
  overage_status?: RateLimitStatus;
  overage_resets_at?: number;
  overage_disabled_reason?: string;
  is_using_overage?: boolean;
  surpassed_threshold?: number;
  can_user_purchase_credits?: boolean;
  has_chargeable_saved_payment_method?: boolean;
}

export type ApiRetryError =
  | "authentication_failed"
  | "oauth_org_not_allowed"
  | "billing_error"
  | "rate_limit"
  | "overloaded"
  | "invalid_request"
  | "model_not_found"
  | "server_error"
  | "unknown"
  | "max_output_tokens";

export type RuntimeSessionState = "idle" | "running" | "requires_action";
export type SystemNoticeSeverity = "info" | "warning" | "error";

export interface SettingsParseErrorUpdate {
  file?: string;
  path: string;
  message: string;
}

export type ContentBlock =
  | { type: "text"; text: string }
  | { type: "image"; mime_type?: string; uri?: string; data?: string };

export type ToolCallContent =
  | { type: "content"; content: ContentBlock }
  | {
      type: "diff";
      old_path: string;
      new_path: string;
      old: string;
      new: string;
      repository?: string;
    }
  | {
      type: "mcp_resource";
      uri: string;
      mime_type?: string;
      text?: string;
      blob_saved_to?: string;
    };

export interface BashOutputMetadata {
  assistant_auto_backgrounded?: boolean;
  timed_out_after_ms?: number;
  background_cwd_hint?: string;
}

export interface AgentOutputMetadata {
  resolved_model?: string;
  models_used?: string[];
}

export interface WebFetchArtifactReadMetadata {
  slug: string;
  ver: string;
}

export interface WebFetchOutputMetadata {
  artifact_read?: WebFetchArtifactReadMetadata;
}

export interface SkillOutputMetadata {
  background?: boolean;
}

export interface ToolNonExecutionMetadata {
  kind: string;
  user_feedback?: string;
}

export interface ToolOutputMetadata {
  bash?: BashOutputMetadata;
  agent?: AgentOutputMetadata;
  web_fetch?: WebFetchOutputMetadata;
  skill?: SkillOutputMetadata;
  non_execution?: ToolNonExecutionMetadata;
}

export type SubagentRetryUpdate =
  | {
      state: "waiting";
      agent_id?: string;
      attempt: number;
      max_retries: number;
      retry_delay_ms: number;
      error_status?: number;
      error_category?: string;
    }
  | { state: "clear" };

export interface TaskMetadata {
  end_time?: number;
  total_paused_ms?: number;
  error?: string;
  is_backgrounded?: boolean;
  request_id?: string;
  subagent_type?: string;
  task_description?: string;
  task_type?: string;
  workflow_name?: string;
  prompt?: string;
  output_file?: string;
  summary?: string;
  terminal_status?: string;
  blocked?: boolean;
  parent_agent_id?: string;
  subagent_retry?: SubagentRetryUpdate;
}

export interface ToolLocation {
  path: string;
  line?: number;
}

export interface ToolCall {
  tool_call_id: string;
  title: string;
  kind: string;
  status: string;
  source_message_uuid?: string;
  content: ToolCallContent[];
  raw_input?: Json;
  raw_output?: string;
  output_metadata?: ToolOutputMetadata;
  task_metadata?: TaskMetadata;
  locations: ToolLocation[];
  meta?: Json;
}

export interface ToolCallUpdateFields {
  title?: string;
  kind?: string;
  status?: string;
  content?: ToolCallContent[];
  raw_input?: Json;
  raw_output?: string;
  output_metadata?: ToolOutputMetadata;
  task_metadata?: TaskMetadata;
  locations?: ToolLocation[];
  meta?: Json;
}

export interface ToolCallUpdate {
  tool_call_id: string;
  source_message_uuid?: string;
  fields: ToolCallUpdateFields;
}

export type TranscriptRetractionReason =
  | "model_refusal_fallback"
  | "model_fallback"
  | "assistant_supersedes";

export interface TranscriptRetraction {
  message_uuids: string[];
  reason: TranscriptRetractionReason;
  request_id?: string;
  trigger?: string;
  direction?: string;
  original_model?: string;
  fallback_model?: string;
  api_refusal_category?: string;
  api_refusal_explanation?: string;
  content?: string;
}

export type TaskStatus = "pending" | "in_progress" | "completed";
export type TaskUpdateSource =
  | "task_create"
  | "task_update"
  | "task_get"
  | "task_list"
  | "task_lifecycle"
  | "background_tasks";

export interface TaskItem {
  task_id: string;
  subject: string;
  description?: string;
  active_form?: string;
  status: TaskStatus;
  owner?: string;
  blocks: string[];
  blocked_by: string[];
  metadata?: Json;
  source_tool_call_id?: string;
}

export interface TaskStateUpdate {
  source: TaskUpdateSource;
  tasks: TaskItem[];
  removed_task_ids: string[];
  is_complete_snapshot: boolean;
}

export type SessionUpdate =
  | { type: "agent_message_chunk"; content: ContentBlock; source_message_uuid?: string }
  | { type: "user_message_chunk"; content: ContentBlock; source_message_uuid?: string }
  | { type: "agent_thought_chunk"; content: ContentBlock; source_message_uuid?: string }
  | { type: "tool_call"; tool_call: ToolCall }
  | { type: "tool_call_update"; tool_call_update: ToolCallUpdate }
  | ({ type: "transcript_retraction" } & TranscriptRetraction)
  | ({ type: "task_state_update" } & TaskStateUpdate)
  | {
      type: "available_commands_update";
      commands: AvailableCommand[];
      source?: AvailableCommandsSource;
      generation?: number;
    }
  | { type: "available_agents_update"; agents: AvailableAgent[] }
  | { type: "mode_state_update"; mode: ModeState }
  | { type: "current_mode_update"; current_mode_id: string }
  | { type: "current_model_update"; current_model: CurrentModel }
  | { type: "config_option_update"; option_id: string; value: Json }
  | {
      type: "fast_mode_update";
      fast_mode_state: FastModeState;
      fast_mode_disabled_reason?: string;
    }
  | ({ type: "rate_limit_update" } & RateLimitUpdate)
  | {
      type: "api_retry_update";
      attempt: number;
      max_retries: number;
      retry_delay_ms: number;
      error_status: number | null;
      error: ApiRetryError;
    }
  | { type: "prompt_suggestion_update"; suggestion: string }
  | { type: "runtime_session_state_update"; state: RuntimeSessionState }
  | ({ type: "settings_parse_error" } & SettingsParseErrorUpdate)
  | { type: "session_status_update"; status: "requesting" | "idle" }
  | { type: "system_notice_update"; severity: SystemNoticeSeverity; message: string }
  | { type: "compaction_update"; phase: "started" }
  | {
      type: "compaction_update";
      phase: "boundary";
      trigger: "manual" | "auto";
      pre_tokens: number;
      post_tokens?: number;
      duration_ms?: number;
    }
  | {
      type: "compaction_update";
      phase: "finished";
      result: "success";
    }
  | {
      type: "compaction_update";
      phase: "finished";
      result: "failed";
      error_code: "too_few_groups" | "unknown";
      error?: string;
    };

export interface PermissionOption {
  option_id: string;
  name: string;
  description?: string;
  kind: string;
}

export interface PermissionRequest {
  tool_call: ToolCall;
  options: PermissionOption[];
  display?: PermissionDisplay;
}

export interface PermissionDisplay {
  title?: string;
  display_name?: string;
  description?: string;
}

export type ElicitationMode = "form" | "url";

export type ElicitationAction = "accept" | "decline" | "cancel";

export interface QuestionOption {
  option_id: string;
  label: string;
  description?: string;
  preview?: string;
}

export interface QuestionPrompt {
  question: string;
  header: string;
  multi_select: boolean;
  options: QuestionOption[];
}

export interface QuestionRequest {
  tool_call: ToolCall;
  prompt: QuestionPrompt;
  question_index: number;
  total_questions: number;
}

export interface QuestionAnnotation {
  preview?: string;
  notes?: string;
}

export interface ElicitationRequest {
  request_id: string;
  server_name: string;
  message: string;
  mode: ElicitationMode;
  url?: string;
  elicitation_id?: string;
  requested_schema?: Record<string, Json>;
}

export interface ElicitationComplete {
  elicitation_id: string;
  server_name?: string;
}

export interface McpAuthRedirect {
  server_name: string;
  auth_url: string;
  requires_user_action: boolean;
}

export interface McpAuthCapabilities {
  authenticate: boolean;
  clear_auth: boolean;
  submit_oauth_callback_url: boolean;
}

export interface McpOperationError {
  server_name?: string;
  operation: string;
  message: string;
}

export type PermissionOutcome =
  | { outcome: "selected"; option_id: string }
  | { outcome: "cancelled" };

export type QuestionOutcome =
  | {
      outcome: "answered";
      selected_option_ids: string[];
      annotation?: QuestionAnnotation;
    }
  | { outcome: "cancelled" };

/**
 * The host-selectable choices of a `refusal_fallback_prompt` dialog. `cancelled`
 * is the CLI default and is delivered via the Esc/decline path, not as a listed
 * option, so it is excluded from this union (see {@link UserDialogOutcome}).
 */
export type RefusalFallbackPromptChoice = "retry_fallback" | "edit_prompt";

/**
 * Snake-case host wire shape of the `refusal_fallback_prompt` payload. The CLI
 * builds it with camelCase keys (`originalModel`, …); the bridge normalizes the
 * casing before emitting.
 */
export interface RefusalFallbackPromptPayload {
  original_model: string;
  fallback_model: string;
  api_refusal_category?: string;
  guidance_text?: string;
  retracted_message_uuids?: string[];
}

export interface UserDialogOption {
  option_id: RefusalFallbackPromptChoice;
  label: string;
}

/**
 * Host-facing shape of a `request_user_dialog` control request. The bridge owns
 * `request_id` (generated per callback) and the rendered option labels.
 */
export interface UserDialogRequestPayload {
  request_id: string;
  dialog_kind: "refusal_fallback_prompt";
  payload: RefusalFallbackPromptPayload;
  options: UserDialogOption[];
}

export type UserDialogOutcome =
  | { outcome: "selected"; option_id: RefusalFallbackPromptChoice }
  | { outcome: "cancelled" };

export interface SessionListEntry {
  session_id: string;
  summary: string;
  last_modified_ms: number;
  file_size_bytes: number;
  cwd?: string;
  git_branch?: string;
  custom_title?: string;
  first_prompt?: string;
}

export interface AccountInfo {
  email?: string;
  organization?: string;
  subscription_type?: string;
  token_source?: string;
  api_key_source?: string;
  api_provider?: string;
}

export type McpServerConnectionStatus =
  | "connected"
  | "failed"
  | "needs-auth"
  | "pending"
  | "disabled";

export interface McpServerInfo {
  name: string;
  version: string;
}

export interface McpToolAnnotations {
  read_only?: boolean;
  destructive?: boolean;
  open_world?: boolean;
}

export interface McpTool {
  name: string;
  description?: string;
  annotations?: McpToolAnnotations;
}

export type McpServerToolPermissionPolicy =
  | "always_allow"
  | "always_ask"
  | "always_deny";

export type McpServerOrgMaxPermission = "allow" | "ask" | "blocked";

export interface McpServerToolPolicy {
  name: string;
  permission_policy?: McpServerToolPermissionPolicy;
  org_max_permission?: McpServerOrgMaxPermission;
}

export type McpServerConfig =
  | {
      type: "stdio";
      command: string;
      args?: string[];
      env?: Record<string, string>;
      timeout?: number;
      request_timeout_ms?: number;
      always_load?: boolean;
    }
  | {
      type: "sse";
      url: string;
      headers?: Record<string, string>;
      tools?: McpServerToolPolicy[];
      timeout?: number;
      request_timeout_ms?: number;
      always_load?: boolean;
    }
  | {
      type: "http";
      url: string;
      headers?: Record<string, string>;
      tools?: McpServerToolPolicy[];
      timeout?: number;
      request_timeout_ms?: number;
      always_load?: boolean;
    };

export type McpServerStatusConfig =
  | McpServerConfig
  | {
      type: "sdk";
      name: string;
    }
  | {
      type: "claudeai-proxy";
      url: string;
      id: string;
      timeout?: number;
    }
  | {
      type: "unknown";
      raw_type: string;
    };

export interface McpServerStatus {
  name: string;
  status: McpServerConnectionStatus;
  server_info?: McpServerInfo;
  error?: string;
  config?: McpServerStatusConfig;
  scope?: string;
  tools: McpTool[];
}

export interface McpSetServersResult {
  added: string[];
  removed: string[];
  errors: Record<string, string>;
}

export type McpSnapshotSource =
  | "reload_plugins"
  | "mcp_status"
  | "mcp_set_servers"
  | "init";

export interface SessionLaunchSettings {
  language?: string;
  settings?: { [key: string]: Json };
  agent_progress_summaries?: boolean;
}

export interface RewindTarget {
  uuid: string;
  first_text: string;
  input_text: string;
  index: number;
  previous_assistant_uuid?: string;
}

export type RewindRestoreMode = "both" | "conversation" | "code";
export type RewindResultStatus = "success" | "failure" | "partial_failure";

export interface RewindFilesResult {
  can_rewind: boolean;
  error?: string;
  files_changed: string[];
  insertions?: number;
  deletions?: number;
  skipped_links?: number;
}

export interface BridgeCommandEnvelope {
  request_id?: string;
  command: string;
  [key: string]: unknown;
}

export type BridgeCommand =
  | {
      command: "initialize";
      cwd: string;
      metadata?: Record<string, Json>;
    }
  | {
      command: "create_session";
      cwd: string;
      resume?: string;
      launch_settings: SessionLaunchSettings;
      metadata?: Record<string, Json>;
    }
  | {
      command: "resume_session";
      session_id: string;
      launch_settings: SessionLaunchSettings;
      metadata?: Record<string, Json>;
    }
  | {
      command: "prompt";
      session_id: string;
      chunks: PromptChunk[];
    }
  | {
      command: "cancel_turn";
      session_id: string;
    }
  | {
      command: "set_model";
      session_id: string;
      model: string;
    }
  | {
      command: "set_mode";
      session_id: string;
      mode: string;
    }
  | {
      command: "set_effort";
      session_id: string;
      effort: EffortLevel;
    }
  | {
      command: "set_agent";
      session_id: string;
      agent: string | null;
    }
  | {
      command: "set_fast_mode";
      session_id: string;
      enabled: boolean;
    }
  | {
      command: "generate_session_title";
      session_id: string;
      description: string;
    }
  | {
      command: "rename_session";
      session_id: string;
      title: string;
    }
  | {
      command: "new_session";
      cwd: string;
      launch_settings: SessionLaunchSettings;
    }
  | {
      command: "permission_response";
      session_id: string;
      tool_call_id: string;
      outcome: PermissionOutcome;
    }
  | {
      command: "question_response";
      session_id: string;
      tool_call_id: string;
      outcome: QuestionOutcome;
    }
  | {
      command: "user_dialog_response";
      session_id: string;
      request_id: string;
      outcome: UserDialogOutcome;
    }
  | {
      command: "elicitation_response";
      session_id: string;
      elicitation_request_id: string;
      action: ElicitationAction;
      content?: Record<string, Json>;
    }
  | {
      command: "get_status_snapshot";
      session_id: string;
    }
  | {
      command: "get_context_usage";
      session_id: string;
    }
  | {
      command: "get_rewind_targets";
      session_id: string;
    }
  | {
      command: "rewind";
      session_id: string;
      target_user_message_id: string;
      restore_mode: RewindRestoreMode;
      launch_settings: SessionLaunchSettings;
    }
  | {
      command: "reload_plugins";
      session_id: string;
    }
  | {
      command: "mcp_status";
      session_id: string;
    }
  | {
      command: "mcp_reconnect";
      session_id: string;
      server_name: string;
    }
  | {
      command: "mcp_toggle";
      session_id: string;
      server_name: string;
      enabled: boolean;
    }
  | {
      command: "mcp_set_servers";
      session_id: string;
      servers: Record<string, McpServerConfig>;
    }
  | {
      command: "mcp_authenticate";
      session_id: string;
      server_name: string;
    }
  | {
      command: "mcp_clear_auth";
      session_id: string;
      server_name: string;
    }
  | {
      command: "mcp_oauth_callback_url";
      session_id: string;
      server_name: string;
      callback_url: string;
    }
  | {
      command: "shutdown";
    };

export interface BridgeEventEnvelope {
  request_id?: string;
  event: string;
  [key: string]: unknown;
}

export interface InitializeResult {
  agent_name: string;
  agent_version: string;
  auth_methods: Array<{ id: string; name: string; description: string }>;
  capabilities: {
    prompt_image: boolean;
    prompt_embedded_context: boolean;
    supports_session_listing: boolean;
    supports_resume_session: boolean;
  };
}

export type TurnErrorKind =
  | "plan_limit"
  | "auth_required"
  | "account_access"
  | "model_unavailable"
  | "transient_service"
  | "internal"
  | "other";

export type BridgeEvent =
  | {
      event: "connected";
      session_id: string;
      cwd: string;
      current_model: CurrentModel;
      available_models: AvailableModel[];
      mode: ModeState | null;
      fast_mode_state: FastModeState;
      fast_mode_disabled_reason?: string;
      history_updates?: SessionUpdate[];
    }
  | { event: "auth_required"; method_name: string; method_description: string }
  | { event: "connection_failed"; message: string }
  | { event: "session_update"; session_id: string; update: SessionUpdate }
  | { event: "permission_request"; session_id: string; request: PermissionRequest }
  | { event: "question_request"; session_id: string; request: QuestionRequest }
  | { event: "user_dialog_request"; session_id: string; request: UserDialogRequestPayload }
  | { event: "elicitation_request"; session_id: string; request: ElicitationRequest }
  | { event: "elicitation_complete"; session_id: string; completion: ElicitationComplete }
  | { event: "mcp_auth_redirect"; session_id: string; redirect: McpAuthRedirect }
  | { event: "mcp_operation_error"; session_id: string; error: McpOperationError }
  | { event: "mcp_set_servers_result"; session_id: string; result: McpSetServersResult }
  | { event: "turn_complete"; session_id: string; terminal_reason?: TerminalReason }
  | {
      event: "turn_error";
      session_id: string;
      message: string;
      error_kind?: TurnErrorKind;
      sdk_result_subtype?: string;
      assistant_error?: ApiRetryError;
      api_error_status?: number;
      terminal_reason?: TerminalReason;
    }
  | { event: "slash_error"; session_id: string; message: string }
  | { event: "runtime_reload_completed"; session_id: string }
  | { event: "runtime_reload_failed"; session_id: string; message: string }
  | {
      event: "session_replaced";
      session_id: string;
      cwd: string;
      current_model: CurrentModel;
      available_models: AvailableModel[];
      mode: ModeState | null;
      fast_mode_state: FastModeState;
      fast_mode_disabled_reason?: string;
      history_updates?: SessionUpdate[];
      restored_input?: string;
    }
  | { event: "initialized"; result: InitializeResult }
  | { event: "sessions_listed"; sessions: SessionListEntry[] }
  | { event: "status_snapshot"; session_id: string; account: AccountInfo }
  | {
      event: "context_usage";
      session_id: string;
      percentage?: number;
    }
  | {
      event: "rewind_targets";
      session_id: string;
      targets: RewindTarget[];
    }
  | {
      event: "rewind_result";
      session_id: string;
      restore_mode: RewindRestoreMode;
      status: RewindResultStatus;
      file_result?: RewindFilesResult;
      message?: string;
    }
  | {
      event: "mcp_snapshot";
      session_id: string;
      servers: McpServerStatus[];
      auth_capabilities: McpAuthCapabilities;
      source?: McpSnapshotSource;
      error?: string;
    };
