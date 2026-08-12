import {
  getSessionMessages,
  listSessions,
  renameSession,
} from "@anthropic-ai/claude-agent-sdk";
import type {
  Query,
  SessionMessage,
  SessionMutationOptions,
} from "@anthropic-ai/claude-agent-sdk";
import type {
  BridgeCommand,
  RewindTarget,
  StructuredActivityWindow,
  StructuredExtraUsage,
  StructuredBehaviorAttribution,
  StructuredModelUsageWindow,
  StructuredNamedAttribution,
  StructuredSessionUsage,
  StructuredUsageSnapshot,
  StructuredUsageWindow,
} from "../types.js";
import { asRecordOrNull } from "./shared.js";
import { mapSdkAccountInfo } from "./account_metadata.js";
import {
  currentSessionListOptions,
  emitSessionsList,
  setSessionListingDir,
  slashError,
  writeEvent,
} from "./events.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";
import {
  refreshCurrentModel,
  sessionById,
  type SessionState,
} from "./session_lifecycle.js";

type SessionDataCommand = Extract<
  BridgeCommand,
  {
    command:
      | "generate_session_title"
      | "rename_session"
      | "get_status_snapshot"
      | "get_context_usage"
      | "get_usage"
      | "get_rewind_targets"
      | "rewind";
  }
>;

export type SessionDataCommandDeps = {
  generatePersistedSessionTitle: (
    query: Query,
    description: string,
  ) => Promise<string>;
  buildSessionMutationOptions: (
    cwd?: string,
  ) => SessionMutationOptions | undefined;
  rewindTargetsFromSessionMessages: (
    messages: SessionMessage[],
  ) => RewindTarget[];
  handleRewind: (
    command: Extract<BridgeCommand, { command: "rewind" }>,
    requestId?: string,
  ) => Promise<void>;
};

export async function handleSessionDataCommand(
  command: SessionDataCommand,
  requestId: string | undefined,
  deps: SessionDataCommandDeps,
): Promise<void> {
  switch (command.command) {
    case "generate_session_title":
      await generateTitle(command, requestId, deps);
      return;
    case "rename_session":
      await rename(command, requestId, deps);
      return;
    case "get_status_snapshot":
      await getStatusSnapshot(command, requestId);
      return;
    case "get_context_usage":
      await getContextUsage(command, requestId);
      return;
    case "get_usage":
      await getUsage(command, requestId);
      return;
    case "get_rewind_targets":
      await getRewindTargets(command, requestId, deps);
      return;
    case "rewind":
      await deps.handleRewind(command, requestId);
  }
}

async function generateTitle(
  command: Extract<SessionDataCommand, { command: "generate_session_title" }>,
  requestId: string | undefined,
  deps: SessionDataCommandDeps,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
    return;
  }
  try {
    await deps.generatePersistedSessionTitle(
      session.query,
      command.description,
    );
    setSessionListingDir(session.cwd);
    await emitSessionsList(requestId);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    slashError(
      command.session_id,
      `failed to generate session title: ${message}`,
      requestId,
    );
  }
}

async function rename(
  command: Extract<SessionDataCommand, { command: "rename_session" }>,
  requestId: string | undefined,
  deps: SessionDataCommandDeps,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
    return;
  }
  try {
    await renameSession(
      command.session_id,
      command.title,
      deps.buildSessionMutationOptions(session.cwd),
    );
    setSessionListingDir(session.cwd);
    await emitSessionsList(requestId);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    slashError(
      command.session_id,
      `failed to rename session: ${message}`,
      requestId,
    );
  }
}

async function getStatusSnapshot(
  command: Extract<SessionDataCommand, { command: "get_status_snapshot" }>,
  requestId: string | undefined,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
    return;
  }
  try {
    const account = await session.query.accountInfo();
    bridgeLogger.info({
      target: LOG_TARGETS.APP_AUTH,
      eventName: "status_snapshot_emitted",
      message: "status snapshot emitted",
      outcome: "success",
      ...(requestId ? { requestId } : {}),
      sessionId: session.sessionId,
      fields: {
        has_email:
          typeof account.email === "string" && account.email.trim().length > 0,
        has_organization: account.organization !== undefined,
        subscription_type: account.subscriptionType,
        token_source: account.tokenSource,
        api_key_source: account.apiKeySource,
        api_provider: account.apiProvider,
      },
    });
    writeEvent(
      {
        event: "status_snapshot",
        session_id: session.sessionId,
        account: mapSdkAccountInfo(account),
      },
      requestId,
    );
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    bridgeLogger.warn({
      target: LOG_TARGETS.APP_AUTH,
      eventName: "status_snapshot_failed",
      message: "failed to build status snapshot",
      outcome: "failure",
      ...(requestId ? { requestId } : {}),
      sessionId: session.sessionId,
      fields: { error_message: message },
    });
    throw error;
  }
}

