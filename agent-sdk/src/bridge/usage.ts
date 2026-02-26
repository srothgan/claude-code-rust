import type { SessionUpdate } from "../types.js";
import { asRecordOrNull } from "./shared.js";

export type UsageSessionContext = {
  model?: string;
  lastTotalCostUsd?: number;
};

function numberField(record: Record<string, unknown>, ...keys: string[]): number | undefined {
  for (const key of keys) {
    const value = record[key];
    if (typeof value === "number" && Number.isFinite(value)) {
      return value;
    }
  }
  return undefined;
}

function selectModelUsageRecord(
  session: UsageSessionContext | undefined,
  message: Record<string, unknown>,
): Record<string, unknown> | null {
  const modelUsageRaw = asRecordOrNull(message.modelUsage);
  if (!modelUsageRaw) {
    return null;
  }
  const sortedKeys = Object.keys(modelUsageRaw).sort();
  if (sortedKeys.length === 0) {
    return null;
  }

  const preferredKeys = new Set<string>();
  if (session?.model) {
    preferredKeys.add(session.model);
  }
  if (typeof message.model === "string") {
    preferredKeys.add(message.model);
  }

  for (const key of preferredKeys) {
    const value = asRecordOrNull(modelUsageRaw[key]);
    if (value) {
      return value;
    }
  }
  for (const key of sortedKeys) {
    const value = asRecordOrNull(modelUsageRaw[key]);
    if (value) {
      return value;
    }
  }
  return null;
}

export function buildUsageUpdateFromResultForSession(
  session: UsageSessionContext | undefined,
  message: Record<string, unknown>,
): SessionUpdate | null {
  const usage = asRecordOrNull(message.usage);
  const inputTokens = usage ? numberField(usage, "inputTokens", "input_tokens") : undefined;
  const outputTokens = usage ? numberField(usage, "outputTokens", "output_tokens") : undefined;
  const cacheReadTokens = usage
    ? numberField(
        usage,
        "cacheReadInputTokens",
        "cache_read_input_tokens",
        "cache_read_tokens",
      )
    : undefined;
  const cacheWriteTokens = usage
    ? numberField(
        usage,
        "cacheCreationInputTokens",
        "cache_creation_input_tokens",
        "cache_write_tokens",
      )
    : undefined;

  const totalCostUsd = numberField(message, "total_cost_usd", "totalCostUsd");
  let turnCostUsd: number | undefined;
  if (totalCostUsd !== undefined && session) {
    if (session.lastTotalCostUsd === undefined) {
      turnCostUsd = totalCostUsd;
    } else {
      turnCostUsd = Math.max(0, totalCostUsd - session.lastTotalCostUsd);
    }
    session.lastTotalCostUsd = totalCostUsd;
  }

  const modelUsage = selectModelUsageRecord(session, message);
  const contextWindow = modelUsage
    ? numberField(modelUsage, "contextWindow", "context_window")
    : undefined;
  const maxOutputTokens = modelUsage
    ? numberField(modelUsage, "maxOutputTokens", "max_output_tokens")
    : undefined;

  if (
    inputTokens === undefined &&
    outputTokens === undefined &&
    cacheReadTokens === undefined &&
    cacheWriteTokens === undefined &&
    totalCostUsd === undefined &&
    turnCostUsd === undefined &&
    contextWindow === undefined &&
    maxOutputTokens === undefined
  ) {
    return null;
  }

  return {
    type: "usage_update",
    usage: {
      ...(inputTokens !== undefined ? { input_tokens: inputTokens } : {}),
      ...(outputTokens !== undefined ? { output_tokens: outputTokens } : {}),
      ...(cacheReadTokens !== undefined ? { cache_read_tokens: cacheReadTokens } : {}),
      ...(cacheWriteTokens !== undefined ? { cache_write_tokens: cacheWriteTokens } : {}),
      ...(totalCostUsd !== undefined ? { total_cost_usd: totalCostUsd } : {}),
      ...(turnCostUsd !== undefined ? { turn_cost_usd: turnCostUsd } : {}),
      ...(contextWindow !== undefined ? { context_window: contextWindow } : {}),
      ...(maxOutputTokens !== undefined ? { max_output_tokens: maxOutputTokens } : {}),
    },
  };
}

export function buildUsageUpdateFromResult(message: Record<string, unknown>): SessionUpdate | null {
  return buildUsageUpdateFromResultForSession(undefined, message);
}

