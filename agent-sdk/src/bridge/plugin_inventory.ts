import { isAbsolute, normalize } from "node:path";
import type { SessionState } from "./session_lifecycle.js";
import { writeEvent } from "./events.js";
import { asRecordOrNull } from "./shared.js";

function displayText(value: unknown, limit: number): string | undefined {
  return typeof value === "string" &&
    value.trim().length > 0 &&
    value.length <= limit &&
    !/[\p{Cc}\p{Cf}\p{Cs}\p{Zl}\p{Zp}]/u.test(value)
    ? value
    : undefined;
}

/** The SDK inventory is the sole evidence that a folder actually loaded. */
export function updateSessionPlugins(session: SessionState, value: unknown): void {
  if (!Array.isArray(value)) return;
  session.pluginInventory = value.flatMap((entry) => {
    const plugin = asRecordOrNull(entry);
    const name = displayText(plugin?.name, 256);
    const path = displayText(plugin?.path, 32768);
    if (!name || !path) return [];
    const source = displayText(plugin?.source, 512);
    const version = displayText(plugin?.version, 128);
    return [
      {
        name,
        path,
        ...(source ? { source } : {}),
        ...(version ? { version } : {}),
      },
    ];
  });
  emitSessionPluginInventory(session);
}

export function emitSessionPluginInventory(session: SessionState): void {
  if (session.connected && !session.closing && session.pluginInventory) {
    writeEvent({
      event: "plugin_inventory",
      session_id: session.sessionId,
      plugins: attachedPlugins(session),
    });
  }
}

function attachedPlugins(session: SessionState) {
  return (session.pluginInventory ?? []).filter(
    (plugin) =>
      isAbsolute(plugin.path) &&
      (plugin.source?.endsWith("@inline") ||
        session.pluginLaunchPaths?.some((path) => normalize(path) === normalize(plugin.path))),
  );
}

/** Carry confirmed attachment intent through replacements before the first SDK inventory arrives. */
export function sessionPluginFolders(session: SessionState): string[] {
  if (session.pluginInventory === undefined) return [...(session.pluginLaunchPaths ?? [])];
  return attachedPlugins(session).map((plugin) => plugin.path);
}
