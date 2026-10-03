import type { Query } from "@anthropic-ai/claude-agent-sdk";
import { readQuerySettings } from "./query_settings.js";
import { asRecordOrNull } from "./shared.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";

/** Native question timing uses trusted sources only, not the raw file cascade. */
export async function questionIdleTimeout(query: Query): Promise<number | undefined> {
  try {
    const settings = await readQuerySettings(query);
    const sources = Array.isArray(settings.sources) ? settings.sources : [];
    for (const name of ["policySettings", "flagSettings", "userSettings"]) {
      const source = sources.map(asRecordOrNull).find(source => source?.source === name);
      const value = asRecordOrNull(source?.settings)?.askUserQuestionTimeout;
      if (value !== undefined) {
        return value === "60s" ? 60_000 : value === "5m" ? 300_000 : value === "10m" ? 600_000 : undefined;
      }
    }
  } catch {
    bridgeLogger.warn({ target: LOG_TARGETS.BRIDGE_PERMISSION, eventName: "question_timing_unavailable", message: "Question timing could not be verified; awaiting explicit answers", outcome: "unavailable" });
  }
  return undefined;
}
