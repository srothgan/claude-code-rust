import type { SlashCommand } from "@anthropic-ai/claude-agent-sdk";
import type { AvailableCommand } from "../types.js";
import { emitSessionUpdate } from "./events.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";
import type { SessionState } from "./session_lifecycle.js";

export type AvailableCommandsSource =
  | "session_result_commands"
  | "init_slash_commands"
  | "supportedCommands"
  | "commands_changed"
  | "reload_plugins";

export type AvailableCommandsSnapshot = {
  generation: number;
  source: AvailableCommandsSource;
  signature: string;
  dynamicSeen: boolean;
  commands: AvailableCommand[];
};

function isDynamicSource(source: AvailableCommandsSource): boolean {
  return source === "commands_changed" || source === "reload_plugins";
}

function commandSignature(commands: AvailableCommand[]): string {
  return JSON.stringify(
    commands.map((command) => [command.name, command.description, command.input_hint ?? ""]),
  );
}

function logAvailableCommandsDecision(
  session: SessionState,
  source: AvailableCommandsSource,
  commands: AvailableCommand[],
  outcome: "accepted" | "ignored",
  reason: string,
  generation: number,
): void {
  bridgeLogger.info({
    target: LOG_TARGETS.APP_SESSION,
    eventName:
      outcome === "accepted"
        ? "available_commands_snapshot_accepted"
        : "available_commands_snapshot_ignored",
    message:
      outcome === "accepted"
        ? "available commands snapshot accepted"
        : "available commands snapshot ignored",
    outcome,
    sessionId: session.sessionId,
    count: commands.length,
    fields: {
      source,
      previous_source: session.availableCommands?.source,
      generation,
      command_count: commands.length,
      command_names: commands.map((command) => command.name),
      reason,
    },
  });
}

function shouldAcceptAvailableCommandsSnapshot(
  session: SessionState,
  source: AvailableCommandsSource,
  commands: AvailableCommand[],
): { accept: true; reason: string } | { accept: false; reason: string } {
  const current = session.availableCommands;
  if (isDynamicSource(source)) {
    return { accept: true, reason: "dynamic source is authoritative" };
  }
  if (!current) {
    if (commands.length === 0) {
      return { accept: false, reason: "empty bootstrap snapshot ignored before first command list" };
    }
    return { accept: true, reason: "initial command snapshot" };
  }
  if (current.dynamicSeen) {
    return {
      accept: false,
      reason: "bootstrap source cannot replace dynamic command snapshot",
    };
  }
  if (source === "supportedCommands" && current.commands.length > 0) {
    return {
      accept: false,
      reason: "supportedCommands is an initialize-time fallback and current snapshot already exists",
    };
  }
  if (commands.length === 0) {
    return { accept: false, reason: "empty bootstrap snapshot ignored" };
  }
  return { accept: true, reason: "bootstrap refresh before dynamic command source" };
}

export function updateAvailableCommands(
  session: SessionState,
  source: AvailableCommandsSource,
  commands: AvailableCommand[],
): boolean {
  const decision = shouldAcceptAvailableCommandsSnapshot(session, source, commands);
  const currentGeneration = session.availableCommands?.generation ?? 0;
  const nextGeneration = currentGeneration + 1;
  if (!decision.accept) {
    logAvailableCommandsDecision(
      session,
      source,
      commands,
      "ignored",
      decision.reason,
      currentGeneration,
    );
    return false;
  }

  const dynamicSeen = session.availableCommands?.dynamicSeen === true || isDynamicSource(source);
  session.availableCommands = {
    generation: nextGeneration,
    source,
    signature: commandSignature(commands),
    dynamicSeen,
    commands,
  };
  logAvailableCommandsDecision(
    session,
    source,
    commands,
    "accepted",
    decision.reason,
    nextGeneration,
  );
  bridgeLogger.info({
    target: LOG_TARGETS.APP_SESSION,
    eventName: "available_commands_update_emitted",
    message: "available commands update emitted",
    outcome: "success",
    sessionId: session.sessionId,
    count: commands.length,
    fields: {
      source,
      generation: nextGeneration,
      command_count: commands.length,
      command_names: commands.map((command) => command.name),
    },
  });
  emitSessionUpdate(session.sessionId, {
    type: "available_commands_update",
    commands,
    source,
    generation: nextGeneration,
  });
  return true;
}

export function mapSdkSlashCommand(command: unknown): AvailableCommand | null {
  if (!command || typeof command !== "object") {
    return null;
  }
  const record = command as Partial<SlashCommand> & Record<string, unknown>;
  const name = typeof record.name === "string" ? record.name : "";
  if (!name) {
    return null;
  }
  return {
    name,
    description: typeof record.description === "string" ? record.description : "",
    input_hint: typeof record.argumentHint === "string" ? record.argumentHint : undefined,
  };
}

export function mapSdkSlashCommands(commands: unknown): AvailableCommand[] {
  if (!Array.isArray(commands)) {
    return [];
  }
  return commands.flatMap((command) => {
    const mapped = mapSdkSlashCommand(command);
    return mapped ? [mapped] : [];
  });
}

export function mapInitSlashCommands(commands: unknown): AvailableCommand[] {
  if (!Array.isArray(commands)) {
    return [];
  }
  return commands
    .filter((entry): entry is string => typeof entry === "string")
    .map((name) => ({ name, description: "", input_hint: undefined }));
}
