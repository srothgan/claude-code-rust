import type { BridgeCommand } from "../types.js";

type CommandTask = () => Promise<void>;

/**
 * How long a command may wait in its lane before it would start, measured from when it is
 * scheduled. On expiry `onExpired` answers for the command and the command is skipped when its
 * turn comes; a command that has started is not cut short.
 */
export interface QueueDeadline {
  ms: number;
  onExpired: () => void;
}

type CommandLane =
  | { kind: "lifecycle" }
  | { kind: "session"; sessionId: string }
  | { kind: "unblocking" };

const LIFECYCLE_COMMANDS = new Set<BridgeCommand["command"]>([
  "initialize",
  "inspect_settings",
  "mutate_setting",
  "create_session",
  "resume_session",
  "resume_session_at",
  "new_session",
  "rewind",
  "shutdown",
]);

const UNBLOCKING_COMMANDS = new Set<BridgeCommand["command"]>([
  "side_question",
  "cancel_turn",
  // stopTask can stay pending until its query ends; later session commands
  // must not wait behind it.
  "stop_subagent",
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

// The timer is armed before the lane can start the task, so a task that starts at once clears it.
function withQueueDeadline(
  task: CommandTask,
  deadline: QueueDeadline,
): CommandTask {
  let expired = false;
  const timer = setTimeout(() => {
    expired = true;
    deadline.onExpired();
  }, deadline.ms);
  return async () => {
    clearTimeout(timer);
    if (!expired) {
      await task();
    }
  };
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
    queueDeadline?: QueueDeadline,
  ): Promise<void> | undefined {
    const lane = commandLane(command);
    if (!this.accepts(command, lane)) {
      return undefined;
    }

    const boundedTask = queueDeadline
      ? withQueueDeadline(task, queueDeadline)
      : task;
    let scheduledTask = boundedTask;
    if (command.command === "shutdown") {
      this.shutdownState = "queued";
      scheduledTask = async () => {
        this.shutdownState = "running";
        try {
          await boundedTask();
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