async function getContextUsage(
  command: Extract<SessionDataCommand, { command: "get_context_usage" }>,
  requestId: string | undefined,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
    return;
  }
  try {
    const usage = await session.query.getContextUsage();
    if (typeof usage.model === "string" && usage.model.trim().length > 0) {
      session.resolvedRuntimeModelId = usage.model.trim();
      refreshCurrentModel(session, true);
    }
    const rawPercentage =
      typeof usage.percentage === "number" ? usage.percentage : undefined;
    const normalizedPercentage =
      rawPercentage === undefined || !Number.isFinite(rawPercentage)
        ? undefined
        : Math.max(0, Math.min(100, Math.round(rawPercentage)));
    bridgeLogger.debug({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "context_usage_succeeded",
      message: "session context usage received from SDK",
      outcome: "success",
      ...(requestId ? { requestId } : {}),
      sessionId: session.sessionId,
      fields: {
        raw_percentage: rawPercentage,
        normalized_percentage: normalizedPercentage,
        total_tokens:
          typeof usage.totalTokens === "number" ? usage.totalTokens : undefined,
        max_tokens:
          typeof usage.maxTokens === "number" ? usage.maxTokens : undefined,
        raw_max_tokens:
          typeof usage.rawMaxTokens === "number"
            ? usage.rawMaxTokens
            : undefined,
        model: typeof usage.model === "string" ? usage.model : undefined,
      },
    });
    writeEvent(
      {
        event: "context_usage",
        session_id: session.sessionId,
        ...(normalizedPercentage !== undefined
          ? { percentage: normalizedPercentage }
          : {}),
      },
      requestId,
    );
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    bridgeLogger.warn({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "context_usage_failed",
      message: "failed to get session context usage",
      outcome: "failure",
      ...(requestId ? { requestId } : {}),
      sessionId: session.sessionId,
      fields: { error_message: message },
    });
    writeEvent(
      {
        event: "context_usage",
        session_id: session.sessionId,
      },
      requestId,
    );
  }
}

function finiteNumber(
  record: Record<string, unknown> | null,
  key: string,
): number | undefined {
  const value = record?.[key];
  return typeof value === "number" && Number.isFinite(value)
    ? value
    : undefined;
}

function structuredUsageWindow(
  value: unknown,
): StructuredUsageWindow | undefined {
  const record = asRecordOrNull(value);
  const utilization = finiteNumber(record, "utilization");
  if (utilization === undefined) {
    return undefined;
  }
  const resetsAt =
    typeof record?.resets_at === "string" ? record.resets_at.trim() : "";
  return {
    utilization: Math.max(0, Math.min(100, utilization)),
    ...(resetsAt ? { resets_at: resetsAt } : {}),
  };
}

function structuredModelUsageWindows(
  value: unknown,
): StructuredModelUsageWindow[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.flatMap((entry) => {
    const record = asRecordOrNull(entry);
    const displayName =
      typeof record?.display_name === "string"
        ? record.display_name.trim()
        : "";
    const window = structuredUsageWindow(record);
    return displayName && window
      ? [{ display_name: displayName, ...window }]
      : [];
  });
}

function structuredExtraUsage(
  value: unknown,
): StructuredExtraUsage | undefined {
  const record = asRecordOrNull(value);
  if (!record || record.is_enabled === false) {
    return undefined;
  }
  const monthlyLimit = finiteNumber(record, "monthly_limit");
  const usedCredits = finiteNumber(record, "used_credits");
  const utilization = finiteNumber(record, "utilization");
  const normalized: StructuredExtraUsage = {
    ...(monthlyLimit !== undefined ? { monthly_limit: monthlyLimit } : {}),
    ...(usedCredits !== undefined ? { used_credits: usedCredits } : {}),
    ...(utilization !== undefined
      ? { utilization: Math.max(0, Math.min(100, utilization)) }
      : {}),
    ...(typeof record.currency === "string" && record.currency.trim()
      ? { currency: record.currency.trim() }
      : {}),
  };
  return Object.keys(normalized).length > 0 ? normalized : undefined;
}

