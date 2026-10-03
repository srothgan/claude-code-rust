import type { Query } from "@anthropic-ai/claude-agent-sdk";
import { asRecordOrNull } from "./shared.js";

interface QuerySettingsRuntime {
  getSettings(): Promise<unknown>;
}

function hasSettings(query: Query): query is Query & QuerySettingsRuntime {
  return "getSettings" in query && typeof query.getSettings === "function";
}

/** Adapter for the pinned runtime's get_settings response, absent from Query's declarations. */
export async function readAppliedSettings(query: Query): Promise<Record<string, unknown>> {
  const settings = await readQuerySettings(query);
  const applied = asRecordOrNull(settings.applied);
  if (!applied) {
    throw new Error("Invalid applied settings response from the Agent SDK runtime.");
  }
  return applied;
}

export async function readQuerySettings(query: Query): Promise<Record<string, unknown>> {
  if (!hasSettings(query)) {
    throw new Error("Applied settings are unavailable with the installed Agent SDK runtime.");
  }
  const settings = asRecordOrNull(await query.getSettings());
  if (!settings) throw new Error("Invalid settings response from the Agent SDK runtime.");
  return settings;
}
