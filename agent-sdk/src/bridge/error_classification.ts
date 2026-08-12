import type { ApiRetryError, TurnErrorKind } from "../types.js";
import { looksLikeAuthRequired } from "./auth.js";
import { writeEvent } from "./events.js";
import { emitSessionUpdate } from "./events.js";
import type { SessionState } from "./session_lifecycle.js";
import {
  parseFastModeDisabledReason,
  parseFastModeState,
} from "./state_parsing.js";

export function emitAuthRequired(session: SessionState, detail?: string): void {
  if (session.deferConnect) {
    session.deferredAuthRequired = detail ? { detail } : {};
    return;
  }
  if (session.authHintSent) {
    return;
  }
  session.authHintSent = true;
  writeEvent({
    event: "auth_required",
    method_name: "Claude Login",
    method_description:
      detail && detail.trim().length > 0
        ? detail
        : "Type /login to authenticate.",
  });
}

export function looksLikePlanLimitError(input: string): boolean {
  const normalized = input.toLowerCase();
  return (
    normalized.includes("rate limit") ||
    normalized.includes("rate-limit") ||
    normalized.includes("max turns") ||
    normalized.includes("max budget") ||
    normalized.includes("quota") ||
    normalized.includes("plan limit") ||
    normalized.includes("too many requests") ||
    normalized.includes("insufficient quota") ||
    normalized.includes("429")
  );
}

export function classifyTurnErrorKind(
  subtype: string,
  errors: string[],
  assistantError?: ApiRetryError,
): TurnErrorKind {
  const combined = errors.join("\n");

  switch (assistantError) {
    case "billing_error":
    case "rate_limit":
      return "plan_limit";
    case "authentication_failed":
      return "auth_required";
    case "oauth_org_not_allowed":
      return "account_access";
    case "model_not_found":
      return "model_unavailable";
    case "overloaded":
    case "server_error":
      return "transient_service";
    case "invalid_request":
    case "max_output_tokens":
    case "unknown":
    case undefined:
      break;
  }

  if (
    subtype === "error_max_turns" ||
    subtype === "error_max_budget_usd" ||
    (combined.length > 0 && looksLikePlanLimitError(combined))
  ) {
    return "plan_limit";
  }

  if (errors.some((entry) => looksLikeAuthRequired(entry))) {
    return "auth_required";
  }

  return "other";
}

export function setFastModeSnapshotIfChanged(
  session: SessionState,
  stateValue: unknown,
  disabledReasonValue: unknown,
): boolean {
  const nextState = parseFastModeState(stateValue) ?? session.fastModeState;
  const nextDisabledReason = parseFastModeDisabledReason(disabledReasonValue);
  if (
    nextState === session.fastModeState &&
    nextDisabledReason === session.fastModeDisabledReason
  ) {
    return false;
  }
  session.fastModeState = nextState;
  session.fastModeDisabledReason = nextDisabledReason;
  return true;
}

export function emitFastModeUpdate(session: SessionState): void {
  emitSessionUpdate(session.sessionId, {
    type: "fast_mode_update",
    fast_mode_state: session.fastModeState,
    ...(session.fastModeDisabledReason
      ? { fast_mode_disabled_reason: session.fastModeDisabledReason }
      : {}),
  });
}

export function emitFastModeUpdateIfChanged(
  session: SessionState,
  stateValue: unknown,
  disabledReasonValue: unknown,
): void {
  if (setFastModeSnapshotIfChanged(session, stateValue, disabledReasonValue)) {
    emitFastModeUpdate(session);
  }
}