function structuredSessionUsage(
  value: unknown,
): StructuredSessionUsage | undefined {
  const record = asRecordOrNull(value);
  if (!record) {
    return undefined;
  }
  const modelUsage = asRecordOrNull(record.model_usage);
  const normalized: StructuredSessionUsage = {
    ...(finiteNumber(record, "total_cost_usd") !== undefined
      ? { total_cost_usd: finiteNumber(record, "total_cost_usd") }
      : {}),
    ...(finiteNumber(record, "total_api_duration_ms") !== undefined
      ? { total_api_duration_ms: finiteNumber(record, "total_api_duration_ms") }
      : {}),
    ...(finiteNumber(record, "total_duration_ms") !== undefined
      ? { total_duration_ms: finiteNumber(record, "total_duration_ms") }
      : {}),
    ...(finiteNumber(record, "total_lines_added") !== undefined
      ? { total_lines_added: finiteNumber(record, "total_lines_added") }
      : {}),
    ...(finiteNumber(record, "total_lines_removed") !== undefined
      ? { total_lines_removed: finiteNumber(record, "total_lines_removed") }
      : {}),
    ...(modelUsage ? { model_count: Object.keys(modelUsage).length } : {}),
  };
  return Object.keys(normalized).length > 0 ? normalized : undefined;
}

function structuredActivityWindow(
  value: unknown,
): StructuredActivityWindow | undefined {
  const record = asRecordOrNull(value);
  const requestCount = finiteNumber(record, "request_count");
  const sessionCount = finiteNumber(record, "session_count");
  if (
    requestCount === undefined ||
    sessionCount === undefined ||
    requestCount < 0 ||
    sessionCount < 0
  ) {
    return undefined;
  }
  return {
    request_count: Math.trunc(requestCount),
    session_count: Math.trunc(sessionCount),
    behaviors: structuredBehaviorAttributions(record?.behaviors),
    agents: structuredNamedAttributions(record?.agents),
    skills: structuredNamedAttributions(record?.skills),
    plugins: structuredNamedAttributions(record?.plugins),
    mcp_servers: structuredNamedAttributions(record?.mcp_servers),
  };
}

function structuredBehaviorAttributions(
  value: unknown,
): StructuredBehaviorAttribution[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.flatMap((entry) => {
    const record = asRecordOrNull(entry);
    const key = typeof record?.key === "string" ? record.key.trim() : "";
    const pct = finiteNumber(record, "pct");
    const count = finiteNumber(record, "count");
    if (!key || pct === undefined || count === undefined || count < 0) {
      return [];
    }
    return [
      { key, pct: Math.max(0, Math.min(100, pct)), count: Math.trunc(count) },
    ];
  });
}

function structuredNamedAttributions(
  value: unknown,
): StructuredNamedAttribution[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.flatMap((entry) => {
    const record = asRecordOrNull(entry);
    const name = typeof record?.name === "string" ? record.name.trim() : "";
    const pct = finiteNumber(record, "pct");
    return name && pct !== undefined
      ? [{ name, pct: Math.max(0, Math.min(100, pct)) }]
      : [];
  });
}

