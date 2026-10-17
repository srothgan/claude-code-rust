import type { McpServerConfig, Settings } from "@anthropic-ai/claude-agent-sdk";
import type { SessionLaunchSettings } from "../types.js";

/**
 * Maps the launch configuration flags to Agent SDK query options and layers them over the
 * saved settings. The fork flag and the resume rename live in command_lifecycle.ts, and
 * session_lifecycle.ts wires the file-checkpointing setting. The app has already resolved
 * paths and checked that files were readable and JSON parsed, so these functions only map and
 * layer.
 */

/**
 * The SDK takes one flag-settings layer, so the user's `--settings` object is that layer and
 * the host capability restrictions are laid over it: draft review and held peer-message review
 * require native UI surfaces. The flag layer outranks the user, project and local files, so
 * the value in effect for `crossSessionInbound` is the flag's, then the saved cascade's.
 */
export function buildFlagSettings(
  launchSettings: SessionLaunchSettings,
  resolvedSettings: Settings,
): Settings {
  const flagSettings = (launchSettings.settings ?? {}) as Settings;
  const crossSessionInbound =
    flagSettings.crossSessionInbound ?? resolvedSettings.crossSessionInbound;
  return {
    ...flagSettings,
    feedbackDrafts: "off",
    ...(!["accept", "refuse"].includes(crossSessionInbound ?? "")
      ? { crossSessionInbound: "refuse" as const }
      : {}),
  };
}

/** `fileCheckpointingEnabled` as the flag layer and then the saved cascade set it. */
export function fileCheckpointingEnabled(
  launchSettings: SessionLaunchSettings,
  resolvedSettings: Settings,
): boolean {
  const flag = launchSettings.settings?.fileCheckpointingEnabled;
  return typeof flag === "boolean" ? flag : (resolvedSettings.fileCheckpointingEnabled ?? true);
}

/**
 * The servers of every `--mcp-config` in command-line order, a later server of the same name
 * replacing an earlier one. A Map keeps a server named `__proto__` an ordinary entry. The SDK
 * validates each definition.
 */
export function mergeMcpServers(
  configs: SessionLaunchSettings["mcp_configs"],
): Record<string, McpServerConfig> | undefined {
  if (!configs || configs.length === 0) {
    return undefined;
  }
  const servers = new Map<string, McpServerConfig>();
  for (const config of configs) {
    for (const [name, server] of Object.entries(config)) {
      servers.set(name, server as McpServerConfig);
    }
  }
  return Object.fromEntries(servers);
}

/**
 * The runtime guard is a bridge invariant, so it stays in every system prompt: appended to the
 * claude_code preset, or after a `--system-prompt` that replaces the preset. Text from
 * `--append-system-prompt` follows the guard. The SDK records the prompt with the
 * conversation, so a resumed session follows what it persisted.
 */
export function buildSystemPrompt(launchSettings: SessionLaunchSettings, runtimeGuard: string) {
  const append = [runtimeGuard, launchSettings.append_system_prompt]
    .filter((text): text is string => text !== undefined)
    .join("\n\n");
  return launchSettings.system_prompt !== undefined
    ? {
        type: "custom" as const,
        prompt: [launchSettings.system_prompt, append],
        snapshot: true,
      }
    : { type: "preset" as const, preset: "claude_code" as const, append, snapshot: true };
}

/**
 * Query options for the launch name, added directories and MCP servers. The SDK applies
 * `title` to a new or forked session only (measured against Claude Code: a forked session
 * stores it as its custom title and the original keeps its own); a resumed session keeps its persisted title, so the
 * resume path renames it instead. `renameSession` cannot name a new session, because its
 * transcript file does not exist before the first turn (measured on the pinned SDK:
 * "Session ... not found in project directory").
 */
export function buildLaunchQueryOptions(
  launchSettings: SessionLaunchSettings,
  session: { resume?: string; forkSession?: boolean },
) {
  const mcpServers = mergeMcpServers(launchSettings.mcp_configs);
  const startsNewConversation = !session.resume || session.forkSession === true;
  return {
    ...(launchSettings.name !== undefined && startsNewConversation
      ? { title: launchSettings.name }
      : {}),
    ...(launchSettings.additional_directories && launchSettings.additional_directories.length > 0
      ? { additionalDirectories: launchSettings.additional_directories }
      : {}),
    ...(mcpServers ? { mcpServers } : {}),
    ...(launchSettings.strict_mcp_config ? { strictMcpConfig: true } : {}),
  };
}
