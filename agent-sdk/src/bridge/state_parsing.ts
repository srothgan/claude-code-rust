import type {
  ApiRetryError,
  FastModeState,
  RateLimitStatus,
  RuntimeSessionState,
  SessionUpdate,
  SettingsParseErrorUpdate,
} from "../types.js";
import { asRecordOrNull } from "./shared.js";

export function numberField(record: Record<string, unknown>, ...keys: string[]): number | undefined {
  for (const key of keys) {
    const value = record[key];
    if (typeof value === "number" && Number.isFinite(value)) {
      return value;
    }
  }
  return undefined;
}

export function parseFastModeState(value: unknown): FastModeState | null {
  if (value === "off" || value === "cooldown" || value === "on") {
    return value;
  }
  return null;
}

export function parseRateLimitStatus(value: unknown): RateLimitStatus | null {
  if (value === "allowed" || value === "allowed_warning" || value === "rejected") {
    return value;
  }
  return null;
}

export function parseRuntimeSessionState(value: unknown): RuntimeSessionState | null {
  if (value === "idle" || value === "running" || value === "requires_action") {
    return value;
  }
  return null;
}

export function parseApiRetryError(value: unknown): ApiRetryError {
  switch (value) {
    case "authentication_failed":
    case "billing_error":
    case "rate_limit":
    case "invalid_request":
    case "server_error":
    case "max_output_tokens":
      return value;
    default:
      return "unknown";
  }
}

export function buildRateLimitUpdate(
  rateLimitInfo: unknown,
): Extract<SessionUpdate, { type: "rate_limit_update" }> | null {
  const info = asRecordOrNull(rateLimitInfo);
  if (!info) {
    return null;
  }

  const status = parseRateLimitStatus(info.status);
  if (!status) {
    return null;
  }

  const update: Extract<SessionUpdate, { type: "rate_limit_update" }> = {
    type: "rate_limit_update",
    status,
  };

  const resetsAt = numberField(info, "resetsAt");
  if (resetsAt !== undefined) {
    update.resets_at = resetsAt;
  }

  const utilization = numberField(info, "utilization");
  if (utilization !== undefined) {
    update.utilization = utilization;
  }

  if (typeof info.rateLimitType === "string" && info.rateLimitType.length > 0) {
    update.rate_limit_type = info.rateLimitType;
  }

  const overageStatus = parseRateLimitStatus(info.overageStatus);
  if (overageStatus) {
    update.overage_status = overageStatus;
  }

  const overageResetsAt = numberField(info, "overageResetsAt");
  if (overageResetsAt !== undefined) {
    update.overage_resets_at = overageResetsAt;
  }

  if (typeof info.overageDisabledReason === "string" && info.overageDisabledReason.length > 0) {
    update.overage_disabled_reason = info.overageDisabledReason;
  }

  if (typeof info.isUsingOverage === "boolean") {
    update.is_using_overage = info.isUsingOverage;
  }

  const surpassedThreshold = numberField(info, "surpassedThreshold");
  if (surpassedThreshold !== undefined) {
    update.surpassed_threshold = surpassedThreshold;
  }

  return update;
}

export function buildApiRetryUpdate(
  message: Record<string, unknown>,
): Extract<SessionUpdate, { type: "api_retry_update" }> | null {
  const attempt = numberField(message, "attempt");
  const maxRetries = numberField(message, "max_retries", "maxRetries");
  const retryDelayMs = numberField(message, "retry_delay_ms", "retryDelayMs");
  if (attempt === undefined || maxRetries === undefined || retryDelayMs === undefined) {
    return null;
  }

  const rawStatus = message.error_status ?? message.errorStatus;
  const errorStatus = typeof rawStatus === "number" && Number.isFinite(rawStatus) ? rawStatus : null;

  return {
    type: "api_retry_update",
    attempt,
    max_retries: maxRetries,
    retry_delay_ms: retryDelayMs,
    error_status: errorStatus,
    error: parseApiRetryError(message.error),
  };
}

export function normalizeSettingsParseError(value: unknown): SettingsParseErrorUpdate | null {
  const record = asRecordOrNull(value);
  if (!record) {
    return null;
  }
  const message = typeof record.message === "string" ? record.message.trim() : "";
  if (!message) {
    return null;
  }
  const path = typeof record.path === "string" ? record.path : "";
  const file = typeof record.file === "string" && record.file.trim() ? record.file : undefined;
  return {
    ...(file ? { file } : {}),
    path,
    message,
  };
}

export function normalizeSettingsParseErrors(value: unknown): SettingsParseErrorUpdate[] {
  const entries = Array.isArray(value) ? value : [value];
  return entries.flatMap((entry) => {
    const normalized = normalizeSettingsParseError(entry);
    return normalized ? [normalized] : [];
  });
}