export function normalizeStructuredUsage(
  value: unknown,
): StructuredUsageSnapshot {
  const root = asRecordOrNull(value);
  const limits = asRecordOrNull(root?.rate_limits);
  const behaviors = asRecordOrNull(root?.behaviors);
  const fiveHour = structuredUsageWindow(limits?.five_hour);
  const sevenDay = structuredUsageWindow(limits?.seven_day);
  const sevenDayOauthApps = structuredUsageWindow(limits?.seven_day_oauth_apps);
  const sevenDayOpus = structuredUsageWindow(limits?.seven_day_opus);
  const sevenDaySonnet = structuredUsageWindow(limits?.seven_day_sonnet);
  const modelScoped = structuredModelUsageWindows(limits?.model_scoped);
  const extraUsage = structuredExtraUsage(limits?.extra_usage);
  const session = structuredSessionUsage(root?.session);
  const activityDay = structuredActivityWindow(behaviors?.day);
  const activityWeek = structuredActivityWindow(behaviors?.week);
  return {
    ...(typeof root?.subscription_type === "string" &&
    root.subscription_type.trim()
      ? { subscription_type: root.subscription_type.trim() }
      : {}),
    ...(typeof root?.rate_limits_available === "boolean"
      ? { rate_limits_available: root.rate_limits_available }
      : {}),
    ...(fiveHour ? { five_hour: fiveHour } : {}),
    ...(sevenDay ? { seven_day: sevenDay } : {}),
    ...(sevenDayOauthApps ? { seven_day_oauth_apps: sevenDayOauthApps } : {}),
    ...(sevenDayOpus ? { seven_day_opus: sevenDayOpus } : {}),
    ...(sevenDaySonnet ? { seven_day_sonnet: sevenDaySonnet } : {}),
    ...(modelScoped.length > 0 ? { model_scoped: modelScoped } : {}),
    ...(extraUsage ? { extra_usage: extraUsage } : {}),
    ...(session ? { session } : {}),
    ...(activityDay ? { activity_day: activityDay } : {}),
    ...(activityWeek ? { activity_week: activityWeek } : {}),
  };
}

async function getUsage(
  command: Extract<SessionDataCommand, { command: "get_usage" }>,
  requestId: string | undefined,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
    return;
  }
  try {
    const usageMethod = (
      session.query as Query & {
        usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET?: () => Promise<unknown>;
      }
    ).usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET;
    if (typeof usageMethod !== "function") {
      throw new Error("structured SDK usage is unavailable in this runtime");
    }
    const snapshot = normalizeStructuredUsage(
      await usageMethod.call(session.query),
    );
    if (Object.keys(snapshot).length === 0) {
      throw new Error(
        "structured SDK usage returned an incompatible empty payload",
      );
    }
    writeEvent(
      { event: "usage_snapshot", session_id: session.sessionId, snapshot },
      requestId,
    );
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    bridgeLogger.warn({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "structured_usage_failed",
      message: "experimental structured SDK usage failed",
      outcome: "fallback_required",
      ...(requestId ? { requestId } : {}),
      sessionId: session.sessionId,
      fields: { error_message: message },
    });
    writeEvent(
      {
        event: "usage_snapshot",
        session_id: session.sessionId,
        error: message,
      },
      requestId,
    );
  }
}

async function getRewindTargets(
  command: Extract<SessionDataCommand, { command: "get_rewind_targets" }>,
  requestId: string | undefined,
  deps: SessionDataCommandDeps,
): Promise<void> {
  const activeSession = sessionById(command.session_id);
  try {
    let cwd = activeSession?.cwd;
    if (!cwd) {
      const listedSession = (
        await listSessions(currentSessionListOptions())
      ).find((entry) => entry.sessionId === command.session_id);
      if (!listedSession) {
        throw new Error(`unknown session: ${command.session_id}`);
      }
      cwd = listedSession.cwd?.trim() || currentSessionListOptions().dir;
    }
    const historyMessages = await getSessionMessages(command.session_id, {
      ...(cwd ? { dir: cwd } : {}),
      includeSystemMessages: true,
    });
    const targets = deps.rewindTargetsFromSessionMessages(historyMessages);
    bridgeLogger.info({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "rewind_targets_loaded",
      message: "rewind targets loaded from session history",
      outcome: "success",
      ...(requestId ? { requestId } : {}),
      sessionId: command.session_id,
      fields: {
        history_message_count: historyMessages.length,
        target_count: targets.length,
      },
    });
    writeEvent(
      {
        event: "rewind_targets",
        session_id: command.session_id,
        targets,
      },
      requestId,
    );
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    bridgeLogger.warn({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "rewind_targets_failed",
      message: "failed to load rewind targets",
      outcome: "failure",
      ...(requestId ? { requestId } : {}),
      sessionId: command.session_id,
      fields: { error_message: message },
    });
    writeEvent(
      {
        event: "rewind_targets",
        session_id: command.session_id,
        targets: [],
        error: `failed to load rewind targets: ${message}`,
      },
      requestId,
    );
  }
}

function requireSession(
  sessionId: string,
  requestId?: string,
): SessionState | null {
  const session = sessionById(sessionId);
  if (!session) {
    slashError(sessionId, `unknown session: ${sessionId}`, requestId);
  }
  return session;
}
