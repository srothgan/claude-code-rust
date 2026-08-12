import type { BridgeCommand } from "../types.js";

type CommandTask = () => Promise<void>;

type CommandLane =
  | { kind: "lifecycle" }
  | { kind: "session"; sessionId: string }
  | { kind: "unblocking" };

const LIFECYCLE_COMMANDS = new Set<BridgeCommand["command"]>([
  "initialize",
  "create_session",
  "resume_session",
  "resume_session_at",
  "new_session",
  "rewind",
  "shutdown",
]);

const UNBLOCKING_COMMANDS = new Set<BridgeCommand["command"]>([
  "cancel_turn",
  "permission_response",
  "question_response",
  "user_dialog_response",
  "elicitation_response",
  "mcp_oauth_callback_url",
]);

function commandLane(command: BridgeCommand): CommandLane {
  if (UNBLOCKING_COMMANDS.has(command.command)) {
    return { kind: "unblocking" };
  }
  if (LIFECYCLE_COMMANDS.has(command.command)) {
    return { kind: "lifecycle" };
  }
  if ("session_id" in command) {
    return { kind: "session", sessionId: command.session_id };
  }
  return { kind: "lifecycle" };
}

function settle(task: Promise<void>): Promise<void> {
  return task.then(
    () => undefined,
    () => undefined,
  );
}

export class BridgeCommandScheduler {
  private lifecycleTail: Promise<void> | undefined;
  private readonly sessionTails = new Map<string, Promise<void>>();
  private readonly activeTasks = new Set<Promise<void>>();
  private shutdownState: "accepting" | "queued" | "running" | "complete" =
    "accepting";

  schedule(
    command: BridgeCommand,
    task: CommandTask,
  ): Promise<void> | undefined {
    const lane = commandLane(command);
    if (!this.accepts(command, lane)) {
      return undefined;
    }

    let scheduledTask = task;
    if (command.command === "shutdown") {
      this.shutdownState = "queued";
      scheduledTask = async () => {
        this.shutdownState = "running";
        try {
          await task();
        } finally {
          this.shutdownState = "complete";
        }
      };
    }

    const operation = this.scheduleInLane(lane, scheduledTask);
    this.track(operation);
    return operation;
  }

  stopAccepting(): void {
    if (this.shutdownState === "accepting") {
      this.shutdownState = "complete";
    }
  }

  async whenIdle(): Promise<void> {
    while (this.activeTasks.size > 0) {
      await Promise.all(this.activeTasks);
    }
  }

  private accepts(command: BridgeCommand, lane: CommandLane): boolean {
    if (this.shutdownState === "accepting") {
      return true;
    }
    return (
      this.shutdownState === "queued" &&
      command.command !== "shutdown" &&
      lane.kind === "unblocking"
    );
  }

  private scheduleInLane(lane: CommandLane, task: CommandTask): Promise<void> {
    switch (lane.kind) {
      case "unblocking":
        return this.start(task);
      case "lifecycle":
        return this.scheduleLifecycle(task);
      case "session":
        return this.scheduleSession(lane.sessionId, task);
    }
  }

  private scheduleLifecycle(task: CommandTask): Promise<void> {
    const dependencies = new Set(this.sessionTails.values());
    if (this.lifecycleTail) {
      dependencies.add(this.lifecycleTail);
    }
    const operation =
      dependencies.size === 0
        ? this.start(task)
        : Promise.all(dependencies).then(task);
    const tail = settle(operation);
    this.lifecycleTail = tail;
    void tail.then(() => {
      if (this.lifecycleTail === tail) {
        this.lifecycleTail = undefined;
      }
    });
    return operation;
  }

  private scheduleSession(sessionId: string, task: CommandTask): Promise<void> {
    const dependencies: Promise<void>[] = [];
    if (this.lifecycleTail) {
      dependencies.push(this.lifecycleTail);
    }
    const previousSessionTask = this.sessionTails.get(sessionId);
    if (previousSessionTask) {
      dependencies.push(previousSessionTask);
    }
    const operation =
      dependencies.length === 0
        ? this.start(task)
        : Promise.all(dependencies).then(task);
    const tail = settle(operation);
    this.sessionTails.set(sessionId, tail);
    void tail.then(() => {
      if (this.sessionTails.get(sessionId) === tail) {
        this.sessionTails.delete(sessionId);
      }
    });
    return operation;
  }

  private track(operation: Promise<void>): void {
    const tracked = settle(operation);
    this.activeTasks.add(tracked);
    void tracked.then(() => {
      this.activeTasks.delete(tracked);
    });
  }

  private start(task: CommandTask): Promise<void> {
    try {
      return task();
    } catch (error) {
      return Promise.reject(error);
    }
  }
}
