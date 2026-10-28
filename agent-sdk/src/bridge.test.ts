import { registerHooks } from "node:module";
// Only title/list reads are controlled; actual SDK query and transcript APIs remain available.
const bridgeSdkUrl = import.meta.resolve("@anthropic-ai/claude-agent-sdk");
const bridgeControlledSdkUrl = `data:text/javascript,${encodeURIComponent(`
  export * from ${JSON.stringify(bridgeSdkUrl)};
  const pending = new Set();
  function read(value) { const operation=Promise.resolve().then(()=>value);pending.add(operation);void operation.finally(()=>pending.delete(operation));return operation; }
  export function getSessionInfo() { return read(undefined); }
  export function listSessions() { return read([]); }
  export async function settleReads() { while(pending.size)await Promise.all([...pending]); }
`)}`;
const bridgeSdkLoader = registerHooks({
  resolve(specifier, context, next) {
    return specifier === "@anthropic-ai/claude-agent-sdk" && !context.conditions.includes("require")
      ? { url: bridgeControlledSdkUrl, shortCircuit: true }
      : next(specifier, context);
  },
});
const { settleReads: settleBridgeSdkReads } = await import(bridgeControlledSdkUrl);

const { applySessionEffort, applySessionAgent, applySessionFastMode, emitAgentConfigOptionUpdate } =
  await import("./bridge/session_preferences.js");
import test, { mock } from "node:test";
const { refreshSessionEffort } = await import("./bridge/effort.js");
import assert from "node:assert/strict";
import { query } from "@anthropic-ai/claude-agent-sdk";
const { AsyncQueue, buildApiRetryUpdate, buildRateLimitUpdate, buildRewindConversationPlan, buildQueryOptions, buildPromptUserMessage, resolveClaudeCodeSpawnCommand, generatePersistedSessionTitle, buildSessionMutationOptions, createToolCall, handleTaskSystemMessage, handleSdkMessage, mapRewindFilesResult, mapAvailableAgents, mapAvailableModels, mapSessionMessagesToUpdates, mapSdkSessions, mapMcpServerStatusConfig, parseCommandEnvelope, rewindTargetsFromSessionMessages, updateAvailableCommands, handleReloadPluginsCommand, scheduleBridgeCommand } = await import("./bridge.js");
import type { SessionState } from "./bridge.js";
import {
  getSessionMessages,
  type Options,
  type SessionMessage,
} from "@anthropic-ai/claude-agent-sdk";
const { refreshSupportedModesForSession } = await import("./bridge/commands.js");
const { emitMcpSnapshotEvent, handleMcpSetServersCommand, startMcpAuthSnapshotMonitor } = await import("./bridge/mcp.js");
const { awaitSessionInitialization, beginSessionClose, closeAllSessions, closeSession, commitDeferredSession, refreshCurrentModel, resolveCurrentModel, sessions, startSessionTasks, updateSessionId } = await import("./bridge/session_lifecycle.js");
const { dispatchSideQuestion, closeSideQuestions } = await import("./bridge/side_questions.js");
const { emitAuthRequired } = await import(
  "./bridge/error_classification.js"
);
const { emitConnectEvent, emitSessionUpdate, replaceProtocolEventWriter, writeEvent } = await import("./bridge/events.js");
const { emitToolCall, emitToolProgressUpdate, emitToolResultUpdate } = await import(
  "./bridge/tool_calls.js"
);
const { requestAskUserQuestionAnswers } = await import("./bridge/user_interaction.js");
const { handleInteractionCommand } = await import("./bridge/command_interactions.js");
const { handleResultMessage } = await import("./bridge/message_handlers.js");

const { handleMcpCommand } = await import("./bridge/command_mcp.js");
const { handleSessionControlCommand } = await import("./bridge/command_session_control.js");
const { BridgeCommandScheduler } = await import("./bridge/command_scheduler.js");
const { handleSessionDataCommand, normalizeStructuredUsage } = await import(
  "./bridge/command_session_data.js"
);
const { handleLifecycleCommand } = await import("./bridge/command_lifecycle.js");

const {emitAvailableAgentsIfChanged,refreshAvailableAgents}=await import("./bridge/agents.js");

bridgeSdkLoader.deregister();
const bridgeFixtureTimers = { setTimeout, clearTimeout };

const STARTUP_REASONS = {
  org_pin_api_key_conflict: true,
  provider_not_allowed: true,
  org_verify_failed: true,
  org_pin_mismatch: true,
  managed_settings_invalid: true,
  remote_settings_required_unavailable: true,
  org_config_required_unavailable: true,
  org_config_refused: true,
  gateway_signin_required: true,
  gateway_access_denied: true,
  proxy_invalid: true,
  temp_dir_unusable: true,
  cwd_unavailable: true,
  shell_tool_missing: true,
  session_held_by_background: true,
  worktree_resume_refused: true,
  worktree_unverified: true,
  cli_version_too_old: true,
  bypass_root: true,
} satisfies Record<import("@anthropic-ai/claude-agent-sdk").SDKStartupFailureReason, boolean>;

test("startup message dispatch retains each structured reason and exclusively owns the correlated failure", async () => {
  for (const reason of [...Object.keys(STARTUP_REASONS), "future_startup_reason"]) {
    const session = makeSessionState();
    session.connected = false;
    session.connectRequestId = `connect-${reason}`;
    session.lastAssistantError = "authentication_failed";
    const events = await captureBridgeEventsAsync(async () => {
      const result = {
        type: "result",
        subtype: "error_during_execution",
        startup_failure_reason: reason,
        errors: ["Safe original SDK guidance"],
      };
      handleSdkMessage(
        session,
        result as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage,
      );
      handleSdkMessage(
        session,
        result as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage,
      );
      handleSdkMessage(session, {
        type: "result",
        subtype: "error_during_execution",
        errors: ["Please log in late"],
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    });
    const afterFailure = events;
    assert.equal(afterFailure.length, 1);
    assert.equal(afterFailure[0]?.event, "connection_failed");
    assert.equal(afterFailure[0]?.request_id, `connect-${reason}`);
    assert.deepEqual(afterFailure[0]?.startup_failure, {
      reason,
      errors: ["Safe original SDK guidance"],
    });
    assert.ok(String(afterFailure[0]?.message).trim());
    // Failure blocks every subsequent terminal/auth outcome.
    const failureIndex = events.findIndex((event) => event.event === "connection_failed");
    assert.equal(events.slice(failureIndex + 1).length, 0);
  }
  const ordinary = [
    { subtype: "error_during_execution" },
    { subtype: "error_during_execution", startup_failure_reason: null },
    { subtype: "error_during_execution", startup_failure_reason: 1 },
    { subtype: "error_during_execution", startup_failure_reason: "" },
    { subtype: "error_max_turns", startup_failure_reason: "proxy_invalid" },
    { subtype: "success", startup_failure_reason: "proxy_invalid" },
  ];
  for (const row of ordinary) {
    const session = makeSessionState();
    const events = await captureBridgeEventsAsync(async () =>
      handleSdkMessage(session, {
        type: "result",
        ...row,
        errors: row.subtype === "success" ? [] : ["ORDINARY-CAUSE"],
        is_error: false,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    assert.equal(
      events.some((event) => event.event === "connection_failed"),
      false,
    );
    const terminal = events.filter(
      (event) => event.event === "turn_error" || event.event === "turn_complete",
    );
    assert.equal(terminal.length, 1);
    assert.equal(terminal[0]?.event, row.subtype === "success" ? "turn_complete" : "turn_error");
    assert.equal(terminal[0]?.session_id, session.sessionId);
    if (row.subtype !== "success")
      assert.ok(String(terminal[0]?.message).includes("ORDINARY-CAUSE"));
  }
});
test("startup dispatch emits useful bounded redacted details on the wire and in detailed diagnostics", async () => {
  const { spawnSync } = await import("node:child_process");
  const { mkdtempSync, rmSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const profile = mkdtempSync(join(tmpdir(), "startup-privacy-"));
  try {
    const reason = `future_reason password=REASON_PRIVATE ${"safe reason ".repeat(150)}`;
    const errors = [
      "Short safe ENOENT cause",
      'Safe proxy cause password="DETAIL_PRIVATE" fix proxy',
      `Safe long cause ${"safe detail ".repeat(160)}`,
      `${"p".repeat(1000)} password="ZXPRIVATE ${"secret tail ".repeat(40)}"`,
      "Fourth distinct guidance",
      "Fifth distinct guidance",
      "Sixth distinct guidance",
      "Seventh distinct guidance",
      "TAIL-OUTSIDE-DETAIL-BUDGET",
    ];
    const cases = [errors, undefined, null, "stderr", {}, ["safe but malformed", 1], []];
    const child = spawnSync(process.execPath, ["--input-type=module", "-"], {
      input: `import {handleSdkMessage} from ${JSON.stringify(new URL("./bridge/message_handlers.js", import.meta.url).href)};const cases=${JSON.stringify(cases)};cases[1]=undefined;for(const [index,errors] of cases.entries()){const session={sessionId:'privacy-session-'+index,connected:false,connectRequestId:'privacy-request-'+index};handleSdkMessage(session,{type:'result',subtype:'error_during_execution',startup_failure_reason:${JSON.stringify(reason)},errors});}`,
      env: {
        ...process.env,
        CLAUDE_RS_BRIDGE_DIAGNOSTICS: "1",
        CLAUDE_CONFIG_DIR: profile,
        HOME: profile,
        USERPROFILE: profile,
      },
      encoding: "utf8",
      windowsHide: true,
      timeout: 15_000,
      maxBuffer: 2 * 1024 * 1024,
    });
    assert.equal(child.error, undefined);
    assert.equal(child.status, 0, child.stderr);
    const wire = child.stdout
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line));
    const logs = child.stderr
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line))
      .filter((record) => record.event_name === "sdk_startup_failed");
    assert.equal(wire.length, cases.length);
    assert.equal(logs.length, cases.length);
    for (const [index, event] of wire.entries()) {
      assert.equal(event.event, "connection_failed");
      assert.equal(event.request_id, `privacy-request-${index}`);
      assert.equal(event.startup_failure.reason, reason);
      assert.ok(String(event.message).trim());
      const details = event.startup_failure.errors as string[];
      const log = logs.find((record) => record.request_id === `privacy-request-${index}`);
      assert.ok(log);
      assert.equal(log.session_id, `privacy-session-${index}`);
      assert.deepEqual(log.fields.startup_failure_errors, details);
      assert.equal(log.fields.errors_valid, index === 0 || index === 6);
      assert.ok(String(log.fields.startup_failure_reason).startsWith("future_reason"));
      assert.ok(String(log.fields.startup_failure_reason).length <= 1024);
      assert.doesNotMatch(String(log.fields.startup_failure_reason), /REASON_PRIVATE/);
      if (index === 0) {
        assert.equal(details.length, 8);
        assert.equal(details[0], "Short safe ENOENT cause");
        assert.equal(details[1], "Safe proxy cause password=[redacted] fix proxy");
        assert.ok(details[2]?.startsWith("Safe long cause"));
        assert.ok(details.every((detail) => detail.length <= 1024));
        assert.deepEqual(details.slice(4), [
          "Fourth distinct guidance",
          "Fifth distinct guidance",
          "Sixth distinct guidance",
          "Seventh distinct guidance",
        ]);
        assert.doesNotMatch(JSON.stringify(details), /DETAIL_PRIVATE|ZXPRIVATE|TAIL-OUTSIDE/);
        assert.doesNotMatch(JSON.stringify(log.fields), /DETAIL_PRIVATE|ZXPRIVATE|TAIL-OUTSIDE/);
      } else assert.deepEqual(details, []);
    }
  } finally {
    rmSync(profile, { recursive: true, force: true });
  }
});
test("invalid-request dispatch preserves its cause without classifying login guidance as authentication", async () => {
  const session = makeSessionState();
  const invalid = await captureBridgeEventsAsync(async () => {
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "invalid-assistant",
      parent_tool_use_id: null,
      error: "invalid_request",
      message: { role: "assistant", content: [] },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "result",
      subtype: "error_during_execution",
      errors: ["ENOSPC while saving refreshed credentials. Please log in with /login."],
      api_error_status: null,
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });
  assert.equal(
    invalid.some((event) => event.event === "auth_required"),
    false,
  );
  const errors = invalid.filter((event) => event.event === "turn_error");
  assert.equal(errors.length, 1);
  assert.equal(errors[0]?.session_id, session.sessionId);
  assert.equal(errors[0]?.error_kind, "other");
  assert.equal(errors[0]?.assistant_error, "invalid_request");
  assert.match(String(errors[0]?.message), /ENOSPC/);
  assert.equal(Object.hasOwn(errors[0] ?? {}, "api_error_status"), false);
  const recovered = await captureBridgeEventsAsync(async () =>
    handleSdkMessage(session, {
      type: "result",
      subtype: "error_during_execution",
      errors: ["Session expired. Please log in."],
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
  );
  assert.equal(recovered.filter((event) => event.event === "auth_required").length, 1);
  assert.equal(
    recovered.find((event) => event.event === "turn_error")?.error_kind,
    "auth_required",
  );
  const genuine = makeSessionState();
  const auth = await captureBridgeEventsAsync(async () => {
    handleSdkMessage(genuine, {
      type: "assistant",
      uuid: "auth-assistant",
      parent_tool_use_id: null,
      error: "authentication_failed",
      message: { role: "assistant", content: [] },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(genuine, {
      type: "result",
      subtype: "error_during_execution",
      errors: ["ACTUAL-AUTH-FAILURE"],
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });
  assert.equal(auth.filter((event) => event.event === "auth_required").length, 1);
  assert.equal(auth.find((event) => event.event === "turn_error")?.error_kind, "auth_required");
});

function bridgeDeferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise=new Promise<T>((accept,refuse)=>{resolve=accept;reject=refuse;});
  return {promise,resolve,reject};
}

async function boundedBridgeTask<T>(operation: Promise<T>): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      operation,
      new Promise<never>((_, reject) => {
        timer = bridgeFixtureTimers.setTimeout(
          () => reject(new Error("Bridge fixture task did not settle")),
          5000,
        );
      }),
    ]);
  } finally {
    if (timer) bridgeFixtureTimers.clearTimeout(timer);
  }
}

test("startup failure exclusively survives queued initialization rejection followed by stream EOF or throw", async () => {
  for (const rejectFirst of [false, true]) {
    for (const streamThrows of [false, true]) {
      const session = makeSessionState();
      session.connected = false;
      session.connectRequestId = "startup-race-request";
      const initialized = bridgeDeferred<never>(),
        published = bridgeDeferred<void>(),
        end = bridgeDeferred<void>();
      session.query = {
        initializationResult: () => initialized.promise,
        async *[Symbol.asyncIterator]() {
          yield {
            type: "result",
            subtype: "error_during_execution",
            startup_failure_reason: "gateway_signin_required",
            errors: ['Safe gateway cause password="RACE-PRIVATE" retry sign-in'],
          };
          await end.promise;
          if (streamThrows) throw new Error("generic process exit");
        },
      } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
      const events = await captureBridgeEventsAsync(
        async () => {
          try {
            startSessionTasks(session, "startup-race-request");
            if (rejectFirst) initialized.reject(new Error("generic initialization rejection"));
            await boundedBridgeTask(published.promise);
            if (!rejectFirst) initialized.reject(new Error("generic initialization rejection"));
            end.resolve();
            await boundedBridgeTask(
              Promise.all([session.initializationTask, session.queryConsumerTask]),
            );
          } finally {
            initialized.reject(new Error("fixture cleanup"));
            end.resolve();
            session.input.close();
            await boundedBridgeTask(
              Promise.allSettled([session.initializationTask, session.queryConsumerTask]),
            );
          }
        },
        (event) => {
          if (event.event === "connection_failed") published.resolve();
        },
      );
      assert.equal(events.length, 1);
      assert.equal(events[0]?.event, "connection_failed");
      assert.equal(events[0]?.request_id, "startup-race-request");
      assert.deepEqual(events[0]?.startup_failure, {
        reason: "gateway_signin_required",
        errors: ["Safe gateway cause password=[redacted] retry sign-in"],
      });
      assert.ok(String(events[0]?.message).trim());
    }
  }
});
test("late initialization success cannot reconnect or commit after startup failure before or during metadata reads", async () => {
  for (const deferred of [false, true]) {
    for (const pause of ["before-initialize", "model-read"]) {
      const session = makeSessionState();
      session.connected = false;
      session.deferConnect = deferred;
      session.connectRequestId = "late-success-request";
      const initialized = bridgeDeferred<Record<string, unknown>>(),
        published = bridgeDeferred<void>(),
        readEntered = bridgeDeferred<void>(),
        releaseRead = bridgeDeferred<void>();
      const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKMessage>();
      let reads = 0;
      const result = {
        models: [],
        agents: [{ name: "POST-FAILURE-AGENT", description: "must not publish" }],
        commands: [
          { name: "post-failure-command", description: "must not publish", argumentHint: "" },
        ],
        account: {},
      };
      session.query = {
        initializationResult: () => initialized.promise,
        getSettings: async () => {
          reads++;
          if (pause === "model-read" && reads === 2) {
            readEntered.resolve();
            await releaseRead.promise;
          }
          return {
            applied: {
              model: "haiku",
              ultracodeAvailable: true,
              ultracodeRequested: false,
              ultracode: false,
            },
          };
        },
        [Symbol.asyncIterator]: () => input[Symbol.asyncIterator](),
      } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
      const events = await captureBridgeEventsAsync(
        async () => {
          try {
            startSessionTasks(session, "late-success-request");
            if (pause === "model-read") {
              initialized.resolve(result);
              await boundedBridgeTask(readEntered.promise);
            }
            input.enqueue({
              type: "result",
              subtype: "error_during_execution",
              startup_failure_reason: "proxy_invalid",
              errors: ["SAFE-ORIGINAL-PROXY-CAUSE"],
            } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
            await boundedBridgeTask(published.promise);
            initialized.resolve(result);
            releaseRead.resolve();
            input.close();
            await boundedBridgeTask(
              Promise.all([session.initializationTask, session.queryConsumerTask]),
            );
            if (deferred) assert.throws(() => commitDeferredSession(session), /not ready/i);
          } finally {
            initialized.resolve(result);
            releaseRead.resolve();
            input.close();
            session.input.close();
            await boundedBridgeTask(
              Promise.allSettled([session.initializationTask, session.queryConsumerTask]),
            );
          }
        },
        (event) => {
          if (event.event === "connection_failed") published.resolve();
        },
      );
      assert.equal(events.length, 1);
      assert.equal(events[0]?.event, "connection_failed");
      assert.equal(events[0]?.request_id, "late-success-request");
      assert.deepEqual(events[0]?.startup_failure, {
        reason: "proxy_invalid",
        errors: ["SAFE-ORIGINAL-PROXY-CAUSE"],
      });
    }
  }
});

function makeSessionState(): SessionState {
  const input = new AsyncQueue<
    import("@anthropic-ai/claude-agent-sdk").SDKUserMessage
  >();
  return {
    sessionId: "session-1",
    cwd: "C:/work",
    resolvedRuntimeModelId: "haiku",
    availableModels: [],
    mode: null,
    supportedModeIds: [],
    runtimeUnavailableModeIds: [],
    supportsBypassPermissionsMode: false,
    fastModeState: "off",
    query: {} as import("@anthropic-ai/claude-agent-sdk").Query,
    input,
    connected: true,
    closing: false,
    connectEvent: "connected",
    toolCalls: new Map(),
    tasksById: new Map(),
    taskOrder: [],
    taskToolUseIds: new Map(),
    taskIdsByToolUseId: new Map(),
    pendingPermissions: new Map(),
    pendingQuestions: new Map(),
    pendingUserDialogs: new Map(),
    pendingElicitations: new Map(),
    informationalDedupKeys: new Set(),
    knownConnectedMcpServers: new Set(),
    mcpStatusRevalidatedAt: new Map(),
    mcpAuthMonitors: new Map(),
    hiddenToolUseIds: new Set(),
    authHintSent: false,
  };
}

// The SDK assigns a fresh wrapper UUID per frame; message_start.message.id
// scopes its ordered content-block boundaries to one API response.
function activityStream(session: SessionState, event: Record<string, unknown>, parent: string | null = null): void {
  handleSdkMessage(session, {
    type: "stream_event", uuid: crypto.randomUUID(), session_id: session.sessionId,
    parent_tool_use_id: parent, event,
  } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
}

function activityCheckpoint(
  session: SessionState,
  run: () => void,
  expected: Array<"started" | "thinking" | "working">,
): Array<Record<string, unknown>> {
  const events = captureBridgeEvents(run);
  const actual = events.flatMap((event) => {
    const update = event.update as Record<string, unknown> | undefined;
    return update?.type === "agent_activity_update" || update?.type === "agent_response_started"
      ? [
          {
            session_id: event.session_id,
            type: update.type,
            ...(update.type === "agent_activity_update" ? { phase: update.phase } : {}),
          },
        ]
      : [];
  });
  assert.deepEqual(
    actual,
    expected.map((phase) => ({
      session_id: session.sessionId,
      type: phase === "started" ? "agent_response_started" : "agent_activity_update",
      ...(phase === "started" ? {} : { phase }),
    })),
  );
  return events;
}

function activityPhases(events: ReturnType<typeof captureBridgeEvents>): string[] {
  return events.flatMap(event => {
    const update = event.update as Record<string, unknown> | undefined;
    return update?.type === "agent_activity_update" ? [String(update.phase)] : [];
  });
}

test("each main response activates its output owner before exact streamed text and ignores child activation", () => {
  const session = makeSessionState();
  activityCheckpoint(session, () => {
    activityStream(session, { type: "message_start", message: { id: "child" } }, "parent-agent");
    activityStream(session, { type: "message_stop" }, "parent-agent");
  }, []);
  for (const [id, pieces, expected] of [
    ["response-A", ["First ", "answer\n exact  "], "First answer\n exact  "],
    ["response-B", ["Second", " answer"], "Second answer"],
  ] as const) {
    const events = captureBridgeEvents(() => {
      activityStream(session, { type: "message_start", message: { id } });
      activityStream(session, { type: "message_start", message: { id } });
      activityStream(session, {
        type: "content_block_start",
        index: 0,
        content_block: { type: "text", text: "" },
      });
      for (const text of pieces)
        activityStream(session, {
          type: "content_block_delta",
          index: 0,
          delta: { type: "text_delta", text },
        });
      activityStream(session, { type: "message_stop" });
    });
    const updates = events.filter((event) => event.event === "session_update");
    assert.ok(updates.every((event) => event.session_id === session.sessionId));
    const starts = updates.filter(
      (event) => (event.update as Record<string, unknown>).type === "agent_response_started",
    );
    assert.equal(starts.length, 1);
    const texts = updates.filter(
      (event) => (event.update as Record<string, unknown>).type === "agent_message_chunk",
    );
    assert.ok(texts.length > 0);
    const firstStart = starts[0],
      firstText = texts[0];
    assert.ok(firstStart);
    assert.ok(firstText);
    assert.ok(events.indexOf(firstStart) < events.indexOf(firstText));
    assert.equal(
      texts.map((event) => (event.update as { content: { text: string } }).content.text).join(""),
      expected,
    );
    assert.deepEqual(activityPhases(events), []);
    assert.equal(
      events.some((event) => event.event === "user_message_started"),
      false,
    );
  }
});
test("live thinking is owned by its main block boundaries while estimates and private reasoning never become display content", () => {
  for (const block of [
    { type: "thinking", thinking: "" },
    { type: "thinking", thinking: "PRIVATE-THINKING-START" },
    { type: "thinking" },
    { type: "redacted_thinking", data: "PRIVATE-REDACTED" },
  ]) {
    const session = makeSessionState();
    const all: Array<Record<string, unknown>> = [];
    const step = (run: () => void, expected: Array<"started" | "thinking" | "working">) =>
      all.push(...activityCheckpoint(session, run, expected));
    step(
      () => activityStream(session, { type: "message_start", message: { id: "main" } }),
      ["started"],
    );
    step(
      () =>
        activityStream(session, { type: "content_block_start", index: 0, content_block: block }),
      ["thinking"],
    );
    for (const delta of [
      { type: "thinking_delta", thinking: "" },
      { type: "thinking_delta", thinking: "PRIVATE-THINKING-DELTA" },
      { type: "signature_delta", signature: "PRIVATE-SIGNATURE" },
    ])
      step(() => activityStream(session, { type: "content_block_delta", index: 0, delta }), []);
    for (const attribution of [
      {},
      { user_message_uuid: "typed-user" },
      { parent_tool_use_id: "child", user_message_uuid: "typed-user" },
    ])
      step(
        () =>
          handleSdkMessage(session, {
            type: "system",
            subtype: "thinking_tokens",
            estimated_tokens: 120,
            estimated_tokens_delta: 20,
            ...attribution,
          } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
        [],
      );
    step(() => {
      activityStream(session, { type: "message_start", message: { id: "child" } }, "child-tool");
      activityStream(
        session,
        {
          type: "content_block_start",
          index: 0,
          content_block: { type: "thinking", thinking: "PRIVATE-CHILD" },
        },
        "child-tool",
      );
      activityStream(session, { type: "content_block_stop", index: 0 }, "child-tool");
      activityStream(session, { type: "message_stop" }, "child-tool");
    }, []);
    step(
      () =>
        activityStream(session, { type: "content_block_start", index: 0, content_block: block }),
      [],
    );
    step(() => activityStream(session, { type: "content_block_stop", index: 9 }), []);
    step(() => activityStream(session, { type: "content_block_stop", index: 0 }), ["working"]);
    step(() => activityStream(session, { type: "content_block_stop", index: 0 }), []);
    step(() => {
      activityStream(session, {
        type: "content_block_start",
        index: 1,
        content_block: { type: "text", text: "" },
      });
      activityStream(session, {
        type: "content_block_delta",
        index: 1,
        delta: { type: "text_delta", text: "PUBLIC-ANSWER" },
      });
      activityStream(session, { type: "content_block_stop", index: 1 });
      activityStream(session, {
        type: "content_block_start",
        index: 2,
        content_block: {
          type: "tool_use",
          id: "public-read",
          name: "Read",
          input: { file_path: "public.txt" },
        },
      });
    }, []);
    step(() => activityStream(session, { type: "message_stop" }), []);
    assert.ok(
      all.some(
        (event) =>
          (event.update as { content?: { text?: string } })?.content?.text === "PUBLIC-ANSWER",
      ),
    );
    assert.equal(publishedTool(all, session.sessionId, "public-read").status, "in_progress");
    assert.doesNotMatch(
      JSON.stringify(all),
      /PRIVATE-THINKING|PRIVATE-REDACTED|PRIVATE-SIGNATURE|PRIVATE-CHILD/,
    );
  }
  const idle = makeSessionState();
  const events = activityCheckpoint(idle, () => {
    for (const attribution of [
      {},
      { user_message_uuid: "idle-user" },
      { parent_tool_use_id: "child", user_message_uuid: "idle-user" },
    ])
      handleSdkMessage(idle, {
        type: "system",
        subtype: "thinking_tokens",
        estimated_tokens: 300,
        ...attribution,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(idle, {
      type: "assistant",
      parent_tool_use_id: null,
      message: {
        id: "completed-thought",
        stop_reason: "end_turn",
        content: [{ type: "thinking", thinking: "PRIVATE-COMPLETED-THOUGHT" }],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  }, []);
  assert.doesNotMatch(JSON.stringify(events), /PRIVATE-COMPLETED-THOUGHT/);
});

test("a replacement response clears the old phase and only its own main terminal frame ends new thinking", () => {
  const session = makeSessionState();
  activityCheckpoint(session, () => {
    activityStream(session, { type: "message_start", message: { id: "A" } });
    activityStream(session, {
      type: "content_block_start",
      index: 0,
      content_block: { type: "thinking" },
    });
  }, ["started", "thinking"]);
  activityCheckpoint(
    session,
    () => activityStream(session, { type: "message_start", message: { id: "B" } }),
    ["working", "started"],
  );
  activityCheckpoint(
    session,
    () =>
      activityStream(session, {
        type: "content_block_start",
        index: 1,
        content_block: { type: "thinking" },
      }),
    ["thinking"],
  );
  for (const [id, parent] of [
    ["A", null],
    ["B", "child-tool"],
  ])
    activityCheckpoint(
      session,
      () =>
        handleSdkMessage(session, {
          type: "assistant",
          parent_tool_use_id: parent,
          message: { id, content: [], stop_reason: "end_turn" },
        } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
      [],
    );
  activityCheckpoint(
    session,
    () => activityStream(session, { type: "content_block_stop", index: 0 }),
    [],
  );
  activityCheckpoint(
    session,
    () =>
      handleSdkMessage(session, {
        type: "assistant",
        parent_tool_use_id: null,
        message: { id: "B", content: [], stop_reason: "end_turn" },
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    ["working"],
  );
  activityCheckpoint(session, () => activityStream(session, { type: "message_stop" }), []);
});
test("per-block assistant frames keep the response open for subsequent thinking blocks", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    activityStream(session, { type: "message_start", message: { id: "msg-blocks" } });
    activityStream(session, { type: "content_block_start", index: 0, content_block: { type: "thinking" } });
    activityStream(session, { type: "content_block_stop", index: 0 });
    handleSdkMessage(session, { type: "assistant", parent_tool_use_id: null, message: { id: "msg-blocks", stop_reason: null, content: [{ type: "thinking", thinking: "completed block" }] } } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    assert.equal(session.mainAgentResponse?.messageId, "msg-blocks");
    activityStream(session, { type: "content_block_start", index: 1, content_block: { type: "thinking" } });
    handleSdkMessage(session, { type: "assistant", parent_tool_use_id: null, aborted: true, message: { id: "msg-blocks", stop_reason: null, content: [] } } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });
  assert.deepEqual(activityPhases(events), ["thinking", "working", "thinking", "working"]);
  assert.equal(session.mainAgentResponse, undefined);
});

test("each invalidation immediately clears its own thinking and permits only legitimate later response activity", async () => {
  const sdk = (session: SessionState, frame: Record<string, unknown>) =>
    handleSdkMessage(
      session,
      frame as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage,
    );
  const rows = [
    {
      name: "stream-error",
      run: (session: SessionState) =>
        activityStream(session, { type: "error", error: { type: "overloaded_error" } }),
    },
    {
      name: "retry",
      run: (session: SessionState) =>
        sdk(session, {
          type: "system",
          subtype: "api_retry",
          attempt: 1,
          max_retries: 3,
          retry_delay_ms: 100,
        }),
    },
    {
      name: "fallback",
      run: (session: SessionState) =>
        sdk(session, {
          type: "system",
          subtype: "model_fallback",
          retracted_message_uuids: ["retracted"],
          scope: "session",
        }),
    },
    {
      name: "reset",
      run: (session: SessionState) =>
        sdk(session, {
          type: "conversation_reset",
          new_conversation_id: `reset-${crypto.randomUUID()}`,
        }),
    },
    {
      name: "success-result",
      run: (session: SessionState) =>
        sdk(session, { type: "result", subtype: "success", result: "", is_error: false }),
    },
    {
      name: "failed-result",
      run: (session: SessionState) =>
        sdk(session, {
          type: "result",
          subtype: "error_during_execution",
          errors: ["SAFE-RESULT-FAILURE"],
        }),
    },
    { name: "close", run: (session: SessionState) => beginSessionClose(session) },
    {
      name: "replace-id",
      run: (session: SessionState) =>
        updateSessionId(session, `replacement-${crypto.randomUUID()}`),
    },
  ];
  for (const row of rows) {
    const session = makeSessionState();
    const oldId = session.sessionId;
    const resetSettled = bridgeDeferred<void>();
    const resetTypes = new Set(["ultracode_update", "config_option_update"]);
    session.query = {
      getSettings: async () => ({
        applied: {
          model: "haiku",
          effort: "medium",
          ultracodeAvailable: true,
          ultracodeRequested: false,
          ultracode: false,
        },
      }),
    } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
    try {
      await captureBridgeEventsAsync(
        async () => {
          activityCheckpoint(session, () => {
            activityStream(session, { type: "message_start", message: { id: row.name } });
            activityStream(session, {
              type: "content_block_start",
              index: 0,
              content_block: { type: "thinking" },
            });
          }, ["started", "thinking"]);
          for (const subtype of ["api_retry", "model_fallback"])
            activityCheckpoint(
              session,
              () =>
                sdk(session, {
                  type: "system",
                  subtype,
                  parent_tool_use_id: "child-task",
                  attempt: 1,
                  max_retries: 3,
                  retry_delay_ms: 100,
                  retracted_message_uuids: ["child-only"],
                  scope: "session",
                }),
              [],
            );
          activityCheckpoint(session, () => updateSessionId(session, session.sessionId), []);
          const invalidation = captureBridgeEvents(() => row.run(session));
          const phases = invalidation.flatMap((event) => {
            const update = event.update as Record<string, unknown> | undefined;
            return update?.type === "agent_activity_update"
              ? [{ session_id: event.session_id, phase: update.phase }]
              : [];
          });
          assert.deepEqual(phases, [{ session_id: oldId, phase: "working" }], row.name);
          activityCheckpoint(
            session,
            () => activityStream(session, { type: "content_block_stop", index: 0 }),
            [],
          );
          activityCheckpoint(
            session,
            () => {
              activityStream(session, {
                type: "message_start",
                message: { id: `new-${row.name}` },
              });
              activityStream(session, {
                type: "content_block_start",
                index: 2,
                content_block: { type: "thinking" },
              });
            },
            row.name === "close" ? [] : ["started", "thinking"],
          );
          if (row.name === "reset") await boundedBridgeTask(resetSettled.promise);
        },
        (event) => {
          const update = event.update as Record<string, unknown> | undefined;
          if (
            row.name === "reset" &&
            event.session_id === session.sessionId &&
            typeof update?.type === "string"
          ) {
            resetTypes.delete(update.type);
            if (resetTypes.size === 0) resetSettled.resolve();
          }
        },
      );
    } finally {
      session.input.close();
      for (const id of [oldId, session.sessionId])
        if (sessions.get(id) === session) sessions.delete(id);
      await settleBridgeSdkReads();
    }
  }
});
test("query EOF or throw clears delivered thinking without a terminal frame or manual invalidation", async () => {
  for (const throws of [false, true]) {
    const session = makeSessionState();
    const frames = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKMessage>();
    const thinking = bridgeDeferred<void>();
    session.query = {
      initializationResult: async () => ({
        models: [],
        agents: [],
        commands: [],
        account: { apiKeySource: "user" },
      }),
      getSettings: async () => ({
        applied: {
          model: "haiku",
          effort: "medium",
          ultracodeAvailable: true,
          ultracodeRequested: false,
          ultracode: false,
        },
      }),
      async *[Symbol.asyncIterator]() {
        for await (const frame of frames) yield frame;
        if (throws) throw new Error("OWNED-STREAM-CAUSE");
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
    const events = await captureBridgeEventsAsync(
      async () => {
        try {
          startSessionTasks(session);
          await boundedBridgeTask(Promise.resolve(session.initializationTask));
          frames.enqueue({
            type: "stream_event",
            uuid: "response-start",
            parent_tool_use_id: null,
            event: { type: "message_start", message: { id: "owned-response" } },
          } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
          frames.enqueue({
            type: "stream_event",
            uuid: "thinking-start",
            parent_tool_use_id: null,
            event: { type: "content_block_start", index: 0, content_block: { type: "thinking" } },
          } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
          await boundedBridgeTask(thinking.promise);
          frames.close();
          await boundedBridgeTask(Promise.resolve(session.queryConsumerTask));
        } finally {
          frames.close();
          session.input.close();
          await boundedBridgeTask(
            Promise.allSettled([session.initializationTask, session.queryConsumerTask]),
          );
        }
      },
      (event) => {
        const update = event.update as Record<string, unknown> | undefined;
        if (
          event.session_id === session.sessionId &&
          update?.type === "agent_activity_update" &&
          update.phase === "thinking"
        )
          thinking.resolve();
      },
    );
    const phases = events.flatMap((event) => {
      const update = event.update as Record<string, unknown> | undefined;
      return update?.type === "agent_activity_update"
        ? [{ session_id: event.session_id, phase: update.phase }]
        : [];
    });
    assert.deepEqual(phases, [
      { session_id: session.sessionId, phase: "thinking" },
      { session_id: session.sessionId, phase: "working" },
    ]);
    if (throws) {
      const failures = events.filter((event) => event.event === "connection_failed");
      assert.equal(failures.length, 1);
      assert.ok(String(failures[0]?.message).includes("OWNED-STREAM-CAUSE"));
    }
  }
});
function promptControlDeps(): Parameters<typeof handleSessionControlCommand>[2] {
  return {
    buildPromptUserMessage,
    applySessionEffort,
    applySessionAgent,
    applySessionFastMode,
    emitAgentConfigOptionUpdate,
    handleReloadPluginsCommand,
  };
}

function eventLoopCheckpoint(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

test("session close shuts input and query then waits independently for initialization, consumption and monitor settlement", async () => {
  for (const held of ["initialization", "consumer"]) {
    const session = makeSessionState();
    const initialization = bridgeDeferred<void>(),
      consumer = bridgeDeferred<void>(),
      nativeClosed = bridgeDeferred<void>();
    const order: string[] = [];
    session.initializationTask = initialization.promise.then(() => {
      order.push("initialized");
    });
    session.queryConsumerTask = consumer.promise.then(() => {
      order.push("consumed");
    });
    session.query = {
      close: () => {
        order.push("query-closed");
        nativeClosed.resolve();
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
    let closed = false;
    const closing = closeSession(session).then(() => {
      closed = true;
      order.push("closed");
    });
    try {
      await boundedBridgeTask(nativeClosed.promise);
      assert.equal(
        session.input.enqueue({
          type: "user",
          session_id: session.sessionId,
          parent_tool_use_id: null,
          message: { role: "user", content: "late input" },
        }),
        false,
      );
      assert.deepEqual(await nextPromptItem(session.input[Symbol.asyncIterator]()), {
        done: true,
        value: undefined,
      });
      if (held === "initialization") consumer.resolve();
      else initialization.resolve();
      await eventLoopCheckpoint();
      assert.equal(closed, false, `${held} is still owned`);
      initialization.resolve();
      consumer.resolve();
      await boundedBridgeTask(closing);
      assert.ok(order.indexOf("closed") > order.indexOf("initialized"));
      assert.ok(order.indexOf("closed") > order.indexOf("consumed"));
      assert.ok(order.indexOf("query-closed") < order.indexOf("closed"));
    } finally {
      initialization.resolve();
      consumer.resolve();
      session.input.close();
      await boundedBridgeTask(closing);
    }
  }
  const delayed = makeSessionState();
  const delayEntries: string[] = [];
  const bothEntered = bridgeDeferred<void>();
  let statusCalls = 0;
  const order: string[] = [];
  delayed.query = {
    close: () => order.push("query-closed"),
    mcpServerStatus: async () => {
      statusCalls++;
      return [];
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
  const timing = (server: string) => ({
    sleep: async (_ms: number, signal: AbortSignal) => {
      delayEntries.push(server);
      if (delayEntries.length === 2) bothEntered.resolve();
      await new Promise<void>((resolve) => {
        if (signal.aborted) resolve();
        else signal.addEventListener("abort", () => resolve(), { once: true });
      });
    },
  });
  const first = startMcpAuthSnapshotMonitor(delayed, "docs", timing("docs")).then((result) => {
    order.push("docs-finished");
    return result;
  });
  const duplicate = startMcpAuthSnapshotMonitor(delayed, "docs", timing("duplicate"));
  const other = startMcpAuthSnapshotMonitor(delayed, "other", timing("other")).then((result) => {
    order.push("other-finished");
    return result;
  });
  try {
    await boundedBridgeTask(bothEntered.promise);
    assert.deepEqual(delayEntries, ["docs", "other"]);
    await boundedBridgeTask(closeSession(delayed));
    order.push("closed");
    assert.equal(statusCalls, 0);
    for (const result of await Promise.all([first, duplicate, other]))
      assert.deepEqual(result, { outcome: "cancelled", attempts: 0 });
    assert.ok(order.indexOf("closed") > order.indexOf("docs-finished"));
    assert.ok(order.indexOf("closed") > order.indexOf("other-finished"));
  } finally {
    await boundedBridgeTask(closeSession(delayed));
    await boundedBridgeTask(Promise.all([first, duplicate, other]));
  }
  const session = makeSessionState();
  const entered = bridgeDeferred<void>(),
    release = bridgeDeferred<void>();
  let reads = 0,
    closed = false;
  session.query = {
    close: () => {},
    mcpServerStatus: async () => {
      reads++;
      entered.resolve();
      await release.promise;
      return [{ name: "docs", status: "connected" }];
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
  const monitor = startMcpAuthSnapshotMonitor(session, "docs", { sleep: async () => {} });
  const events = await captureBridgeEventsAsync(async () => {
    let closing: Promise<void> | undefined;
    try {
      await boundedBridgeTask(entered.promise);
      const duplicate = startMcpAuthSnapshotMonitor(session, "docs", { sleep: async () => {} });
      closing = closeSession(session).then(() => {
        closed = true;
      });
      await eventLoopCheckpoint();
      assert.equal(closed, false, "the actual outstanding status read is still owned");
      assert.equal(reads, 1);
      release.resolve();
      await boundedBridgeTask(closing);
      assert.equal((await monitor).outcome, "cancelled");
      assert.equal((await duplicate).outcome, "cancelled");
    } finally {
      release.resolve();
      if (closing) await boundedBridgeTask(closing);
      else await boundedBridgeTask(closeSession(session));
      await boundedBridgeTask(monitor);
    }
  });
  assert.equal(
    events.some((event) => event.event === "mcp_snapshot"),
    false,
  );
});
test("global shutdown waits for both the registered replacement and its real detached handoff cleanup", async () => {
  for (const first of ["old", "replacement"]) {
    assert.equal(sessions.size, 0, "the shutdown fixture starts with an isolated registry");
    const old = makeSessionState(),
      replacement = makeSessionState();
    old.sessionId = `shutdown-${crypto.randomUUID()}`;
    replacement.sessionId = old.sessionId;
    replacement.connectEvent = "session_replaced";
    replacement.connectRequestId = "handoff-request";
    replacement.sessionsToCloseAfterConnect = [old];
    const oldDone = bridgeDeferred<void>(),
      replacementDone = bridgeDeferred<void>(),
      oldClosed = bridgeDeferred<void>(),
      replacementClosed = bridgeDeferred<void>();
    const order: string[] = [];
    old.queryConsumerTask = oldDone.promise.then(() => {
      order.push("old-consumer-finished");
    });
    replacement.queryConsumerTask = replacementDone.promise.then(() => {
      order.push("replacement-consumer-finished");
    });
    old.query = {
      close: () => {
        order.push("old-query-closed");
        oldClosed.resolve();
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
    replacement.query = {
      close: () => {
        order.push("replacement-query-closed");
        replacementClosed.resolve();
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
    let closed = false;
    let shutdown: Promise<void> | undefined;
    const events = await captureBridgeEventsAsync(async () => {
      try {
        sessions.set(replacement.sessionId, replacement);
        await boundedBridgeTask(emitConnectEvent(replacement));
        await boundedBridgeTask(oldClosed.promise);
        assert.equal(sessions.get(replacement.sessionId), replacement);
        shutdown = closeAllSessions({ reason: "test_shutdown" }).then(() => {
          closed = true;
          order.push("all-closed");
        });
        await boundedBridgeTask(replacementClosed.promise);
        if (first === "old") oldDone.resolve();
        else replacementDone.resolve();
        await eventLoopCheckpoint();
        assert.equal(
          closed,
          false,
          `shutdown still owns ${first === "old" ? "replacement" : "old"}`,
        );
        oldDone.resolve();
        replacementDone.resolve();
        await boundedBridgeTask(shutdown);
        assert.ok(order.indexOf("all-closed") > order.indexOf("old-consumer-finished"));
        assert.ok(order.indexOf("all-closed") > order.indexOf("replacement-consumer-finished"));
      } finally {
        oldDone.resolve();
        replacementDone.resolve();
        if (shutdown) await boundedBridgeTask(shutdown);
        await boundedBridgeTask(closeAllSessions({ reason: "fixture_cleanup" }));
        for (const id of [old.sessionId, replacement.sessionId])
          if (sessions.get(id) === old || sessions.get(id) === replacement) sessions.delete(id);
      }
    });
    const connected = events.filter((event) => event.event === "session_replaced");
    assert.equal(connected.length, 1);
    assert.equal(connected[0]?.request_id, "handoff-request");
    assert.equal(connected[0]?.session_id, replacement.sessionId);
    assert.ok(order.includes("old-query-closed"));
    assert.ok(order.includes("replacement-query-closed"));
  }
});

test("session close cancels real permission, plan, question and dialog callbacks before waiting for their consumer", async () => {
  const { spawnSync } = await import("node:child_process");
  const { fileURLToPath } = await import("node:url");
  const fixture = fileURLToPath(
    new URL("../../tests/fixtures/bridge-close-interactions.mjs", import.meta.url),
  );
  const child = spawnSync(process.execPath, [fixture], {
    encoding: "utf8",
    windowsHide: true,
    timeout: 25_000,
  });
  assert.equal(child.error, undefined);
  assert.equal(child.status, 0, child.stderr);
  const result = JSON.parse(child.stdout) as {
    outcomes: Array<{
      kind: string;
      cancelled: { behavior: string; result?: string };
      accepted: { behavior: string; result?: string };
      callbackBeforeConsumerClose: boolean;
      lateEvents: unknown[];
    }>;
  };
  assert.deepEqual(
    result.outcomes.map((row) => row.kind),
    ["permission", "plan", "question", "dialog"],
  );
  for (const row of result.outcomes) {
    assert.equal(row.cancelled.behavior, row.kind === "dialog" ? "cancelled" : "deny");
    if (row.kind === "dialog") assert.equal(row.cancelled.result, undefined);
    assert.equal(row.accepted.behavior, row.kind === "dialog" ? "completed" : "allow");
    assert.equal(row.callbackBeforeConsumerClose, true);
    assert.deepEqual(row.lateEvents, []);
  }
});
test("replacement handoff suppresses an old session's in-flight MCP monitor snapshot", async () => {
  const { spawnSync } = await import("node:child_process");
  const { fileURLToPath } = await import("node:url");
  const fixture = fileURLToPath(
    new URL("../../tests/fixtures/plugin-inventory-runtime.mjs", import.meta.url),
  );
  const child = spawnSync(process.execPath, [fixture, "handoff"], {
    encoding: "utf8",
    windowsHide: true,
    timeout: 15_000,
  });
  assert.equal(child.error, undefined, String(child.error));
  assert.equal(child.signal, null, child.stderr);
  assert.equal(child.status, 0, child.stderr);
  type Event = {
    event: string;
    session_id?: string;
    request_id?: string;
    source?: string;
    plugins?: unknown;
    servers?: Array<{ name: string; status: string; server_info?: unknown }>;
  };
  const output = JSON.parse(child.stdout) as {
    id: string;
    folder: string;
    replacementFolder: string;
    snapshots: Array<{
      kind: string;
      registered: boolean;
      oldClosed: boolean;
      oldOutcome: { outcome: string };
      freshOutcome: { outcome: string };
      carried: string[];
      events: Event[];
      lateEvents: Event[];
    }>;
    queries: Array<{ name: string; closed: boolean }>;
  };
  const handoff = output.snapshots.find((snapshot) => snapshot.kind === "handoff");
  assert.ok(handoff, "the actual same-key handoff completed");
  assert.equal(
    handoff.registered,
    true,
    "detaching the old object preserves the replacement registry entry",
  );
  assert.equal(handoff.oldClosed, true);
  assert.equal(handoff.oldOutcome.outcome, "cancelled");
  assert.equal(handoff.freshOutcome.outcome, "completed");
  assert.deepEqual(
    handoff.carried,
    [output.replacementFolder],
    "late old init cannot corrupt replacement attachment knowledge",
  );
  const connected = handoff.events.find((event) => event.event === "session_replaced");
  assert.equal(connected?.session_id, output.id);
  assert.equal(connected?.request_id, "connect-1");
  assert.deepEqual(
    handoff.events.filter((event) => event.event === "plugin_inventory"),
    [
      {
        event: "plugin_inventory",
        session_id: output.id,
        plugins: [
          {
            name: "replacement-local",
            path: output.replacementFolder,
            source: "replacement-local@inline",
            version: "2.0.0",
          },
        ],
      },
    ],
    "the replacement publishes its own independently specified plugin data",
  );
  assert.deepEqual(
    handoff.lateEvents.filter((event) => event.event === "plugin_inventory"),
    [],
    "the released old producer publishes no stale inventory",
  );
  const snapshots = handoff.events.filter(
    (event) => event.event === "mcp_snapshot" && event.source === "mcp_status",
  );
  assert.deepEqual(
    snapshots.map((event) => ({
      session: event.session_id,
      servers: event.servers?.map((server) => ({
        name: server.name,
        status: server.status,
        info: server.server_info,
      })),
    })),
    [
      {
        session: output.id,
        servers: [
          {
            name: "replacement-server",
            status: "connected",
            info: { name: "replacement", version: "fresh" },
          },
        ],
      },
    ],
    "old in-flight status is fenced while the replacement's real read remains publishable",
  );
  assert.ok(
    output.queries.every((query) => query.closed),
    "owned query consumers and detached cleanup finish before capture ends",
  );
});

function modeFixture(auto: boolean | undefined, bypass: boolean): SessionState {
  const session = makeSessionState();
  session.sessionId = `mode-${crypto.randomUUID()}`;
  session.requestedModelId = "sonnet";
  session.resolvedRuntimeModelId = "haiku";
  session.supportsBypassPermissionsMode = bypass;
  session.availableModels = [
    {
      id: "haiku",
      display_name: "Actual runtime",
      supports_effort: false,
      supported_effort_levels: [],
      ...(auto === undefined ? {} : { supports_auto_mode: auto }),
    },
    {
      id: "sonnet",
      display_name: "Requested alternative",
      supports_effort: false,
      supported_effort_levels: [],
      supports_auto_mode: auto !== true,
    },
  ];
  refreshCurrentModel(session);
  sessions.set(session.sessionId, session);
  return session;
}
function publishedModes(
  events: Array<Record<string, unknown>>,
): Array<{
  session_id: unknown;
  mode: { current_mode_id: string; available_modes: Array<{ id: string; name: string }> };
}> {
  return events.flatMap((event) => {
    const update = event.update as Record<string, unknown> | undefined;
    return update?.type === "mode_state_update"
      ? [
          {
            session_id: event.session_id,
            mode: update.mode as {
              current_mode_id: string;
              available_modes: Array<{ id: string; name: string }>;
            },
          },
        ]
      : [];
  });
}
async function sendMode(
  session: SessionState,
  mode: string,
  request: string,
): Promise<Array<Record<string, unknown>>> {
  return captureBridgeEventsAsync(async () => {
    const parsed = parseCommandEnvelope(
      JSON.stringify({
        request_id: request,
        command: "set_mode",
        session_id: session.sessionId,
        mode,
      }),
    );
    await handleSessionControlCommand(
      parsed.command as Parameters<typeof handleSessionControlCommand>[0],
      parsed.requestId,
      promptControlDeps(),
    );
  });
}
function observeMode(session: SessionState, mode: string): void {
  handleSdkMessage(session, {
    type: "system",
    subtype: "status",
    permissionMode: mode,
    status: null,
  } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
}

test("published permission choices and actual command admission follow independent runtime capabilities and observations", async () => {
  for (const [auto, bypass] of [
    [false, false],
    [true, false],
    [false, true],
    [true, true],
    [undefined, false],
  ] as const) {
    const session = modeFixture(auto, bypass);
    const calls: string[] = [];
    session.query = {
      setPermissionMode: async (mode: string) => {
        calls.push(mode);
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
    try {
      refreshSupportedModesForSession(session);
      const connected = await captureBridgeEventsAsync(async () => {
        await emitConnectEvent(session);
      });
      const connection = connected.find((event) => event.event === "connected");
      assert.ok(connection);
      assert.equal(connection.session_id, session.sessionId);
      const mode = connection.mode as {
        current_mode_id: string;
        available_modes: Array<{ id: string; name: string }>;
      };
      assert.equal(mode.current_mode_id, "unknown");
      for (const base of ["default", "acceptEdits", "plan", "dontAsk"])
        assert.ok(mode.available_modes.some((entry) => entry.id === base));
      assert.equal(
        mode.available_modes.some((entry) => entry.id === "auto"),
        auto === true,
      );
      assert.equal(
        mode.available_modes.some((entry) => entry.id === "bypassPermissions"),
        bypass,
      );
      for (const [requested, allowed] of [
        ["auto", auto === true],
        ["bypassPermissions", bypass],
      ] as const) {
        const before = calls.length,
          events = await sendMode(session, requested, `request-${requested}`);
        if (allowed) {
          assert.deepEqual(calls.slice(before), [requested]);
          const updates = publishedModes(events);
          assert.equal(updates.length, 1);
          assert.equal(updates[0]?.session_id, session.sessionId);
          assert.equal(updates[0]?.mode.current_mode_id, requested);
          assert.equal(
            events.some((event) => event.event === "slash_error"),
            false,
          );
        } else {
          assert.equal(calls.length, before);
          const errors = events.filter((event) => event.event === "slash_error");
          assert.equal(errors.length, 1);
          assert.equal(errors[0]?.session_id, session.sessionId);
          assert.equal(errors[0]?.request_id, `request-${requested}`);
          assert.ok(String(errors[0]?.message).trim());
          assert.deepEqual(publishedModes(events), []);
        }
      }
    } finally {
      session.input.close();
      if (sessions.get(session.sessionId) === session) sessions.delete(session.sessionId);
    }
  }
  const session = modeFixture(undefined, false);
  try {
    for (const mode of ["auto", "default"]) {
      const events = captureBridgeEvents(() => observeMode(session, mode));
      const updates = publishedModes(events);
      assert.equal(updates.length, 1);
      assert.equal(updates[0]?.session_id, session.sessionId);
      assert.equal(updates[0]?.mode.current_mode_id, mode);
      const automatic = updates[0]?.mode.available_modes.find((entry) => entry.id === "auto");
      assert.equal(Boolean(automatic), mode === "auto");
      if (automatic) assert.ok(automatic.name.trim());
    }
  } finally {
    session.input.close();
    if (sessions.get(session.sessionId) === session) sessions.delete(session.sessionId);
  }
});

test("permission-mode capability refusals remove only their own choice while transient failures remain retryable", async () => {
  const rows = [
    {
      requested: "auto",
      error: "Cannot set permission mode to auto: not available in my plan",
      unsupported: true,
    },
    {
      requested: "bypassPermissions",
      error:
        "Cannot set permission mode to bypassPermissions because the session was not launched with --dangerously-skip-permissions",
      unsupported: true,
    },
    {
      requested: "auto",
      error: "bridge disconnected before request completed",
      unsupported: false,
    },
    {
      requested: "auto",
      error:
        "Cannot set permission mode to bypassPermissions because the session was not launched with --dangerously-skip-permissions",
      unsupported: false,
    },
  ];
  for (const row of rows) {
    const session = modeFixture(true, true),
      calls: string[] = [];
    let first = true;
    session.query = {
      setPermissionMode: async (mode: string) => {
        calls.push(mode);
        if (first) {
          first = false;
          throw new Error(row.error);
        }
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
    try {
      captureBridgeEvents(() => observeMode(session, "default"));
      const events = await sendMode(session, row.requested, "rejected-mode-request");
      assert.deepEqual(calls, [row.requested]);
      const errors = events.filter((event) => event.event === "slash_error");
      assert.equal(errors.length, 1);
      assert.equal(errors[0]?.request_id, "rejected-mode-request");
      assert.equal(errors[0]?.session_id, session.sessionId);
      assert.ok(String(errors[0]?.message).includes(row.error));
      if (row.unsupported) {
        const updates = publishedModes(events);
        assert.equal(updates.length, 1);
        assert.equal(updates[0]?.session_id, session.sessionId);
        assert.equal(updates[0]?.mode.current_mode_id, "default");
        assert.equal(
          updates[0]?.mode.available_modes.some((mode) => mode.id === row.requested),
          false,
        );
        const other = row.requested === "auto" ? "bypassPermissions" : "auto";
        assert.ok(updates[0]?.mode.available_modes.some((mode) => mode.id === other));
        const retry = await sendMode(session, row.requested, "refused-locally");
        assert.deepEqual(calls, [row.requested]);
        assert.equal(
          retry.filter(
            (event) => event.event === "slash_error" && event.request_id === "refused-locally",
          ).length,
          1,
        );
        const accepted = await sendMode(session, other, "other-mode");
        assert.equal(publishedModes(accepted).at(-1)?.mode.current_mode_id, other);
        assert.deepEqual(calls, [row.requested, other]);
      } else {
        const accepted = await sendMode(session, row.requested, "retry-mode");
        assert.equal(publishedModes(accepted).at(-1)?.mode.current_mode_id, row.requested);
        assert.equal(
          accepted.some((event) => event.event === "slash_error"),
          false,
        );
        assert.deepEqual(calls, [row.requested, row.requested]);
      }
    } finally {
      session.input.close();
      if (sessions.get(session.sessionId) === session) sessions.delete(session.sessionId);
    }
  }
});
test("pending mode completions acknowledge newer observations and cannot publish for a closed or replaced owner", async () => {
  for (const [observed, rejected] of [
    [false, false],
    [true, false],
    [true, true],
  ]) {
    const session = modeFixture(true, true),
      entered = bridgeDeferred<void>(),
      release = bridgeDeferred<void>();
    const calls: string[] = [];
    session.query = {
      setPermissionMode: async (mode: string) => {
        calls.push(mode);
        entered.resolve();
        await release.promise;
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
    let pending: Promise<unknown> | undefined;
    try {
      captureBridgeEvents(() => observeMode(session, "default"));
      const events = await captureBridgeEventsAsync(async () => {
        pending = handleSessionControlCommand(
          { command: "set_mode", session_id: session.sessionId, mode: "plan" },
          "held-mode",
          promptControlDeps(),
        );
        await boundedBridgeTask(entered.promise);
        if (observed) observeMode(session, "acceptEdits");
        if (rejected) release.reject(new Error("LIVE-COMMAND-FAILURE"));
        else release.resolve();
        await boundedBridgeTask(pending);
      });
      assert.deepEqual(calls, ["plan"]);
      const updates = publishedModes(events);
      assert.ok(updates.length > 0);
      for (const update of updates) {
        assert.equal(update.session_id, session.sessionId);
        assert.equal(update.mode.current_mode_id, observed ? "acceptEdits" : "plan");
      }
      assert.equal(
        events.some((event) => event.event === "slash_error"),
        rejected,
      );
      if (rejected) {
        const error = events.find((event) => event.event === "slash_error");
        assert.equal(error?.request_id, "held-mode");
        assert.equal(error?.session_id, session.sessionId);
        assert.ok(String(error?.message).includes("LIVE-COMMAND-FAILURE"));
      }
    } finally {
      release.resolve();
      if (pending) await boundedBridgeTask(pending);
      session.input.close();
      if (sessions.get(session.sessionId) === session) sessions.delete(session.sessionId);
    }
  }
  for (const lifetime of ["close", "replace-id"]) {
    for (const reject of [false, true]) {
      const session = modeFixture(true, true),
        oldId = session.sessionId,
        entered = bridgeDeferred<void>(),
        release = bridgeDeferred<void>();
      let first = true;
      const calls: string[] = [];
      session.query = {
        close: () => {},
        setPermissionMode: async (mode: string) => {
          calls.push(mode);
          if (first) {
            first = false;
            entered.resolve();
            await release.promise;
          }
        },
      } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
      let pending: Promise<unknown> | undefined,
        positive = session;
      try {
        captureBridgeEvents(() => observeMode(session, "default"));
        pending = handleSessionControlCommand(
          { command: "set_mode", session_id: oldId, mode: "auto" },
          "stale-mode-request",
          promptControlDeps(),
        );
        await boundedBridgeTask(entered.promise);
        if (lifetime === "close") {
          await closeSession(session);
          positive = modeFixture(true, true);
          sessions.delete(positive.sessionId);
          positive.sessionId = oldId;
          sessions.set(oldId, positive);
          positive.query = {
            setPermissionMode: async (mode: string) => {
              calls.push(`replacement-${mode}`);
            },
          } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
        } else
          captureBridgeEvents(() => updateSessionId(session, `replaced-${crypto.randomUUID()}`));
        const events = await captureBridgeEventsAsync(async () => {
          if (reject)
            release.reject(
              new Error("Cannot set permission mode to auto: not available in my plan"),
            );
          else release.resolve();
          assert.ok(pending);
          await boundedBridgeTask(pending);
        });
        assert.deepEqual(
          events,
          [],
          `${lifetime}/${reject ? "reject" : "resolve"} has lost command ownership`,
        );
        const accepted = await sendMode(positive, "auto", "current-owner-mode");
        assert.equal(publishedModes(accepted).at(-1)?.mode.current_mode_id, "auto");
        assert.equal(
          accepted.some((event) => event.event === "slash_error"),
          false,
        );
        assert.equal(calls.at(-1), lifetime === "close" ? "replacement-auto" : "auto");
      } finally {
        release.resolve();
        if (pending) await boundedBridgeTask(pending);
        for (const owner of [session, positive]) {
          owner.input.close();
          if (sessions.get(owner.sessionId) === owner) sessions.delete(owner.sessionId);
        }
        if (sessions.get(oldId) === session || sessions.get(oldId) === positive)
          sessions.delete(oldId);
      }
    }
  }
});

function publishedTool(
  events: Array<Record<string, unknown>>,
  sessionId: string,
  id: string,
): Record<string, unknown> {
  const state: Record<string, unknown> = {};
  let created = 0;
  for (const event of events) {
    const update = event.update as
      | {
          type?: string;
          tool_call?: Record<string, unknown>;
          tool_call_update?: {
            tool_call_id: string;
            source_message_uuid?: string;
            fields: Record<string, unknown>;
          };
        }
      | undefined;
    if (update?.type === "tool_call" && update.tool_call?.tool_call_id === id) {
      assert.equal(event.session_id, sessionId);
      created++;
      Object.assign(state, update.tool_call);
    }
    if (update?.type === "tool_call_update" && update.tool_call_update?.tool_call_id === id) {
      assert.equal(event.session_id, sessionId);
      Object.assign(state, update.tool_call_update.fields);
      if (update.tool_call_update.source_message_uuid)
        state.source_message_uuid = update.tool_call_update.source_message_uuid;
    }
  }
  assert.equal(created, 1, `one published row for ${id}`);
  return state;
}

function captureBridgeEvents(run: () => void): Array<Record<string, unknown>> {
  const writes: string[] = [];
  const restoreWriter = replaceProtocolEventWriter((line) => {
    writes.push(line);
  });

  try {
    run();
  } finally {
    restoreWriter();
  }

  return writes.map((line) => {
    const event: unknown = JSON.parse(line);
    assert.ok(event !== null && typeof event === "object" && !Array.isArray(event));
    return event as Record<string, unknown>;
  });
}

async function captureBridgeEventsAsync(
  run: () => Promise<void>,
  observe?: (event: Record<string, unknown>) => void,
): Promise<Array<Record<string, unknown>>> {
  const writes: string[] = [];
  const restoreWriter = replaceProtocolEventWriter((line) => {
    writes.push(line);
    observe?.(JSON.parse(line) as Record<string, unknown>);
  });

  try {
    await run();
  } finally {
    await settleBridgeSdkReads();
    restoreWriter();
  }

  return writes.map((line) => {
    const event: unknown = JSON.parse(line);
    assert.ok(event !== null && typeof event === "object" && !Array.isArray(event));
    return event as Record<string, unknown>;
  });
}

test("resume-at validation refuses the original operation before planning or native work", async () => {
  let plans = 0;
  const events = await captureBridgeEventsAsync(async () =>
    handleLifecycleCommand(
      {
        command: "resume_session_at",
        session_id: "source-resume-owner",
        target_user_message_id: " \t ",
        launch_settings: {},
      },
      "resume-operation-request",
      undefined,
      {
        buildRewindConversationPlan: () => {
          plans++;
          throw new Error("UNEXPECTED-PLANNING");
        },
      },
    ),
  );
  assert.equal(plans, 0);
  assert.equal(events.length, 1);
  assert.equal(events[0]?.event, "session_resume_failed");
  assert.equal(events[0]?.session_id, "source-resume-owner");
  assert.equal(events[0]?.request_id, "resume-operation-request");
  assert.match(String(events[0]?.message), /target.*empty/i);
});
test("command envelopes retain independent payload and correlation while rejecting targeted required fields and invalid runtime choices", () => {
  const cases: Array<{
    wire: Record<string, unknown>;
    expected: Record<string, unknown>;
    required: string;
  }> = [
    {
      wire: {
        request_id: "initialize-envelope-request",
        command: "initialize",
        cwd: "C:/audit/initialize project",
      },
      expected: { command: "initialize", cwd: "C:/audit/initialize project" },
      required: "cwd",
    },
    {
      wire: {
        request_id: "resume-envelope-request",
        command: "resume_session",
        session_id: "resume-source-session",
        launch_settings: {
          model: "claude-opus-4-7",
          permission_mode: "plan",
          effort: "max",
          agent: "reviewer",
        },
      },
      expected: {
        command: "resume_session",
        session_id: "resume-source-session",
        launch_settings: {
          model: "claude-opus-4-7",
          permission_mode: "plan",
          effort: "max",
          agent: "reviewer",
        },
      },
      required: "session_id",
    },
    {
      wire: {
        request_id: "resume-at-envelope-request",
        command: "resume_session_at",
        session_id: "resume-at-source-session",
        target_user_message_id: "independent-selected-user",
        launch_settings: {
          model: "claude-sonnet-4-7",
          permission_mode: "acceptEdits",
          effort: "xhigh",
          agent: "planner",
        },
      },
      expected: {
        command: "resume_session_at",
        session_id: "resume-at-source-session",
        target_user_message_id: "independent-selected-user",
        launch_settings: {
          model: "claude-sonnet-4-7",
          permission_mode: "acceptEdits",
          effort: "xhigh",
          agent: "planner",
        },
      },
      required: "target_user_message_id",
    },
    {
      wire: {
        request_id: "rename-envelope-request",
        command: "rename_session",
        session_id: "rename-source-session",
        title: "  Independent title\nExact  ",
      },
      expected: {
        command: "rename_session",
        session_id: "rename-source-session",
        title: "  Independent title\nExact  ",
      },
      required: "title",
    },
    {
      wire: {
        request_id: "generate-envelope-request",
        command: "generate_session_title",
        session_id: "generate-source-session",
        description: "Independent description\n\ttail  ",
      },
      expected: {
        command: "generate_session_title",
        session_id: "generate-source-session",
        description: "Independent description\n\ttail  ",
      },
      required: "description",
    },
    {
      wire: {
        request_id: "model-envelope-request",
        command: "set_model",
        session_id: "model-source-session",
        model: "claude-sonnet-4-7",
      },
      expected: {
        command: "set_model",
        session_id: "model-source-session",
        model: "claude-sonnet-4-7",
      },
      required: "model",
    },
  ];
  for (const row of cases) {
    const parsed = parseCommandEnvelope(JSON.stringify(row.wire));
    assert.equal(parsed.requestId, row.wire.request_id);
    for (const [field, expected] of Object.entries(row.expected))
      assert.deepEqual(
        (parsed.command as unknown as Record<string, unknown>)[field],
        expected,
        `${row.wire.command}.${field}`,
      );
    for (const invalid of [undefined, null, 7, false, []]) {
      assert.throws(
        () => parseCommandEnvelope(JSON.stringify({ ...row.wire, [row.required]: invalid })),
        new RegExp(row.required),
      );
    }
  }
  for (const effort of ["low", "medium", "high", "xhigh", "max", null]) {
    const parsed = parseCommandEnvelope(
      JSON.stringify({
        command: "set_effort",
        request_id: `effort-envelope-${String(effort)}`,
        session_id: "effort-envelope-owner",
        effort,
      }),
    );
    assert.equal(parsed.requestId, `effort-envelope-${String(effort)}`);
    assert.deepEqual(parsed.command, {
      command: "set_effort",
      session_id: "effort-envelope-owner",
      effort,
    });
  }
  for (const invalid of [undefined, "unsupported", "HIGH", 7, false, {}])
    assert.throws(
      () =>
        parseCommandEnvelope(
          JSON.stringify({
            command: "set_effort",
            session_id: "effort-envelope-owner",
            effort: invalid,
          }),
        ),
      /effort/,
    );
  for (const agent of ["independent-reviewer", null]) {
    const parsed = parseCommandEnvelope(
      JSON.stringify({
        command: "set_agent",
        request_id: `agent-envelope-${String(agent)}`,
        session_id: "agent-envelope-owner",
        agent,
      }),
    );
    assert.equal(parsed.requestId, `agent-envelope-${String(agent)}`);
    assert.deepEqual(parsed.command, {
      command: "set_agent",
      session_id: "agent-envelope-owner",
      agent,
    });
  }
  for (const invalid of [undefined, "", "   ", 7, false, {}])
    assert.throws(
      () =>
        parseCommandEnvelope(
          JSON.stringify({
            command: "set_agent",
            session_id: "agent-envelope-owner",
            agent: invalid,
          }),
        ),
      /agent/,
    );
  for (const enabled of [false, true]) {
    const parsed = parseCommandEnvelope(
      JSON.stringify({
        command: "set_fast_mode",
        request_id: `fast-envelope-${String(enabled)}`,
        session_id: "fast-envelope-owner",
        enabled,
      }),
    );
    assert.equal(parsed.requestId, `fast-envelope-${String(enabled)}`);
    assert.deepEqual(parsed.command, {
      command: "set_fast_mode",
      session_id: "fast-envelope-owner",
      enabled,
    });
  }
  for (const invalid of [undefined, null, "false", 0, 1, {}])
    assert.throws(
      () =>
        parseCommandEnvelope(
          JSON.stringify({
            command: "set_fast_mode",
            session_id: "fast-envelope-owner",
            enabled: invalid,
          }),
        ),
      /enabled/,
    );
});

test("parseCommandEnvelope preserves a side question as one complete string", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      command: "side_question",
      session_id: "session-123",
      btw_id: "btw-1",
      question: "Why  two spaces?\nAnd this line?",
    }),
  );

  assert.deepEqual(parsed.command, {
    command: "side_question",
    session_id: "session-123",
    btw_id: "btw-1",
    question: "Why  two spaces?\nAnd this line?",
  });
  assert.throws(
    () =>
      parseCommandEnvelope(
        JSON.stringify({
          command: "side_question",
          session_id: "session-123",
          btw_id: "btw-2",
          question: "  \n  ",
        }),
      ),
    /must not be empty/,
  );
});

test("AsyncQueue reports closed-input rejection without silently dropping an item", async () => {
  const input = new AsyncQueue<string>();
  assert.equal(input.enqueue("accepted"), true);
  input.close();
  assert.equal(input.enqueue("rejected"), false);
  const iterator = input[Symbol.asyncIterator]();
  assert.deepEqual(await iterator.next(), { value: "accepted", done: false });
  assert.deepEqual(await iterator.next(), { value: undefined, done: true });
});

async function nextPromptItem<T>(iterator: AsyncIterator<T>): Promise<IteratorResult<T>> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      iterator.next(),
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error("SDK prompt input did not settle")), 3000);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

async function promptAdmissionFixture(
  run: (target: SessionState, sibling: SessionState) => Promise<void>,
): Promise<void> {
  const target = makeSessionState(),
    sibling = makeSessionState();
  target.sessionId = `admission-target-${crypto.randomUUID()}`;
  sibling.sessionId = `admission-sibling-${crypto.randomUUID()}`;
  sessions.set(target.sessionId, target);
  sessions.set(sibling.sessionId, sibling);
  try {
    await run(target, sibling);
  } finally {
    for (const session of [target, sibling]) {
      session.input.close();
      if (sessions.get(session.sessionId) === session) sessions.delete(session.sessionId);
    }
  }
}

async function dispatchParsedPrompt(
  wire: Record<string, unknown>,
): Promise<Array<Record<string, unknown>>> {
  const parsed = parseCommandEnvelope(JSON.stringify(wire));
  assert.equal(parsed.command.command, "prompt");
  return captureBridgeEventsAsync(async () => {
    await handleSessionControlCommand(
      parsed.command as Parameters<typeof handleSessionControlCommand>[0],
      parsed.requestId,
      promptControlDeps(),
    );
  });
}

test("parsed prompt admission delivers exact human input and one correlated receipt or closed-input refusal", async () => {
  await promptAdmissionFixture(async (target, sibling) => {
    const iterator = target.input[Symbol.asyncIterator]();
    const events = await dispatchParsedPrompt({
      request_id: "admit-request",
      command: "prompt",
      session_id: target.sessionId,
      message_uuid: "admit-message",
      chunks: [{ kind: "text", value: " first\nsecond  " }],
      inline_pastes: ["first\nsecond"],
    });
    assert.deepEqual(events, [
      {
        request_id: "admit-request",
        event: "user_message_queued",
        session_id: target.sessionId,
        message_uuid: "admit-message",
      },
    ]);
    assert.deepEqual(await nextPromptItem(iterator), {
      done: false,
      value: {
        type: "user",
        uuid: "admit-message",
        session_id: target.sessionId,
        parent_tool_use_id: null,
        origin: { kind: "human" },
        message: { role: "user", content: [{ type: "text", text: " first\nsecond  " }] },
        inline_pastes: ["first\nsecond"],
      },
    });
    assert.throws(
      () =>
        parseCommandEnvelope(
          JSON.stringify({
            command: "prompt",
            session_id: target.sessionId,
            chunks: [{ kind: "text", value: "missing UUID" }],
          }),
        ),
      /message_uuid/,
    );
    target.input.close();
    const refused = await dispatchParsedPrompt({
      request_id: "closed-request",
      command: "prompt",
      session_id: target.sessionId,
      message_uuid: "closed-message",
      chunks: [{ kind: "text", value: "restore this" }],
    });
    assert.equal(refused.length, 1);
    assert.deepEqual(
      { ...refused[0], reason: undefined },
      {
        request_id: "closed-request",
        event: "user_message_rejected",
        session_id: target.sessionId,
        message_uuid: "closed-message",
        reason: undefined,
      },
    );
    assert.match(String(refused[0]?.reason), /closed/i);
    assert.deepEqual(await nextPromptItem(iterator), { done: true, value: undefined });
    sibling.input.close();
    assert.deepEqual(await nextPromptItem(sibling.input[Symbol.asyncIterator]()), {
      done: true,
      value: undefined,
    });
  });
});
test("parsed prompt refusal retains identity and leaves the live session usable without delivering rejected items", async () => {
  await promptAdmissionFixture(async (target, sibling) => {
    const iterator = target.input[Symbol.asyncIterator]();
    const rows = [
      {
        session: "unknown-admission-session",
        chunks: [{ kind: "text", value: "unknown session prompt" }],
        cause: /unknown/i,
      },
      {
        session: target.sessionId,
        chunks: [{ kind: "text", value: " \n\t " }],
        cause: /supported content/i,
      },
      {
        session: target.sessionId,
        chunks: [
          { kind: "image", value: { mime_type: "image/svg+xml", data: "PHN2Zz4=" } },
          { kind: "image", value: { mime_type: "image/png", data: "!invalid-base64!" } },
        ],
        cause: /supported content/i,
      },
    ];
    for (const [index, row] of rows.entries()) {
      const request = `refused-request-${index}`,
        uuid = `refused-message-${index}`;
      const events = await dispatchParsedPrompt({
        request_id: request,
        command: "prompt",
        session_id: row.session,
        message_uuid: uuid,
        chunks: row.chunks,
      });
      const rejection = events.filter((event) => event.event === "user_message_rejected");
      assert.equal(rejection.length, 1);
      assert.equal(rejection[0]?.request_id, request);
      assert.equal(rejection[0]?.session_id, row.session);
      assert.equal(rejection[0]?.message_uuid, uuid);
      assert.match(String(rejection[0]?.reason), row.cause);
      assert.equal(
        events.some(
          (event) =>
            event.event === "user_message_queued" || event.event === "user_message_started",
        ),
        false,
      );
    }
    const accepted = await dispatchParsedPrompt({
      request_id: "retry-request",
      command: "prompt",
      session_id: target.sessionId,
      message_uuid: "retry-message",
      chunks: [{ kind: "text", value: "exact retry text" }],
    });
    assert.deepEqual(accepted, [
      {
        request_id: "retry-request",
        event: "user_message_queued",
        session_id: target.sessionId,
        message_uuid: "retry-message",
      },
    ]);
    assert.deepEqual(await nextPromptItem(iterator), {
      done: false,
      value: {
        type: "user",
        uuid: "retry-message",
        session_id: target.sessionId,
        parent_tool_use_id: null,
        origin: { kind: "human" },
        message: { role: "user", content: [{ type: "text", text: "exact retry text" }] },
      },
    });
    target.input.close();
    assert.deepEqual(await nextPromptItem(iterator), { done: true, value: undefined });
    sibling.input.close();
    assert.deepEqual(await nextPromptItem(sibling.input[Symbol.asyncIterator]()), {
      done: true,
      value: undefined,
    });
  });
});
test("parsed mixed prompt filters invalid images without losing valid neighbors or the default PNG payload", async () => {
  await promptAdmissionFixture(async (target, sibling) => {
    const events = await dispatchParsedPrompt({
      request_id: "mixed-request",
      command: "prompt",
      session_id: target.sessionId,
      message_uuid: "mixed-message",
      chunks: [
        { kind: "text", value: "before\n  " },
        { kind: "image", value: { mime_type: "image/jpeg", data: "aGVsbG8=" } },
        { kind: "image", value: { mime_type: "image/svg+xml", data: "PHN2Zz4=" } },
        { kind: "text", value: "between\t" },
        { kind: "image", value: { mime_type: "image/png", data: "!bad-base64!" } },
        { kind: "image", value: { data: "ZGVmYXVsdA==" } },
        { kind: "text", value: "after\n" },
      ],
    });
    assert.deepEqual(events, [
      {
        request_id: "mixed-request",
        event: "user_message_queued",
        session_id: target.sessionId,
        message_uuid: "mixed-message",
      },
    ]);
    const iterator = target.input[Symbol.asyncIterator]();
    assert.deepEqual(await nextPromptItem(iterator), {
      done: false,
      value: {
        type: "user",
        uuid: "mixed-message",
        session_id: target.sessionId,
        parent_tool_use_id: null,
        origin: { kind: "human" },
        message: {
          role: "user",
          content: [
            { type: "text", text: "before\n  " },
            {
              type: "image",
              source: { type: "base64", media_type: "image/jpeg", data: "aGVsbG8=" },
            },
            { type: "text", text: "between\t" },
            {
              type: "image",
              source: { type: "base64", media_type: "image/png", data: "ZGVmYXVsdA==" },
            },
            { type: "text", text: "after\n" },
          ],
        },
      },
    });
    target.input.close();
    assert.deepEqual(await nextPromptItem(iterator), { done: true, value: undefined });
    sibling.input.close();
    assert.deepEqual(await nextPromptItem(sibling.input[Symbol.asyncIterator]()), {
      done: true,
      value: undefined,
    });
  });
});

async function withCommandTargets(
  run: (target: SessionState, sibling: SessionState) => Promise<void>,
): Promise<void> {
  const target = makeSessionState(),
    sibling = makeSessionState();
  target.sessionId = `mcp-target-${crypto.randomUUID()}`;
  sibling.sessionId = `mcp-sibling-${crypto.randomUUID()}`;
  sibling.query = {
    close: () => {},
    mcpServerStatus: async () => {
      throw new Error("WRONG-SIBLING-STATUS");
    },
    setMcpServers: async () => {
      throw new Error("WRONG-SIBLING-MUTATION");
    },
    toggleMcpServer: async () => {
      throw new Error("WRONG-SIBLING-TOGGLE");
    },
    reconnectMcpServer: async () => {
      throw new Error("WRONG-SIBLING-RECONNECT");
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
  sessions.set(target.sessionId, target);
  sessions.set(sibling.sessionId, sibling);
  try {
    await run(target, sibling);
  } finally {
    for (const owner of [target, sibling]) {
      if (typeof owner.query.close === "function") await boundedBridgeTask(closeSession(owner));
      else owner.input.close();
      if (sessions.get(owner.sessionId) === owner) sessions.delete(owner.sessionId);
    }
  }
}
async function dispatchMcp(wire: Record<string, unknown>): Promise<Array<Record<string, unknown>>> {
  const parsed = parseCommandEnvelope(JSON.stringify(wire));
  return captureBridgeEventsAsync(async () =>
    handleMcpCommand(parsed.command as Parameters<typeof handleMcpCommand>[0], parsed.requestId),
  );
}
function mcpSnapshot(
  events: Array<Record<string, unknown>>,
  sessionId: string,
  request: string,
  source: string,
): Record<string, unknown> {
  const snapshots = events.filter((event) => event.event === "mcp_snapshot");
  assert.equal(snapshots.length, 1);
  const snapshot = snapshots[0];
  assert.ok(snapshot);
  assert.equal(snapshot.session_id, sessionId);
  assert.equal(snapshot.request_id, request);
  assert.equal(snapshot.source, source);
  return snapshot;
}

test("parsed MCP control and native or alias status requests reach the intended receiver without changing server identities", async () => {
  await withCommandTargets(async (target) => {
    const calls: unknown[][] = [];
    const native = {
      close: () => {},
      async toggleMcpServer(name: string, enabled: boolean) {
        assert.equal(this, native);
        calls.push(["toggle", name, enabled]);
      },
      async reconnectMcpServer(name: string) {
        assert.equal(this, native);
        calls.push(["reconnect", name]);
      },
      async mcpServerStatus() {
        assert.equal(this, native);
        calls.push(["status"]);
        return [
          {
            name: "foo",
            scope: "local",
            status: "connected",
            tools: [{ name: "read_actual", description: "Observed tool" }],
          },
          { name: "foo__bar", scope: "project", status: "disabled", tools: [] },
        ];
      },
    };
    target.query = native as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
    for (const row of [
      { command: "mcp_toggle", server_name: "foo", enabled: false },
      { command: "mcp_toggle", server_name: "foo__bar", enabled: true },
      { command: "mcp_reconnect", server_name: "foo__bar" },
    ]) {
      const events = await dispatchMcp({
        ...row,
        request_id: "control-request",
        session_id: target.sessionId,
      });
      assert.deepEqual(events, []);
    }
    assert.deepEqual(calls, [
      ["toggle", "foo", false],
      ["toggle", "foo__bar", true],
      ["reconnect", "foo__bar"],
    ]);
    for (const command of ["mcp_status", "get_mcp_snapshot"]) {
      const request = `request-${command}`,
        events = await dispatchMcp({ command, request_id: request, session_id: target.sessionId });
      const snapshot = mcpSnapshot(events, target.sessionId, request, "mcp_status");
      assert.deepEqual(snapshot.servers, [
        {
          name: "foo",
          scope: "local",
          status: "connected",
          tools: [{ name: "read_actual", description: "Observed tool" }],
        },
        { name: "foo__bar", scope: "project", status: "disabled", tools: [] },
      ]);
    }
    assert.deepEqual(calls.slice(3), [["status"], ["status"]]);
  });
});

test("invalid MCP fields never reach native mutation while valid boundary and policy controls do", async () => {
  await withCommandTargets(async (target) => {
    const received: unknown[] = [];
    target.query = {
      close: () => {},
      setMcpServers: async (value: unknown) => {
        received.push(value);
        return { added: [], removed: [], errors: {} };
      },
      mcpServerStatus: async () => [],
    } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
    const base = { type: "http", url: "https://fixture.invalid/mcp" };
    const invalid = [
      {
        config: { ...base, tools: [{ name: "read", org_max_permission: "deny" }] },
        cause: /org_max_permission/,
      },
      { config: { ...base, timeout: 999 }, cause: /\.timeout\b/ },
      { config: { ...base, request_timeout_ms: 999 }, cause: /request_timeout_ms/ },
      { config: { ...base, always_load: "yes" }, cause: /always_load/ },
      {
        config: { ...base, tools: [{ name: "read", permission_policy: "sometimes" }] },
        cause: /permission_policy/,
      },
      {
        config: {
          type: "stdio",
          command: "owned-server",
          tools: [{ name: "read", permission_policy: "always_allow" }],
        },
        cause: /tools/,
      },
    ];
    for (const row of invalid) {
      await assert.rejects(
        () =>
          dispatchMcp({
            command: "mcp_set_servers",
            session_id: target.sessionId,
            servers: { candidate: row.config },
          }),
        row.cause,
      );
      assert.equal(received.length, 0);
    }
    for (const config of [
      {
        ...base,
        timeout: 1000,
        request_timeout_ms: 1000,
        always_load: false,
        tools: [{ name: "read", permission_policy: "always_deny", org_max_permission: "ask" }],
      },
      {
        type: "sse",
        url: "https://fixture.invalid/events",
        always_load: true,
        tools: [{ name: "read", permission_policy: "always_allow", org_max_permission: "blocked" }],
      },
      { type: "stdio", command: "owned-server" },
    ]) {
      const events = await dispatchMcp({
        command: "mcp_set_servers",
        session_id: target.sessionId,
        request_id: "valid-config",
        servers: { candidate: config },
      });
      assert.equal(
        events.some(
          (event) => event.event === "mcp_operation_error" || event.event === "slash_error",
        ),
        false,
      );
      assert.equal(events.filter((event) => event.event === "mcp_set_servers_result").length, 1);
    }
    assert.equal(received.length, 3);
  });
});
test("parsed HTTP and SSE configuration reaches native casing while snapshots retain independently returned server authority", async () => {
  await withCommandTargets(async (target) => {
    const received: unknown[] = [];
    target.query = {
      close: () => {},
      setMcpServers: async (value: unknown) => {
        received.push(value);
        return {
          added: ["reported-add"],
          removed: ["reported-remove"],
          errors: { "reported-warning": "native warning" },
        };
      },
      mcpServerStatus: async () => [
        {
          name: "observed-http",
          scope: "project",
          source: "plugin",
          status: "connected",
          config: {
            type: "http",
            url: "https://observed.invalid/http",
            headers: { "X-Observed": "http" },
            timeout: 9000,
            requestTimeoutMs: 12000,
            alwaysLoad: false,
            tools: [
              {
                name: "observed-read",
                permission_policy: "always_deny",
                org_max_permission: "ask",
              },
            ],
          },
          tools: [{ name: "actual_resource", description: "Observed resource" }],
        },
        {
          name: "observed-sse",
          scope: "local",
          status: "connected",
          config: {
            type: "sse",
            url: "https://observed.invalid/events",
            timeout: 7000,
            requestTimeoutMs: 15000,
            alwaysLoad: true,
            tools: [
              {
                name: "observed-write",
                permission_policy: "always_ask",
                org_max_permission: "blocked",
              },
            ],
          },
          tools: [],
        },
      ],
    } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
    const rows = [
      {
        wire: {
          type: "http",
          url: "https://requested.invalid/http",
          headers: { "X-Requested": "http" },
          timeout: 2500,
          request_timeout_ms: 11000,
          always_load: true,
          tools: [
            { name: "search" },
            { name: "write", permission_policy: "always_allow", org_max_permission: "blocked" },
          ],
        },
        expected: {
          type: "http",
          url: "https://requested.invalid/http",
          headers: { "X-Requested": "http" },
          timeout: 2500,
          requestTimeoutMs: 11000,
          alwaysLoad: true,
          tools: [
            { name: "search" },
            { name: "write", permission_policy: "always_allow", org_max_permission: "blocked" },
          ],
        },
      },
      {
        wire: {
          type: "sse",
          url: "https://requested.invalid/events",
          headers: { "X-Requested": "sse" },
          timeout: 1000,
          request_timeout_ms: 1000,
          always_load: false,
          tools: [{ name: "read", permission_policy: "always_ask", org_max_permission: "ask" }],
        },
        expected: {
          type: "sse",
          url: "https://requested.invalid/events",
          headers: { "X-Requested": "sse" },
          timeout: 1000,
          requestTimeoutMs: 1000,
          alwaysLoad: false,
          tools: [{ name: "read", permission_policy: "always_ask", org_max_permission: "ask" }],
        },
      },
    ];
    for (const [index, row] of rows.entries()) {
      const request = `set-config-${index}`,
        events = await dispatchMcp({
          command: "mcp_set_servers",
          session_id: target.sessionId,
          request_id: request,
          servers: { requested: row.wire },
        });
      assert.deepEqual(received[index], { requested: row.expected });
      const result = events.filter((event) => event.event === "mcp_set_servers_result");
      assert.equal(result.length, 1);
      assert.equal(result[0]?.request_id, request);
      assert.equal(result[0]?.session_id, target.sessionId);
      assert.deepEqual(result[0]?.result, {
        added: ["reported-add"],
        removed: ["reported-remove"],
        errors: { "reported-warning": "native warning" },
      });
      const snapshot = mcpSnapshot(events, target.sessionId, request, "mcp_set_servers");
      assert.deepEqual(snapshot.servers, [
        {
          name: "observed-http",
          scope: "project",
          source: "plugin",
          status: "connected",
          config: {
            type: "http",
            url: "https://observed.invalid/http",
            headers: { "X-Observed": "http" },
            timeout: 9000,
            request_timeout_ms: 12000,
            always_load: false,
            tools: [
              {
                name: "observed-read",
                permission_policy: "always_deny",
                org_max_permission: "ask",
              },
            ],
          },
          tools: [{ name: "actual_resource", description: "Observed resource" }],
        },
        {
          name: "observed-sse",
          scope: "local",
          status: "connected",
          config: {
            type: "sse",
            url: "https://observed.invalid/events",
            timeout: 7000,
            request_timeout_ms: 15000,
            always_load: true,
            tools: [
              {
                name: "observed-write",
                permission_policy: "always_ask",
                org_max_permission: "blocked",
              },
            ],
          },
          tools: [],
        },
      ]);
    }
  });
});
test("MCP failed-add status remains authoritative when the SDK result reports the server as added", async () => {
  const session = makeSessionState();
  session.query = {
    setMcpServers: async () => ({
      added: ["throws-on-connect"],
      removed: [],
      errors: {},
    }),
    mcpServerStatus: async () => [
      {
        name: "throws-on-connect",
        scope: "dynamic",
        status: "failed",
        error: "connection threw during initialization",
        tools: [],
      },
    ],
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  const events = await captureBridgeEventsAsync(async () => {
    await handleMcpSetServersCommand(
      session,
      {
        command: "mcp_set_servers",
        session_id: "session-1",
        servers: {
          "throws-on-connect": {
            type: "stdio",
            command: "missing-mcp-server",
          },
        },
      },
      "req-mcp-failed-add",
    );
  });

  const result = events.find((event) => event.event === "mcp_set_servers_result");
  assert.deepEqual(result && "result" in result ? result.result : undefined, {
    added: ["throws-on-connect"],
    removed: [],
    errors: {},
  });
  const snapshot = events.find((event) => event.event === "mcp_snapshot") as
    | { servers?: Array<{ name: string; status: string; error?: string }> }
    | undefined;
  assert.deepEqual(snapshot?.servers, [
    {
      name: "throws-on-connect",
      scope: "dynamic",
      status: "failed",
      error: "connection threw during initialization",
      tools: [],
    },
  ]);
});

test("MCP authentication failures reach their requester without redirects or polling and valid dispatch still works", async () => {
  for (const kind of ["absent", "malformed", "valid"]) {
    await withCommandTargets(async (target) => {
      const nativeCalls: string[] = [];
      let statusCalls = 0;
      target.query = {
        close: () => {},
        mcpServerStatus: async () => {
          statusCalls++;
          return [];
        },
        ...(kind === "absent"
          ? {}
          : {
              mcpAuthenticate: async (name: string) => {
                nativeCalls.push(name);
                return kind === "malformed"
                  ? { authUrl: 42 }
                  : { authUrl: "https://auth.invalid/actual", requiresUserAction: true };
              },
            }),
      } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
      const events = await dispatchMcp({
        command: "mcp_authenticate",
        session_id: target.sessionId,
        request_id: `authenticate-${kind}`,
        server_name: "auth__server",
      });
      if (kind === "valid") {
        const redirects = events.filter((event) => event.event === "mcp_auth_redirect");
        assert.equal(redirects.length, 1);
        assert.equal(redirects[0]?.request_id, "authenticate-valid");
        assert.equal(redirects[0]?.session_id, target.sessionId);
        assert.deepEqual(redirects[0]?.redirect, {
          server_name: "auth__server",
          auth_url: "https://auth.invalid/actual",
          requires_user_action: true,
        });
        assert.deepEqual(nativeCalls, ["auth__server"]);
      } else {
        const failures = events.filter((event) => event.event === "mcp_operation_error");
        assert.equal(failures.length, 1);
        assert.equal(failures[0]?.request_id, `authenticate-${kind}`);
        assert.equal(failures[0]?.session_id, target.sessionId);
        const error = failures[0]?.error as {
          server_name: string;
          operation: string;
          message: string;
        };
        assert.equal(error.server_name, "auth__server");
        assert.equal(error.operation, "authenticate");
        assert.match(error.message, kind === "absent" ? /mcpAuthenticate/ : /authUrl/);
        assert.equal(
          events.some((event) => event.event === "mcp_auth_redirect"),
          false,
        );
        assert.equal(target.mcpAuthMonitors.has("auth__server"), false);
        assert.deepEqual(nativeCalls, kind === "absent" ? [] : ["auth__server"]);
        assert.equal(statusCalls, 0);
      }
    });
  }
});

test("failed MCP mutation still publishes the actual scoped inventory without conflating duplicate or prefix names", async () => {
  await withCommandTargets(async (target) => {
    target.query = {
      close: () => {},
      setMcpServers: async () => {
        throw new Error("MUTATION-REFUSED");
      },
      mcpServerStatus: async () => [
        {
          name: "docs",
          scope: "local",
          status: "connected",
          config: { type: "stdio", command: "local-server" },
          tools: [{ name: "local_read" }],
        },
        {
          name: "docs",
          scope: "project",
          status: "disabled",
          config: { type: "http", url: "https://project.invalid/mcp" },
          tools: [],
        },
        { name: "foo", scope: "dynamic", status: "connected", tools: [] },
        {
          name: "foo__bar",
          scope: "dynamic",
          status: "failed",
          error: "PREFIX-SERVER-CAUSE",
          tools: [],
        },
      ],
    } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
    const events = await dispatchMcp({
      command: "mcp_set_servers",
      session_id: target.sessionId,
      request_id: "failed-mutation",
      servers: {},
    });
    const failures = events.filter((event) => event.event === "mcp_operation_error");
    assert.equal(failures.length, 1);
    assert.equal(failures[0]?.session_id, target.sessionId);
    assert.equal(failures[0]?.request_id, "failed-mutation");
    assert.deepEqual(failures[0]?.error, { operation: "set-servers", message: "MUTATION-REFUSED" });
    const slash = events.find((event) => event.event === "slash_error");
    assert.equal(slash?.session_id, target.sessionId);
    assert.equal(slash?.request_id, "failed-mutation");
    assert.ok(String(slash?.message).includes("MUTATION-REFUSED"));
    assert.equal(
      events.some((event) => event.event === "mcp_set_servers_result"),
      false,
    );
    const snapshot = mcpSnapshot(events, target.sessionId, "failed-mutation", "mcp_set_servers");
    const servers = snapshot.servers as Array<Record<string, unknown>>;
    assert.deepEqual(
      new Map(servers.map((server) => [`${server.name}/${server.scope}`, server])),
      new Map([
        [
          "docs/local",
          {
            name: "docs",
            scope: "local",
            status: "connected",
            config: { type: "stdio", command: "local-server" },
            tools: [{ name: "local_read" }],
          },
        ],
        [
          "docs/project",
          {
            name: "docs",
            scope: "project",
            status: "disabled",
            config: { type: "http", url: "https://project.invalid/mcp" },
            tools: [],
          },
        ],
        ["foo/dynamic", { name: "foo", scope: "dynamic", status: "connected", tools: [] }],
        [
          "foo__bar/dynamic",
          {
            name: "foo__bar",
            scope: "dynamic",
            status: "failed",
            error: "PREFIX-SERVER-CAUSE",
            tools: [],
          },
        ],
      ]),
    );
  });
});
test("MCP mutation and refresh failures retain their separate correlated causes without a false empty success", async () => {
  await withCommandTargets(async (target) => {
    target.query = {
      close: () => {},
      setMcpServers: async () => {
        throw new Error("ORIGINAL-MUTATION-CAUSE");
      },
      mcpServerStatus: async () => {
        throw new Error("SEPARATE-REFRESH-CAUSE");
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
    const events = await dispatchMcp({
      command: "mcp_set_servers",
      session_id: target.sessionId,
      request_id: "dual-failure",
      servers: {},
    });
    const failures = events.filter((event) => event.event === "mcp_operation_error");
    assert.equal(failures.length, 1);
    assert.equal(failures[0]?.session_id, target.sessionId);
    assert.equal(failures[0]?.request_id, "dual-failure");
    assert.deepEqual(failures[0]?.error, {
      operation: "set-servers",
      message: "ORIGINAL-MUTATION-CAUSE",
    });
    const snapshot = mcpSnapshot(events, target.sessionId, "dual-failure", "mcp_set_servers");
    assert.deepEqual(snapshot.servers, []);
    assert.equal(snapshot.error, "SEPARATE-REFRESH-CAUSE");
    assert.equal(
      events.some((event) => event.event === "mcp_set_servers_result"),
      false,
    );
  });
});

test("mapMcpServerStatusConfig maps unknown config types without throwing", () => {
  const mapped = mapMcpServerStatusConfig({
    type: "future-transport",
    url: "future://server",
  } as unknown as NonNullable<
    import("@anthropic-ai/claude-agent-sdk").McpServerStatus["config"]
  >);

  assert.deepEqual(mapped, { type: "unknown", raw_type: "future-transport" });
});

function seedReloadState(session: SessionState): void {
  captureBridgeEvents(() =>
    updateAvailableCommands(session, "commands_changed", [
      { name: "before-command", description: "Previous command" },
    ]),
  );
  session.availableAgents = [{ name: "before-agent", description: "Previous worker" }];
  session.pluginInventory = [
    { name: "before-plugin", path: session.cwd, source: "before-plugin@inline" },
  ];
}
async function executeParsedReload(
  session: SessionState,
  request: string,
  fields: Record<string, unknown> = {},
): Promise<void> {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      command: "reload_plugins",
      session_id: session.sessionId,
      request_id: request,
      ...fields,
    }),
  );
  await handleSessionControlCommand(
    parsed.command as Parameters<typeof handleSessionControlCommand>[0],
    parsed.requestId,
    promptControlDeps(),
  );
}
async function dispatchReload(
  session: SessionState,
  request: string,
  fields: Record<string, unknown> = {},
): Promise<Array<Record<string, unknown>>> {
  return captureBridgeEventsAsync(() => executeParsedReload(session, request, fields));
}
function reloadOutcome(
  events: Array<Record<string, unknown>>,
  session: SessionState,
  request: string,
  eventName: string,
): Record<string, unknown> {
  const matching = events.filter((event) => event.event === eventName);
  assert.equal(matching.length, 1);
  const event = matching[0];
  assert.ok(event);
  assert.equal(event.session_id, session.sessionId);
  assert.equal(event.request_id, request);
  return event;
}
function assertHeldAuthorities(
  session: SessionState,
  events: Array<Record<string, unknown>>,
): void {
  assert.deepEqual(
    session.availableCommands?.commands.map((command) => command.name),
    ["before-command"],
  );
  assert.deepEqual(session.availableAgents, [
    { name: "before-agent", description: "Previous worker" },
  ]);
  assert.equal(
    events.some(
      (event) =>
        event.event === "mcp_snapshot" ||
        (event.update as { type?: string })?.type === "available_commands_update" ||
        (event.update as { type?: string })?.type === "available_agents_update",
    ),
    false,
  );
}
async function runParsedStyles(session: SessionState, request: string): Promise<void> {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      command: "reload_skills_and_styles",
      session_id: session.sessionId,
      request_id: request,
    }),
  );
  await handleSessionControlCommand(
    parsed.command as Parameters<typeof handleSessionControlCommand>[0],
    parsed.requestId,
    promptControlDeps(),
  );
}

test("parsed folder reloads refuse malformed structures before dispatch and unsafe text with a correlated unchanged-state outcome", async () => {
  await withCommandTargets(async (session) => {
    seedReloadState(session);
    let calls = 0;
    session.query = {
      close: () => {},
      reloadPlugins: async () => {
        calls++;
        return {
          plugins: [],
          commands: [],
          agents: [],
          mcpServers: [],
          held: true,
          error_count: 0,
        };
      },
    } as unknown as SessionState["query"];
    for (const invalid of [null, "folder", [3], [""], [" \t"]])
      assert.throws(
        () =>
          parseCommandEnvelope(
            JSON.stringify({
              command: "reload_plugins",
              session_id: session.sessionId,
              add_plugins: invalid,
            }),
          ),
        /local folder paths/,
      );
    for (const folder of ["bad\nfolder", "bad\u202efolder"]) {
      const events = await dispatchReload(session, "unsafe-folder-request", {
        add_plugins: [folder],
      });
      const failure = reloadOutcome(
        events,
        session,
        "unsafe-folder-request",
        "runtime_reload_failed",
      );
      assert.match(String(failure.message), /control|format/i);
      assert.equal(events.length, 1);
      assert.deepEqual(session.pluginInventory, [
        { name: "before-plugin", path: session.cwd, source: "before-plugin@inline" },
      ]);
      assertHeldAuthorities(session, events);
    }
    assert.equal(calls, 0);
    const valid = await dispatchReload(session, "safe-folder-request", {
      add_plugins: ["Local plugin"],
    });
    reloadOutcome(valid, session, "safe-folder-request", "runtime_reload_held");
    assert.equal(calls, 1);
  });
});







// Runs the command against a stubbed query; `during` runs while the reload is in flight.


test("parsed styles and skills reloads settle independently in cache-invalidation order with attributable partial failures", async () => {
  for (const failure of ["none", "styles-reject", "skills-throw", "skills-reject"]) {
    await withCommandTargets(async (session) => {
      const entered = bridgeDeferred<void>(),
        release = bridgeDeferred<void>();
      const calls: string[] = [];
      session.query = {
        close: () => {},
        reloadOutputStyles: async () => {
          calls.push("styles");
          entered.resolve();
          await release.promise;
          if (failure === "styles-reject") throw new Error("STYLES-CAUSE");
          return { available_output_styles: ["ActualStyle", "DifferentStyle"] };
        },
        reloadSkills: () => {
          calls.push("skills");
          if (failure === "skills-throw") throw new Error("SKILLS-SYNC-CAUSE");
          if (failure === "skills-reject") return Promise.reject(new Error("SKILLS-ASYNC-CAUSE"));
          return Promise.resolve({
            skills: [{ name: "actual-skill", description: "Observed", argumentHint: "" }],
          });
        },
      } as unknown as SessionState["query"];
      let pending: Promise<void> | undefined;
      const events = await captureBridgeEventsAsync(async () => {
        try {
          pending = runParsedStyles(session, "styles-skills-request");
          await boundedBridgeTask(entered.promise);
          await eventLoopCheckpoint();
          assert.deepEqual(calls, ["styles"]);
          release.resolve();
          await boundedBridgeTask(pending);
        } finally {
          release.resolve();
          if (pending) await boundedBridgeTask(pending);
        }
      });
      assert.deepEqual(calls, ["styles", "skills"]);
      const outcome = reloadOutcome(
        events,
        session,
        "styles-skills-request",
        "skills_and_styles_reloaded",
      );
      assert.equal(events.length, 1);
      assert.deepEqual(
        outcome.output_styles,
        failure === "styles-reject"
          ? { status: "failed", message: "STYLES-CAUSE" }
          : { status: "reloaded", names: ["ActualStyle", "DifferentStyle"] },
      );
      assert.deepEqual(
        outcome.skills,
        failure === "skills-throw"
          ? { status: "failed", message: "SKILLS-SYNC-CAUSE" }
          : failure === "skills-reject"
            ? { status: "failed", message: "SKILLS-ASYNC-CAUSE" }
            : { status: "reloaded", names: ["actual-skill"] },
      );
    });
  }
});

test("each live reload half honors its deadline and late native answers cannot replace the reported outcome", async () => {
  for (const held of ["styles", "skills"]) {
    await withCommandTargets(async (session) => {
      const entered = bridgeDeferred<void>(),
        release = bridgeDeferred<void>();
      const calls: string[] = [];
      const observed: Array<Record<string, unknown>> = [];
      let pending: Promise<void> | undefined;
      session.query = {
        close: () => {},
        reloadOutputStyles: async () => {
          calls.push("styles");
          if (held === "styles") {
            entered.resolve();
            await release.promise;
          }
          return { available_output_styles: ["ON-TIME-STYLE"] };
        },
        reloadSkills: async () => {
          calls.push("skills");
          if (held === "skills") {
            entered.resolve();
            await release.promise;
          }
          return { skills: [{ name: "ON-TIME-SKILL", description: "", argumentHint: "" }] };
        },
      } as unknown as SessionState["query"];
      mock.timers.enable({ apis: ["setTimeout"] });
      try {
        const events = await captureBridgeEventsAsync(
          async () => {
            try {
              pending = runParsedStyles(session, "bounded-halves");
              await boundedBridgeTask(entered.promise);
              mock.timers.tick(9999);
              await eventLoopCheckpoint();
              assert.deepEqual(observed, []);
              if (held === "styles") assert.deepEqual(calls, ["styles"]);
              mock.timers.tick(1);
              await boundedBridgeTask(pending);
              const snapshot = structuredClone(observed);
              assert.equal(snapshot.length, 1);
              release.resolve();
              await release.promise;
              await eventLoopCheckpoint();
              assert.deepEqual(observed, snapshot);
            } finally {
              release.resolve();
              mock.timers.reset();
              if (pending) await boundedBridgeTask(pending);
            }
          },
          (event) => observed.push(event),
        );
        const outcome = reloadOutcome(
          events,
          session,
          "bounded-halves",
          "skills_and_styles_reloaded",
        );
        assert.deepEqual(
          outcome.output_styles,
          held === "styles"
            ? { status: "timed_out", after_ms: 10000 }
            : { status: "reloaded", names: ["ON-TIME-STYLE"] },
        );
        assert.deepEqual(
          outcome.skills,
          held === "skills"
            ? { status: "timed_out", after_ms: 10000 }
            : { status: "reloaded", names: ["ON-TIME-SKILL"] },
        );
        assert.deepEqual(calls, ["styles", "skills"]);
      } finally {
        release.resolve();
        mock.timers.reset();
      }
    });
  }
});

test("scheduled reload expires while its actual session blocker is pending and only a later explicit request reloads", async () => {
  await withCommandTargets(async (session) => {
    const entered = bridgeDeferred<void>(),
      release = bridgeDeferred<{ percentage: number }>();
    const calls: string[] = [];
    const observed: Array<Record<string, unknown>> = [];
    const scheduler = new BridgeCommandScheduler();
    let earlier: Promise<void> | undefined, reload: Promise<void> | undefined;
    session.query = {
      close: () => {},
      getContextUsage: async () => {
        entered.resolve();
        return release.promise;
      },
      reloadOutputStyles: async () => {
        calls.push("styles");
        return { available_output_styles: ["RETRY-STYLE"] };
      },
      reloadSkills: async () => {
        calls.push("skills");
        return { skills: [{ name: "RETRY-SKILL", description: "", argumentHint: "" }] };
      },
    } as unknown as SessionState["query"];
    mock.timers.enable({ apis: ["setTimeout"] });
    try {
      const events = await captureBridgeEventsAsync(
        async () => {
          try {
            earlier = scheduleBridgeCommand(scheduler, {
              requestId: "blocked-context",
              command: { command: "get_context_usage", session_id: session.sessionId },
            });
            await boundedBridgeTask(entered.promise);
            reload = scheduleBridgeCommand(scheduler, {
              requestId: "queued-expiring-reload",
              command: { command: "reload_skills_and_styles", session_id: session.sessionId },
            });
            mock.timers.tick(9999);
            await eventLoopCheckpoint();
            assert.deepEqual(
              observed.filter((event) => event.request_id === "queued-expiring-reload"),
              [],
            );
            assert.deepEqual(calls, []);
            mock.timers.tick(1);
            await eventLoopCheckpoint();
            const expired = reloadOutcome(
              observed,
              session,
              "queued-expiring-reload",
              "skills_and_styles_reload_expired",
            );
            assert.equal(expired.after_ms, 10000);
            assert.deepEqual(calls, []);
            release.resolve({ percentage: 17 });
            await boundedBridgeTask(Promise.all([earlier, reload]));
            await boundedBridgeTask(scheduler.whenIdle());
            assert.deepEqual(calls, []);
            assert.equal(
              observed.filter((event) => event.request_id === "queued-expiring-reload").length,
              1,
            );
            await runParsedStyles(session, "explicit-reload-retry");
          } finally {
            release.resolve({ percentage: 17 });
            mock.timers.reset();
            await boundedBridgeTask(Promise.allSettled([earlier, reload]));
            await boundedBridgeTask(scheduler.whenIdle());
          }
        },
        (event) => observed.push(event),
      );
      const outcome = reloadOutcome(
        events,
        session,
        "explicit-reload-retry",
        "skills_and_styles_reloaded",
      );
      assert.deepEqual(outcome.skills, { status: "reloaded", names: ["RETRY-SKILL"] });
      assert.deepEqual(outcome.output_styles, { status: "reloaded", names: ["RETRY-STYLE"] });
      assert.deepEqual(calls, ["styles", "skills"]);
    } finally {
      release.resolve({ percentage: 17 });
      mock.timers.reset();
    }
  });
});
test("parsed plugin reload holds preserve authority and only an explicit optionless force applies the new inventories", async () => {
  await withCommandTargets(async (session) => {
    session.cwd = process.platform === "win32" ? "C:\\fixture-project" : "/fixture-project";
    seedReloadState(session);
    const args: unknown[][] = [];
    let held = true;
    session.query = {
      close: () => {},
      reloadPlugins: async (...options: unknown[]) => {
        args.push(options);
        return {
          commands: [{ name: "applied-command", description: "Actual command", argumentHint: "" }],
          agents: [{ name: "applied-agent", description: "Actual agent" }],
          plugins: [
            { name: "observed-plugin", path: session.cwd, source: "observed-plugin@inline" },
          ],
          mcpServers: [{ name: "applied-server", status: "connected", tools: [] }],
          error_count: 2,
          held,
          cache_impact: {
            mcp_servers_added: ["plugin:docs:search", "unscoped", "plugin:bad:\u202eline"],
            mcp_servers_removed: ["plugin:old:server"],
            lsp_tool_change: "adds",
          },
        };
      },
    } as unknown as SessionState["query"];
    const events = await dispatchReload(session, "held-default");
    assert.deepEqual(args, [[{ holdOnCacheImpact: true }]]);
    assertHeldAuthorities(session, events);
    const outcome = reloadOutcome(events, session, "held-default", "runtime_reload_held");
    assert.equal(outcome.error_count, 2);
    const impact = outcome.cache_impact as {
      mcp_servers_added: string[];
      mcp_servers_removed: string[];
      invalid_server_name_count: number;
      lsp_tool_change: string;
    };
    assert.deepEqual(new Set(impact.mcp_servers_added), new Set(["plugin:docs:search"]));
    assert.deepEqual(new Set(impact.mcp_servers_removed), new Set(["plugin:old:server"]));
    assert.equal(impact.invalid_server_name_count, 2);
    assert.equal(impact.lsp_tool_change, "adds");
    assert.deepEqual(events.find((event) => event.event === "plugin_inventory")?.plugins, [
      { name: "observed-plugin", path: session.cwd, source: "observed-plugin@inline" },
    ]);
    assert.equal(
      events.some((event) => event.event === "runtime_reload_completed"),
      false,
    );
    held = false;
    const applied = await dispatchReload(session, "explicit-force", { force: true });
    assert.deepEqual(args[1], []);
    reloadOutcome(applied, session, "explicit-force", "runtime_reload_completed");
    assert.deepEqual(
      session.availableCommands?.commands.map((command) => command.name),
      ["applied-command"],
    );
    assert.deepEqual(
      session.availableAgents?.map(({ name, description }) => ({ name, description })),
      [{ name: "applied-agent", description: "Actual agent" }],
    );
    assert.deepEqual(
      mcpSnapshot(applied, session.sessionId, "explicit-force", "reload_plugins").servers,
      [{ name: "applied-server", status: "connected", tools: [] }],
    );
  });
});

test("local folder intent survives parsed hold and force with independent absolute paths and partial applied inventories", async () => {
  await withCommandTargets(async (session) => {
    session.cwd = process.platform === "win32" ? "C:\\fixture-project" : "/fixture-project";
    seedReloadState(session);
    const absolute =
      process.platform === "win32"
        ? "C:\\fixture-project\\Local plugin"
        : "/fixture-project/Local plugin";
    const args: unknown[] = [];
    let held = true;
    session.query = {
      close: () => {},
      reloadPlugins: async (options: unknown) => {
        args.push(options);
        return {
          commands: [{ name: "folder-command", description: "Folder command", argumentHint: "" }],
          agents: [{ name: "folder-agent", description: "Folder agent" }],
          plugins: [
            {
              name: held ? "held-observed" : "confirmed-folder",
              path: absolute,
              source: held ? "held-observed@inline" : "confirmed-folder@inline",
              version: "2.0.0",
            },
          ],
          mcpServers: [
            { name: "folder-server", status: "connected", tools: [{ name: "folder-tool" }] },
          ],
          error_count: 3,
          held,
          cache_impact: { mcp_servers_added: ["plugin:folder:tools"], mcp_servers_removed: [] },
        };
      },
    } as unknown as SessionState["query"];
    const before = await dispatchReload(session, "folder-held", { add_plugins: ["Local plugin"] });
    assert.deepEqual(args[0], {
      holdOnCacheImpact: true,
      addPlugins: [{ type: "local", path: absolute }],
    });
    assertHeldAuthorities(session, before);
    reloadOutcome(before, session, "folder-held", "runtime_reload_held");
    assert.deepEqual(before.find((event) => event.event === "plugin_inventory")?.plugins, [
      { name: "held-observed", path: absolute, source: "held-observed@inline", version: "2.0.0" },
    ]);
    held = false;
    const applied = await dispatchReload(session, "folder-applied", {
      force: true,
      add_plugins: ["Local plugin"],
    });
    assert.deepEqual(args[1], { addPlugins: [{ type: "local", path: absolute }] });
    const outcome = reloadOutcome(applied, session, "folder-applied", "runtime_reload_completed");
    assert.equal(outcome.error_count, 3);
    assert.equal(
      applied.some(
        (event) => event.event === "runtime_reload_failed" || event.event === "runtime_reload_held",
      ),
      false,
    );
    assert.deepEqual(applied.find((event) => event.event === "plugin_inventory")?.plugins, [
      {
        name: "confirmed-folder",
        path: absolute,
        source: "confirmed-folder@inline",
        version: "2.0.0",
      },
    ]);
    const updates = applied.flatMap((event) =>
      event.update ? [event.update as Record<string, unknown>] : [],
    );
    const commandsUpdate = updates.find((update) => update.type === "available_commands_update");
    assert.ok(commandsUpdate);
    assert.ok(Array.isArray(commandsUpdate.commands));
    assert.deepEqual(
      commandsUpdate.commands.map((command: { name: string }) => command.name),
      ["folder-command"],
    );
    assert.deepEqual(updates.find((update) => update.type === "available_agents_update")?.agents, [
      { name: "folder-agent", description: "Folder agent" },
    ]);
    assert.deepEqual(
      mcpSnapshot(applied, session.sessionId, "folder-applied", "reload_plugins").servers,
      [{ name: "folder-server", status: "connected", tools: [{ name: "folder-tool" }] }],
    );
  });
});
test("native folder refusal preserves confirmed content and a corrected explicit request applies independently", async () => {
  await withCommandTargets(async (session) => {
    session.cwd = process.platform === "win32" ? "C:\\fixture-project" : "/fixture-project";
    seedReloadState(session);
    const calls: unknown[] = [];
    let rejected = true;
    session.query = {
      close: () => {},
      reloadPlugins: async (options: unknown) => {
        calls.push(options);
        if (rejected) throw new Error("MANAGED-PLUGIN-REFUSAL");
        return {
          commands: [{ name: "corrected-command", description: "Corrected", argumentHint: "" }],
          agents: [{ name: "corrected-agent", description: "Corrected" }],
          plugins: [
            { name: "corrected-plugin", path: session.cwd, source: "corrected-plugin@inline" },
          ],
          mcpServers: [],
          error_count: 0,
        };
      },
    } as unknown as SessionState["query"];
    const refused = await dispatchReload(session, "native-refused", {
      add_plugins: ["Local plugin"],
    });
    const failure = reloadOutcome(refused, session, "native-refused", "runtime_reload_failed");
    assert.equal(failure.message, "MANAGED-PLUGIN-REFUSAL");
    assert.deepEqual(session.pluginInventory, [
      { name: "before-plugin", path: session.cwd, source: "before-plugin@inline" },
    ]);
    assertHeldAuthorities(session, refused);
    assert.equal(
      refused.some(
        (event) => event.event === "plugin_inventory" || event.event === "runtime_reload_completed",
      ),
      false,
    );
    rejected = false;
    const accepted = await dispatchReload(session, "native-corrected", { force: true });
    assert.deepEqual(calls[1], undefined);
    reloadOutcome(accepted, session, "native-corrected", "runtime_reload_completed");
    assert.deepEqual(accepted.find((event) => event.event === "plugin_inventory")?.plugins, [
      { name: "corrected-plugin", path: session.cwd, source: "corrected-plugin@inline" },
    ]);
    assert.deepEqual(
      session.availableCommands?.commands.map((command) => command.name),
      ["corrected-command"],
    );
    assert.deepEqual(
      session.availableAgents?.map(({ name, description }) => ({ name, description })),
      [{ name: "corrected-agent", description: "Corrected" }],
    );
  });
});
test("an applied folder reload reports host reconciliation failure without offering native retry", async () => {
  const session = makeSessionState();
  let nativeCalls = 0;
  session.query = { reloadPlugins: async () => {
    nativeCalls += 1;
    return { plugins: [{ name: "confirmed", path: session.cwd, source: "confirmed@inline" }], commands: [], agents: [], mcpServers: [], error_count: 1 };
  } } as unknown as SessionState["query"];
  const events: Array<Record<string, unknown>> = [];
  const restore = replaceProtocolEventWriter(line => {
    const event = JSON.parse(line);
    if (event.event === "mcp_snapshot") throw new Error("MCP snapshot delivery failed");
    events.push(event);
  });
  try {
    await handleReloadPluginsCommand(session, "reconcile", false, ["confirmed"]);
    assert.equal(nativeCalls, 1);
    assert.ok(events.some(event => event.event === "plugin_inventory"));
    assert.deepEqual(events.at(-1), { request_id: "reconcile", event: "runtime_reload_completed", session_id: session.sessionId, error_count: 1, reconciliation_error: "MCP snapshot delivery failed" });
    assert.equal(events.some(event => event.event === "runtime_reload_failed"), false);
  } finally { restore(); }
});

test("closing during native folder reload suppresses late success and rejection while a live control still publishes", async () => {
  for (const closed of [false, true]) {
    for (const rejected of [false, true]) {
      await withCommandTargets(async (session) => {
        seedReloadState(session);
        const entered = bridgeDeferred<void>(),
          release = bridgeDeferred<void>();
        session.query = {
          close: () => {},
          reloadPlugins: async () => {
            entered.resolve();
            await release.promise;
            if (rejected) throw new Error("NATIVE-LATE-REFUSAL");
            return { plugins: [], commands: [], agents: [], mcpServers: [], error_count: 0 };
          },
        } as unknown as SessionState["query"];
        let pending: Promise<unknown> | undefined;
        const events = await captureBridgeEventsAsync(async () => {
          try {
            pending = executeParsedReload(session, "native-held", {
              add_plugins: ["Local plugin"],
            });
            await boundedBridgeTask(entered.promise);
            if (closed) await closeSession(session);
            release.resolve();
            await boundedBridgeTask(pending);
          } finally {
            release.resolve();
            if (pending) await boundedBridgeTask(pending);
          }
        });
        const outcome = events;
        if (closed) {
          assert.deepEqual(outcome, []);
          assert.deepEqual(session.pluginInventory, [
            { name: "before-plugin", path: session.cwd, source: "before-plugin@inline" },
          ]);
        } else
          reloadOutcome(
            outcome,
            session,
            "native-held",
            rejected ? "runtime_reload_failed" : "runtime_reload_completed",
          );
      });
    }
  }
});
test("plugin reconciliation cannot issue another native call or publish outcomes after close during either later await", async () => {
  for (const phase of ["reconnect", "status"]) {
    for (const closed of [false, true]) {
      for (const rejected of closed ? [false, true] : [false]) {
        await withCommandTargets(async (session) => {
          const entered = bridgeDeferred<void>(),
            release = bridgeDeferred<void>();
          const calls: string[] = [];
          let reloads = 0;
          const connected = { name: "observed-server", status: "connected", tools: [] };
          session.query = {
            close: () => calls.push("close"),
            reloadPlugins: async () => ({
              commands: [],
              agents: [],
              plugins: [],
              mcpServers: [reloads++ === 0 ? connected : { ...connected, status: "needs-auth" }],
              error_count: 0,
            }),
            reconnectMcpServer: async (name: string) => {
              assert.equal(name, "observed-server");
              calls.push("reconnect");
              if (phase === "reconnect") {
                entered.resolve();
                await release.promise;
                if (rejected) throw new Error("LATE-RECONNECT-CAUSE");
              }
            },
            mcpServerStatus: async () => {
              calls.push("status");
              if (phase === "status") {
                entered.resolve();
                await release.promise;
                if (rejected) throw new Error("LATE-STATUS-CAUSE");
              }
              return [connected];
            },
          } as unknown as SessionState["query"];
          await dispatchReload(session, "seed-connected");
          let pending: Promise<unknown> | undefined;
          const observed: Array<Record<string, unknown>> = [];
          let boundary = 0;
          const events = await captureBridgeEventsAsync(
            async () => {
              try {
                pending = handleReloadPluginsCommand(session, "late-reconciliation");
                await boundedBridgeTask(entered.promise);
                boundary = observed.length;
                if (closed) await closeSession(session);
                release.resolve();
                await boundedBridgeTask(pending);
              } finally {
                release.resolve();
                if (pending) await boundedBridgeTask(pending);
              }
            },
            (event) => observed.push(event),
          );
          if (closed) {
            assert.deepEqual(events.slice(boundary), []);
            assert.deepEqual(
              calls,
              phase === "reconnect" ? ["reconnect", "close"] : ["reconnect", "status", "close"],
            );
          } else {
            reloadOutcome(events, session, "late-reconciliation", "runtime_reload_completed");
            assert.deepEqual(
              mcpSnapshot(events, session.sessionId, "late-reconciliation", "reload_plugins")
                .servers,
              [connected],
            );
            assert.deepEqual(calls, ["reconnect", "status"]);
          }
        });
      }
    }
  }
});
test("styles and skills reloads stop advancing or publishing when close wins a native settlement or timeout", async () => {
  for (const phase of ["styles", "skills"]) {
    for (const result of ["resolve", "reject", "timeout"]) {
      await withCommandTargets(async (session) => {
        const entered = bridgeDeferred<void>(),
          release = bridgeDeferred<void>();
        const calls: string[] = [];
        let pending: Promise<void> | undefined;
        session.query = {
          close: () => {},
          reloadOutputStyles: async () => {
            calls.push("styles");
            if (phase === "styles") {
              entered.resolve();
              await release.promise;
              if (result === "reject") throw new Error("STYLES-LATE-CAUSE");
            }
            return { available_output_styles: ["LATE-STYLE"] };
          },
          reloadSkills: async () => {
            calls.push("skills");
            if (phase === "skills") {
              entered.resolve();
              await release.promise;
              if (result === "reject") throw new Error("SKILLS-LATE-CAUSE");
            }
            return { skills: [{ name: "LATE-SKILL", description: "", argumentHint: "" }] };
          },
        } as unknown as SessionState["query"];
        if (result === "timeout") mock.timers.enable({ apis: ["setTimeout"] });
        try {
          const events = await captureBridgeEventsAsync(async () => {
            try {
              pending = runParsedStyles(session, "closed-halves");
              await boundedBridgeTask(entered.promise);
              await closeSession(session);
              if (result === "timeout") mock.timers.tick(10000);
              else release.resolve();
              await boundedBridgeTask(pending);
              release.resolve();
              await eventLoopCheckpoint();
            } finally {
              release.resolve();
              mock.timers.reset();
              if (pending) await boundedBridgeTask(pending);
            }
          });
          assert.deepEqual(events, []);
          assert.deepEqual(calls, phase === "styles" ? ["styles"] : ["styles", "skills"]);
        } finally {
          release.resolve();
          mock.timers.reset();
        }
      });
    }
  }
});
test("handleReloadPluginsCommand emits MCP snapshot from reload result", async () => {
  const session = makeSessionState();
  let mcpServerStatusCalls = 0;
  session.query = {
    reloadPlugins: async () => ({
      commands: [],
      agents: [],
      plugins: [],
      mcpServers: [
        {
          name: "docs",
          status: "connected",
          config: {
            type: "http",
            url: "https://example.test/mcp",
          },
          tools: [],
        },
      ],
      error_count: 0,
    }),
    mcpServerStatus: async () => {
      mcpServerStatusCalls += 1;
      throw new Error("mcpServerStatus should not be called");
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  const events = await captureBridgeEventsAsync(async () => {
    await handleReloadPluginsCommand(session, "req-reload");
  });

  assert.equal(mcpServerStatusCalls, 0);
  assert.deepEqual(
    events.filter((event) => event.event === "mcp_snapshot"),
    [
      {
        request_id: "req-reload",
        event: "mcp_snapshot",
        session_id: "session-1",
        source: "reload_plugins",
        auth_capabilities: {
          authenticate: false,
          clear_auth: false,
          submit_oauth_callback_url: false,
        },
        servers: [
          {
            name: "docs",
            status: "connected",
            config: {
              type: "http",
              url: "https://example.test/mcp",
            },
            tools: [],
          },
        ],
      },
    ],
  );
  assert.deepEqual(
    events.filter((event) => event.event === "runtime_reload_completed"),
    [
      {
        request_id: "req-reload",
        event: "runtime_reload_completed",
        session_id: "session-1",
      error_count: 0,
      },
    ],
  );
});

test("handleReloadPluginsCommand revalidates stale MCP auth statuses", async () => {
  const session = makeSessionState();
  const serverName = "reload-auth-revalidation";
  let reloadCalls = 0;
  let mcpServerStatusCalls = 0;
  const reconnectCalls: string[] = [];
  const connectedServer = {
    name: serverName,
    status: "connected" as const,
    config: {
      type: "http" as const,
      url: "https://example.test/mcp",
    },
    tools: [],
  };

  session.query = {
    reloadPlugins: async () => {
      reloadCalls += 1;
      return {
        commands: [],
        agents: [],
        plugins: [],
        mcpServers:
          reloadCalls === 1
            ? [connectedServer]
            : [
                {
                  ...connectedServer,
                  status: "needs-auth" as const,
                },
              ],
        error_count: 0,
      };
    },
    reconnectMcpServer: async (name: string) => {
      reconnectCalls.push(name);
    },
    mcpServerStatus: async () => {
      mcpServerStatusCalls += 1;
      return [connectedServer];
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  await captureBridgeEventsAsync(async () => {
    await handleReloadPluginsCommand(session, "req-reload-seed");
  });
  const events = await captureBridgeEventsAsync(async () => {
    await handleReloadPluginsCommand(session, "req-reload");
  });

  assert.deepEqual(reconnectCalls, [serverName]);
  assert.equal(mcpServerStatusCalls, 1);
  assert.deepEqual(
    events.filter((event) => event.event === "mcp_snapshot"),
    [
      {
        request_id: "req-reload",
        event: "mcp_snapshot",
        session_id: "session-1",
        source: "reload_plugins",
        auth_capabilities: {
          authenticate: false,
          clear_auth: false,
          submit_oauth_callback_url: false,
        },
        servers: [connectedServer],
      },
    ],
  );
});

function dataCommandDeps():Parameters<typeof handleSessionDataCommand>[2] {
  return {generatePersistedSessionTitle,buildSessionMutationOptions,rewindTargetsFromSessionMessages,handleRewind:async()=>assert.fail("unexpected rewind delegation")};
}
async function dispatchData(session:SessionState,command:string,request:string):Promise<Array<Record<string,unknown>>> {
  const parsed=parseCommandEnvelope(JSON.stringify({command,session_id:session.sessionId,request_id:request}));
  return captureBridgeEventsAsync(async()=>handleSessionDataCommand(parsed.command as Parameters<typeof handleSessionDataCommand>[0],parsed.requestId,dataCommandDeps()));
}

test("parsed context requests publish independent bounded percentages and the returned model on the intended session", async () => {
  const rows = [
    { value: { percentage: 42.4, model: " claude-sonnet-4-7 " }, expected: 42, model: true },
    { value: { percentage: 42.6 }, expected: 43 },
    { value: { percentage: -18.6 }, expected: 0 },
    { value: { percentage: 145.2 }, expected: 100 },
    { value: { percentage: 0 }, expected: 0 },
    { value: { percentage: Number.NaN }, expected: undefined },
    { value: { percentage: Number.POSITIVE_INFINITY }, expected: undefined },
    { value: {}, expected: undefined },
    { value: { percentage: "42" }, expected: undefined },
  ];
  for (const row of rows) {
    await withCommandTargets(async (target, sibling) => {
      const calls: unknown[] = [];
      const native = {
        close: () => {},
        async getContextUsage(options: unknown) {
          assert.equal(this, native);
          calls.push(options);
          return row.value;
        },
      };
      target.query = native as unknown as SessionState["query"];
      const events = await dispatchData(target, "get_context_usage", "context-summary-request");
      assert.deepEqual(calls, [{ detail: "summary" }]);
      const replies = events.filter((event) => event.event === "context_usage");
      assert.equal(replies.length, 1);
      assert.deepEqual(replies[0], {
        event: "context_usage",
        session_id: target.sessionId,
        request_id: "context-summary-request",
        ...(row.expected === undefined ? {} : { percentage: row.expected }),
      });
      assert.equal(
        events.some((event) => event.session_id === sibling.sessionId),
        false,
      );
      assert.equal(sibling.currentModel, undefined);
      if (row.model) {
        const updates = events.filter(
          (event) => (event.update as { type?: string })?.type === "current_model_update",
        );
        assert.equal(updates.length, 1);
        const update = updates[0];
        assert.ok(update);
        assert.equal(update.session_id, target.sessionId);
        assert.equal(
          (update.update as { current_model: { resolved_id: string } }).current_model.resolved_id,
          "claude-sonnet-4-7",
        );
      }
    });
  }
});

test("structured usage normalizes named windows, aggregate costs and activity without erasing valid siblings", () => {
  const rows = [
    {
      name: "valid and malformed rate windows",
      raw: {
        rate_limits: {
          five_hour: { utilization: 11.5, resets_at: " 2026-08-12T10:00:00Z " },
          seven_day: { utilization: "bad" },
          seven_day_oauth_apps: { utilization: 18, resets_at: null },
          seven_day_opus: { utilization: -5 },
          seven_day_sonnet: { utilization: 150 },
          model_scoped: [
            { display_name: " Fable ", utilization: 33, resets_at: "2026-08-18T10:00:00Z" },
            { display_name: "Broken", utilization: Number.NaN },
            { display_name: " Zero ", utilization: -7 },
          ],
        },
      },
      expected: {
        five_hour: { utilization: 11.5, resets_at: "2026-08-12T10:00:00Z" },
        seven_day_oauth_apps: { utilization: 18 },
        seven_day_opus: { utilization: 0 },
        seven_day_sonnet: { utilization: 100 },
        model_scoped: [
          { display_name: "Fable", utilization: 33, resets_at: "2026-08-18T10:00:00Z" },
          { display_name: "Zero", utilization: 0 },
        ],
      },
    },
    {
      name: "trimmed identity and explicit unavailability",
      raw: {
        subscription_type: " max ",
        rate_limits_available: false,
        rate_limits: { extra_usage: { is_enabled: false, monthly_limit: 2000, utilization: 20 } },
      },
      expected: { subscription_type: "max", rate_limits_available: false },
    },
    {
      name: "active extra usage retains valid money and currency",
      raw: {
        rate_limits: {
          extra_usage: {
            is_enabled: true,
            monthly_limit: 2000,
            used_credits: 375,
            utilization: -4,
            currency: " USD ",
          },
        },
      },
      expected: {
        extra_usage: { monthly_limit: 2000, used_credits: 375, utilization: 0, currency: "USD" },
      },
    },
    {
      name: "SDK aggregate differs from managed model cost sum",
      raw: {
        session: {
          total_cost_usd: 7.25,
          total_api_duration_ms: 1500,
          total_duration_ms: 2000,
          total_lines_added: 12,
          total_lines_removed: 3,
          model_usage: {
            "managed-first": { costUSD: 1, inputTokens: 10, thinkingTokens: 2 },
            "managed-second": { costUSD: 2, privatePricingBasis: "SDK-owned" },
          },
        },
      },
      expected: {
        session: {
          total_cost_usd: 7.25,
          total_api_duration_ms: 1500,
          total_duration_ms: 2000,
          total_lines_added: 12,
          total_lines_removed: 3,
          model_count: 2,
        },
      },
    },
    {
      name: "activity association and invalid neighboring week",
      raw: {
        behaviors: {
          day: {
            request_count: 4.9,
            session_count: 2.8,
            behaviors: [
              { key: " future_signal ", pct: 125, count: 1.9 },
              { key: "negative", pct: 3, count: -1 },
              { key: "not-finite", pct: Number.POSITIVE_INFINITY, count: 2 },
            ],
            agents: [
              { name: " Explore ", pct: -3 },
              { name: "", pct: 99 },
            ],
            skills: [{ name: "review", pct: 20 }],
            plugins: [{ name: "plugin-good", pct: 63 }],
            mcp_servers: [{ name: "github", pct: 12 }],
          },
          week: { request_count: -1, session_count: 3 },
        },
      },
      expected: {
        activity_day: {
          request_count: 4,
          session_count: 2,
          behaviors: [{ key: "future_signal", pct: 100, count: 1 }],
          agents: [{ name: "Explore", pct: 0 }],
          skills: [{ name: "review", pct: 20 }],
          plugins: [{ name: "plugin-good", pct: 63 }],
          mcp_servers: [{ name: "github", pct: 12 }],
        },
      },
    },
    {
      name: "valid zero counts and independent weekly activity",
      raw: {
        behaviors: {
          day: { request_count: 0, session_count: 0 },
          week: {
            request_count: 100,
            session_count: 9,
            behaviors: [{ key: "weekly", pct: 17, count: 13 }],
          },
        },
      },
      expected: {
        activity_day: {
          request_count: 0,
          session_count: 0,
          behaviors: [],
          agents: [],
          skills: [],
          plugins: [],
          mcp_servers: [],
        },
        activity_week: {
          request_count: 100,
          session_count: 9,
          behaviors: [{ key: "weekly", pct: 17, count: 13 }],
          agents: [],
          skills: [],
          plugins: [],
          mcp_servers: [],
        },
      },
    },
    {
      name: "malformed optional money does not erase a valid sibling",
      raw: {
        session: { total_cost_usd: Number.POSITIVE_INFINITY, total_duration_ms: 19 },
        rate_limits: { five_hour: { utilization: Number.NaN }, seven_day: { utilization: 7 } },
      },
      expected: { session: { total_duration_ms: 19 }, seven_day: { utilization: 7 } },
    },
    ...[null, [], "incompatible", {}, 17].map((raw) => ({
      name: `incompatible root ${JSON.stringify(raw)}`,
      raw,
      expected: {},
    })),
  ];
  for (const row of rows) {
    const before = structuredClone(row.raw);
    assert.deepEqual(normalizeStructuredUsage(row.raw), row.expected, row.name);
    assert.deepEqual(row.raw, before, `${row.name} leaves its SDK input unchanged`);
  }
});
test("usage query failures settle their original requests and allow a later successful context or structured query", async () => {
  await withCommandTargets(async (target) => {
    target.query = {
      close: () => {},
      getContextUsage: async () => {
        throw new Error("CONTEXT-QUERY-CAUSE");
      },
    } as unknown as SessionState["query"];
    const context = await dispatchData(target, "get_context_usage", "failed-context-request");
    assert.deepEqual(context, [
      {
        event: "context_usage",
        session_id: target.sessionId,
        request_id: "failed-context-request",
      },
    ]);
    target.query = {
      close: () => {},
      getContextUsage: async () => ({ percentage: 36.6 }),
    } as unknown as SessionState["query"];
    assert.deepEqual(await dispatchData(target, "get_context_usage", "recovered-context-request"), [
      {
        event: "context_usage",
        session_id: target.sessionId,
        request_id: "recovered-context-request",
        percentage: 37,
      },
    ]);
    const method = "usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET";
    for (const row of [
      { kind: "absent", cause: /unavailable/i },
      { kind: "rejected", cause: /STRUCTURED-QUERY-CAUSE/ },
      { kind: "empty", cause: /incompatible|empty/i },
      { kind: "incompatible", cause: /incompatible|empty/i },
    ]) {
      target.query = {
        close: () => {},
        ...(row.kind === "absent"
          ? {}
          : {
              [method]: async () => {
                if (row.kind === "rejected") throw new Error("STRUCTURED-QUERY-CAUSE");
                return row.kind === "empty" ? {} : [];
              },
            }),
      } as unknown as SessionState["query"];
      const events = await dispatchData(target, "get_usage", `usage-${row.kind}`);
      assert.equal(events.length, 1);
      assert.equal(events[0]?.event, "usage_snapshot");
      assert.equal(events[0]?.session_id, target.sessionId);
      assert.equal(events[0]?.request_id, `usage-${row.kind}`);
      assert.match(String(events[0]?.error), row.cause);
      assert.equal(events[0]?.snapshot, undefined);
    }
    target.query = {
      close: () => {},
      [method]: async () => ({ rate_limits: { five_hour: { utilization: 27 } } }),
    } as unknown as SessionState["query"];
    assert.deepEqual(await dispatchData(target, "get_usage", "recovered-structured-request"), [
      {
        event: "usage_snapshot",
        session_id: target.sessionId,
        request_id: "recovered-structured-request",
        snapshot: { five_hour: { utilization: 27 } },
      },
    ]);
  });
});

test("parsed rewind-target requests use active or saved directories and distinguish empty history from a recoverable read failure", async () => {
  const { spawnSync } = await import("node:child_process");
  const { fileURLToPath } = await import("node:url");
  const child = spawnSync(
    process.execPath,
    [
      fileURLToPath(new URL("../../tests/fixtures/bridge-rewind-runtime.mjs", import.meta.url)),
      "targets",
    ],
    { encoding: "utf8", windowsHide: true, timeout: 15000 },
  );
  assert.equal(child.error, undefined);
  assert.equal(child.status, 0, child.stderr);
  const result = JSON.parse(child.stdout) as {
    ids: Record<string, string>;
    dirs: Record<string, string>;
    journal: Array<{
      request?: string;
      method: string;
      id?: string;
      options: Record<string, unknown>;
    }>;
    responses: Array<Record<string, unknown>>;
  };
  for (const [key, session] of [
    ["active", "active"],
    ["saved", "saved"],
    ["empty", "empty"],
    ["failure", "failure"],
    ["recovered", "failure"],
  ]) {
    const request = `targets-${key}`,
      responses = result.responses.filter((event) => event.request_id === request);
    assert.equal(responses.length, 1);
    const response = responses[0];
    assert.ok(response);
    assert.equal(response.event, "rewind_targets");
    assert.equal(response.session_id, result.ids[session]);
    const reads = result.journal.filter(
      (call) => call.request === request && call.method === "getSessionMessages",
    );
    assert.equal(reads.length, 1);
    assert.equal(reads[0]?.id, result.ids[session]);
    assert.deepEqual(reads[0]?.options, { dir: result.dirs[session], includeSystemMessages: true });
    const lists = result.journal.filter(
      (call) => call.request === request && call.method === "listSessions",
    );
    assert.equal(lists.length, key === "active" ? 0 : 1);
    for (const call of lists) {
      assert.equal(call.options.dir, result.dirs.listing);
      assert.equal(call.options.includeProgrammatic, true);
      assert.equal(call.options.includeWorktrees, true);
    }
    if (key === "active")
      assert.deepEqual(response.targets, [
        {
          uuid: "active-latest",
          first_text: "Active latest input",
          input_text: "Active latest\ninput",
          index: 3,
          previous_assistant_uuid: "active-reply",
          resume_anchor_uuid: "active-tool",
        },
        {
          uuid: "active-first",
          first_text: "Active first input",
          input_text: "Active first input",
          index: 0,
        },
      ]);
    else if (key === "saved")
      assert.deepEqual(response.targets, [
        {
          uuid: "saved-latest",
          first_text: "Saved latest input",
          input_text: "Saved latest\ninput",
          index: 3,
          previous_assistant_uuid: "saved-reply",
          resume_anchor_uuid: "saved-tool",
        },
        {
          uuid: "saved-first",
          first_text: "Saved first input",
          input_text: "Saved first input",
          index: 0,
        },
      ]);
    else if (key === "recovered")
      assert.deepEqual(response.targets, [
        {
          uuid: "recovered-user",
          first_text: "Recovered input",
          input_text: "Recovered input",
          index: 0,
        },
      ]);
    else assert.deepEqual(response.targets, []);
    if (key === "failure") assert.match(String(response.error), /OWNED-HISTORY-READ-CAUSE/);
    else assert.equal(response.error, undefined);
  }
});
test("scheduled rewind validates before applying files and reports success, refusal, recovery and post-apply partial failure", async () => {
  const { spawnSync } = await import("node:child_process");
  const { fileURLToPath } = await import("node:url");
  const child = spawnSync(
    process.execPath,
    [fileURLToPath(new URL("../../tests/fixtures/bridge-rewind-application.mjs", import.meta.url))],
    { encoding: "utf8", windowsHide: true, timeout: 15000 },
  );
  assert.equal(child.error, undefined);
  assert.equal(child.status, 0, child.stderr);
  interface Call {
    method: string;
    index?: number;
    phase?: string;
    target?: string;
    id?: string;
    options?: Record<string, unknown>;
  }
  interface Snapshot {
    bytes: string;
    events: Array<Record<string, unknown>>;
    journal: Call[];
    candidatesClosed: boolean;
    activeIds: string[];
  }
  const reports = JSON.parse(child.stdout) as Array<{
    name: string;
    sourceId: string;
    cwd: string;
    file: string;
    operation: Snapshot;
    validationBarriers?: Array<{ stage: string; bytes: string; applications: number }>;
    finalBytes: string;
    events: Array<Record<string, unknown>>;
    journal: Call[];
    allQueriesClosed: boolean;
  }>;
  const before = "original file\nkept before rewind\n",
    after = "restored checkpoint\nindependent file bytes\n";
  assert.deepEqual(
    reports.map((row) => row.name),
    [
      "conversation",
      "code",
      "both",
      "first-prompt",
      "dry-refusal",
      "init-failure",
      "guard-failure",
      "apply-failure",
      "post-apply-failure",
    ],
  );
  for (const row of reports) {
    const request = `operation-${row.name}`;
    const responses = row.operation.events.filter((event) => event.request_id === request);
    const replacements = responses.filter((event) => event.event === "session_replaced");
    const results = responses.filter((event) => event.event === "rewind_result");
    const errors = responses.filter((event) => event.event === "slash_error");
    const calls = row.operation.journal.filter((call) => call.method === "rewindFiles");
    const applications = calls.filter((call) => call.options?.dryRun === false);
    const candidates = row.operation.journal.filter(
      (call) => call.method === "query" && call.index === 1,
    );
    const fileResult = {
      can_rewind: true,
      files_changed: [row.file],
      insertions: 13,
      deletions: 4,
      skipped_links: 2,
    };
    assert.equal(row.allQueriesClosed, true, row.name);
    assert.equal(
      row.events.some((event) => event.event === "connection_failed"),
      false,
      row.name,
    );
    for (const call of calls) {
      assert.equal(call.index, 0);
      assert.equal(call.target, "selected-input");
    }
    if (applications.length && row.name !== "code") {
      const initialized = row.operation.journal.findIndex(
        (call) => call.method === "init-complete" && call.index === 1,
      );
      const guard = row.operation.journal.findIndex(
        (call) => call.method === "context" && call.index === 1,
      );
      const apply = row.operation.journal.findIndex(
        (call) => call.method === "rewindFiles" && call.options?.dryRun === false,
      );
      assert.ok(initialized >= 0 && initialized < guard && guard < apply, row.name);
    }
    for (const call of row.operation.journal.filter((call) => call.method === "history")) {
      assert.equal(call.id, row.sourceId);
      assert.deepEqual(call.options, { dir: row.cwd, includeSystemMessages: true });
    }
    if (["conversation", "code", "both", "first-prompt"].includes(row.name)) {
      assert.deepEqual(errors, [], row.name);
      assert.equal(row.operation.bytes, ["code", "both"].includes(row.name) ? after : before);
      if (row.name === "code") {
        assert.deepEqual(replacements, []);
        assert.deepEqual(candidates, []);
        assert.deepEqual(row.operation.activeIds, [row.sourceId]);
      } else {
        assert.equal(replacements.length, 1);
        const replacement = replacements[0];
        assert.ok(replacement);
        assert.equal(typeof replacement.session_id, "string");
        assert.notEqual(replacement.session_id, row.sourceId);
        assert.deepEqual(row.operation.activeIds, [replacement.session_id]);
        assert.equal(
          replacement.restored_input,
          row.name === "first-prompt" ? "FIRST input\n exact  " : "SELECTED input\n exact  ",
        );
        assert.equal(candidates.length, 1);
        assert.deepEqual(
          candidates[0]?.options,
          row.name === "first-prompt"
            ? { cwd: row.cwd, sessionId: replacement.session_id }
            : {
                cwd: row.cwd,
                sessionId: replacement.session_id,
                resume: row.sourceId,
                resumeSessionAt: "kept-tool-anchor",
                resumeDropsTurn: "selected-input",
                forkSession: true,
              },
        );
        if (row.name === "first-prompt") assert.equal(replacement.history_updates, undefined);
        else {
          const updates = replacement.history_updates as Array<Record<string, unknown>>;
          assert.deepEqual(
            updates.map(
              (update) =>
                update.source_message_uuid ??
                (update.tool_call as Record<string, unknown> | undefined)?.source_message_uuid ??
                (update.tool_call_update as Record<string, unknown> | undefined)
                  ?.source_message_uuid,
            ),
            ["first-input", "kept-assistant", "kept-assistant", "kept-tool-anchor"],
          );
          assert.deepEqual(
            updates
              .filter(
                (update) =>
                  update.type === "user_message_chunk" || update.type === "agent_message_chunk",
              )
              .map((update) => update.content),
            [
              { type: "text", text: "FIRST input\n exact  " },
              { type: "text", text: "Retained answer" },
            ],
          );
        }
      }
      if (row.name === "conversation" || row.name === "first-prompt") {
        assert.deepEqual(calls, []);
        assert.deepEqual(results, []);
      } else {
        assert.deepEqual(
          calls.map((call) => call.options),
          [{ dryRun: true }, { dryRun: false }],
        );
        assert.equal(results.length, 1);
        assert.deepEqual(results[0], {
          request_id: request,
          event: "rewind_result",
          session_id: row.name === "code" ? row.sourceId : replacements[0]?.session_id,
          restore_mode: row.name,
          status: "success",
          file_result: fileResult,
        });
      }
      if (row.name === "both") {
        assert.deepEqual(row.validationBarriers, [
          { stage: "initialization", bytes: before, applications: 0 },
          { stage: "guard", bytes: before, applications: 0 },
        ]);
        const guard = row.operation.journal.findIndex(
          (call) => call.method === "context" && call.index === 1,
        );
        const apply = row.operation.journal.findIndex(
          (call) => call.method === "rewindFiles" && call.options?.dryRun === false,
        );
        assert.ok(guard >= 0 && guard < apply);
        assert.deepEqual(row.operation.journal[guard]?.options, { detail: "summary" });
      }
    } else {
      assert.deepEqual(replacements, [], row.name);
      assert.deepEqual(row.operation.activeIds, [row.sourceId]);
      assert.equal(row.operation.candidatesClosed, true, row.name);
      assert.equal(candidates.length, row.name === "dry-refusal" ? 0 : 1);
      assert.equal(row.operation.bytes, row.name === "post-apply-failure" ? after : before);
      assert.deepEqual(
        calls.map((call) => call.options),
        ["apply-failure", "post-apply-failure"].includes(row.name)
          ? [{ dryRun: true }, { dryRun: false }]
          : [{ dryRun: true }],
      );
      if (row.name === "post-apply-failure") {
        assert.deepEqual(errors, []);
        assert.equal(results.length, 1);
        assert.equal(results[0]?.event, "rewind_result");
        assert.equal(results[0]?.status, "partial_failure");
        assert.equal(results[0]?.session_id, row.sourceId);
        assert.equal(results[0]?.restore_mode, "both");
        assert.deepEqual(results[0]?.file_result, fileResult);
        assert.match(String(results[0]?.message), /candidate session is not ready to commit/);
        assert.equal(applications.length, 1);
      } else {
        assert.deepEqual(results, []);
        assert.equal(errors.length, 1);
        assert.equal(errors[0]?.session_id, row.sourceId);
        const cause = {
          "dry-refusal": "OWNED-DRY-REFUSAL",
          "init-failure": "OWNED-CANDIDATE-INIT-CAUSE",
          "guard-failure": "OWNED-GUARD-CAUSE",
          "apply-failure": "OWNED-APPLY-CAUSE",
        }[row.name];
        assert.ok(cause);
        assert.ok(String(errors[0]?.message).includes(cause));
      }
      const usable = row.events.filter((event) => event.request_id === `usable-${row.name}`);
      assert.equal(usable.length, 1);
      assert.equal(usable[0]?.event, "context_usage");
      assert.equal(usable[0]?.session_id, row.sourceId);
      assert.equal(usable[0]?.percentage, 23);
      const retry = row.events.filter((event) => event.request_id === `retry-${row.name}`);
      assert.deepEqual(retry, [
        {
          request_id: `retry-${row.name}`,
          event: "rewind_result",
          session_id: row.sourceId,
          restore_mode: "code",
          status: "success",
          file_result: fileResult,
        },
      ]);
      assert.equal(row.finalBytes, after);
      assert.deepEqual(
        row.journal
          .filter((call) => call.method === "rewindFiles" && call.phase === "recovery")
          .map((call) => call.options),
        [{ dryRun: true }, { dryRun: false }],
      );
    }
  }
});
test("file rewind projection preserves valid siblings while rejecting only malformed skipped-link observations", () => {
  for (const row of [
    { value: undefined, expected: undefined },
    { value: 0, expected: 0 },
    { value: 7, expected: 7 },
    { value: -1, expected: undefined },
    { value: 1.2, expected: undefined },
    { value: Number.NaN, expected: undefined },
    { value: Number.POSITIVE_INFINITY, expected: undefined },
  ]) {
    const actual = mapRewindFilesResult({
      canRewind: true,
      filesChanged: ["src/one.rs", "src/two.ts"],
      insertions: 17,
      deletions: 3,
      ...(row.value === undefined ? {} : { skippedLinks: row.value }),
    });
    assert.deepEqual(actual, {
      can_rewind: true,
      files_changed: ["src/one.rs", "src/two.ts"],
      insertions: 17,
      deletions: 3,
      ...(row.expected === undefined ? {} : { skipped_links: row.expected }),
    });
  }
  assert.deepEqual(
    mapRewindFilesResult({
      canRewind: false,
      error: "CHECKPOINT-MISSING",
      filesChanged: [],
      insertions: 0,
      deletions: 0,
      skippedLinks: 4,
    }),
    {
      can_rewind: false,
      error: "CHECKPOINT-MISSING",
      files_changed: [],
      insertions: 0,
      deletions: 0,
      skipped_links: 4,
    },
  );
});

test("rewind parsing delegates exact identities, restore modes and launch choices while refusing invalid modes", async () => {
  const calls: Array<{ command: unknown; request?: string }> = [];
  const deps = {
    ...dataCommandDeps(),
    handleRewind: async (
      command: Parameters<Parameters<typeof handleSessionDataCommand>[2]["handleRewind"]>[0],
      request?: string,
    ) => {
      calls.push({ command, request });
    },
  };
  for (const restore of ["both", "conversation", "code"]) {
    const parsed = parseCommandEnvelope(
      JSON.stringify({
        request_id: `rewind-${restore}`,
        command: "rewind",
        session_id: "rewind-source-owner",
        target_user_message_id: "selected-user",
        restore_mode: restore,
        launch_settings: { model: "sonnet", permission_mode: "acceptEdits" },
      }),
    );
    await handleSessionDataCommand(
      parsed.command as Parameters<typeof handleSessionDataCommand>[0],
      parsed.requestId,
      deps,
    );
    assert.deepEqual(calls.at(-1), {
      request: `rewind-${restore}`,
      command: {
        command: "rewind",
        session_id: "rewind-source-owner",
        target_user_message_id: "selected-user",
        restore_mode: restore,
        launch_settings: { model: "sonnet", permission_mode: "acceptEdits" },
      },
    });
  }
  await assert.rejects(async () => {
    const parsed = parseCommandEnvelope(
      JSON.stringify({
        command: "rewind",
        session_id: "rewind-source-owner",
        target_user_message_id: "selected-user",
        restore_mode: "future-mode",
        launch_settings: {},
      }),
    );
    await handleSessionDataCommand(
      parsed.command as Parameters<typeof handleSessionDataCommand>[0],
      parsed.requestId,
      deps,
    );
  }, /restore_mode/);
  assert.equal(calls.length, 3);
});

test("rewindTargetsFromSessionMessages filters user text messages with UUIDs", () => {
  const messages = [
    {
      type: "user",
      uuid: "user-1",
      message: {
        role: "user",
        content: [{ type: "text", text: " first prompt\nline " }],
      },
    },
    {
      type: "assistant",
      uuid: "assistant-1",
      message: {
        role: "assistant",
        content: [{ type: "text", text: "ignored" }],
      },
    },
    {
      type: "user",
      uuid: "tool-result",
      message: {
        role: "user",
        content: [{ type: "tool_result", content: "ignored" }],
      },
    },
    {
      type: "user",
      uuid: "user-2",
      message: {
        role: "user",
        content: [{ type: "text", text: "second prompt" }],
      },
    },
  ] as unknown as SessionMessage[];

  assert.deepEqual(rewindTargetsFromSessionMessages(messages), [
    {
      uuid: "user-2",
      first_text: "second prompt",
      input_text: "second prompt",
      index: 3,
      previous_assistant_uuid: "assistant-1",
      resume_anchor_uuid: "tool-result",
    },
    {
      uuid: "user-1",
      first_text: "first prompt line",
      input_text: " first prompt\nline ",
      index: 0,
    },
  ]);
});

test("rewind plans retain exact earlier source associations and distinguish previous replies from the true chain anchor", () => {
  const first = {
    type: "user",
    uuid: "user-first",
    message: { role: "user", content: "FIRST-INPUT" },
  };
  const reply = {
    type: "assistant",
    uuid: "reply-first",
    message: { role: "assistant", content: [{ type: "text", text: "FIRST-REPLY" }] },
  };
  const selected = {
    type: "user",
    uuid: "user-selected",
    message: { role: "user", content: "SELECTED-INPUT\n exact  " },
  };
  const laterReply = {
    type: "assistant",
    uuid: "reply-later",
    message: { role: "assistant", content: "LATER-REPLY" },
  };
  const later = {
    type: "user",
    uuid: "user-later",
    message: { role: "user", content: "LATER-INPUT" },
  };
  const structure = {
    type: "system",
    uuid: "structured-anchor",
    subtype: "structured_output",
    data: { result: "KEPT-STRUCTURE" },
  };
  const toolReply = {
    type: "assistant",
    uuid: "reply-first",
    message: {
      role: "assistant",
      content: [
        { type: "text", text: "FIRST-REPLY" },
        { type: "tool_use", id: "kept-tool", name: "Bash", input: { command: "echo kept" } },
      ],
    },
  };
  const toolResult = {
    type: "user",
    uuid: "tool-anchor",
    message: {
      role: "user",
      content: [{ type: "tool_result", tool_use_id: "kept-tool", content: "KEPT-TOOL-RECEIPT" }],
    },
  };
  const rows = [
    {
      name: "latest",
      messages: [first, reply, selected],
      ids: ["user-first", "reply-first"],
      anchor: "reply-first",
      previous: "reply-first",
      guard: "user-selected",
      index: 2,
    },
    {
      name: "queued",
      messages: [first, selected],
      ids: ["user-first"],
      anchor: "user-first",
      previous: undefined,
      guard: "user-selected",
      index: 1,
    },
    {
      name: "older",
      messages: [first, reply, selected, laterReply, later],
      ids: ["user-first", "reply-first"],
      anchor: "reply-first",
      previous: "reply-first",
      guard: undefined,
      index: 2,
    },
    {
      name: "structured",
      messages: [first, reply, structure, selected],
      ids: ["user-first", "reply-first", "structured-anchor"],
      anchor: "structured-anchor",
      previous: "reply-first",
      guard: "user-selected",
      index: 3,
    },
    {
      name: "tool",
      messages: [first, toolReply, toolResult, selected],
      ids: ["user-first", "reply-first", "tool-anchor"],
      anchor: "tool-anchor",
      previous: "reply-first",
      guard: "user-selected",
      index: 3,
    },
  ];
  for (const row of rows) {
    const plan = buildRewindConversationPlan(
      row.messages as unknown as SessionMessage[],
      "user-selected",
    );
    assert.ok(plan, row.name);
    assert.equal(plan.inputText, "SELECTED-INPUT\n exact  ");
    assert.equal(plan.previousAssistantUuid, row.previous);
    assert.equal(plan.resumeSessionAtUuid, row.anchor);
    assert.equal(plan.resumeDropsTurnId, row.guard);
    assert.equal(plan.targetIndex, row.index);
    assert.deepEqual(
      plan.retainedMessages.map((message) => message.uuid),
      row.ids,
    );
    const chunks = plan.resumeUpdates.flatMap((update) =>
      update.type === "user_message_chunk" || update.type === "agent_message_chunk"
        ? [
            {
              type: update.type,
              source: update.source_message_uuid,
              text: update.content.type === "text" ? update.content.text : undefined,
            },
          ]
        : [],
    );
    assert.deepEqual(
      chunks,
      row.name === "queued"
        ? [{ type: "user_message_chunk", source: "user-first", text: "FIRST-INPUT" }]
        : [
            { type: "user_message_chunk", source: "user-first", text: "FIRST-INPUT" },
            { type: "agent_message_chunk", source: "reply-first", text: "FIRST-REPLY" },
          ],
    );
    assert.doesNotMatch(
      JSON.stringify(plan.resumeUpdates),
      /SELECTED-INPUT|LATER-INPUT|LATER-REPLY/,
    );
    if (row.name === "tool")
      assert.ok(
        plan.resumeUpdates.some(
          (update) =>
            update.type === "tool_call_update" &&
            update.tool_call_update.tool_call_id === "kept-tool" &&
            update.tool_call_update.source_message_uuid === "tool-anchor" &&
            update.tool_call_update.fields.raw_output === "KEPT-TOOL-RECEIPT",
        ),
      );
    assert.equal(
      buildRewindConversationPlan(row.messages as unknown as SessionMessage[], "stale-user"),
      null,
    );
  }
});
test("buildRewindConversationPlan treats first user message as fresh replacement", () => {
  const messages = [
    {
      type: "user",
      uuid: "user-1",
      message: { role: "user", content: " first prompt\nline " },
    },
    {
      type: "assistant",
      uuid: "assistant-1",
      message: {
        role: "assistant",
        content: [{ type: "text", text: "reply" }],
      },
    },
  ] as unknown as SessionMessage[];

  const plan = buildRewindConversationPlan(messages, "user-1");

  assert.ok(plan);
  assert.equal(plan.inputText, " first prompt\nline ");
  assert.equal(plan.previousAssistantUuid, undefined);
  assert.equal(plan.resumeSessionAtUuid, undefined);
  assert.equal(plan.resumeDropsTurnId, undefined);
  assert.equal(plan.targetIndex, 0);
  assert.deepEqual(plan.retainedMessages, []);
  assert.deepEqual(plan.resumeUpdates, []);
});

test("buildRewindConversationPlan rejects empty and duplicate chain UUIDs", () => {
  const duplicate = [
    { type: "user", uuid: "same", message: { role: "user", content: "first" } },
    {
      type: "assistant",
      uuid: "same",
      message: { role: "assistant", content: "reply" },
    },
  ] as unknown as SessionMessage[];
  const empty = [
    {
      type: "user",
      uuid: "user-1",
      message: { role: "user", content: "first" },
    },
    {
      type: "assistant",
      uuid: " ",
      message: { role: "assistant", content: "reply" },
    },
  ] as unknown as SessionMessage[];

  assert.equal(buildRewindConversationPlan(duplicate, "same"), null);
  assert.equal(buildRewindConversationPlan(empty, "user-1"), null);
});

test("MCP reconciliation reconnects only eligible native statuses and honors the exact cooldown boundary", async () => {
  mock.timers.enable({ apis: ["Date"], now: 100_000 });
  try {
    await withCommandTargets(async (target, sibling) => {
      type NativeStatus = import("@anthropic-ai/claude-agent-sdk").McpServerStatus;
      let generation = 0,
        peerReads = 0;
      const reconnects: Array<{ name: string; time: number }> = [];
      const statuses = (eligible: string): NativeStatus[] =>
        [
          {
            name: "eligible",
            status: eligible,
            serverInfo: { name: "eligible-native", version: `generation-${generation}` },
          },
          { name: "never-connected", status: "needs-auth", error: "NEVER-AUTHORIZED" },
          {
            name: "still-connected",
            status: "connected",
            serverInfo: { name: "stable-native", version: "stable-v" },
          },
        ] as NativeStatus[];
      let nativeStatuses = statuses("connected");
      const native = {
        close: () => {},
        async mcpServerStatus() {
          assert.equal(this, native);
          return nativeStatuses;
        },
        async reconnectMcpServer(name: string) {
          assert.equal(this, native);
          reconnects.push({ name, time: Date.now() });
          generation++;
          nativeStatuses = statuses("connected");
        },
      };
      target.query = native as unknown as SessionState["query"];
      sibling.query = {
        close: () => {},
        mcpServerStatus: async () => {
          peerReads++;
          return [];
        },
      } as unknown as SessionState["query"];
      const snapshot = async (
        request: string,
        expected: Array<{ name: string; status: string; version?: string; error?: string }>,
      ) => {
        const events = await captureBridgeEventsAsync(async () => {
          await emitMcpSnapshotEvent(target, request);
        });
        assert.equal(events.length, 1);
        const event = events[0];
        assert.ok(event);
        assert.equal(event.event, "mcp_snapshot");
        assert.equal(event.session_id, target.sessionId);
        assert.equal(event.request_id, request);
        assert.equal(event.source, "mcp_status");
        const rows = event.servers as Array<{
          name: string;
          status: string;
          server_info?: { version?: string };
          error?: string;
        }>;
        assert.deepEqual(
          rows.map((row) => ({
            name: row.name,
            status: row.status,
            ...(row.server_info?.version ? { version: row.server_info.version } : {}),
            ...(row.error ? { error: row.error } : {}),
          })),
          expected,
        );
      };
      const expected = (status: string, version: string) => [
        { name: "eligible", status, version },
        { name: "never-connected", status: "needs-auth", error: "NEVER-AUTHORIZED" },
        { name: "still-connected", status: "connected", version: "stable-v" },
      ];
      await snapshot("mcp-policy-seed", expected("connected", "generation-0"));
      assert.deepEqual(reconnects, []);
      nativeStatuses = statuses("needs-auth");
      await snapshot("mcp-policy-first", expected("connected", "generation-1"));
      assert.deepEqual(reconnects, [{ name: "eligible", time: 100_000 }]);
      nativeStatuses = statuses("needs-auth");
      mock.timers.tick(29_999);
      await snapshot("mcp-policy-before", expected("needs-auth", "generation-1"));
      assert.deepEqual(reconnects, [{ name: "eligible", time: 100_000 }]);
      mock.timers.tick(1);
      await snapshot("mcp-policy-at", expected("connected", "generation-2"));
      assert.deepEqual(reconnects, [
        { name: "eligible", time: 100_000 },
        { name: "eligible", time: 130_000 },
      ]);
      assert.equal(peerReads, 0);
    });
  } finally {
    mock.timers.reset();
  }
});

test("MCP connection history and cooldown remain independent for two queries sharing a server name", async () => {
  mock.timers.enable({ apis: ["Date"], now: 200_000 });
  try {
    await withCommandTargets(async (first, second) => {
      type NativeStatus = import("@anthropic-ai/claude-agent-sdk").McpServerStatus;
      const reconnects: string[] = [];
      const controls = [
        { session: first, name: "first-native", connected: true },
        { session: second, name: "second-native", connected: false },
      ];
      for (const control of controls) {
        const native = {
          close: () => {},
          async mcpServerStatus() {
            assert.equal(this, native);
            return [
              {
                name: "shared-docs",
                status: control.connected ? "connected" : "needs-auth",
                serverInfo: { name: control.name, version: "owned-v" },
              },
            ] as NativeStatus[];
          },
          async reconnectMcpServer(name: string) {
            assert.equal(this, native);
            assert.equal(name, "shared-docs");
            reconnects.push(control.name);
            control.connected = true;
          },
        };
        control.session.query = native as unknown as SessionState["query"];
      }
      const observe = async (index: number, request: string, status: string) => {
        const control = controls[index];
        assert.ok(control);
        const events = await captureBridgeEventsAsync(async () => {
          await emitMcpSnapshotEvent(control.session, request);
        });
        assert.equal(events.length, 1);
        const event = events[0];
        assert.ok(event);
        assert.equal(event.event, "mcp_snapshot");
        assert.equal(event.session_id, control.session.sessionId);
        assert.equal(event.request_id, request);
        assert.deepEqual(
          (event.servers as Array<{ name: string; status: string; server_info: unknown }>).map(
            (row) => ({ name: row.name, status: row.status, server_info: row.server_info }),
          ),
          [
            {
              name: "shared-docs",
              status,
              server_info: { name: control.name, version: "owned-v" },
            },
          ],
        );
      };
      await observe(0, "first-seed", "connected");
      controls[0].connected = false;
      await observe(1, "second-never", "needs-auth");
      assert.deepEqual(reconnects, []);
      await observe(0, "first-repair", "connected");
      assert.deepEqual(reconnects, ["first-native"]);
      controls[1].connected = true;
      await observe(1, "second-seed", "connected");
      controls[1].connected = false;
      await observe(1, "second-independent-repair", "connected");
      assert.deepEqual(reconnects, ["first-native", "second-native"]);
      controls[0].connected = false;
      controls[1].connected = false;
      await observe(0, "first-in-cooldown", "needs-auth");
      await observe(1, "second-in-cooldown", "needs-auth");
      assert.deepEqual(reconnects, ["first-native", "second-native"]);
      mock.timers.tick(30_000);
      await observe(0, "first-next-repair", "connected");
      await observe(1, "second-next-repair", "connected");
      assert.deepEqual(reconnects, [
        "first-native",
        "second-native",
        "first-native",
        "second-native",
      ]);
    });
  } finally {
    mock.timers.reset();
  }
});

test("persisted title generation uses its query receiver and exact persist intent and refuses unsupported or empty results", async () => {
  const calls: Array<{ description: string; options: unknown }> = [];
  const native = {
    async generateSessionTitle(description: string, options: unknown) {
      assert.equal(this, native);
      calls.push({ description, options });
      return "Independent generated title";
    },
  };
  assert.equal(
    await generatePersistedSessionTitle(
      native as unknown as SessionState["query"],
      "Chosen summary\n preserve whitespace  ",
    ),
    "Independent generated title",
  );
  assert.deepEqual(calls, [
    { description: "Chosen summary\n preserve whitespace  ", options: { persist: true } },
  ]);
  for (const unsupported of [{}, { generateSessionTitle: "not callable" }])
    await assert.rejects(
      generatePersistedSessionTitle(
        unsupported as unknown as SessionState["query"],
        "Unsupported description",
      ),
      /does not support generateSessionTitle/,
    );
  for (const title of [undefined, null, "", " \n ", 7])
    await assert.rejects(
      generatePersistedSessionTitle(
        { generateSessionTitle: async () => title } as unknown as SessionState["query"],
        "Invalid title",
      ),
      /did not return a generated session title/,
    );
  assert.equal(calls.length, 1);
});

test("query launch options preserve distinct fresh, resumed and forked identities and the supplied permission decision", async () => {
  for (const row of [
    {
      name: "fresh-inherited",
      resume: undefined,
      fork: false,
      launch: {},
      model: undefined,
      mode: undefined,
      dangerous: undefined,
    },
    {
      name: "resume-plan",
      resume: "original-session-id",
      fork: false,
      launch: { model: "claude-opus-4-7", permission_mode: "plan" as const },
      model: "claude-opus-4-7",
      mode: "plan",
      dangerous: undefined,
    },
    {
      name: "fork-auto",
      resume: "original-session-id",
      fork: true,
      launch: { permission_mode: "auto" as const },
      model: undefined,
      mode: "auto",
      dangerous: undefined,
    },
    {
      name: "fork-bypass",
      resume: "original-session-id",
      fork: true,
      launch: { permission_mode: "bypassPermissions" as const },
      model: undefined,
      mode: "bypassPermissions",
      dangerous: true,
    },
  ]) {
    const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
    const signal = new AbortController().signal;
    const calls: Array<{ tool: string; input: Record<string, unknown>; toolUseID: string }> = [];
    const canUseTool: Parameters<typeof buildQueryOptions>[0]["canUseTool"] = async (
      tool,
      toolInput,
      context,
    ) => {
      assert.equal(context.signal, signal);
      calls.push({ tool, input: toolInput, toolUseID: context.toolUseID });
      return tool === "Read"
        ? { behavior: "allow", updatedInput: { file_path: "/independently-approved.txt" } }
        : { behavior: "deny", message: "Independent permission refusal" };
    };
    try {
      const options = buildQueryOptions({
        resolvedSettings: {
          model: "inherited-sdk-model",
          permissions: { defaultMode: "acceptEdits" },
        },
        cwd: "C:/work/project with spaces",
        pluginPaths: ["C:/plugins/confirmed one", "C:/plugins/confirmed two"],
        resume: row.resume,
        ...(row.resume
          ? { resumeSessionAt: "kept-tool-anchor", resumeDropsTurn: "discarded-user-id" }
          : {}),
        forkSession: row.fork,
        launchSettings: row.launch,
        provisionalSessionId: "new-provisional-id",
        input,
        canUseTool,
        enableSdkDebug: false,
        enableSpawnDebug: false,
        sessionIdForLogs: () => "new-provisional-id",
      });
      assert.equal(options.cwd, "C:/work/project with spaces");
      assert.equal(options.resume, row.resume);
      assert.equal(options.sessionId, !row.resume || row.fork ? "new-provisional-id" : undefined);
      assert.equal(options.resumeSessionAt, row.resume ? "kept-tool-anchor" : undefined);
      assert.equal(options.resumeDropsTurn, row.resume ? "discarded-user-id" : undefined);
      assert.equal(options.forkSession, row.fork ? true : undefined);
      assert.deepEqual(options.plugins, [
        { type: "local", path: "C:/plugins/confirmed one" },
        { type: "local", path: "C:/plugins/confirmed two" },
      ]);
      assert.equal(options.model, row.model);
      assert.equal(options.permissionMode, row.mode);
      assert.equal(options.allowDangerouslySkipPermissions, row.dangerous);
      assert.ok(options.canUseTool);
      assert.deepEqual(
        await options.canUseTool(
          "Bash",
          { command: "unsafe-fixture-command" },
          { signal, toolUseID: "deny-call", requestId: "deny-request" },
        ),
        { behavior: "deny", message: "Independent permission refusal" },
      );
      assert.deepEqual(
        await options.canUseTool(
          "Read",
          { file_path: "/requested.txt" },
          { signal, toolUseID: "allow-call", requestId: "allow-request" },
        ),
        { behavior: "allow", updatedInput: { file_path: "/independently-approved.txt" } },
      );
      assert.deepEqual(calls, [
        { tool: "Bash", input: { command: "unsafe-fixture-command" }, toolUseID: "deny-call" },
        { tool: "Read", input: { file_path: "/requested.txt" }, toolUseID: "allow-call" },
      ]);
    } finally {
      input.close();
    }
  }
});

test("scheduled interrupts wait for the target native receipt and preserve exact queued identities without touching a sibling", async () => {
  for (const row of [
    { receipt: undefined, expected: [] },
    { receipt: { still_queued: [] }, expected: [] },
    {
      receipt: { still_queued: ["queued-first", 7, null, "queued-second"] },
      expected: ["queued-first", "queued-second"],
    },
  ]) {
    await withCommandTargets(async (target, sibling) => {
      const entered = bridgeDeferred<void>(),
        release = bridgeDeferred<void>();
      let targetCalls = 0,
        siblingCalls = 0;
      const observed: Array<Record<string, unknown>> = [];
      const scheduler = new BridgeCommandScheduler();
      let pending: Promise<void> | undefined;
      const native = {
        close: () => {},
        async interrupt() {
          assert.equal(this, native);
          targetCalls++;
          entered.resolve();
          await release.promise;
          return row.receipt;
        },
      };
      target.query = native as unknown as SessionState["query"];
      sibling.query = {
        close: () => {},
        interrupt: async () => {
          siblingCalls++;
          return { still_queued: ["peer-queue"] };
        },
      } as unknown as SessionState["query"];
      const events = await captureBridgeEventsAsync(
        async () => {
          try {
            const parsed = parseCommandEnvelope(
              JSON.stringify({
                command: "cancel_turn",
                session_id: target.sessionId,
                request_id: "interrupt-target-request",
              }),
            );
            pending = scheduleBridgeCommand(scheduler, parsed);
            assert.ok(pending);
            await boundedBridgeTask(entered.promise);
            await eventLoopCheckpoint();
            assert.deepEqual(observed, []);
            release.resolve();
            await boundedBridgeTask(pending);
            await boundedBridgeTask(scheduler.whenIdle());
          } finally {
            release.resolve();
            if (pending) await boundedBridgeTask(pending);
            await boundedBridgeTask(scheduler.whenIdle());
          }
        },
        (event) => observed.push(event),
      );
      assert.deepEqual(events, [
        {
          event: "turn_interrupt_receipt",
          session_id: target.sessionId,
          request_id: "interrupt-target-request",
          still_queued: row.expected,
        },
      ]);
      assert.equal(targetCalls, 1);
      assert.equal(siblingCalls, 0);
      const unknown = await captureBridgeEventsAsync(async () => {
        const parsed = parseCommandEnvelope(
          JSON.stringify({
            command: "cancel_turn",
            session_id: "unknown-interrupt-session",
            request_id: "unknown-interrupt-request",
          }),
        );
        await scheduleBridgeCommand(scheduler, parsed);
      });
      assert.equal(unknown.length, 1);
      assert.equal(unknown[0]?.event, "slash_error");
      assert.equal(unknown[0]?.session_id, "unknown-interrupt-session");
      assert.equal(unknown[0]?.request_id, "unknown-interrupt-request");
      assert.match(String(unknown[0]?.message), /unknown session/i);
      assert.equal(targetCalls, 1);
      assert.equal(siblingCalls, 0);
    });
  }
});
test("a failed scheduled native interrupt reports an operation error and the same query accepts an explicit retry", async () => {
  await withCommandTargets(async (target, sibling) => {
    let targetCalls = 0,
      siblingCalls = 0;
    target.query = {
      close: () => {},
      interrupt: async () => {
        targetCalls++;
        if (targetCalls === 1) throw new Error("OWNED-INTERRUPT-CAUSE");
        return { still_queued: ["retry-queued"] };
      },
    } as unknown as SessionState["query"];
    sibling.query = {
      close: () => {},
      interrupt: async () => {
        siblingCalls++;
        return { still_queued: [] };
      },
    } as unknown as SessionState["query"];
    const scheduler = new BridgeCommandScheduler();
    const failed = await captureBridgeEventsAsync(async () => {
      const parsed = parseCommandEnvelope(
        JSON.stringify({
          command: "cancel_turn",
          session_id: target.sessionId,
          request_id: "failed-interrupt-request",
        }),
      );
      await scheduleBridgeCommand(scheduler, parsed);
      await boundedBridgeTask(scheduler.whenIdle());
    });
    assert.equal(failed.length, 1);
    assert.equal(failed[0]?.event, "slash_error");
    assert.equal(failed[0]?.request_id, "failed-interrupt-request");
    assert.equal(failed[0]?.session_id, target.sessionId);
    assert.match(String(failed[0]?.message), /OWNED-INTERRUPT-CAUSE/);
    const retried = await captureBridgeEventsAsync(async () => {
      const parsed = parseCommandEnvelope(
        JSON.stringify({
          command: "cancel_turn",
          session_id: target.sessionId,
          request_id: "retried-interrupt-request",
        }),
      );
      await scheduleBridgeCommand(scheduler, parsed);
      await boundedBridgeTask(scheduler.whenIdle());
    });
    assert.deepEqual(retried, [
      {
        event: "turn_interrupt_receipt",
        session_id: target.sessionId,
        request_id: "retried-interrupt-request",
        still_queued: ["retry-queued"],
      },
    ]);
    assert.equal(targetCalls, 2);
    assert.equal(siblingCalls, 0);
  });
});

test("resolveClaudeCodeSpawnCommand preserves commands with path separators", () => {
  assert.equal(
    resolveClaudeCodeSpawnCommand("/opt/runtime/bun"),
    "/opt/runtime/bun",
  );
  assert.equal(
    resolveClaudeCodeSpawnCommand("C:\\runtime\\bun.exe"),
    "C:\\runtime\\bun.exe",
  );
});

function spawnOwnedBridgeChild(
  args: string[],
  config: { command?: string; env?: Record<string, string | undefined>; signal?: AbortSignal } = {},
) {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    resolvedSettings: {},
    cwd: process.cwd(),
    launchSettings: {},
    provisionalSessionId: "owned-spawn-fixture",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "Independent unused permission" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "owned-spawn-fixture",
  });
  const child = options.spawnClaudeCodeProcess({
    command: config.command ?? process.execPath,
    args,
    cwd: process.cwd(),
    env: config.env ?? {},
    signal: config.signal ?? new AbortController().signal,
  });
  let stdout = "",
    stderr = "",
    pending = "",
    closed = false;
  const errors: Error[] = [],
    lines: string[] = [];
  const waiters: Array<{
    matches: (line: string) => boolean;
    resolve: (line: string) => void;
    reject: (error: Error) => void;
  }> = [];
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  const onOutput = (chunk: string) => {
    stdout += chunk;
    pending += chunk;
    let boundary = pending.indexOf("\n");
    while (boundary >= 0) {
      const line = pending.slice(0, boundary);
      pending = pending.slice(boundary + 1);
      lines.push(line);
      for (let i = waiters.length - 1; i >= 0; i--) {
        const waiter = waiters[i];
        if (waiter?.matches(line)) {
          waiters.splice(i, 1);
          waiter.resolve(line);
        }
      }
      boundary = pending.indexOf("\n");
    }
  };
  const onError = (error: Error) => errors.push(error);
  const onStderr = (chunk: string) => {
    stderr += chunk;
  };
  child.stdout.on("data", onOutput);
  child.stderr.on("data", onStderr);
  child.on("error", onError);
  const completion = new Promise<{ code: number | null; signal: NodeJS.Signals | null }>(
    (resolve) =>
      child.once("close", (code, signal) => {
        closed = true;
        for (const waiter of waiters.splice(0))
          waiter.reject(new Error("Owned child closed before its expected output"));
        resolve({ code, signal });
      }),
  );
  return {
    child,
    options,
    readLine(matches: (line: string) => boolean) {
      const existing = lines.find(matches);
      if (existing !== undefined) return Promise.resolve(existing);
      if (closed) return Promise.reject(new Error("Owned child already closed"));
      return new Promise<string>((resolve, reject) => waiters.push({ matches, resolve, reject }));
    },
    async complete() {
      const result = await boundedBridgeTask(completion);
      return { ...result, stdout, stderr, errors };
    },
    async dispose() {
      try {
        if (!closed) child.kill();
        await boundedBridgeTask(completion);
      } finally {
        input.close();
        child.stdout.off("data", onOutput);
        child.stderr.off("data", onStderr);
        child.off("error", onError);
      }
    },
  };
}
test("the real spawn hook remaps bare bun and drains the child executable identity before close", async () => {
  const owned = spawnOwnedBridgeChild(["-e", "process.stdout.write(process.execPath)"], {
    command: "bun",
  });
  try {
    const result = await owned.complete();
    assert.equal(result.code, 0, result.stderr);
    assert.equal(result.signal, null);
    assert.deepEqual(result.errors, []);
    assert.equal(result.stdout, process.execPath);
  } finally {
    await owned.dispose();
  }
});

test("spawned children observe frozen SDK environment authority including absent, disabled and enabled Todo values", async () => {
  const keys = ["AUDIT_PARENT_ONLY", "CLAUDE_CODE_ENABLE_TODO_TOOLS"] as const;
  const previous = Object.fromEntries(keys.map((key) => [key, process.env[key]]));
  try {
    process.env.AUDIT_PARENT_ONLY = "MUST-NOT-LEAK";
    for (const row of [
      { todo: undefined, parent: "1", expected: null },
      { todo: "0", parent: "1", expected: "0" },
      { todo: "1", parent: "0", expected: "1" },
    ]) {
      process.env.CLAUDE_CODE_ENABLE_TODO_TOOLS = row.parent;
      const env = Object.freeze({
        AUDIT_SDK_SENTINEL: "independent-sdk-value",
        CLAUDE_CODE_STARTUP_FAILURE_RESULTS: "0",
        ...(row.todo === undefined ? {} : { CLAUDE_CODE_ENABLE_TODO_TOOLS: row.todo }),
      });
      const owned = spawnOwnedBridgeChild(
        [
          "-e",
          "process.stdout.write(JSON.stringify({sentinel:process.env.AUDIT_SDK_SENTINEL,parentOnly:process.env.AUDIT_PARENT_ONLY??null,startup:process.env.CLAUDE_CODE_STARTUP_FAILURE_RESULTS,todo:process.env.CLAUDE_CODE_ENABLE_TODO_TOOLS??null}))",
        ],
        { env },
      );
      try {
        assert.equal("env" in owned.options, false);
        const result = await owned.complete();
        assert.equal(result.code, 0, result.stderr);
        assert.deepEqual(result.errors, []);
        assert.deepEqual(JSON.parse(result.stdout), {
          sentinel: "independent-sdk-value",
          parentOnly: null,
          startup: "1",
          todo: row.expected,
        });
        assert.deepEqual(env, {
          AUDIT_SDK_SENTINEL: "independent-sdk-value",
          CLAUDE_CODE_STARTUP_FAILURE_RESULTS: "0",
          ...(row.todo === undefined ? {} : { CLAUDE_CODE_ENABLE_TODO_TOOLS: row.todo }),
        });
      } finally {
        await owned.dispose();
      }
    }
  } finally {
    for (const key of keys) {
      if (previous[key] === undefined) delete process.env[key];
      else process.env[key] = previous[key];
    }
  }
});

test("the supplied abort signal terminates only its ready SDK child while a peer and fresh spawn remain usable", async () => {
  const script = `const {createInterface}=require('node:readline');const send=value=>process.stdout.write(JSON.stringify(value)+'\\n');send({event:'ready',pid:process.pid});createInterface({input:process.stdin}).on('line',text=>send({event:'echo',text,pid:process.pid}));`;
  const controller = new AbortController();
  const target = spawnOwnedBridgeChild(["-e", script], { signal: controller.signal });
  const peer = spawnOwnedBridgeChild(["-e", script]);
  const record = (event: string) => (line: string) => JSON.parse(line).event === event;
  try {
    const ready = JSON.parse(await boundedBridgeTask(target.readLine(record("ready"))));
    const peerReady = JSON.parse(await boundedBridgeTask(peer.readLine(record("ready"))));
    assert.equal(ready.pid, target.child.pid);
    assert.equal(peerReady.pid, peer.child.pid);
    assert.notEqual(ready.pid, peerReady.pid);
    controller.abort();
    const stopped = await target.complete();
    assert.notEqual(stopped.code, 0);
    assert.ok(stopped.errors.some((error) => error.name === "AbortError"));
    assert.deepEqual(
      stopped.stdout
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line)),
      [{ event: "ready", pid: target.child.pid }],
    );
    assert.equal(target.child.stdout.readableEnded, true);
    peer.child.stdin.write("PEER-AFTER-ABORT\n");
    assert.deepEqual(JSON.parse(await boundedBridgeTask(peer.readLine(record("echo")))), {
      event: "echo",
      text: "PEER-AFTER-ABORT",
      pid: peer.child.pid,
    });
    const fresh = spawnOwnedBridgeChild(["-e", "process.stdout.write('FRESH-SPAWN-OK')"]);
    try {
      const result = await fresh.complete();
      assert.equal(result.code, 0, result.stderr);
      assert.deepEqual(result.errors, []);
      assert.equal(result.stdout, "FRESH-SPAWN-OK");
    } finally {
      await fresh.dispose();
    }
  } finally {
    await Promise.all([target.dispose(), peer.dispose()]);
  }
});

function publishedToolFrames(
  events: Array<Record<string, unknown>>,
  sessionId: string,
  id: string,
): Array<Record<string, unknown>> {
  return events.flatMap((event) => {
    const update = event.update as
      | {
          type?: string;
          tool_call?: Record<string, unknown>;
          tool_call_update?: { tool_call_id: string; fields: Record<string, unknown> };
        }
      | undefined;
    const fields =
      update?.type === "tool_call" && update.tool_call?.tool_call_id === id
        ? update.tool_call
        : update?.type === "tool_call_update" && update.tool_call_update?.tool_call_id === id
          ? update.tool_call_update.fields
          : undefined;
    if (!fields) return [];
    assert.equal(event.session_id, sessionId);
    return [fields];
  });
}
test("task progress publishes the selected summary or meaningful fallback only on its linked provisional Agent", () => {
  for (const row of [
    {
      name: "summary",
      summary: "INDEPENDENT-PROGRESS-SUMMARY",
      expected: ["INDEPENDENT-PROGRESS-SUMMARY"],
      excluded: ["FALLBACK-DESCRIPTION", "Read"],
    },
    {
      name: "fallback",
      summary: undefined,
      expected: ["FALLBACK-DESCRIPTION", "Read"],
      excluded: ["INDEPENDENT-PROGRESS-SUMMARY"],
    },
  ]) {
    const session = makeSessionState();
    session.sessionId = `task-progress-${row.name}`;
    const events = captureBridgeEvents(() => {
      emitToolCall(session, "progress-agent", "Agent", { prompt: "Inspect a concrete subsystem" });
      emitToolCall(session, "progress-peer", "Agent", { prompt: "Unrelated peer" });
      handleTaskSystemMessage(session, "task_started", {
        task_id: "progress-task",
        tool_use_id: "progress-agent",
        description: "START-DESCRIPTION-NOT-OUTPUT",
      });
      handleTaskSystemMessage(session, "task_progress", {
        task_id: "progress-task",
        summary: row.summary,
        description: "FALLBACK-DESCRIPTION",
        last_tool_name: "Read",
      });
    });
    const frames = publishedToolFrames(events, session.sessionId, "progress-agent");
    assert.ok(frames.length > 0);
    assert.ok(frames.every((frame) => !frame.status || frame.status === "in_progress"));
    assert.doesNotMatch(JSON.stringify(frames), /START-DESCRIPTION-NOT-OUTPUT/);
    const outputs = frames.filter((frame) => frame.raw_output !== undefined);
    assert.equal(outputs.length, 1);
    for (const expected of row.expected)
      assert.ok(String(outputs[0]?.raw_output).includes(expected));
    for (const excluded of row.excluded)
      assert.ok(!String(outputs[0]?.raw_output).includes(excluded));
    const current = publishedTool(events, session.sessionId, "progress-agent");
    assert.equal(current.status, "in_progress");
    const content = current.content as Array<{
      type: string;
      content: { type: string; text: string };
    }>;
    assert.equal(content.length, 1);
    assert.equal(content[0]?.type, "content");
    assert.equal(content[0]?.content.type, "text");
    for (const expected of row.expected) assert.ok(content[0]?.content.text.includes(expected));
    for (const excluded of row.excluded) assert.ok(!content[0]?.content.text.includes(excluded));
    const peer = publishedTool(events, session.sessionId, "progress-peer");
    assert.equal(peer.status, "in_progress");
    assert.equal(peer.raw_output, undefined);
    assert.deepEqual(peer.content, []);
  }
});

test("task starts publish scoped metadata with distinct parent identity, explicit false and validated spawn depth", () => {
  for (const row of [
    { name: "positive-one", depth: 1, expected: 1, blocked: true },
    { name: "positive-three", depth: 3, expected: 3, blocked: false },
    { name: "zero", depth: 0, expected: undefined, blocked: false },
    { name: "negative", depth: -1, expected: undefined, blocked: true },
    { name: "fraction", depth: 1.5, expected: undefined, blocked: false },
    { name: "string", depth: "2", expected: undefined, blocked: false },
  ]) {
    const session = makeSessionState();
    session.sessionId = `task-start-${row.name}`;
    const events = captureBridgeEvents(() => {
      emitToolCall(session, "metadata-target", "Agent", { prompt: "Target work" });
      emitToolCall(session, "metadata-peer", "Agent", { prompt: "Peer work" });
      handleTaskSystemMessage(session, "task_started", {
        task_id: "peer-task",
        tool_use_id: "metadata-peer",
        blocked: true,
        parent_agent_id: "peer-parent-agent",
        spawn_depth: 4,
        is_backgrounded: false,
      });
      handleTaskSystemMessage(session, "task_started", {
        task_id: "target-task",
        tool_use_id: "metadata-target",
        description: "Target independent work",
        blocked: row.blocked,
        parent_agent_id: "distinct-parent-agent",
        parent_tool_use_id: "different-parent-tool",
        spawn_depth: row.depth,
        is_backgrounded: false,
      });
    });
    const expected = {
      blocked: row.blocked,
      parent_agent_id: "distinct-parent-agent",
      is_backgrounded: false,
      ...(row.expected === undefined ? {} : { spawn_depth: row.expected }),
    };
    const frames = publishedToolFrames(events, session.sessionId, "metadata-target").filter(
      (frame) => frame.task_metadata !== undefined,
    );
    assert.ok(frames.length > 0);
    for (const frame of frames) assert.deepEqual(frame.task_metadata, expected);
    assert.deepEqual(
      publishedTool(events, session.sessionId, "metadata-target").task_metadata,
      expected,
    );
    assert.deepEqual(publishedTool(events, session.sessionId, "metadata-peer").task_metadata, {
      blocked: true,
      parent_agent_id: "peer-parent-agent",
      spawn_depth: 4,
      is_backgrounded: false,
    });
    const tasks = events.flatMap((event) => {
      const update = event.update as
        | { type?: string; tasks?: Array<Record<string, unknown>> }
        | undefined;
      if (update?.type !== "task_state_update") return [];
      assert.equal(event.session_id, session.sessionId);
      return update.tasks ?? [];
    });
    const task = tasks.find((task) => task.task_id === "target-task");
    assert.ok(task);
    assert.equal(task.source_tool_call_id, "metadata-target");
    assert.equal(
      (task.metadata as Record<string, unknown>).parent_agent_id,
      "distinct-parent-agent",
    );
  }
});

test("a foreground Agent stays provisional through task completion and publishes only its real result and model", () => {
  const session = makeSessionState();
  session.sessionId = "provisional-agent-owner";
  const initial = captureBridgeEvents(() => {
    emitToolCall(session, "agent-result-target", "Agent", { prompt: "Inspect exact target" });
    emitToolCall(session, "agent-result-peer", "Read", { file_path: "peer.txt" });
    handleTaskSystemMessage(session, "task_started", {
      task_id: "agent-result-task",
      tool_use_id: "agent-result-target",
      description: "START-NOT-RESULT",
    });
    handleTaskSystemMessage(session, "task_progress", {
      task_id: "agent-result-task",
      summary: "ALLOWED-PROGRESS",
    });
    handleTaskSystemMessage(session, "task_notification", {
      task_id: "agent-result-task",
      status: "completed",
      summary: "NOTIFICATION-NOT-RESULT",
    });
  });
  const before = publishedToolFrames(initial, session.sessionId, "agent-result-target");
  assert.ok(before.every((frame) => !frame.status || frame.status === "in_progress"));
  assert.doesNotMatch(
    JSON.stringify(
      before.map((frame) => ({ raw_output: frame.raw_output, content: frame.content })),
    ),
    /NOTIFICATION-NOT-RESULT|START-NOT-RESULT/,
  );
  assert.equal(
    publishedTool(initial, session.sessionId, "agent-result-target").raw_output,
    "ALLOWED-PROGRESS",
  );
  const taskUpdates = initial.flatMap((event) => {
    const update = event.update as
      | { type?: string; tasks?: Array<{ task_id: string; status: string }> }
      | undefined;
    return update?.type === "task_state_update" ? (update.tasks ?? []) : [];
  });
  assert.ok(
    taskUpdates.some((task) => task.task_id === "agent-result-task" && task.status === "completed"),
  );
  const result = captureBridgeEvents(() =>
    emitToolResultUpdate(
      session,
      "agent-result-target",
      false,
      [{ type: "text", text: "ACTUAL-AGENT-RESULT\n exact  " }],
      {
        agentId: "independent-worker",
        agentType: "general-purpose",
        resolvedModel: "claude-opus-4-8",
        content: [{ type: "text", text: "ACTUAL-AGENT-RESULT\n exact  " }],
        status: "completed",
        prompt: "Inspect exact target",
      },
    ),
  );
  const finished = publishedTool([...initial, ...result], session.sessionId, "agent-result-target");
  assert.equal(finished.status, "completed");
  assert.deepEqual(finished.output_metadata, { agent: { resolved_model: "claude-opus-4-8" } });
  assert.deepEqual(finished.content, [
    { type: "content", content: { type: "text", text: "ACTUAL-AGENT-RESULT\n exact  " } },
  ]);
  assert.doesNotMatch(
    JSON.stringify(
      publishedToolFrames(result, session.sessionId, "agent-result-target").map((frame) => ({
        raw_output: frame.raw_output,
        content: frame.content,
      })),
    ),
    /NOTIFICATION-NOT-RESULT/,
  );
  const later = captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_progress", {
      task_id: "agent-result-task",
      tool_use_id: "agent-result-target",
      summary: "STALE-REOPEN-ATTEMPT",
    });
    handleTaskSystemMessage(session, "task_notification", {
      task_id: "agent-result-task",
      tool_use_id: "agent-result-target",
      status: "completed",
      summary: "STALE-RESULT-REPLACEMENT",
    });
  });
  assert.deepEqual(publishedToolFrames(later, session.sessionId, "agent-result-target"), []);
  assert.deepEqual(
    publishedTool([...initial, ...result, ...later], session.sessionId, "agent-result-target"),
    finished,
  );
  assert.equal(
    publishedTool([...initial, ...result, ...later], session.sessionId, "agent-result-peer").status,
    "in_progress",
  );
});

test("foreground shell and file tools publish concrete results without fabricated task output or early completion", () => {
  for (const row of [
    {
      name: "PowerShell",
      input: { command: "Get-Content owned.txt" },
      body: { stdout: "ACTUAL-SHELL-STDOUT\n", stderr: "", interrupted: false },
      text: "ACTUAL-SHELL-STDOUT",
    },
    {
      name: "Bash",
      input: { command: "cat owned.txt" },
      body: { stdout: "ACTUAL-BASH-STDOUT\n", stderr: "", interrupted: false },
      text: "ACTUAL-BASH-STDOUT",
    },
    {
      name: "Read",
      input: { file_path: "owned.txt" },
      body: {
        type: "text",
        file: {
          filePath: "owned.txt",
          content: "ACTUAL-READ-CONTENT\n",
          numLines: 1,
          startLine: 1,
          totalLines: 1,
        },
      },
      text: "ACTUAL-READ-CONTENT",
    },
    {
      name: "Write",
      input: { file_path: "owned.txt", content: "REQUESTED-WRITE-PREVIEW" },
      body: {
        type: "update",
        filePath: "owned.txt",
        originalFile: "BEFORE-WRITE",
        content: "ACTUAL-WRITE-CONTENT\n",
        structuredPatch: [],
      },
      text: "ACTUAL-WRITE-CONTENT",
    },
  ]) {
    const session = makeSessionState();
    session.sessionId = `foreground-${row.name}`;
    const initial = captureBridgeEvents(() => {
      emitToolCall(session, "concrete-target", row.name, row.input);
      emitToolCall(session, "concrete-peer", "Read", { file_path: "peer.txt" });
      handleTaskSystemMessage(session, "task_started", {
        task_id: "concrete-task",
        tool_use_id: "concrete-target",
        description: "LIFECYCLE-DESCRIPTION-NOT-OUTPUT",
      });
      handleTaskSystemMessage(session, "task_notification", {
        task_id: "concrete-task",
        tool_use_id: "concrete-target",
        status: "completed",
        summary: "LIFECYCLE-SUMMARY-NOT-OUTPUT",
      });
    });
    const before = publishedToolFrames(initial, session.sessionId, "concrete-target");
    assert.ok(before.length > 0);
    assert.ok(before.every((frame) => !frame.status || frame.status === "in_progress"));
    assert.ok(before.every((frame) => frame.raw_output === undefined));
    assert.doesNotMatch(
      JSON.stringify(
        before.map((frame) => ({ raw_output: frame.raw_output, content: frame.content })),
      ),
      /LIFECYCLE-DESCRIPTION-NOT-OUTPUT|LIFECYCLE-SUMMARY-NOT-OUTPUT/,
    );
    const result = captureBridgeEvents(() =>
      emitToolResultUpdate(session, "concrete-target", false, row.body),
    );
    const finished = publishedTool([...initial, ...result], session.sessionId, "concrete-target");
    assert.equal(finished.status, "completed");
    assert.ok(JSON.stringify(finished.content).includes(row.text));
    assert.doesNotMatch(
      JSON.stringify(
        publishedToolFrames(result, session.sessionId, "concrete-target").map((frame) => ({
          raw_output: frame.raw_output,
          content: frame.content,
        })),
      ),
      /LIFECYCLE-DESCRIPTION-NOT-OUTPUT|LIFECYCLE-SUMMARY-NOT-OUTPUT/,
    );
    if (row.name === "Bash" || row.name === "PowerShell")
      assert.equal(
        finished.raw_output,
        row.name === "Bash" ? "ACTUAL-BASH-STDOUT\n" : "ACTUAL-SHELL-STDOUT\n",
      );
    const later = captureBridgeEvents(() => {
      handleTaskSystemMessage(session, "task_progress", {
        task_id: "concrete-task",
        tool_use_id: "concrete-target",
        summary: "STALE-PROGRESS",
      });
      handleTaskSystemMessage(session, "task_notification", {
        task_id: "concrete-task",
        tool_use_id: "concrete-target",
        status: "completed",
        summary: "STALE-FINAL",
      });
    });
    assert.deepEqual(publishedToolFrames(later, session.sessionId, "concrete-target"), []);
    assert.deepEqual(
      publishedTool([...initial, ...result, ...later], session.sessionId, "concrete-target"),
      finished,
    );
    assert.equal(
      publishedTool([...initial, ...result, ...later], session.sessionId, "concrete-peer").status,
      "in_progress",
    );
  }
});

test("handleSdkMessage keeps real tool output when tool_use_summary arrives", () => {
  const session = makeSessionState();
  const protectedTools = [
    createToolCall("tool-bash", "Bash", { command: "git diff" }),
    createToolCall("tool-read", "Read", { file_path: "src/main.rs" }),
    createToolCall("tool-write", "Write", {
      file_path: "src/main.rs",
      content: "updated file contents",
    }),
    createToolCall("tool-agent", "Agent", { prompt: "Inspect auth flow" }),
    createToolCall("tool-task", "Task", { prompt: "Inspect auth flow" }),
    createToolCall("tool-search", "WebSearch", { query: "ratatui" }),
    createToolCall("tool-fetch", "WebFetch", { url: "https://example.com" }),
    createToolCall("tool-plan", "ExitPlanMode", { plan: "Ship it" }),
  ];

  for (const toolCall of protectedTools) {
    toolCall.status = "completed";
    toolCall.raw_output = `actual output for ${toolCall.tool_call_id}`;
    session.toolCalls.set(toolCall.tool_call_id, toolCall);
  }

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "tool_use_summary",
      summary: "Show commits on this branch since diverging from main",
      preceding_tool_use_ids: protectedTools.map(
        (toolCall) => toolCall.tool_call_id,
      ),
      uuid: "message-summary",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events, []);
  for (const toolCall of protectedTools) {
    assert.equal(
      session.toolCalls.get(toolCall.tool_call_id)?.raw_output,
      `actual output for ${toolCall.tool_call_id}`,
    );
  }
});

test("the installed ToolSearch policy allows exact input and identity while hiding its frames and retaining ordinary permissions and output", async () => {
  const { spawnSync } = await import("node:child_process");
  const { fileURLToPath } = await import("node:url");
  const child = spawnSync(
    process.execPath,
    [fileURLToPath(new URL("../../tests/fixtures/bridge-tool-policy.mjs", import.meta.url))],
    { encoding: "utf8", windowsHide: true, timeout: 15000 },
  );
  assert.equal(child.error, undefined);
  assert.equal(child.status, 0, child.stderr);
  const report = JSON.parse(child.stdout) as {
    sessionId: string;
    hiddenResult: unknown;
    hiddenEvents: Array<Record<string, unknown>>;
    normalResult: unknown;
    normalEvents: Array<Record<string, unknown>>;
    allQueriesClosed: boolean;
  };
  assert.deepEqual(report.hiddenResult, {
    behavior: "allow",
    updatedInput: { query: "Independent indexed search", limit: 7 },
    toolUseID: "hidden-search",
  });
  assert.deepEqual(report.hiddenEvents, []);
  assert.equal(report.allQueriesClosed, true);
  const requests = report.normalEvents.filter((event) => event.event === "permission_request");
  assert.equal(requests.length, 1);
  assert.equal(requests[0]?.session_id, report.sessionId);
  const request = requests[0]?.request as {
    tool_call: { tool_call_id: string; raw_input: unknown };
    options: Array<{ option_id: string }>;
  };
  assert.equal(request.tool_call.tool_call_id, "ordinary-tool");
  assert.deepEqual(request.tool_call.raw_input, { command: "printf ordinary-control" });
  assert.ok(request.options.some((option) => option.option_id === "allow_once"));
  assert.deepEqual(report.normalResult, {
    behavior: "allow",
    updatedInput: { command: "printf ordinary-control" },
    toolUseID: "ordinary-tool",
  });
  const normal = publishedTool(report.normalEvents, report.sessionId, "ordinary-tool");
  assert.equal(normal.status, "completed");
  assert.equal(normal.raw_output, "ORDINARY-ACTUAL-OUTPUT\n");
  assert.equal(normal.source_message_uuid, "ordinary-result-source");
  assert.doesNotMatch(
    JSON.stringify(report.normalEvents),
    /HIDDEN-SEARCH-OUTPUT|HIDDEN-SEARCH-SUMMARY|hidden-search/,
  );
});

test("fallback retractions preserve exact valid targets, reason and scope without broad removal or substitute notices", () => {
  for (const row of [
    { subtype: "model_refusal_fallback", scope: "local", expectedScope: "local" },
    { subtype: "model_fallback", scope: undefined, expectedScope: "session" },
    { subtype: "model_fallback", scope: "subagent", expectedScope: "subagent" },
  ]) {
    const session = makeSessionState();
    session.sessionId = `retraction-${row.subtype}-${row.expectedScope}`;
    const events = captureBridgeEvents(() =>
      handleSdkMessage(session, {
        type: "system",
        subtype: row.subtype,
        uuid: "fallback-frame",
        session_id: session.sessionId,
        scope: row.scope,
        trigger: "refusal",
        direction: "retry",
        request_id: "retraction-request",
        original_model: "original-native-model",
        fallback_model: "fallback-native-model",
        api_refusal_category: "independent-category",
        api_refusal_explanation: "Independent supplied explanation",
        content: "Independent supplied fallback content",
        retracted_message_uuids: [
          "old-first",
          "old-first",
          null,
          "",
          7,
          false,
          "old-second",
          {},
          "old-second",
        ],
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    assert.deepEqual(events, [
      {
        event: "session_update",
        session_id: session.sessionId,
        update: {
          type: "transcript_retraction",
          message_uuids: ["old-first", "old-second"],
          reason: row.subtype,
          request_id: "retraction-request",
          trigger: "refusal",
          direction: "retry",
          original_model: "original-native-model",
          fallback_model: "fallback-native-model",
          scope: row.expectedScope,
          api_refusal_category: "independent-category",
          api_refusal_explanation: "Independent supplied explanation",
          content: "Independent supplied fallback content",
        },
      },
    ]);
    for (const invalid of [[], [null, 7, "", {}, false], undefined, "old-first"]) {
      const refused = captureBridgeEvents(() =>
        handleSdkMessage(session, {
          type: "system",
          subtype: row.subtype,
          uuid: "invalid-fallback-frame",
          session_id: session.sessionId,
          retracted_message_uuids: invalid,
          request_id: 7,
          trigger: null,
          scope: null,
        } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
      );
      assert.deepEqual(refused, []);
    }
  }
});

test("refusal notices choose supplied explanation, category or content in priority order without retracting messages", () => {
  for (const row of [
    {
      name: "explanation",
      model: "model-refused",
      explanation: "EXPLANATION-SELECTED",
      category: "CATEGORY-LOWER",
      content: "CONTENT-LOWER",
      selected: "EXPLANATION-SELECTED",
      excluded: ["CATEGORY-LOWER", "CONTENT-LOWER"],
    },
    {
      name: "category",
      model: "model-refused",
      explanation: undefined,
      category: "CATEGORY-SELECTED",
      content: "CONTENT-LOWER",
      selected: "CATEGORY-SELECTED",
      excluded: ["CONTENT-LOWER"],
    },
    {
      name: "content",
      model: "model-refused",
      explanation: null,
      category: " ",
      content: "CONTENT-SELECTED!",
      selected: "CONTENT-SELECTED!",
      excluded: [],
    },
    {
      name: "missing",
      model: " ",
      explanation: null,
      category: undefined,
      content: " ",
      selected: undefined,
      excluded: ["undefined", "null"],
    },
  ]) {
    const session = makeSessionState();
    session.sessionId = `refusal-${row.name}`;
    const events = captureBridgeEvents(() =>
      handleSdkMessage(session, {
        type: "system",
        subtype: "model_refusal_no_fallback",
        session_id: session.sessionId,
        uuid: "refusal-frame",
        original_model: row.model,
        api_refusal_explanation: row.explanation,
        api_refusal_category: row.category,
        content: row.content,
        refused_user_message_uuid: "must-not-retract",
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    assert.equal(events.length, 1);
    const event = events[0];
    assert.ok(event);
    assert.equal(event.event, "session_update");
    assert.equal(event.session_id, session.sessionId);
    const update = event.update as { type: string; severity: string; message: string };
    assert.equal(update.type, "system_notice_update");
    assert.equal(update.severity, "warning");
    assert.match(update.message, /refused/i);
    assert.match(update.message, /no fallback/i);
    if (row.model.trim()) assert.ok(update.message.includes("model-refused"));
    if (row.selected) assert.ok(update.message.includes(row.selected));
    for (const excluded of row.excluded) assert.ok(!update.message.includes(excluded));
  }
});

test("handleSdkMessage emits assistant supersedes before replacement content", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "assistant-new",
      supersedes: ["assistant-old"],
      session_id: "session-1",
      message: {
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "tool-1",
            name: "Bash",
            input: { command: "echo ok" },
          },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(
    events.map((event) => event.update),
    [
      {
        type: "transcript_retraction",
        message_uuids: ["assistant-old"],
        reason: "assistant_supersedes",
      },
      {
        type: "tool_call",
        tool_call: {
          tool_call_id: "tool-1",
          title: "echo ok",
          kind: "execute",
          status: "in_progress",
          source_message_uuid: "assistant-new",
          content: [],
          raw_input: { command: "echo ok" },
          locations: [],
          meta: { claudeCode: { toolName: "Bash", parentToolUseId: null } },
        },
      },
    ],
  );
});

test("stream text, tool creation and result retain their own source frame and intended tool association", () => {
  const session = makeSessionState();
  session.sessionId = "source-ownership-session";
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "stream_event",
      uuid: "independent-text-source",
      session_id: session.sessionId,
      event: {
        type: "content_block_delta",
        delta: { type: "text_delta", text: "INDEPENDENT-STREAM-TEXT" },
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "stream_event",
      uuid: "independent-tool-source",
      session_id: session.sessionId,
      event: {
        type: "content_block_start",
        content_block: {
          type: "tool_use",
          id: "source-target",
          name: "Bash",
          input: { command: "printf exact" },
        },
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "peer-tool-source",
      session_id: session.sessionId,
      message: {
        content: [
          { type: "tool_use", id: "source-peer", name: "Read", input: { file_path: "peer.txt" } },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "user",
      uuid: "independent-result-source",
      session_id: session.sessionId,
      tool_use_result: { stdout: "INDEPENDENT-RESULT-OUTPUT\n", stderr: "", interrupted: false },
      message: {
        content: [
          {
            type: "tool_result",
            tool_use_id: "source-target",
            content: "INDEPENDENT-RESULT-OUTPUT\n",
            is_error: false,
          },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });
  const chunks = events.filter(
    (event) => (event.update as { type?: string } | undefined)?.type === "agent_message_chunk",
  );
  assert.deepEqual(chunks, [
    {
      event: "session_update",
      session_id: session.sessionId,
      update: {
        type: "agent_message_chunk",
        content: { type: "text", text: "INDEPENDENT-STREAM-TEXT" },
        source_message_uuid: "independent-text-source",
      },
    },
  ]);
  const updates = events.map(
    (event) =>
      event.update as {
        type?: string;
        tool_call?: Record<string, unknown>;
        tool_call_update?: Record<string, unknown>;
      },
  );
  const start = updates.find(
    (update) => update.type === "tool_call" && update.tool_call?.tool_call_id === "source-target",
  );
  assert.ok(start);
  assert.equal(start.tool_call?.source_message_uuid, "independent-tool-source");
  assert.deepEqual(start.tool_call?.raw_input, { command: "printf exact" });
  const result = updates.filter(
    (update) =>
      update.type === "tool_call_update" &&
      update.tool_call_update?.tool_call_id === "source-target",
  );
  assert.ok(result.length > 0);
  assert.ok(
    result.every(
      (update) => update.tool_call_update?.source_message_uuid === "independent-result-source",
    ),
  );
  const target = publishedTool(events, session.sessionId, "source-target");
  assert.equal(target.status, "completed");
  assert.equal(target.raw_output, "INDEPENDENT-RESULT-OUTPUT\n");
  assert.equal(target.source_message_uuid, "independent-result-source");
  const peer = publishedTool(events, session.sessionId, "source-peer");
  assert.equal(peer.status, "in_progress");
  assert.equal(peer.source_message_uuid, "peer-tool-source");
  assert.equal(peer.raw_output, undefined);
});

test("the submitted user UUID starts before its actual owned stream text and source identity", () => {
  const session = makeSessionState();
  session.sessionId = "stream-start-owner";
  const events = captureBridgeEvents(() =>
    handleSdkMessage(session, {
      type: "stream_event",
      uuid: "assistant-text-source",
      session_id: session.sessionId,
      parent_tool_use_id: null,
      user_message_uuid: "submitted-user-id",
      event: {
        type: "content_block_delta",
        delta: { type: "text_delta", text: "OWNED-TEXT\n exact  " },
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
  );
  const starts = events.filter((event) => event.event === "user_message_started");
  assert.deepEqual(starts, [
    {
      event: "user_message_started",
      session_id: session.sessionId,
      message_uuid: "submitted-user-id",
      source: "stream_event",
    },
  ]);
  const content = events.filter(
    (event) => (event.update as { type?: string } | undefined)?.type === "agent_message_chunk",
  );
  assert.deepEqual(content, [
    {
      event: "session_update",
      session_id: session.sessionId,
      update: {
        type: "agent_message_chunk",
        content: { type: "text", text: "OWNED-TEXT\n exact  " },
        source_message_uuid: "assistant-text-source",
      },
    },
  ]);
  assert.ok(
    events.indexOf(starts[0] as Record<string, unknown>) <
      events.indexOf(content[0] as Record<string, unknown>),
  );
});

test("plural user UUID admission preserves the full ordered valid budget and uses singular fallback only without valid plural identities", () => {
  const malformed = [null, 7, false, {}, [], "", " "];
  const incoming = [
    ...malformed,
    "  admitted-0  ",
    "admitted-0",
    ...Array.from({ length: 70 }, (_, index) => `admitted-${index}`),
    "admitted-1",
  ];
  const expected = Array.from({ length: 64 }, (_, index) => `admitted-${index}`);
  for (const row of [
    { name: "bounded", plural: incoming, singular: "must-not-use-singular", expected },
    {
      name: "plural-authority",
      plural: [null, " chosen-plural ", "chosen-plural"],
      singular: "must-not-use-singular",
      expected: ["chosen-plural"],
    },
    {
      name: "malformed-fallback",
      plural: malformed,
      singular: " fallback-singular ",
      expected: ["fallback-singular"],
    },
    {
      name: "absent-fallback",
      plural: undefined,
      singular: "legacy-singular",
      expected: ["legacy-singular"],
    },
    { name: "no-valid", plural: malformed, singular: " ", expected: [] },
  ]) {
    const session = makeSessionState();
    session.sessionId = `uuid-admission-${row.name}`;
    const events = captureBridgeEvents(() =>
      handleSdkMessage(session, {
        type: "assistant",
        uuid: `frame-${row.name}`,
        session_id: session.sessionId,
        parent_tool_use_id: null,
        user_message_uuids: row.plural,
        user_message_uuid: row.singular,
        message: { role: "assistant", content: [] },
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    assert.deepEqual(
      events,
      row.expected.map((uuid) => ({
        event: "user_message_started",
        session_id: session.sessionId,
        message_uuid: uuid,
        source: "assistant",
      })),
      row.name,
    );
  }
});

test("MCP result publication retains public links and exact text on direct and fallback routes without private metadata", () => {
  for (const route of ["direct", "fallback"]) {
    for (const failed of [false, true]) {
      const session = makeSessionState();
      const body = failed ? `${route}-RESOURCE-FAILURE` : `${route}-RESOURCE-ANSWER`;
      const events = captureBridgeEvents(() => {
        handleSdkMessage(session, {
          type: "assistant",
          uuid: "mcp-source",
          message: {
            content: [
              {
                type: "tool_use",
                id: "mcp-target",
                name: "mcp__docs__export",
                input: { target: "report" },
              },
              {
                type: "tool_use",
                id: "mcp-sibling",
                name: "mcp__docs__export",
                input: { target: "other" },
              },
            ],
          },
        } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
        const wrapper = {
          content: body,
          is_error: failed,
          structuredContentOmitted: true,
          resourceLinks: [
            { uri: "mcp://public/report.csv", name: "report.csv", mimeType: "text/csv", size: 42 },
            { uri: 7, name: "INVALID-DESCRIPTOR" },
          ],
          _meta: {
            private: "PRIVATE-META-SENTINEL",
            resourceLinks: [{ uri: "mcp://private/secret", name: "PRIVATE-LINK-SENTINEL" }],
          },
        };
        handleSdkMessage(session, {
          type: "user",
          uuid: "mcp-result",
          parent_tool_use_id: route === "fallback" ? "mcp-target" : null,
          tool_use_result: wrapper,
          message: {
            content:
              route === "direct"
                ? [
                    {
                      type: "tool_result",
                      tool_use_id: "mcp-target",
                      is_error: failed,
                      content: body,
                    },
                  ]
                : [],
          },
        } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      });
      const tool = publishedTool(events, session.sessionId, "mcp-target");
      assert.equal(tool.status, failed ? "failed" : "completed");
      assert.equal(tool.raw_output, body);
      assert.equal(tool.source_message_uuid, "mcp-result");
      assert.deepEqual(tool.content, [
        { type: "content", content: { type: "text", text: body } },
        {
          type: "resource_link",
          uri: "mcp://public/report.csv",
          name: "report.csv",
          mime_type: "text/csv",
          size: 42,
        },
      ]);
      assert.equal(
        (tool.output_metadata as { structured_content_omitted?: boolean })
          ?.structured_content_omitted,
        true,
      );
      const sibling = publishedTool(events, session.sessionId, "mcp-sibling");
      assert.equal(sibling.status, "in_progress");
      assert.equal(sibling.raw_output, undefined);
      assert.doesNotMatch(
        JSON.stringify(events),
        /PRIVATE-META-SENTINEL|PRIVATE-LINK-SENTINEL|mcp:\/\/private|INVALID-DESCRIPTOR/,
      );
    }
  }
});
test("confirmed background notifications publish singular public links and actual completion without reopening on repeated delivery", () => {
  for (const summary of ["INDEPENDENT-EXPORT-SUMMARY", undefined]) {
    const session = makeSessionState();
    session.sessionId = `background-links-${summary ? "summary" : "links-only"}`;
    const initial = captureBridgeEvents(() => {
      emitToolCall(session, "background-link-target", "mcp__docs__export", { target: "report" });
      emitToolCall(session, "background-link-peer", "mcp__docs__export", { target: "other" });
      handleTaskSystemMessage(session, "task_started", {
        task_id: "background-link-task",
        tool_use_id: "background-link-target",
        description: "Independent export job",
        is_backgrounded: true,
      });
      handleTaskSystemMessage(session, "task_started", {
        task_id: "background-peer-task",
        tool_use_id: "background-link-peer",
        description: "Independent peer job",
        is_backgrounded: true,
      });
    });
    assert.equal(
      publishedTool(initial, session.sessionId, "background-link-target").status,
      "detached",
    );
    const notification = {
      task_id: "background-link-task",
      tool_use_id: "background-link-target",
      status: "completed",
      ...(summary ? { summary } : {}),
      resource_links: [
        { uri: "mcp://docs/report.csv", name: "report.csv", mimeType: "text/csv" },
        { uri: "", name: "INVALID-PUBLIC-DESCRIPTOR" },
      ],
    };
    const completed = captureBridgeEvents(() =>
      handleTaskSystemMessage(session, "task_notification", notification),
    );
    const expected = [
      ...(summary
        ? [{ type: "content", content: { type: "text", text: "INDEPENDENT-EXPORT-SUMMARY" } }]
        : []),
      {
        type: "resource_link",
        uri: "mcp://docs/report.csv",
        name: "report.csv",
        mime_type: "text/csv",
      },
    ];
    const final = publishedTool(
      [...initial, ...completed],
      session.sessionId,
      "background-link-target",
    );
    assert.equal(final.status, "completed");
    assert.deepEqual(final.content, expected);
    assert.doesNotMatch(JSON.stringify(completed), /INVALID-PUBLIC-DESCRIPTOR/);
    const repeated = captureBridgeEvents(() => {
      handleTaskSystemMessage(session, "task_notification", notification);
      handleTaskSystemMessage(session, "task_progress", {
        task_id: "background-link-task",
        tool_use_id: "background-link-target",
        summary: "STALE-BACKGROUND-PROGRESS",
      });
    });
    for (const frame of publishedToolFrames(
      [...completed, ...repeated],
      session.sessionId,
      "background-link-target",
    )) {
      if (frame.status) assert.equal(frame.status, "completed");
      if (frame.content) assert.deepEqual(frame.content, expected);
    }
    assert.deepEqual(
      publishedTool(
        [...initial, ...completed, ...repeated],
        session.sessionId,
        "background-link-target",
      ),
      final,
    );
    const peer = publishedTool(
      [...initial, ...completed, ...repeated],
      session.sessionId,
      "background-link-peer",
    );
    assert.equal(peer.status, "detached");
    assert.equal(peer.raw_output, undefined);
    assert.deepEqual(peer.content, []);
  }
});

test("command lifecycle starts only the submitted command identity and never its unrelated frame UUID", () => {
  for (const row of [
    { state: "queued", command: "submitted-command", expected: false },
    { state: "started", command: "submitted-command", expected: true },
    { state: "completed", command: "submitted-command", expected: false },
    { state: "started", command: undefined, expected: false },
    { state: "started", command: "", expected: false },
  ]) {
    const session = makeSessionState();
    session.sessionId = "command-lifecycle-owner";
    const events = captureBridgeEvents(() =>
      handleSdkMessage(session, {
        type: "command_lifecycle",
        state: row.state,
        command_uuid: row.command,
        uuid: "different-lifecycle-frame",
        session_id: session.sessionId,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    assert.deepEqual(
      events,
      row.expected
        ? [
            {
              event: "user_message_started",
              session_id: session.sessionId,
              message_uuid: "submitted-command",
              source: "command_lifecycle",
            },
          ]
        : [],
    );
  }
});

test("handleSdkMessage uses assistant and result as correlation fallbacks and forwards queue count", () => {
  const assistantSession = makeSessionState();
  const assistantEvents = captureBridgeEvents(() => {
    handleSdkMessage(assistantSession, {
      type: "assistant",
      uuid: "assistant-complete",
      session_id: "session-1",
      parent_tool_use_id: null,
      user_message_uuid: "user-message-2",
      message: { role: "assistant", content: [] },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });
  assert.deepEqual(assistantEvents[0], {
    event: "user_message_started",
    session_id: "session-1",
    message_uuid: "user-message-2",
    source: "assistant",
  });

  const resultSession = makeSessionState();
  const resultEvents = captureBridgeEvents(() => {
    handleSdkMessage(resultSession, {
      type: "result",
      subtype: "success",
      uuid: "result-1",
      session_id: "session-1",
      user_message_uuid: "user-message-3",
      queued_turn_count: 2,
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });
  assert.deepEqual(resultEvents, [
    {
      event: "user_message_started",
      session_id: "session-1",
      message_uuid: "user-message-3",
      source: "result",
    },
    {
      event: "turn_complete",
      session_id: "session-1",
      queued_turn_count: 2,
    },
  ]);
});

test("permission denials fail only the denied open tool once before completion and preserve denied terminal and unrelated results", () => {
  const session = makeSessionState();
  session.sessionId = "denial-owner";
  const initial = captureBridgeEvents(() => {
    emitToolCall(session, "denied-open", "Write", {
      file_path: "owned.txt",
      content: "Unapplied request",
    });
    emitToolCall(session, "denied-terminal", "Read", { file_path: "finished.txt" });
    emitToolResultUpdate(session, "denied-terminal", false, "ACTUAL-PREVIOUS-READ");
    emitToolCall(session, "not-denied-open", "Bash", { command: "printf control" });
  });
  const terminalBefore = publishedTool(initial, session.sessionId, "denied-terminal");
  const result = captureBridgeEvents(() =>
    handleResultMessage(session, {
      type: "result",
      subtype: "success",
      permission_denials: [
        { tool_name: "Write", tool_use_id: "denied-open", tool_input: {} },
        { tool_name: "Write", tool_use_id: "denied-open", tool_input: {} },
        { tool_name: "Read", tool_use_id: "denied-terminal", tool_input: {} },
        { tool_use_id: "missing-tool" },
        { tool_use_id: "" },
        { tool_use_id: 7 },
        null,
        "not-a-denial",
      ],
    }),
  );
  const failed = publishedToolFrames(result, session.sessionId, "denied-open");
  assert.deepEqual(failed, [{ status: "failed" }]);
  assert.deepEqual(publishedToolFrames(result, session.sessionId, "denied-terminal"), []);
  assert.deepEqual(
    publishedTool([...initial, ...result], session.sessionId, "denied-terminal"),
    terminalBefore,
  );
  assert.equal(terminalBefore.raw_output, "ACTUAL-PREVIOUS-READ");
  assert.equal(
    publishedTool([...initial, ...result], session.sessionId, "not-denied-open").status,
    "completed",
  );
  assert.equal(
    publishedTool([...initial, ...result], session.sessionId, "denied-open").status,
    "failed",
  );
  const completed = result.filter((event) => event.event === "turn_complete");
  assert.deepEqual(completed, [{ event: "turn_complete", session_id: session.sessionId }]);
  const failedIndex = result.findIndex(
    (event) =>
      (event.update as { tool_call_update?: { tool_call_id: string } } | undefined)
        ?.tool_call_update?.tool_call_id === "denied-open",
  );
  assert.ok(
    failedIndex >= 0 && failedIndex < result.indexOf(completed[0] as Record<string, unknown>),
  );
  const publishedIds = result.flatMap((event) => {
    const update = event.update as
      | {
          type?: string;
          tool_call?: { tool_call_id: string };
          tool_call_update?: { tool_call_id: string };
        }
      | undefined;
    return update?.tool_call
      ? [update.tool_call.tool_call_id]
      : update?.tool_call_update
        ? [update.tool_call_update.tool_call_id]
        : [];
  });
  assert.ok(publishedIds.every((id) => id === "denied-open" || id === "not-denied-open"));
});

test("supported child replies never claim a root submission while an interleaved root frame still starts and publishes text", () => {
  const session = makeSessionState();
  session.sessionId = "child-correlation-owner";
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "stream_event",
      uuid: "child-stream-source",
      session_id: session.sessionId,
      parent_tool_use_id: "parent-agent-tool",
      user_message_uuid: "child-must-not-start",
      event: {
        type: "content_block_delta",
        delta: { type: "text_delta", text: "CHILD-STREAM-TEXT" },
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "stream_event",
      uuid: "root-text-source",
      session_id: session.sessionId,
      parent_tool_use_id: null,
      user_message_uuid: "root-submitted-user",
      event: {
        type: "content_block_delta",
        delta: { type: "text_delta", text: "ROOT-OWNED-TEXT" },
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    for (const owner of [
      { parent_tool_use_id: "parent-agent-tool" },
      { agent_id: "native-child-agent" },
    ])
      handleSdkMessage(session, {
        type: "assistant",
        uuid: "child-assistant-source",
        session_id: session.sessionId,
        ...owner,
        user_message_uuid: "child-must-not-start",
        message: { role: "assistant", content: [{ type: "text", text: "CHILD-FINAL-TEXT" }] },
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });
  assert.deepEqual(
    events.filter((event) => event.event === "user_message_started"),
    [
      {
        event: "user_message_started",
        session_id: session.sessionId,
        message_uuid: "root-submitted-user",
        source: "stream_event",
      },
    ],
  );
  const root = events.filter(
    (event) => (event.update as { type?: string } | undefined)?.type === "agent_message_chunk",
  );
  assert.deepEqual(root, [
    {
      event: "session_update",
      session_id: session.sessionId,
      update: {
        type: "agent_message_chunk",
        source_message_uuid: "root-text-source",
        content: { type: "text", text: "ROOT-OWNED-TEXT" },
      },
    },
  ]);
});

test("actual SDK diagnostics retain lifecycle identifiers and supported tokens while excluding content and free-form reasons", async () => {
  const { spawnSync } = await import("node:child_process");
  const moduleUrl = new URL("./bridge/message_handlers.js", import.meta.url).href;
  const script = `import {handleSdkMessage} from ${JSON.stringify(moduleUrl)};const session={sessionId:'diagnostic-owner'};handleSdkMessage(session,{type:'command_lifecycle',state:'started',status:'absorbed',reason:'absorbed_mid_turn',uuid:'diagnostic-frame',source_uuid:'submitted-source',parent_tool_use_id:null,request_id:'diagnostic-request',prompt:'PRIVATE-PROMPT-SENTINEL',content:'PRIVATE-CONTENT-SENTINEL',message:{content:'PRIVATE-MESSAGE-SENTINEL'}});handleSdkMessage(session,{type:'future_message',uuid:'future-frame',reason:'PRIVATE-FREE-FORM-REASON SENTINEL',prompt:'PRIVATE-PROMPT-SENTINEL',content:'PRIVATE-CONTENT-SENTINEL',message:{content:'PRIVATE-MESSAGE-SENTINEL'}});`;
  const child = spawnSync(process.execPath, ["--input-type=module", "-e", script], {
    encoding: "utf8",
    windowsHide: true,
    timeout: 15000,
    env: { ...process.env, CLAUDE_RS_BRIDGE_DIAGNOSTICS: "1" },
  });
  assert.equal(child.error, undefined);
  assert.equal(child.status, 0, child.stderr);
  assert.equal(child.stdout, "");
  const records = child.stderr
    .split("\n")
    .filter((line) => line.trim())
    .map(
      (line) =>
        JSON.parse(line) as {
          schema: string;
          event_name: string;
          session_id: string;
          fields: Record<string, unknown>;
        },
    );
  const lifecycle = records.find(
    (record) => record.event_name === "sdk_command_lifecycle_start_invalid",
  );
  assert.ok(lifecycle);
  assert.equal(lifecycle.schema, "claude-rs-log/v1");
  assert.equal(lifecycle.session_id, "diagnostic-owner");
  assert.equal(lifecycle.fields.sdk_type, "command_lifecycle");
  assert.equal(lifecycle.fields.sdk_uuid, "diagnostic-frame");
  assert.deepEqual(lifecycle.fields.sdk_uuid_fields, {
    uuid: "diagnostic-frame",
    source_uuid: "submitted-source",
  });
  assert.equal(lifecycle.fields.request_id, "diagnostic-request");
  assert.equal(lifecycle.fields.parent_tool_use_scope, "root");
  assert.equal(lifecycle.fields.lifecycle_status, "absorbed");
  assert.equal(lifecycle.fields.lifecycle_state, "started");
  assert.equal(lifecycle.fields.lifecycle_reason, "absorbed_mid_turn");
  const future = records.find((record) => record.event_name === "sdk_message_unhandled");
  assert.ok(future);
  assert.equal(future.fields.sdk_uuid, "future-frame");
  assert.equal(future.fields.lifecycle_reason, undefined);
  assert.doesNotMatch(
    child.stderr,
    /PRIVATE-PROMPT-SENTINEL|PRIVATE-CONTENT-SENTINEL|PRIVATE-MESSAGE-SENTINEL|PRIVATE-FREE-FORM-REASON/,
  );
});

test("handleSdkMessage emits SDK-owned context Markdown exactly once", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "context-assistant-1",
      session_id: "session-1",
      parent_tool_use_id: null,
      context_usage: { used_percentage: 42 },
      message: {
        role: "assistant",
        content: [
          { type: "text", text: "## Context usage" },
          {
            type: "text",
            text: "| Category | Tokens |\n| --- | ---: |\n| System | 100 |",
          },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events, [
    {
      event: "session_update",
      session_id: "session-1",
      update: {
        type: "agent_message_chunk",
        content: {
          type: "text",
          text: "## Context usage\n\n| Category | Tokens |\n| --- | ---: |\n| System | 100 |",
        },
        source_message_uuid: "context-assistant-1",
      },
    },
  ]);
  const noResponseId = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "context-without-response-id",
      session_id: session.sessionId,
      parent_tool_use_id: null,
      context_usage: { categories: [] },
      message: {
        role: "assistant",
        content: [{ type: "text", text: "## Context Usage\n\nExact no-ID body" }],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });
  assert.deepEqual(noResponseId, [
    {
      event: "session_update",
      session_id: session.sessionId,
      update: {
        type: "agent_message_chunk",
        content: { type: "text", text: "## Context Usage\n\nExact no-ID body" },
        source_message_uuid: "context-without-response-id",
      },
    },
  ]);
});

test("final tool input enriches the original row and completed Agent results preserve the intended worker identity", () => {
  const rows = [
    {
      name: "Grep",
      input: { pattern: "INDEPENDENT-NEEDLE", output_mode: "content", "-n": true },
      identity: "INDEPENDENT-NEEDLE",
      result: undefined,
      wrapped: false,
    },
    {
      name: "Agent",
      input: {
        name: "named-worker",
        description: "REQUEST-DESCRIPTION",
        prompt: "REQUEST-PROMPT",
        subagent_type: "requested-kind",
        model: "requested-model",
      },
      identity: "named-worker",
      result: { agentType: "different-returned-kind", resolvedModel: "actual-model" },
      wrapped: false,
    },
    {
      name: "Agent",
      input: { description: "REQUEST-DESCRIPTION", prompt: "REQUEST-PROMPT" },
      identity: "actual-returned-kind",
      result: { agentType: "actual-returned-kind", resolvedModel: "actual-model" },
      wrapped: true,
    },
  ];
  for (const row of rows) {
    const session = makeSessionState();
    const events = captureBridgeEvents(() => {
      handleSdkMessage(session, {
        type: "stream_event",
        uuid: "stream-source",
        session_id: session.sessionId,
        parent_tool_use_id: null,
        event: {
          type: "content_block_start",
          content_block: { type: "tool_use", id: "enriched-target", name: row.name, input: {} },
        },
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      handleSdkMessage(session, {
        type: "assistant",
        uuid: "final-source",
        session_id: session.sessionId,
        parent_tool_use_id: null,
        message: {
          content: [
            { type: "tool_use", id: "enriched-target", name: row.name, input: row.input },
            {
              type: "tool_use",
              id: "enriched-sibling",
              name: "Read",
              input: { file_path: "unrelated.txt" },
            },
          ],
        },
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      if (row.result) {
        const result = {
          ...row.result,
          agentId: "actual-worker",
          status: "completed",
          content: [{ type: "text", text: "ACTUAL-WORKER-ANSWER" }],
        };
        handleSdkMessage(session, {
          type: "user",
          uuid: "worker-result",
          tool_use_result: row.wrapped ? { result } : result,
          message: {
            content: [
              {
                type: "tool_result",
                tool_use_id: "enriched-target",
                content: "ACTUAL-WORKER-ANSWER",
              },
            ],
          },
        } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      }
    });
    const tool = publishedTool(events, session.sessionId, "enriched-target");
    assert.ok(String(tool.title).includes(row.identity));
    assert.deepEqual(tool.raw_input, row.input);
    assert.deepEqual(tool.meta, { claudeCode: { toolName: row.name, parentToolUseId: null } });
    if (row.result) {
      assert.equal(tool.status, "completed");
      assert.equal(tool.source_message_uuid, "worker-result");
      assert.equal(tool.raw_output, "ACTUAL-WORKER-ANSWER");
      assert.deepEqual(tool.content, [
        { type: "content", content: { type: "text", text: "ACTUAL-WORKER-ANSWER" } },
      ]);
      assert.equal(
        (tool.output_metadata as { agent?: { resolved_model?: string } })?.agent?.resolved_model,
        "actual-model",
      );
      const late = captureBridgeEvents(() =>
        handleSdkMessage(session, {
          type: "assistant",
          uuid: "late-source",
          message: {
            content: [
              {
                type: "tool_use",
                id: "enriched-target",
                name: row.name,
                input: { name: "LATE-REOPEN" },
              },
            ],
          },
        } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
      );
      const settled = publishedTool([...events, ...late], session.sessionId, "enriched-target");
      assert.equal(settled.status, "completed");
      assert.equal(settled.raw_output, "ACTUAL-WORKER-ANSWER");
    } else {
      assert.equal(tool.source_message_uuid, "final-source");
      assert.match(String(tool.title), /content/);
    }
    const sibling = publishedTool(events, session.sessionId, "enriched-sibling");
    assert.deepEqual(sibling.raw_input, { file_path: "unrelated.txt" });
    assert.equal(sibling.status, "in_progress");
    assert.equal(sibling.raw_output, undefined);
  }
});

function publishedTaskRows(
  events: Array<Record<string, unknown>>,
  sessionId: string,
  id: string,
): Array<Record<string, unknown>> {
  return events.flatMap((event) => {
    const update = event.update as
      | { type?: string; tasks?: Array<Record<string, unknown>> }
      | undefined;
    if (update?.type !== "task_state_update") return [];
    const rows = (update.tasks ?? []).filter((row) => row.task_id === id);
    if (rows.length) assert.equal(event.session_id, sessionId);
    return rows;
  });
}
test("task patches publish linked task and tool projections with background, terminal error and description precedence", () => {
  for (const row of [
    {
      name: "running-background",
      patch: {
        status: "running",
        description: "INDEPENDENT-RUNNING-DESCRIPTION",
        is_backgrounded: true,
      },
      toolStatus: "detached",
      taskStatus: "in_progress",
      text: "INDEPENDENT-RUNNING-DESCRIPTION",
      terminal: undefined,
      error: undefined,
    },
    {
      name: "terminal-error",
      patch: {
        status: "killed",
        error: "INDEPENDENT-TERMINAL-ERROR",
        end_time: 4321,
        total_paused_ms: 275,
      },
      toolStatus: "killed",
      taskStatus: "completed",
      text: "INDEPENDENT-TERMINAL-ERROR",
      terminal: "killed",
      error: "INDEPENDENT-TERMINAL-ERROR",
    },
    {
      name: "description-before-error",
      patch: {
        status: "failed",
        description: "INDEPENDENT-SELECTED-DESCRIPTION",
        error: "LOWER-PRIORITY-ERROR",
        end_time: 4321,
        total_paused_ms: 275,
      },
      toolStatus: "failed",
      taskStatus: "completed",
      text: "INDEPENDENT-SELECTED-DESCRIPTION",
      terminal: "failed",
      error: "LOWER-PRIORITY-ERROR",
    },
  ]) {
    const session = makeSessionState();
    session.sessionId = `task-patch-${row.name}`;
    const initial = captureBridgeEvents(() => {
      emitToolCall(session, "patch-target", "Agent", { prompt: "Target job" });
      emitToolCall(session, "patch-peer", "Agent", { prompt: "Peer job" });
      handleTaskSystemMessage(session, "task_started", {
        task_id: "target-task",
        tool_use_id: "patch-target",
        description: "Initial independent target",
      });
      handleTaskSystemMessage(session, "task_started", {
        task_id: "peer-task",
        tool_use_id: "patch-peer",
        description: "Unrelated peer",
      });
    });
    const events = captureBridgeEvents(() =>
      handleTaskSystemMessage(session, "task_updated", {
        task_id: "target-task",
        patch: row.patch,
      }),
    );
    const target = publishedTool([...initial, ...events], session.sessionId, "patch-target");
    assert.equal(target.status, row.toolStatus);
    assert.equal(target.raw_output, row.text);
    assert.deepEqual(target.content, [
      { type: "content", content: { type: "text", text: row.text } },
    ]);
    const rows = publishedTaskRows(events, session.sessionId, "target-task");
    assert.equal(rows.length, 1);
    const task = rows[0];
    assert.ok(task);
    assert.equal(task.source_tool_call_id, "patch-target");
    assert.equal(task.status, row.taskStatus);
    if (row.patch.description) assert.equal(task.description, row.patch.description);
    const toolMetadata = target.task_metadata as Record<string, unknown>,
      taskMetadata = task.metadata as Record<string, unknown>;
    if (row.terminal) {
      for (const metadata of [toolMetadata, taskMetadata]) {
        assert.equal(metadata.error, row.error);
        assert.equal(metadata.terminal_status, row.terminal);
        assert.equal(metadata.end_time, 4321);
        assert.equal(metadata.total_paused_ms, 275);
      }
    } else {
      assert.equal(toolMetadata.is_backgrounded, true);
      assert.equal(taskMetadata.is_backgrounded, true);
    }
    assert.deepEqual(publishedToolFrames(events, session.sessionId, "patch-peer"), []);
    assert.deepEqual(publishedTaskRows(events, session.sessionId, "peer-task"), []);
    assert.deepEqual(
      publishedTool([...initial, ...events], session.sessionId, "patch-peer"),
      publishedTool(initial, session.sessionId, "patch-peer"),
    );
  }
});

test("successive task patches publish complete retained metadata independently on the target task and tool", () => {
  const session = makeSessionState();
  session.sessionId = "metadata-retention-owner";
  const initial = captureBridgeEvents(() => {
    emitToolCall(session, "retention-target", "Agent", { prompt: "Retained target" });
    emitToolCall(session, "retention-peer", "Agent", { prompt: "Unrelated peer" });
    handleSdkMessage(session, {
      type: "system",
      subtype: "task_started",
      uuid: "independent-retention-source",
      session_id: session.sessionId,
      request_id: "independent-retention-request",
      subagent_type: "independent-retention-agent",
      task_description: "INDEPENDENT-RETENTION-SUBJECT",
      description: "Independent native task description",
      task_id: "retention-task",
      tool_use_id: "retention-target",
      blocked: false,
      parent_agent_id: "independent-parent-agent",
      is_backgrounded: false,
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleTaskSystemMessage(session, "task_started", {
      task_id: "retention-peer-task",
      tool_use_id: "retention-peer",
      blocked: true,
      parent_agent_id: "peer-parent-agent",
    });
  });
  const startTasks = publishedTaskRows(initial, session.sessionId, "retention-task");
  assert.equal(startTasks.length, 1);
  const started = startTasks[0]!;
  assert.equal(started.subject, "INDEPENDENT-RETENTION-SUBJECT");
  assert.equal(started.status, "in_progress");
  assert.equal(started.source_tool_call_id, "retention-target");
  assert.equal(
    (started.metadata as Record<string, unknown>).request_id,
    "independent-retention-request",
  );
  assert.equal(
    (started.metadata as Record<string, unknown>).subagent_type,
    "independent-retention-agent",
  );
  assert.equal(
    (started.metadata as Record<string, unknown>).task_description,
    "INDEPENDENT-RETENTION-SUBJECT",
  );
  const startTool = publishedTool(initial, session.sessionId, "retention-target");
  assert.equal(startTool.raw_output, undefined);
  assert.equal(
    (startTool.task_metadata as Record<string, unknown>).request_id,
    "independent-retention-request",
  );
  let all = [...initial];
  for (const row of [
    {
      patch: { status: "running", is_backgrounded: true },
      expected: {
        request_id: "independent-retention-request",
        subagent_type: "independent-retention-agent",
        task_description: "INDEPENDENT-RETENTION-SUBJECT",
        blocked: false,
        parent_agent_id: "independent-parent-agent",
        is_backgrounded: true,
      },
    },
    {
      patch: { error: "INDEPENDENT-RETAINED-ERROR", end_time: 5678 },
      expected: {
        request_id: "independent-retention-request",
        subagent_type: "independent-retention-agent",
        task_description: "INDEPENDENT-RETENTION-SUBJECT",
        blocked: false,
        parent_agent_id: "independent-parent-agent",
        is_backgrounded: true,
        error: "INDEPENDENT-RETAINED-ERROR",
        end_time: 5678,
      },
    },
    {
      patch: { is_backgrounded: false, total_paused_ms: 91 },
      expected: {
        request_id: "independent-retention-request",
        subagent_type: "independent-retention-agent",
        task_description: "INDEPENDENT-RETENTION-SUBJECT",
        blocked: false,
        parent_agent_id: "independent-parent-agent",
        is_backgrounded: false,
        error: "INDEPENDENT-RETAINED-ERROR",
        end_time: 5678,
        total_paused_ms: 91,
      },
    },
  ]) {
    const events = captureBridgeEvents(() =>
      handleTaskSystemMessage(session, "task_updated", {
        task_id: "retention-task",
        patch: row.patch,
      }),
    );
    all = [...all, ...events];
    const frames = publishedToolFrames(events, session.sessionId, "retention-target").filter(
      (frame) => frame.task_metadata !== undefined,
    );
    assert.ok(frames.length > 0);
    for (const frame of frames) assert.deepEqual(frame.task_metadata, row.expected);
    const tasks = publishedTaskRows(events, session.sessionId, "retention-task");
    assert.equal(tasks.length, 1);
    const task = tasks[0];
    assert.ok(task);
    assert.equal(task.source_tool_call_id, "retention-target");
    for (const [key, value] of Object.entries(row.expected))
      assert.deepEqual((task.metadata as Record<string, unknown>)[key], value, key);
    assert.deepEqual(publishedToolFrames(events, session.sessionId, "retention-peer"), []);
    assert.deepEqual(publishedTaskRows(events, session.sessionId, "retention-peer-task"), []);
  }
  assert.deepEqual(
    publishedTool(all, session.sessionId, "retention-peer"),
    publishedTool(initial, session.sessionId, "retention-peer"),
  );
});

test("Monitor and Workflow native acknowledgements remain detached through parent success and settle their own real outcomes", () => {
  for (const row of [
    {
      name: "Monitor",
      outcome: "completed",
      toolStatus: "completed",
      input: {
        description: "watch independent output",
        timeout_ms: 30000,
        persistent: false,
        command: "tail owned.log",
      },
      launch: { taskId: "monitor-owned-task", timeoutMs: 30000, persistent: false },
      taskId: "monitor-owned-task",
      runId: undefined,
    },
    {
      name: "Workflow",
      outcome: "completed",
      toolStatus: "completed",
      input: { name: "owned-workflow" },
      launch: {
        status: "async_launched",
        taskId: "workflow-completed",
        runId: "owned-run-completed",
      },
      taskId: "workflow-completed",
      runId: "owned-run-completed",
    },
    {
      name: "Workflow",
      outcome: "failed",
      toolStatus: "failed",
      input: { name: "owned-workflow" },
      launch: { status: "async_launched", taskId: "workflow-failed", runId: "owned-run-failed" },
      taskId: "workflow-failed",
      runId: "owned-run-failed",
    },
    {
      name: "Workflow",
      outcome: "stopped",
      toolStatus: "killed",
      input: { name: "owned-workflow" },
      launch: { status: "async_launched", taskId: "workflow-stopped", runId: "owned-run-stopped" },
      taskId: "workflow-stopped",
      runId: "owned-run-stopped",
    },
  ]) {
    const session = makeSessionState();
    session.sessionId = `adapter-${row.name}-${row.outcome}`;
    const initial = captureBridgeEvents(() => {
      handleSdkMessage(session, {
        type: "assistant",
        uuid: "adapter-real-launch",
        session_id: session.sessionId,
        message: {
          role: "assistant",
          content: [
            { type: "tool_use", id: "adapter-target", name: row.name, input: row.input },
            {
              type: "tool_use",
              id: "adapter-peer",
              name: "Read",
              input: { file_path: "unrelated-peer.txt" },
            },
          ],
        },
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      handleSdkMessage(session, {
        type: "user",
        uuid: "adapter-real-ack",
        session_id: session.sessionId,
        tool_use_result: row.launch,
        message: {
          role: "user",
          content: [
            {
              type: "tool_result",
              tool_use_id: "adapter-target",
              content: "INDEPENDENT-NATIVE-ADAPTER-ACK",
              is_error: false,
            },
          ],
        },
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    });
    assert.equal(publishedTool(initial, session.sessionId, "adapter-target").status, "detached");
    const progress = captureBridgeEvents(() =>
      handleTaskSystemMessage(session, "task_updated", {
        task_id: row.taskId,
        ...(row.runId ? { run_id: row.runId } : {}),
        patch: {
          status: "running",
          description: "INDEPENDENT-ADAPTER-RUNNING-OUTPUT",
          is_backgrounded: true,
        },
      }),
    );
    const running = publishedTool([...initial, ...progress], session.sessionId, "adapter-target");
    assert.equal(running.status, "detached");
    assert.equal(running.raw_output, "INDEPENDENT-ADAPTER-RUNNING-OUTPUT");
    const turn = captureBridgeEvents(() => {
      for (const parent of ["adapter-parent-turn", "adapter-later-turn"]) {
        activityStream(session, { type: "message_start", message: { id: parent } });
        handleSdkMessage(session, {
          type: "result",
          uuid: `${parent}-result`,
          session_id: session.sessionId,
          subtype: "success",
          terminal_reason: "completed",
        } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      }
    });
    assert.deepEqual(
      turn.filter((event) => event.event === "turn_complete"),
      [
        { event: "turn_complete", session_id: session.sessionId, terminal_reason: "completed" },
        { event: "turn_complete", session_id: session.sessionId, terminal_reason: "completed" },
      ],
    );
    assert.ok(
      turn.some(
        (event) => event.event === "turn_complete" && event.session_id === session.sessionId,
      ),
    );
    for (const frame of publishedToolFrames(turn, session.sessionId, "adapter-target"))
      if (frame.status) assert.equal(frame.status, "detached");
    assert.equal(
      publishedTool([...initial, ...progress, ...turn], session.sessionId, "adapter-target").status,
      "detached",
    );
    const outputFile = `C:/owned/${row.name}-${row.outcome}.output`,
      summary = `ACTUAL-${row.name}-${row.outcome}-OUTCOME`;
    const result = captureBridgeEvents(() =>
      handleTaskSystemMessage(session, "task_notification", {
        task_id: row.taskId,
        ...(row.runId ? { run_id: row.runId } : {}),
        status: row.outcome,
        output_file: outputFile,
        summary,
      }),
    );
    const all = [...initial, ...progress, ...turn, ...result],
      finished = publishedTool(all, session.sessionId, "adapter-target");
    assert.equal(finished.status, row.toolStatus);
    assert.equal(finished.raw_output, summary);
    assert.deepEqual(finished.content, [
      { type: "content", content: { type: "text", text: summary } },
    ]);
    const metadata = finished.task_metadata as Record<string, unknown>;
    assert.equal(metadata.output_file, outputFile);
    assert.equal(metadata.summary, summary);
    assert.equal(metadata.terminal_status, row.outcome);
    const task = publishedTaskRows(result, session.sessionId, row.taskId).at(-1);
    assert.ok(task);
    assert.equal(task.status, "completed");
    assert.equal(task.source_tool_call_id, "adapter-target");
    assert.equal((task.metadata as Record<string, unknown>).terminal_status, row.outcome);
    const late = captureBridgeEvents(() =>
      handleTaskSystemMessage(session, "task_updated", {
        task_id: row.taskId,
        ...(row.runId ? { run_id: row.runId } : {}),
        patch: { status: "running", description: "STALE-ADAPTER-OUTPUT", is_backgrounded: true },
      }),
    );
    assert.deepEqual(publishedToolFrames(late, session.sessionId, "adapter-target"), []);
    assert.deepEqual(
      publishedTool([...all, ...late], session.sessionId, "adapter-target"),
      finished,
    );
    assert.equal(
      publishedTool([...all, ...late], session.sessionId, "adapter-peer").status,
      "completed",
    );
  }
});

function taskOperationStart(
  session: SessionState,
  id: string,
  name: string,
  input: Record<string, unknown>,
) {
  return captureBridgeEvents(() =>
    handleSdkMessage(session, {
      type: "assistant",
      uuid: `${id}-source`,
      session_id: session.sessionId,
      message: { role: "assistant", content: [{ type: "tool_use", id, name, input }] },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
  );
}
function taskOperationResult(
  session: SessionState,
  id: string,
  result: unknown,
  content = "NATIVE-SUCCESS-RECEIPT",
) {
  return captureBridgeEvents(() =>
    handleSdkMessage(session, {
      type: "user",
      uuid: `${id}-result-source`,
      session_id: session.sessionId,
      toolUseResult: result,
      message: {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: id, content, is_error: false }],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
  );
}
function taskInventoryFixture(activity = true, includeTarget = true) {
  const session = makeSessionState();
  session.sessionId = "task-inventory-owner";
  const target: import("./types.js").TaskItem = {
    task_id: "selected-task",
    subject: "Independent selected subject",
    description: "Independent retained description",
    ...(activity ? { active_form: "Independent existing activity" } : {}),
    status: "pending",
    owner: "original-owner",
    blocks: ["old-block", "retained-unmentioned-block"],
    blocked_by: ["old-prerequisite", "retained-unmentioned-prerequisite"],
    metadata: { preserved: 7, replaced: "before", deleted: "REMOVE-ME" },
    source_tool_call_id: "prior-source",
  };
  const sibling: import("./types.js").TaskItem = {
    task_id: "sibling-task",
    subject: "Independent sibling subject",
    description: "Sibling retained detail",
    status: "in_progress",
    blocks: ["peer-block"],
    blocked_by: [],
    metadata: { sibling: true },
  };
  for (const row of includeTarget ? [target, sibling] : [sibling]) {
    session.tasksById.set(row.task_id, structuredClone(row));
    session.taskOrder.push(row.task_id);
  }
  return { session, target, sibling };
}
function taskStateEvents(events: Array<Record<string, unknown>>, sessionId: string) {
  return events.flatMap((event) => {
    const update = event.update as
      | {
          type?: string;
          source: string;
          tasks: Array<Record<string, unknown>>;
          removed_task_ids: string[];
          is_complete_snapshot: boolean;
        }
      | undefined;
    if (update?.type !== "task_state_update") return [];
    assert.equal(event.session_id, sessionId);
    return [update];
  });
}
test("TaskCreate publishes its typed task only after the native result and keeps generic TodoWrite independent", () => {
  const session = makeSessionState();
  session.sessionId = "task-create-ingress-owner";
  const start = taskOperationStart(session, "create-target", "TaskCreate", {
    subject: "Independent created subject",
    description: "Independent created detail",
    activeForm: "Independent created activity",
    metadata: { phase: "fixture", approved: false },
  });
  assert.deepEqual(taskStateEvents(start, session.sessionId), []);
  const result = taskOperationResult(
    session,
    "create-target",
    { task: { id: "native-created-task", subject: "Independent created subject" } },
    "NATIVE-CREATE-SUCCESS-PROSE",
  );
  assert.deepEqual(taskStateEvents(result, session.sessionId), [
    {
      type: "task_state_update",
      source: "task_create",
      tasks: [
        {
          task_id: "native-created-task",
          subject: "Independent created subject",
          description: "Independent created detail",
          active_form: "Independent created activity",
          status: "pending",
          blocks: [],
          blocked_by: [],
          metadata: { phase: "fixture", approved: false },
          source_tool_call_id: "create-target",
        },
      ],
      removed_task_ids: [],
      is_complete_snapshot: false,
    },
  ]);
  const created = publishedTool([...start, ...result], session.sessionId, "create-target");
  assert.equal(created.status, "completed");
  for (const frame of publishedToolFrames(result, session.sessionId, "create-target")) {
    assert.equal(frame.raw_output, undefined);
    assert.equal(frame.content, undefined);
  }
  assert.doesNotMatch(JSON.stringify(result), /NATIVE-CREATE-SUCCESS-PROSE/);
  const follow = captureBridgeEvents(() =>
    handleTaskSystemMessage(session, "task_updated", {
      task_id: "native-created-task",
      patch: { status: "running", description: "Admitted later lifecycle detail" },
    }),
  );
  const task = publishedTaskRows(follow, session.sessionId, "native-created-task").at(-1);
  assert.ok(task);
  assert.equal(task.source_tool_call_id, "create-target");
  assert.equal(task.description, "Admitted later lifecycle detail");
  assert.equal(task.status, "in_progress");
  assert.deepEqual(publishedToolFrames(follow, session.sessionId, "create-target"), []);
  const before = structuredClone([...session.tasksById.values()]);
  const todoInput = {
    todos: [
      {
        content: "Independent generic todo",
        status: "pending",
        activeForm: "Generic todo activity",
      },
    ],
  };
  const todoStart = taskOperationStart(session, "generic-todo", "TodoWrite", todoInput),
    todoResult = taskOperationResult(
      session,
      "generic-todo",
      { success: true },
      "Independent generic todo receipt",
    );
  assert.deepEqual(taskStateEvents([...todoStart, ...todoResult], session.sessionId), []);
  assert.equal(
    [...todoStart, ...todoResult].some(
      (event) => (event.update as { type?: string } | undefined)?.type === "plan",
    ),
    false,
  );
  const todo = publishedTool([...todoStart, ...todoResult], session.sessionId, "generic-todo");
  assert.deepEqual(todo.raw_input, todoInput);
  assert.equal(
    (todo.meta as { claudeCode: { toolName: string } }).claudeCode.toolName,
    "TodoWrite",
  );
  assert.equal(todo.status, "completed");
  assert.deepEqual([...session.tasksById.values()], before);
});

test("successful TaskUpdate changes only its selected task and retains activity, dependencies and merged metadata", () => {
  for (const activity of [true, false]) {
    const { session, target, sibling } = taskInventoryFixture(activity);
    const label = taskOperationStart(session, "id-only-update", "TaskUpdate", {
      taskId: "selected-task",
    });
    const title = publishedTool(label, session.sessionId, "id-only-update").title;
    assert.ok(String(title).includes("Independent selected subject"));
    assert.ok(!String(title).includes("Independent sibling subject"));
    const unknown = taskOperationStart(session, "unknown-id-update", "TaskUpdate", {
      taskId: "unknown-task-id",
    });
    assert.ok(
      String(publishedTool(unknown, session.sessionId, "unknown-id-update").title).includes(
        "unknown-task-id",
      ),
    );
    const start = taskOperationStart(session, "confirmed-update", "TaskUpdate", {
      taskId: "selected-task",
      subject: "Independently updated subject",
      status: "in_progress",
      addBlocks: ["old-block", "new-block", "new-block"],
      addBlockedBy: ["old-prerequisite", "new-prerequisite"],
      metadata: { replaced: "after", deleted: null, new_value: false },
    });
    assert.deepEqual(taskStateEvents(start, session.sessionId), []);
    const result = taskOperationResult(session, "confirmed-update", {
      success: true,
      taskId: "selected-task",
      updatedFields: ["subject", "status", "addBlocks", "addBlockedBy", "metadata"],
    });
    const expected = {
      ...target,
      subject: "Independently updated subject",
      status: "in_progress",
      blocks: ["old-block", "retained-unmentioned-block", "new-block"],
      blocked_by: ["old-prerequisite", "retained-unmentioned-prerequisite", "new-prerequisite"],
      metadata: { preserved: 7, replaced: "after", new_value: false },
    };
    assert.deepEqual(taskStateEvents(result, session.sessionId), [
      {
        type: "task_state_update",
        source: "task_update",
        tasks: [expected],
        removed_task_ids: [],
        is_complete_snapshot: false,
      },
    ]);
    assert.deepEqual(session.tasksById.get("selected-task"), expected);
    assert.deepEqual(session.tasksById.get("sibling-task"), sibling);
    assert.equal(
      publishedTool([...start, ...result], session.sessionId, "confirmed-update").status,
      "completed",
    );
    for (const frame of publishedToolFrames(result, session.sessionId, "confirmed-update")) {
      assert.equal(frame.raw_output, undefined);
      assert.equal(frame.content, undefined);
    }
  }
});

test("TaskOutput presents observed structured or XML fields once without mutating divergent authoritative tasks", () => {
  for (const structured of [true, false]) {
    const { session, target, sibling } = taskInventoryFixture();
    const start = taskOperationStart(session, "observed-output", "TaskOutput", {
      task_id: "selected-task",
      block: true,
      timeout: 1000,
    });
    const xml =
      "<retrieval_status>success</retrieval_status>\n<task_id>selected-task</task_id>\n<task_type>local_bash</task_type>\n<status>completed</status>\n<description>OBSERVED-DESCRIPTION</description>\n<output>OBSERVED-OUTPUT-PAYLOAD</output>";
    const result = taskOperationResult(
      session,
      "observed-output",
      structured
        ? {
            retrieval_status: "success",
            task: {
              task_id: "selected-task",
              task_type: "local_bash",
              status: "completed",
              description: "OBSERVED-DESCRIPTION",
              output: "OBSERVED-OUTPUT-PAYLOAD",
            },
          }
        : undefined,
      xml,
    );
    const tool = publishedTool([...start, ...result], session.sessionId, "observed-output");
    assert.equal(tool.status, "completed");
    assert.ok(String(tool.title).includes("Independent selected subject"));
    const contents = tool.content as Array<{
      type: string;
      content?: { type: string; text: string };
    }>;
    const text = contents
      .filter((entry) => entry.type === "content" && entry.content?.type === "text")
      .map((entry) => entry.content?.text ?? "")
      .join("\n");
    for (const marker of ["OBSERVED-DESCRIPTION", "OBSERVED-OUTPUT-PAYLOAD"])
      assert.equal(text.split(marker).length - 1, 1, marker);
    assert.match(text, /retrieval[^\n]*success/i);
    assert.match(text, /task type[^\n]*local[ _]bash/i);
    assert.match(text, /(?:^|\n)status[^\n]*completed/i);
    assert.doesNotMatch(text, /<retrieval_status>|<task_id>|<description>|<output>/);
    assert.deepEqual(taskStateEvents([...start, ...result], session.sessionId), []);
    assert.deepEqual(session.tasksById.get("selected-task"), target);
    assert.deepEqual(session.tasksById.get("sibling-task"), sibling);
  }
});

test("TaskStop reports the real linked producer outcome and leaves an unrelated live task unchanged for present and absent targets", () => {
  for (const present of [true, false]) {
    const session = makeSessionState();
    session.sessionId = `task-stop-${present}`;
    const setup = captureBridgeEvents(() => {
      emitToolCall(session, "stop-peer-producer", "Monitor", {
        description: "Peer monitor",
        timeout_ms: 30000,
        persistent: false,
        command: "tail peer.log",
      });
      emitToolResultUpdate(session, "stop-peer-producer", false, {
        taskId: "peer-live-task",
        timeoutMs: 30000,
        persistent: false,
      });
      if (present) {
        emitToolCall(session, "stop-selected-producer", "Monitor", {
          description: "Selected monitor",
          timeout_ms: 30000,
          persistent: false,
          command: "tail selected.log",
        });
        emitToolResultUpdate(session, "stop-selected-producer", false, {
          taskId: "selected-live-task",
          timeoutMs: 30000,
          persistent: false,
        });
      }
    });
    const peerBefore = structuredClone(session.tasksById.get("peer-live-task"));
    assert.ok(peerBefore);
    const start = taskOperationStart(session, "stop-invocation", "TaskStop", {
      task_id: "selected-live-task",
    });
    const result = taskOperationResult(session, "stop-invocation", {
      message: "INDEPENDENT-STOP-RECEIPT",
      task_id: "selected-live-task",
      task_type: "monitor",
      command: "tail selected.log",
    });
    const tool = publishedTool([...start, ...result], session.sessionId, "stop-invocation");
    assert.equal(tool.status, "completed");
    assert.ok(JSON.stringify(tool.content).includes("INDEPENDENT-STOP-RECEIPT"));
    assert.ok(JSON.stringify(tool.content).includes("tail selected.log"));
    assert.deepEqual(session.tasksById.get("peer-live-task"), peerBefore);
    assert.deepEqual(publishedToolFrames(result, session.sessionId, "stop-peer-producer"), []);
    if (present) {
      assert.equal(
        publishedTool(setup, session.sessionId, "stop-selected-producer").status,
        "detached",
      );
      const rows = publishedTaskRows(result, session.sessionId, "selected-live-task");
      assert.equal(rows.length, 1);
      const task = rows[0];
      assert.ok(task);
      assert.equal(task.status, "completed");
      assert.equal(task.source_tool_call_id, "stop-selected-producer");
      const metadata = task.metadata as Record<string, unknown>;
      assert.equal(metadata.terminal_status, "stopped");
      assert.equal(metadata.task_type, "monitor");
      assert.equal(metadata.command, "tail selected.log");
    } else {
      assert.deepEqual(taskStateEvents(result, session.sessionId), []);
      assert.equal(session.tasksById.has("selected-live-task"), false);
    }
  }
});

test("a confirmed task deletion emits only its removal and later native snapshots preserve the sibling without resurrection", () => {
  const { session, sibling } = taskInventoryFixture();
  const start = taskOperationStart(session, "delete-selected", "TaskUpdate", {
    taskId: "selected-task",
    status: "deleted",
  });
  const result = taskOperationResult(session, "delete-selected", {
    success: true,
    taskId: "selected-task",
    updatedFields: ["status"],
  });
  assert.deepEqual(taskStateEvents(result, session.sessionId), [
    {
      type: "task_state_update",
      source: "task_update",
      tasks: [],
      removed_task_ids: ["selected-task"],
      is_complete_snapshot: false,
    },
  ]);
  assert.equal(
    publishedTool([...start, ...result], session.sessionId, "delete-selected").status,
    "completed",
  );
  for (const frame of publishedToolFrames(result, session.sessionId, "delete-selected")) {
    assert.equal(frame.raw_output, undefined);
    assert.equal(frame.content, undefined);
  }
  assert.deepEqual(session.tasksById.get("sibling-task"), sibling);
  const listStart = taskOperationStart(session, "list-after-delete", "TaskList", {});
  const listed = taskOperationResult(session, "list-after-delete", {
    tasks: [
      {
        id: "sibling-task",
        subject: "Independent sibling subject",
        status: "in_progress",
        blockedBy: [],
      },
    ],
  });
  assert.deepEqual(taskStateEvents(listed, session.sessionId), [
    {
      type: "task_state_update",
      source: "task_list",
      tasks: [sibling],
      removed_task_ids: [],
      is_complete_snapshot: true,
    },
  ]);
  assert.equal(session.tasksById.has("selected-task"), false);
  assert.equal(
    publishedTool([...listStart, ...listed], session.sessionId, "list-after-delete").status,
    "completed",
  );
});

test("a native TaskUpdate refusal publishes its exact cause without changing either task and permits an explicit successful retry", () => {
  const { session, target, sibling } = taskInventoryFixture();
  const start = taskOperationStart(session, "failed-task-update", "TaskUpdate", {
    taskId: "selected-task",
    subject: "MUST-NOT-APPLY",
    status: "completed",
    addBlocks: ["must-not-add"],
    metadata: { mustNotPersist: true },
  });
  const result = taskOperationResult(session, "failed-task-update", {
    success: false,
    taskId: "selected-task",
    updatedFields: [],
    error: "INDEPENDENT-TASK-UPDATE-CAUSE",
  });
  const tool = publishedTool([...start, ...result], session.sessionId, "failed-task-update");
  assert.equal(tool.status, "failed");
  assert.ok(JSON.stringify(tool.content).includes("INDEPENDENT-TASK-UPDATE-CAUSE"));
  assert.deepEqual(taskStateEvents([...start, ...result], session.sessionId), []);
  assert.deepEqual(session.tasksById.get("selected-task"), target);
  assert.deepEqual(session.tasksById.get("sibling-task"), sibling);
  const retryStart = taskOperationStart(session, "retry-task-update", "TaskUpdate", {
    taskId: "selected-task",
    subject: "EXPLICIT-RETRY-SUBJECT",
    status: "completed",
  });
  const retry = taskOperationResult(session, "retry-task-update", {
    success: true,
    taskId: "selected-task",
    updatedFields: ["subject", "status"],
  });
  assert.deepEqual(taskStateEvents(retry, session.sessionId), [
    {
      type: "task_state_update",
      source: "task_update",
      tasks: [{ ...target, subject: "EXPLICIT-RETRY-SUBJECT", status: "completed" }],
      removed_task_ids: [],
      is_complete_snapshot: false,
    },
  ]);
  assert.equal(
    publishedTool([...retryStart, ...retry], session.sessionId, "retry-task-update").status,
    "completed",
  );
  assert.deepEqual(session.tasksById.get("sibling-task"), sibling);
});

test("authoritative populated and empty TaskList snapshots replace plan fields while preserving active and completed runtime tasks", () => {
  for (const populated of [true, false]) {
    const { session, target } = taskInventoryFixture();
    const runtime = captureBridgeEvents(() => {
      emitToolCall(session, "active-runtime-tool", "Monitor", {
        description: "Active runtime monitor",
        timeout_ms: 30000,
        persistent: false,
        command: "tail runtime.log",
      });
      emitToolResultUpdate(session, "active-runtime-tool", false, {
        taskId: "active-runtime-task",
        timeoutMs: 30000,
        persistent: false,
      });
      emitToolCall(session, "completed-runtime-tool", "Agent", {
        prompt: "Completed runtime agent",
      });
      handleTaskSystemMessage(session, "task_started", {
        task_id: "completed-runtime-task",
        tool_use_id: "completed-runtime-tool",
        description: "Completed runtime agent",
      });
      handleTaskSystemMessage(session, "task_notification", {
        task_id: "completed-runtime-task",
        tool_use_id: "completed-runtime-tool",
        status: "completed",
        summary: "ACTUAL-RUNTIME-OUTCOME",
      });
    });
    assert.ok(
      publishedTaskRows(runtime, session.sessionId, "active-runtime-task").some(
        (row) => row.status === "in_progress",
      ),
    );
    assert.ok(
      publishedTaskRows(runtime, session.sessionId, "completed-runtime-task").some(
        (row) => row.status === "completed",
      ),
    );
    const before = structuredClone([...session.tasksById.values()]);
    const invalidStart = taskOperationStart(session, "unrecognized-task-list", "TaskList", {});
    const invalid = taskOperationResult(session, "unrecognized-task-list", { unrecognized: [] });
    assert.deepEqual(taskStateEvents([...invalidStart, ...invalid], session.sessionId), []);
    assert.deepEqual([...session.tasksById.values()], before);
    const start = taskOperationStart(session, "authoritative-task-list", "TaskList", {});
    const result = taskOperationResult(session, "authoritative-task-list", {
      tasks: populated
        ? [
            {
              id: "selected-task",
              subject: "NATIVE-LISTED-SUBJECT",
              status: "completed",
              owner: "native-listed-owner",
              blockedBy: ["native-prerequisite"],
            },
          ]
        : [],
    });
    const updates = taskStateEvents(result, session.sessionId);
    assert.equal(updates.length, 1);
    const snapshot = updates[0];
    assert.ok(snapshot);
    assert.equal(snapshot.source, "task_list");
    assert.equal(snapshot.is_complete_snapshot, true);
    assert.deepEqual(
      new Set(snapshot.removed_task_ids),
      new Set(populated ? ["sibling-task"] : ["selected-task", "sibling-task"]),
    );
    const rows = new Map(snapshot.tasks.map((task) => [task.task_id, task]));
    assert.deepEqual(
      new Set(rows.keys()),
      new Set(
        populated
          ? ["selected-task", "active-runtime-task", "completed-runtime-task"]
          : ["active-runtime-task", "completed-runtime-task"],
      ),
    );
    if (populated)
      assert.deepEqual(rows.get("selected-task"), {
        ...target,
        subject: "NATIVE-LISTED-SUBJECT",
        status: "completed",
        owner: "native-listed-owner",
        blocked_by: ["native-prerequisite"],
      });
    const active = rows.get("active-runtime-task"),
      completed = rows.get("completed-runtime-task");
    assert.ok(active);
    assert.ok(completed);
    assert.equal(active.status, "in_progress");
    assert.equal(active.source_tool_call_id, "active-runtime-tool");
    assert.equal(completed.status, "completed");
    assert.equal(completed.source_tool_call_id, "completed-runtime-tool");
    assert.equal((completed.metadata as Record<string, unknown>).terminal_status, "completed");
    assert.equal((completed.metadata as Record<string, unknown>).summary, "ACTUAL-RUNTIME-OUTCOME");
    assert.equal(
      publishedTool([...start, ...result], session.sessionId, "authoritative-task-list").status,
      "completed",
    );
  }
});

test("TaskGet null confirms only the requested absence for present or missing targets and preserves subsequent sibling visibility", () => {
  for (const present of [true, false]) {
    const { session, sibling } = taskInventoryFixture(true, present);
    const start = taskOperationStart(session, "get-null-target", "TaskGet", {
      taskId: "selected-task",
    });
    const result = taskOperationResult(session, "get-null-target", { task: null });
    assert.deepEqual(taskStateEvents(result, session.sessionId), [
      {
        type: "task_state_update",
        source: "task_get",
        tasks: [],
        removed_task_ids: ["selected-task"],
        is_complete_snapshot: false,
      },
    ]);
    const tool = publishedTool([...start, ...result], session.sessionId, "get-null-target");
    assert.equal(tool.status, "completed");
    assert.match(JSON.stringify(tool.content), /not found|no task/i);
    assert.deepEqual(session.tasksById.get("sibling-task"), sibling);
    assert.equal(session.tasksById.has("selected-task"), false);
    taskOperationStart(session, "list-after-null", "TaskList", {});
    const listed = taskOperationResult(session, "list-after-null", {
      tasks: [
        {
          id: "sibling-task",
          subject: "Independent sibling subject",
          status: "in_progress",
          blockedBy: [],
        },
      ],
    });
    assert.deepEqual(taskStateEvents(listed, session.sessionId), [
      {
        type: "task_state_update",
        source: "task_list",
        tasks: [sibling],
        removed_task_ids: [],
        is_complete_snapshot: true,
      },
    ]);
  }
});

test("successful TaskGet native objects refresh existing or unknown identities and retain context absent from the response", () => {
  for (const known of [true, false]) {
    const { session, target, sibling } = taskInventoryFixture(true, known);
    const start = taskOperationStart(session, "get-native-object", "TaskGet", {
      taskId: "selected-task",
    });
    const result = taskOperationResult(session, "get-native-object", {
      task: {
        id: "selected-task",
        subject: "NATIVE-DETAIL-SUBJECT",
        description: "NATIVE-DETAIL-DESCRIPTION",
        status: "completed",
        blocks: ["native-block"],
        blockedBy: ["native-prerequisite"],
      },
    });
    const expected = {
      task_id: "selected-task",
      subject: "NATIVE-DETAIL-SUBJECT",
      description: "NATIVE-DETAIL-DESCRIPTION",
      status: "completed",
      blocks: ["native-block"],
      blocked_by: ["native-prerequisite"],
      ...(known
        ? {
            active_form: "Independent existing activity",
            owner: "original-owner",
            metadata: { preserved: 7, replaced: "before", deleted: "REMOVE-ME" },
            source_tool_call_id: "prior-source",
          }
        : {}),
    };
    assert.deepEqual(taskStateEvents(result, session.sessionId), [
      {
        type: "task_state_update",
        source: "task_get",
        tasks: [expected],
        removed_task_ids: [],
        is_complete_snapshot: false,
      },
    ]);
    assert.deepEqual(session.tasksById.get("sibling-task"), sibling);
    assert.equal(
      publishedTool([...start, ...result], session.sessionId, "get-native-object").status,
      "completed",
    );
    const content = JSON.stringify(
      publishedTool([...start, ...result], session.sessionId, "get-native-object").content,
    );
    assert.ok(content.includes("NATIVE-DETAIL-SUBJECT"));
    assert.ok(content.includes("NATIVE-DETAIL-DESCRIPTION"));
    assert.notEqual(target.subject, "NATIVE-DETAIL-SUBJECT");
    for (const invalid of [
      undefined,
      { unrecognized: true },
      { task: { id: "selected-task", subject: 7, status: "completed" } },
    ]) {
      const before = structuredClone([...session.tasksById.values()]);
      const id = `get-invalid-${invalid === undefined ? "absent" : invalid.unrecognized ? "unknown" : "malformed"}`;
      const invalidStart = taskOperationStart(session, id, "TaskGet", { taskId: "selected-task" });
      const invalidResult = taskOperationResult(session, id, invalid);
      assert.deepEqual(taskStateEvents([...invalidStart, ...invalidResult], session.sessionId), []);
      assert.deepEqual([...session.tasksById.values()], before);
    }
  }
});

test("handleTaskSystemMessage emits task state for unlinked task_updated messages", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_updated", {
      task_id: "task-missing",
      patch: {
        status: "running",
        description: "This should not be emitted",
      },
    });
  });

  assert.deepEqual(
    events.map((event) => event.update),
    [
      {
        type: "task_state_update",
        source: "task_lifecycle",
        tasks: [
          {
            task_id: "task-missing",
            subject: "This should not be emitted",
            description: "This should not be emitted",
            status: "in_progress",
            blocks: [],
            blocked_by: [],
            metadata: { sdk_lifecycle_task: true },
          },
        ],
        removed_task_ids: [],
        is_complete_snapshot: false,
      },
    ],
  );
});

test("handleTaskSystemMessage maps stopped notifications to terminal task state", () => {
  const session = makeSessionState();
  session.tasksById.set("task-1", {
    task_id: "task-1",
    subject: "Watch build",
    status: "in_progress",
    blocks: [],
    blocked_by: [],
  });
  session.taskOrder.push("task-1");

  const events = captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_notification", {
      task_id: "task-1",
      status: "stopped",
      reason: "worker_restart",
      output_file: "C:/tmp/task-1.txt",
      summary: "Stopped background watch",
    });
  });

  assert.deepEqual(
    events.map((event) => event.update),
    [
      {
        type: "task_state_update",
        source: "task_lifecycle",
        tasks: [
          {
            task_id: "task-1",
            subject: "Watch build",
            description: "Stopped background watch",
            status: "completed",
            blocks: [],
            blocked_by: [],
            metadata: {
              output_file: "C:/tmp/task-1.txt",
              sdk_lifecycle_task: true,
              summary: "Stopped background watch",
              terminal_status: "stopped",
              terminal_reason: "worker_restart",
            },
          },
        ],
        removed_task_ids: [],
        is_complete_snapshot: false,
      },
    ],
  );
});

async function runAuthorityFixture() {
  const { spawnSync } = await import("node:child_process");
  const { fileURLToPath } = await import("node:url");
  const child = spawnSync(
    process.execPath,
    [fileURLToPath(new URL("../../tests/fixtures/bridge-authority-runtime.mjs", import.meta.url))],
    { encoding: "utf8", windowsHide: true, timeout: 15000 },
  );
  assert.equal(child.error, undefined);
  assert.equal(child.status, 0, child.stderr);
  const report = JSON.parse(child.stdout) as {
    originalId: string;
    replacementId: string;
    phases: Record<
      string,
      { events: Array<Record<string, unknown>>; journal: Array<{ method: string; mode?: string }> }
    >;
    allQueriesClosed: boolean;
  };
  assert.equal(report.allQueriesClosed, true);
  return report;
}
test("native init publishes declared MCP server identities, sources and runtime capabilities after controlled refreshes settle", async () => {
  const report = await runAuthorityFixture();
  const phase = report.phases.same;
  assert.ok(phase);
  const snapshots = phase.events.filter((event) => event.event === "mcp_snapshot");
  assert.deepEqual(snapshots, [
    {
      event: "mcp_snapshot",
      session_id: report.originalId,
      source: "init",
      auth_capabilities: { authenticate: true, clear_auth: false, submit_oauth_callback_url: true },
      servers: [
        { name: "project-docs", status: "connected", source: "project", tools: [] },
        { name: "plugin-search", status: "pending", source: "plugin", tools: [] },
      ],
    },
  ]);
  for (const method of ["supportedCommands", "supportedAgents", "getSettings"])
    assert.ok(
      phase.journal.some((call) => call.method === method),
      method,
    );
});

test("a mode command acknowledges its own request before later same and different native init observations publish live authority", async () => {
  const report = await runAuthorityFixture();
  const command = report.phases.command;
  assert.ok(command);
  assert.deepEqual(
    command.journal.filter((call) => call.method === "setPermissionMode"),
    [{ method: "setPermissionMode", mode: "plan" }],
  );
  const commandModes = publishedModes(command.events);
  assert.equal(commandModes.length, 1);
  assert.equal(commandModes[0]?.session_id, report.originalId);
  assert.equal(commandModes[0]?.mode.current_mode_id, "plan");
  assert.equal(
    command.events.find(
      (event) => (event.update as { type?: string } | undefined)?.type === "mode_state_update",
    )?.request_id,
    "authority-mode-command",
  );
  for (const row of [
    { name: "same", mode: "plan", label: "Plan" },
    { name: "different", mode: "acceptEdits", label: "Accept Edits" },
  ]) {
    const phase = report.phases[row.name];
    assert.ok(phase);
    const modes = publishedModes(phase.events);
    assert.equal(modes.length, 1);
    const observed = modes[0];
    assert.ok(observed);
    assert.equal(observed.session_id, report.originalId);
    assert.equal(observed.mode.current_mode_id, row.mode);
    assert.ok(
      observed.mode.available_modes.some((mode) => mode.id === row.mode && mode.name === row.label),
    );
    assert.ok(
      phase.events
        .filter(
          (event) => (event.update as { type?: string } | undefined)?.type === "mode_state_update",
        )
        .every((event) => event.request_id === undefined),
    );
    assert.equal(
      phase.events.some(
        (event) => event.event === "connected" || event.event === "session_replaced",
      ),
      false,
    );
  }
});

test("real initial and replacement authority publish initialized fast state before same-session changes become scoped deltas", async () => {
  const report = await runAuthorityFixture();
  const initial = report.phases.initial,
    replacement = report.phases.replacement;
  assert.ok(initial);
  assert.ok(replacement);
  const connected = initial.events.filter((event) => event.event === "connected");
  assert.equal(connected.length, 1);
  assert.equal(connected[0]?.session_id, report.originalId);
  assert.equal(connected[0]?.request_id, "initial-authority-request");
  assert.equal(connected[0]?.fast_mode_state, "off");
  assert.equal(connected[0]?.fast_mode_disabled_reason, "model_not_allowed");
  const replaced = replacement.events.filter((event) => event.event === "session_replaced");
  assert.equal(replaced.length, 1);
  assert.equal(replaced[0]?.session_id, report.replacementId);
  assert.equal(replaced[0]?.fast_mode_state, "cooldown");
  assert.equal(replaced[0]?.fast_mode_disabled_reason, "network_error");
  assert.equal(
    replacement.events.some((event) => event.event === "connected"),
    false,
  );
  for (const row of [
    { name: "same", session: report.originalId, state: "on", reason: undefined },
    { name: "live", session: report.replacementId, state: "off", reason: "future-native-reason" },
  ]) {
    const phase = report.phases[row.name];
    assert.ok(phase);
    assert.deepEqual(fastPublications(phase.events), [
      {
        event: "session_update",
        session_id: row.session,
        update: {
          type: "fast_mode_update",
          fast_mode_state: row.state,
          ...(row.reason ? { fast_mode_disabled_reason: row.reason } : {}),
        },
      },
    ]);
    assert.equal(
      phase.events.some(
        (event) => event.event === "connected" || event.event === "session_replaced",
      ),
      false,
    );
  }
  assert.deepEqual(fastPublications(report.phases.different?.events ?? []), []);
});

function fastPublications(events: Array<Record<string, unknown>>) {
  return events.filter(
    (event) => (event.update as { type?: string } | undefined)?.type === "fast_mode_update",
  );
}
function observeFast(session: SessionState, fields: Record<string, unknown>) {
  return captureBridgeEvents(() =>
    handleSdkMessage(session, {
      type: "system",
      subtype: "status",
      uuid: "fast-observation-source",
      session_id: session.sessionId,
      ...fields,
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
  );
}
test("fast mode observation deduplicates each session's state and reason pair while preserving future reasons and explicit clearing", () => {
  const session = makeSessionState();
  session.sessionId = "fast-pair-owner";
  for (const row of [
    { state: "off", reason: "model_not_allowed", changed: true },
    { state: "off", reason: "model_not_allowed", changed: false },
    { state: "off", reason: "network_error", changed: true },
    { state: "cooldown", reason: "network_error", changed: true },
    { state: "on", reason: undefined, changed: true },
    { state: "off", reason: "future-native-reason", changed: true },
    { state: "off", reason: undefined, changed: true },
    { state: "off", reason: undefined, changed: false },
  ]) {
    const events = observeFast(session, {
      fast_mode_state: row.state,
      ...(row.reason === undefined ? {} : { fast_mode_disabled_reason: row.reason }),
    });
    assert.deepEqual(
      fastPublications(events),
      row.changed
        ? [
            {
              event: "session_update",
              session_id: session.sessionId,
              update: {
                type: "fast_mode_update",
                fast_mode_state: row.state,
                ...(row.reason ? { fast_mode_disabled_reason: row.reason } : {}),
              },
            },
          ]
        : [],
    );
  }
  assert.deepEqual(
    fastPublications(
      observeFast(session, {
        fast_mode_state: "off",
        fast_mode_disabled_reason: "model_not_allowed",
      }),
    ),
    [
      {
        event: "session_update",
        session_id: session.sessionId,
        update: {
          type: "fast_mode_update",
          fast_mode_state: "off",
          fast_mode_disabled_reason: "model_not_allowed",
        },
      },
    ],
  );
  for (const invalid of [undefined, null, "unknown", false, 0, {}]) {
    assert.deepEqual(fastPublications(observeFast(session, { fast_mode_state: invalid })), []);
    assert.equal(session.fastModeState, "off");
    assert.equal(session.fastModeDisabledReason, "model_not_allowed");
  }
  const peer = makeSessionState();
  peer.sessionId = "fast-pair-peer";
  assert.deepEqual(
    fastPublications(
      observeFast(peer, { fast_mode_state: "off", fast_mode_disabled_reason: "model_not_allowed" }),
    ),
    [
      {
        event: "session_update",
        session_id: peer.sessionId,
        update: {
          type: "fast_mode_update",
          fast_mode_state: "off",
          fast_mode_disabled_reason: "model_not_allowed",
        },
      },
    ],
  );
  assert.equal(session.fastModeDisabledReason, "model_not_allowed");
});

test("generic SDK status preserves observed fast knowledge while an explicit fast snapshot still clears its reason", () => {
  for (const row of [
    {
      status: "compacting",
      permissionMode: undefined,
      compact_result: undefined,
      expected: { type: "compaction_update", phase: "started" },
    },
    {
      status: "requesting",
      permissionMode: "plan",
      compact_result: undefined,
      expected: { type: "session_status_update", status: "requesting" },
    },
    {
      status: null,
      permissionMode: undefined,
      compact_result: "success",
      expected: { type: "compaction_update", phase: "finished", result: "success" },
    },
  ]) {
    const session = makeSessionState();
    session.sessionId = `generic-status-${String(row.status)}`;
    assert.deepEqual(
      fastPublications(
        observeFast(session, {
          fast_mode_state: "off",
          fast_mode_disabled_reason: "INDEPENDENT-DISABLED-REASON",
        }),
      ),
      [
        {
          event: "session_update",
          session_id: session.sessionId,
          update: {
            type: "fast_mode_update",
            fast_mode_state: "off",
            fast_mode_disabled_reason: "INDEPENDENT-DISABLED-REASON",
          },
        },
      ],
    );
    const generic = observeFast(session, {
      status: row.status,
      ...(row.permissionMode ? { permissionMode: row.permissionMode } : {}),
      ...(row.compact_result ? { compact_result: row.compact_result } : {}),
    });
    assert.deepEqual(fastPublications(generic), []);
    assert.ok(
      generic.some(
        (event) =>
          event.session_id === session.sessionId &&
          JSON.stringify(event.update) === JSON.stringify(row.expected),
      ),
    );
    if (row.permissionMode)
      assert.deepEqual(
        publishedModes(generic).map((event) => ({
          session_id: event.session_id,
          mode: event.mode.current_mode_id,
        })),
        [{ session_id: session.sessionId, mode: "plan" }],
      );
    assert.equal(session.fastModeState, "off");
    assert.equal(session.fastModeDisabledReason, "INDEPENDENT-DISABLED-REASON");
    assert.deepEqual(fastPublications(observeFast(session, { fast_mode_state: "off" })), [
      {
        event: "session_update",
        session_id: session.sessionId,
        update: { type: "fast_mode_update", fast_mode_state: "off" },
      },
    ]);
    assert.equal(session.fastModeDisabledReason, undefined);
    assert.deepEqual(fastPublications(observeFast(session, { fast_mode_state: "off" })), []);
  }
});

test("advisory progress updates only admitted direct or parent tools and preserves detached and every terminal outcome", () => {
  const session = makeSessionState();
  session.sessionId = "advisory-progress-owner";
  const initial = captureBridgeEvents(() => {
    emitToolCall(session, "live-progress-target", "PowerShell", {
      command: "Get-Content owned.txt",
    });
    emitToolCall(session, "live-progress-peer", "Read", { file_path: "peer.txt" });
    emitToolCall(session, "detached-progress-target", "Monitor", {
      description: "Monitor job",
      timeout_ms: 30000,
      persistent: false,
      command: "tail owned.log",
    });
    emitToolResultUpdate(session, "detached-progress-target", false, {
      taskId: "detached-task",
      timeoutMs: 30000,
      persistent: false,
    });
  });
  let all = [...initial];
  for (const row of [
    { id: "heartbeat-first", parent: "live-progress-target", seconds: 1, ms: 1000 },
    { id: "heartbeat-second", parent: "live-progress-target", seconds: 2.25, ms: 2250 },
    { id: "live-progress-target", parent: undefined, seconds: 4, ms: 4000 },
  ]) {
    const events = captureBridgeEvents(() =>
      handleSdkMessage(session, {
        type: "tool_progress",
        tool_use_id: row.id,
        tool_name: "PowerShell",
        parent_tool_use_id: row.parent,
        task_id: "unknown-task-link",
        elapsed_time_seconds: row.seconds,
        heartbeat: true,
        uuid: `${row.id}-frame`,
        session_id: session.sessionId,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    all = [...all, ...events];
    const frames = publishedToolFrames(events, session.sessionId, "live-progress-target");
    assert.equal(frames.length, 1);
    assert.deepEqual(frames[0]?.output_metadata, {
      timing: { duration_ms: row.ms, source: "progress" },
    });
    assert.equal(
      events.some((event) => (event.update as { type?: string } | undefined)?.type === "tool_call"),
      false,
    );
    assert.deepEqual(publishedToolFrames(events, session.sessionId, "live-progress-peer"), []);
    assert.equal(
      publishedTool(all, session.sessionId, "live-progress-target").status,
      "in_progress",
    );
  }
  const orphan = captureBridgeEvents(() => {
    emitToolProgressUpdate(session, "unknown-direct", {
      elapsedSeconds: 8,
      subagentType: "must-not-attach",
      subagentRetry: { state: "waiting", attempt: 1, max_retries: 2, retry_delay_ms: 3 },
    });
    handleSdkMessage(session, {
      type: "tool_progress",
      tool_use_id: "orphan-heartbeat",
      parent_tool_use_id: "missing-parent",
      elapsed_time_seconds: 9,
      tool_name: "PowerShell",
      uuid: "orphan-frame",
      session_id: session.sessionId,
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });
  assert.deepEqual(orphan, []);
  const detached = captureBridgeEvents(() =>
    handleSdkMessage(session, {
      type: "tool_progress",
      tool_use_id: "detached-progress-target",
      elapsed_time_seconds: 3.125,
      tool_name: "Monitor",
      uuid: "detached-progress-frame",
      session_id: session.sessionId,
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
  );
  assert.equal(
    publishedTool([...initial, ...detached], session.sessionId, "detached-progress-target").status,
    "detached",
  );
  assert.deepEqual(
    publishedToolFrames(detached, session.sessionId, "detached-progress-target")[0]
      ?.output_metadata,
    { timing: { duration_ms: 3125, source: "progress" } },
  );
  for (const row of [
    { name: "completed", error: false, interrupted: false, status: "completed" },
    { name: "failed", error: true, interrupted: false, status: "failed" },
    { name: "killed", error: false, interrupted: true, status: "killed" },
  ]) {
    const id = `terminal-progress-${row.name}`;
    const setup = captureBridgeEvents(() => {
      emitToolCall(session, id, "Bash", { command: "printf terminal" });
      emitToolProgressUpdate(session, id, {
        elapsedSeconds: 0.777,
        subagentType: "terminal-subtype",
        subagentRetry: { state: "waiting", attempt: 1, max_retries: 2, retry_delay_ms: 300 },
      });
      if (row.name === "killed") {
        handleTaskSystemMessage(session, "task_started", {
          task_id: "terminal-killed-task",
          tool_use_id: id,
        });
        handleTaskSystemMessage(session, "task_notification", {
          task_id: "terminal-killed-task",
          tool_use_id: id,
          status: "stopped",
          summary: "ACTUAL-killed-OUTPUT",
        });
      } else
        emitToolResultUpdate(session, id, row.error, {
          stdout: `ACTUAL-${row.name}-OUTPUT`,
          stderr: "",
          interrupted: false,
        });
    });
    const before = publishedTool(setup, session.sessionId, id);
    assert.equal(before.status, row.status);
    assert.deepEqual(before.output_metadata, { timing: { duration_ms: 777, source: "progress" } });
    assert.equal(
      (before.task_metadata as Record<string, unknown>).subagent_type,
      "terminal-subtype",
    );
    assert.equal((before.task_metadata as Record<string, unknown>).subagent_retry, undefined);
    const late = captureBridgeEvents(() => {
      emitToolProgressUpdate(session, id, {
        elapsedSeconds: 99,
        subagentType: "late-type",
        subagentRetry: { state: "waiting", attempt: 4, max_retries: 5, retry_delay_ms: 700 },
      });
      handleSdkMessage(session, {
        type: "tool_progress",
        tool_use_id: id,
        tool_name: "Bash",
        elapsed_time_seconds: 100,
        subagent_retry: { attempt: 4, max_retries: 5, retry_delay_ms: 700 },
        uuid: "late-terminal-progress",
        session_id: session.sessionId,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    });
    assert.deepEqual(late, []);
    assert.deepEqual(publishedTool([...setup, ...late], session.sessionId, id), before);
  }
  assert.deepEqual(
    publishedTool(all, session.sessionId, "live-progress-peer"),
    publishedTool(initial, session.sessionId, "live-progress-peer"),
  );
});

test("task-addressed native progress routes to each independently linked execution without a visible heartbeat or parent fallback", () => {
  const session = makeSessionState();
  session.sessionId = "task-addressed-progress-owner";
  const initial = captureBridgeEvents(() => {
    for (const row of [
      { id: "task-progress-first", task: "native-first-task" },
      { id: "task-progress-second", task: "native-second-task" },
    ]) {
      emitToolCall(session, row.id, "Agent", { prompt: `Independent ${row.id}` });
      handleTaskSystemMessage(session, "task_started", {
        task_id: row.task,
        tool_use_id: row.id,
        blocked: false,
        parent_agent_id: "retained-parent-agent",
      });
    }
  });
  let all = [...initial];
  for (const row of [
    {
      id: "task-progress-first",
      task: "native-first-task",
      heartbeat: "first-unseen-heartbeat",
      seconds: 1.75,
      ms: 1750,
      type: "planner",
    },
    {
      id: "task-progress-second",
      task: "native-second-task",
      heartbeat: "second-unseen-heartbeat",
      seconds: 2.5,
      ms: 2500,
      type: "reviewer",
    },
  ]) {
    const events = captureBridgeEvents(() =>
      handleSdkMessage(session, {
        type: "tool_progress",
        tool_use_id: row.heartbeat,
        tool_name: "Agent",
        task_id: row.task,
        elapsed_time_seconds: row.seconds,
        subagent_type: row.type,
        subagent_retry: {
          agent_id: "progress-worker",
          attempt: 2,
          max_retries: 4,
          retry_delay_ms: 1500,
        },
        heartbeat: true,
        uuid: `${row.heartbeat}-source`,
        session_id: session.sessionId,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    all = [...all, ...events];
    assert.equal(
      events.some((event) => (event.update as { type?: string } | undefined)?.type === "tool_call"),
      false,
    );
    const frames = publishedToolFrames(events, session.sessionId, row.id);
    assert.equal(frames.length, 1);
    assert.deepEqual(frames[0]?.output_metadata, {
      timing: { duration_ms: row.ms, source: "progress" },
    });
    assert.deepEqual(frames[0]?.task_metadata, {
      blocked: false,
      parent_agent_id: "retained-parent-agent",
      subagent_type: row.type,
      subagent_retry: {
        state: "waiting",
        agent_id: "progress-worker",
        attempt: 2,
        max_retries: 4,
        retry_delay_ms: 1500,
      },
    });
    assert.deepEqual(
      publishedToolFrames(
        events,
        session.sessionId,
        row.id === "task-progress-first" ? "task-progress-second" : "task-progress-first",
      ),
      [],
    );
    assert.equal(publishedTool(all, session.sessionId, row.id).status, "in_progress");
  }
});

test("retry progress preserves complete target metadata, rejects independently malformed fields, clears on absence and settles the real result", () => {
  const session = makeSessionState();
  session.sessionId = "retry-progress-owner";
  const retained = {
    blocked: false,
    parent_agent_id: "retained-retry-parent",
    request_id: "retained-retry-request",
  };
  const initial = captureBridgeEvents(() => {
    emitToolCall(session, "retry-target", "Agent", { prompt: "Independent retry work" });
    emitToolCall(session, "retry-peer", "Read", { file_path: "peer.txt" });
    handleTaskSystemMessage(session, "task_started", {
      task_id: "retry-task",
      tool_use_id: "retry-target",
      ...retained,
    });
  });
  let all = [...initial];
  const native = {
    agent_id: "independent-retry-worker",
    attempt: 2,
    max_retries: 4,
    retry_delay_ms: 1500,
    error_status: 429,
    error_category: "rate_limit",
  };
  const expected = {
    state: "waiting",
    agent_id: "independent-retry-worker",
    attempt: 2,
    max_retries: 4,
    retry_delay_ms: 1500,
    error_status: 429,
    error_category: "rate_limit",
  };
  const send = (value: unknown, present: boolean, seconds: number) => {
    const events = captureBridgeEvents(() =>
      handleSdkMessage(session, {
        type: "tool_progress",
        tool_use_id: "retry-target",
        tool_name: "Agent",
        parent_tool_use_id: null,
        elapsed_time_seconds: seconds,
        heartbeat: true,
        subagent_type: "reviewer",
        ...(present ? { subagent_retry: value } : {}),
        uuid: `retry-frame-${seconds}`,
        session_id: session.sessionId,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    all = [...all, ...events];
    return events;
  };
  const waiting = send(native, true, 5);
  const waitingFrames = publishedToolFrames(waiting, session.sessionId, "retry-target");
  assert.equal(waitingFrames.length, 1);
  assert.deepEqual(waitingFrames[0]?.task_metadata, {
    ...retained,
    subagent_type: "reviewer",
    subagent_retry: expected,
  });
  assert.deepEqual(waitingFrames[0]?.output_metadata, {
    timing: { duration_ms: 5000, source: "progress" },
  });
  for (const invalid of [
    { ...native, attempt: -1 },
    { ...native, attempt: 1.5 },
    { ...native, attempt: undefined },
    { ...native, max_retries: -1 },
    { ...native, max_retries: "4" },
    { ...native, max_retries: undefined },
    { ...native, retry_delay_ms: -1 },
    { ...native, retry_delay_ms: 1.5 },
    { ...native, retry_delay_ms: undefined },
  ]) {
    const events = send(invalid, true, 6);
    const frames = publishedToolFrames(events, session.sessionId, "retry-target");
    assert.equal(frames.length, 1);
    assert.deepEqual(frames[0]?.output_metadata, {
      timing: { duration_ms: 6000, source: "progress" },
    });
    if (frames[0]?.task_metadata)
      assert.deepEqual(frames[0].task_metadata, {
        ...retained,
        subagent_type: "reviewer",
        subagent_retry: expected,
      });
    assert.deepEqual(publishedTool(all, session.sessionId, "retry-target").task_metadata, {
      ...retained,
      subagent_type: "reviewer",
      subagent_retry: expected,
    });
  }
  const clear = send(undefined, false, 7);
  const clearFrames = publishedToolFrames(clear, session.sessionId, "retry-target");
  assert.equal(clearFrames.length, 1);
  assert.deepEqual(clearFrames[0]?.task_metadata, { ...retained, subagent_type: "reviewer" });
  assert.deepEqual(clearFrames[0]?.output_metadata, {
    timing: { duration_ms: 7000, source: "progress" },
  });
  const zero = send(
    {
      agent_id: "boundary-worker",
      attempt: 0,
      max_retries: 0,
      retry_delay_ms: 0,
      error_status: null,
    },
    true,
    8,
  );
  assert.deepEqual(publishedToolFrames(zero, session.sessionId, "retry-target")[0]?.task_metadata, {
    ...retained,
    subagent_type: "reviewer",
    subagent_retry: {
      state: "waiting",
      agent_id: "boundary-worker",
      attempt: 0,
      max_retries: 0,
      retry_delay_ms: 0,
    },
  });
  const result = taskOperationResult(
    session,
    "retry-target",
    {
      agentId: "boundary-worker",
      status: "completed",
      content: [{ type: "text", text: "ACTUAL-RETRY-RESULT\n exact  " }],
      resolvedModel: "claude-opus-4-8",
    },
    "ACTUAL-RETRY-RESULT\n exact  ",
  );
  all = [...all, ...result];
  const finished = publishedTool(all, session.sessionId, "retry-target");
  assert.equal(finished.status, "completed");
  assert.equal(finished.source_message_uuid, "retry-target-result-source");
  assert.deepEqual(finished.content, [
    { type: "content", content: { type: "text", text: "ACTUAL-RETRY-RESULT\n exact  " } },
  ]);
  assert.deepEqual(finished.task_metadata, { ...retained, subagent_type: "reviewer" });
  for (const frame of publishedToolFrames(result, session.sessionId, "retry-target"))
    if (frame.task_metadata)
      assert.deepEqual(frame.task_metadata, { ...retained, subagent_type: "reviewer" });
  const late = send(native, true, 99);
  assert.deepEqual(late, []);
  assert.deepEqual(publishedTool(all, session.sessionId, "retry-target"), finished);
  assert.deepEqual(
    publishedTool(all, session.sessionId, "retry-peer"),
    publishedTool(initial, session.sessionId, "retry-peer"),
  );
});

type QuestionWire = {
  event: string;
  session_id: string;
  request: {
    tool_call: { tool_call_id: string };
    prompt: {
      question: string;
      header: string;
      multi_select: boolean;
      options: Array<{ option_id: string; label: string; description: string; preview?: string }>;
    };
    question_index: number;
    total_questions: number;
    idle_timeout_ms?: number;
  };
};
function bridgeEventInbox() {
  const buffered: Array<Record<string, unknown>> = [];
  const waiters: Array<{
    matches: (event: Record<string, unknown>) => boolean;
    resolve: (event: Record<string, unknown>) => void;
    reject: (error: Error) => void;
  }> = [];
  return {
    observe(event: Record<string, unknown>) {
      const index = waiters.findIndex((waiter) => waiter.matches(event));
      if (index < 0) {
        buffered.push(event);
        return;
      }
      const waiter = waiters.splice(index, 1)[0];
      assert.ok(waiter);
      waiter.resolve(event);
    },
    next(matches: (event: Record<string, unknown>) => boolean) {
      const index = buffered.findIndex(matches);
      if (index >= 0) {
        const event = buffered.splice(index, 1)[0];
        assert.ok(event);
        return Promise.resolve(event);
      }
      return new Promise<Record<string, unknown>>((resolve, reject) =>
        waiters.push({ matches, resolve, reject }),
      );
    },
    dispose() {
      for (const waiter of waiters.splice(0)) waiter.reject(new Error("Owned event inbox closed"));
    },
  };
}
function ownedQuestion(
  session: SessionState,
  id: string,
  input: Record<string, unknown>,
  signal: AbortSignal,
) {
  emitToolCall(session, id, "AskUserQuestion", input);
  const base = session.toolCalls.get(id);
  assert.ok(base);
  return requestAskUserQuestionAnswers(session, id, input, base, signal);
}
async function nextQuestion(
  inbox: ReturnType<typeof bridgeEventInbox>,
  session: SessionState,
  id: string,
  index: number,
): Promise<QuestionWire> {
  return (await boundedBridgeTask(
    inbox.next((event) => {
      const request = event.request as QuestionWire["request"] | undefined;
      return (
        event.event === "question_request" &&
        event.session_id === session.sessionId &&
        request?.tool_call.tool_call_id === id &&
        request.question_index === index
      );
    }),
  )) as unknown as QuestionWire;
}
function answerQuestion(
  sessionId: string,
  id: string,
  selected: string[],
  annotation?: { preview?: string; notes?: string },
) {
  const request = `answer-${sessionId}-${id}`;
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      command: "question_response",
      request_id: request,
      session_id: sessionId,
      tool_call_id: id,
      outcome: {
        outcome: "answered",
        selected_option_ids: selected,
        ...(annotation ? { annotation } : {}),
      },
    }),
  );
  assert.equal(parsed.requestId, request);
  assert.equal(parsed.command.command, "question_response");
  if (parsed.command.command !== "question_response")
    throw new Error("Expected a question response");
  handleInteractionCommand(parsed.command);
}
const questionPairInput = {
  questions: [
    {
      question: "Choose target",
      header: "Target",
      multiSelect: false,
      options: [
        { label: "Staging", description: "Validation target", preview: "Staging preview." },
        {
          label: "Production",
          description: "Customer-facing target",
          preview: "Production preview.",
        },
      ],
    },
    {
      question: "Choose region",
      header: "Region",
      multiSelect: false,
      options: [
        { label: "West", description: "Western region" },
        { label: "East", description: "Eastern region" },
      ],
    },
  ],
};
const peerQuestionInput = {
  questions: [
    {
      question: "Choose peer route",
      header: "Peer",
      multiSelect: false,
      options: [
        { label: "Peer North", description: "Northern route" },
        { label: "Peer South", description: "Southern route" },
      ],
    },
  ],
};
test("question responses preserve sequential selection, skip and preview ownership through parsed dispatch and trusted timing", async () => {
  for (const row of [
    {
      name: "untrusted-project-local",
      sources: [
        { source: "projectSettings", settings: { askUserQuestionTimeout: "60s" } },
        { source: "localSettings", settings: { askUserQuestionTimeout: "10m" } },
      ],
      timeout: undefined,
    },
    {
      name: "user-five-minutes",
      sources: [
        { source: "projectSettings", settings: { askUserQuestionTimeout: "never" } },
        { source: "userSettings", settings: { askUserQuestionTimeout: "5m" } },
      ],
      timeout: 300000,
    },
    {
      name: "policy-never",
      sources: [
        { source: "userSettings", settings: { askUserQuestionTimeout: "10m" } },
        { source: "flagSettings", settings: { askUserQuestionTimeout: "60s" } },
        { source: "policySettings", settings: { askUserQuestionTimeout: "never" } },
      ],
      timeout: undefined,
    },
    {
      name: "policy-before-flags",
      sources: [
        { source: "flagSettings", settings: { askUserQuestionTimeout: "5m" } },
        { source: "userSettings", settings: { askUserQuestionTimeout: "10m" } },
        { source: "policySettings", settings: { askUserQuestionTimeout: "60s" } },
      ],
      timeout: 60000,
    },
    {
      name: "flags-before-user",
      sources: [
        { source: "userSettings", settings: { askUserQuestionTimeout: "60s" } },
        { source: "flagSettings", settings: { askUserQuestionTimeout: "5m" } },
      ],
      timeout: 300000,
    },
    {
      name: "user-ten-minutes",
      sources: [{ source: "userSettings", settings: { askUserQuestionTimeout: "10m" } }],
      timeout: 600000,
    },
    { name: "unsupported-read", sources: undefined, timeout: undefined },
  ]) {
    await withCommandTargets(async (target, peer) => {
      const native = {
        close: () => {},
        getSettings: async function () {
          assert.equal(this, native);
          return { sources: row.sources };
        },
      };
      target.query = (row.sources === undefined
        ? { close: () => {} }
        : native) as unknown as SessionState["query"];
      peer.query = {
        close: () => {},
        getSettings: async () => ({ sources: [] }),
      } as unknown as SessionState["query"];
      const inbox = bridgeEventInbox(),
        controller = new AbortController(),
        peerController = new AbortController();
      const operations: Array<ReturnType<typeof requestAskUserQuestionAnswers>> = [];
      let primaryResult: Awaited<ReturnType<typeof requestAskUserQuestionAnswers>> | undefined;
      let peerResult: typeof primaryResult;
      let multiResult: typeof primaryResult;
      const input = structuredClone(questionPairInput),
        originalInput = structuredClone(questionPairInput);
      const multiInput = {
        questions: [
          {
            question: "Choose both environments",
            header: "Environments",
            multiSelect: true,
            options: [
              {
                label: "Staging",
                description: "Validation environment",
                preview: "STAGING PREVIEW\nLINE",
              },
              {
                label: "Production",
                description: "Customer environment",
                preview: "PRODUCTION PREVIEW",
              },
            ],
          },
        ],
      };
      const events = await captureBridgeEventsAsync(
        async () => {
          try {
            const primary = ownedQuestion(target, "sequential-question", input, controller.signal),
              sibling = ownedQuestion(
                peer,
                "peer-question",
                structuredClone(peerQuestionInput),
                peerController.signal,
              );
            operations.push(primary, sibling);
            const first = await nextQuestion(inbox, target, "sequential-question", 0),
              peerRequest = await nextQuestion(inbox, peer, "peer-question", 0);
            assert.equal(first.request.prompt.question, "Choose target");
            assert.equal(first.request.prompt.header, "Target");
            assert.deepEqual(
              first.request.prompt.options.map((option) => [
                option.option_id,
                option.label,
                option.preview,
              ]),
              [
                ["question_0", "Staging", "Staging preview."],
                ["question_1", "Production", "Production preview."],
              ],
            );
            assert.equal(first.request.total_questions, 2);
            assert.equal(first.request.idle_timeout_ms, row.timeout, row.name);
            assert.equal(peerRequest.request.prompt.question, "Choose peer route");
            assert.equal(peerRequest.request.idle_timeout_ms, undefined);
            answerQuestion("unknown-question-session", "sequential-question", ["question_0"]);
            answerQuestion(target.sessionId, "unknown-question-tool", ["question_0"]);
            answerQuestion(target.sessionId, "sequential-question", ["question_1"], {
              preview: "EXPLICIT-PREVIEW\n exact  ",
              notes: "Use production",
            });
            const second = await nextQuestion(inbox, target, "sequential-question", 1);
            assert.equal(second.request.prompt.question, "Choose region");
            assert.equal(second.request.idle_timeout_ms, row.timeout);
            assert.equal(second.request.total_questions, 2);
            answerQuestion(target.sessionId, "sequential-question", []);
            primaryResult = await boundedBridgeTask(primary);
            answerQuestion(peer.sessionId, "peer-question", ["question_0"]);
            peerResult = await boundedBridgeTask(sibling);
            const multiple = ownedQuestion(target, "multi-question", multiInput, controller.signal);
            operations.push(multiple);
            const request = await nextQuestion(inbox, target, "multi-question", 0);
            assert.equal(request.request.prompt.multi_select, true);
            assert.deepEqual(
              request.request.prompt.options.map((option) => [
                option.option_id,
                option.label,
                option.preview,
              ]),
              [
                ["question_0", "Staging", "STAGING PREVIEW\nLINE"],
                ["question_1", "Production", "PRODUCTION PREVIEW"],
              ],
            );
            answerQuestion(target.sessionId, "multi-question", ["question_0", "question_1"], {
              notes: "Both environments",
            });
            multiResult = await boundedBridgeTask(multiple);
          } finally {
            controller.abort();
            peerController.abort();
            await Promise.allSettled(operations);
            inbox.dispose();
          }
        },
        (event) => inbox.observe(event),
      );
      assert.deepEqual(primaryResult, {
        behavior: "allow",
        toolUseID: "sequential-question",
        updatedInput: {
          ...originalInput,
          answers: { "Choose target": "Production" },
          annotations: {
            "Choose target": { preview: "EXPLICIT-PREVIEW\n exact  ", notes: "Use production" },
          },
        },
      });
      assert.deepEqual(input, originalInput);
      assert.deepEqual(peerResult, {
        behavior: "allow",
        toolUseID: "peer-question",
        updatedInput: { ...peerQuestionInput, answers: { "Choose peer route": "Peer North" } },
      });
      assert.deepEqual(multiResult, {
        behavior: "allow",
        toolUseID: "multi-question",
        updatedInput: {
          ...multiInput,
          answers: { "Choose both environments": "Staging, Production" },
          annotations: {
            "Choose both environments": {
              preview: "STAGING PREVIEW\nLINE\n\nPRODUCTION PREVIEW",
              notes: "Both environments",
            },
          },
        },
      });
      const finished = publishedTool(events, target.sessionId, "sequential-question");
      assert.equal(finished.status, "completed");
      assert.deepEqual((finished.raw_input as Record<string, unknown>).question_results, [
        {
          question: "Choose target",
          header: "Target",
          question_index: 0,
          total_questions: 2,
          selected_options: [
            {
              option_id: "question_1",
              label: "Production",
              description: "Customer-facing target",
              preview: "Production preview.",
            },
          ],
          annotation: { preview: "EXPLICIT-PREVIEW\n exact  ", notes: "Use production" },
        },
        {
          question: "Choose region",
          header: "Region",
          question_index: 1,
          total_questions: 2,
          selected_options: [],
        },
      ]);
      assert.deepEqual(
        (
          publishedTool(events, target.sessionId, "multi-question").raw_input as Record<
            string,
            unknown
          >
        ).question_results,
        [
          {
            question: "Choose both environments",
            header: "Environments",
            question_index: 0,
            total_questions: 1,
            selected_options: [
              {
                option_id: "question_0",
                label: "Staging",
                description: "Validation environment",
                preview: "STAGING PREVIEW\nLINE",
              },
              {
                option_id: "question_1",
                label: "Production",
                description: "Customer environment",
                preview: "PRODUCTION PREVIEW",
              },
            ],
            annotation: {
              preview: "STAGING PREVIEW\nLINE\n\nPRODUCTION PREVIEW",
              notes: "Both environments",
            },
          },
        ],
      );
    });
  }
});

test("native question cancellation settles only its correlated operation and leaves a peer and explicit new question usable", async () => {
  for (const alreadyAborted of [false, true]) {
    await withCommandTargets(async (target, peer) => {
      for (const session of [target, peer])
        session.query = {
          close: () => {},
          getSettings: async () => ({ sources: [] }),
        } as unknown as SessionState["query"];
      const inbox = bridgeEventInbox(),
        controller = new AbortController(),
        peerController = new AbortController(),
        freshController = new AbortController();
      if (alreadyAborted) controller.abort();
      const operations: Array<ReturnType<typeof requestAskUserQuestionAnswers>> = [];
      let cancelled: Awaited<ReturnType<typeof requestAskUserQuestionAnswers>> | undefined;
      let peerResult: typeof cancelled;
      let freshResult: typeof cancelled;
      const events = await captureBridgeEventsAsync(
        async () => {
          try {
            const sibling = ownedQuestion(
              peer,
              "cancel-peer-question",
              structuredClone(peerQuestionInput),
              peerController.signal,
            );
            operations.push(sibling);
            await nextQuestion(inbox, peer, "cancel-peer-question", 0);
            const pending = ownedQuestion(
              target,
              "cancel-owned-question",
              structuredClone(questionPairInput),
              controller.signal,
            );
            operations.push(pending);
            if (!alreadyAborted) {
              const request = await nextQuestion(inbox, target, "cancel-owned-question", 0);
              assert.equal(request.request.prompt.question, "Choose target");
              controller.abort();
            }
            cancelled = await boundedBridgeTask(pending);
            answerQuestion(peer.sessionId, "cancel-peer-question", ["question_1"]);
            peerResult = await boundedBridgeTask(sibling);
            const fresh = ownedQuestion(
              target,
              "fresh-after-cancel",
              structuredClone(peerQuestionInput),
              freshController.signal,
            );
            operations.push(fresh);
            await nextQuestion(inbox, target, "fresh-after-cancel", 0);
            answerQuestion(target.sessionId, "fresh-after-cancel", ["question_0"]);
            freshResult = await boundedBridgeTask(fresh);
          } finally {
            controller.abort();
            peerController.abort();
            freshController.abort();
            await Promise.allSettled(operations);
            inbox.dispose();
          }
        },
        (event) => inbox.observe(event),
      );
      assert.equal(cancelled?.behavior, "deny");
      assert.equal(cancelled?.toolUseID, "cancel-owned-question");
      assert.deepEqual(
        events.filter((event) => event.event === "interaction_cancelled"),
        [
          {
            event: "interaction_cancelled",
            session_id: target.sessionId,
            interaction_id: "cancel-owned-question",
          },
        ],
      );
      assert.equal(
        publishedTool(events, target.sessionId, "cancel-owned-question").status,
        "failed",
      );
      const requests = events.filter(
        (event) =>
          event.event === "question_request" &&
          event.session_id === target.sessionId &&
          (event.request as QuestionWire["request"]).tool_call.tool_call_id ===
            "cancel-owned-question",
      );
      assert.deepEqual(
        requests.map((event) => (event.request as QuestionWire["request"]).question_index),
        alreadyAborted ? [] : [0],
      );
      assert.ok(
        publishedToolFrames(events, target.sessionId, "cancel-owned-question").every(
          (frame) => frame.status !== "completed",
        ),
      );
      assert.deepEqual(peerResult, {
        behavior: "allow",
        toolUseID: "cancel-peer-question",
        updatedInput: { ...peerQuestionInput, answers: { "Choose peer route": "Peer South" } },
      });
      assert.deepEqual(freshResult, {
        behavior: "allow",
        toolUseID: "fresh-after-cancel",
        updatedInput: { ...peerQuestionInput, answers: { "Choose peer route": "Peer North" } },
      });
    });
  }
});

test("invalid advertised-question selections deny only their operation without a next prompt while valid peer and reuse controls succeed", async () => {
  for (const invalid of [
    ["unadvertised-option"],
    ["question_0", "question_0"],
    ["question_0", "question_1"],
  ]) {
    await withCommandTargets(async (target, peer) => {
      for (const session of [target, peer])
        session.query = {
          close: () => {},
          getSettings: async () => ({ sources: [] }),
        } as unknown as SessionState["query"];
      const inbox = bridgeEventInbox(),
        controller = new AbortController(),
        peerController = new AbortController();
      const operations: Array<ReturnType<typeof requestAskUserQuestionAnswers>> = [];
      let denied: Awaited<ReturnType<typeof requestAskUserQuestionAnswers>> | undefined;
      let peerResult: typeof denied;
      let reuseResult: typeof denied;
      const events = await captureBridgeEventsAsync(
        async () => {
          try {
            const pending = ownedQuestion(
                target,
                "invalid-question",
                structuredClone(questionPairInput),
                controller.signal,
              ),
              sibling = ownedQuestion(
                peer,
                "valid-peer-question",
                structuredClone(peerQuestionInput),
                peerController.signal,
              );
            operations.push(pending, sibling);
            await nextQuestion(inbox, target, "invalid-question", 0);
            await nextQuestion(inbox, peer, "valid-peer-question", 0);
            answerQuestion(target.sessionId, "invalid-question", invalid);
            denied = await boundedBridgeTask(pending);
            answerQuestion(peer.sessionId, "valid-peer-question", ["question_1"]);
            peerResult = await boundedBridgeTask(sibling);
            const reuse = ownedQuestion(
              target,
              "explicit-after-invalid",
              structuredClone(peerQuestionInput),
              controller.signal,
            );
            operations.push(reuse);
            await nextQuestion(inbox, target, "explicit-after-invalid", 0);
            answerQuestion(target.sessionId, "explicit-after-invalid", []);
            reuseResult = await boundedBridgeTask(reuse);
          } finally {
            controller.abort();
            peerController.abort();
            await Promise.allSettled(operations);
            inbox.dispose();
          }
        },
        (event) => {
          const request = event.request as QuestionWire["request"] | undefined;
          if (
            event.event === "question_request" &&
            event.session_id === target.sessionId &&
            request?.tool_call.tool_call_id === "invalid-question"
          )
            assert.equal(
              request.question_index,
              0,
              "an invalid answer must not advance to another prompt",
            );
          inbox.observe(event);
        },
      );
      assert.equal(denied?.behavior, "deny");
      assert.equal(denied?.toolUseID, "invalid-question");
      assert.equal(publishedTool(events, target.sessionId, "invalid-question").status, "failed");
      assert.ok(
        publishedToolFrames(events, target.sessionId, "invalid-question").every(
          (frame) => frame.status !== "completed",
        ),
      );
      assert.deepEqual(
        events
          .filter(
            (event) =>
              event.event === "question_request" &&
              event.session_id === target.sessionId &&
              (event.request as QuestionWire["request"]).tool_call.tool_call_id ===
                "invalid-question",
          )
          .map((event) => (event.request as QuestionWire["request"]).question_index),
        [0],
      );
      assert.deepEqual(peerResult, {
        behavior: "allow",
        toolUseID: "valid-peer-question",
        updatedInput: { ...peerQuestionInput, answers: { "Choose peer route": "Peer South" } },
      });
      assert.deepEqual(reuseResult, {
        behavior: "allow",
        toolUseID: "explicit-after-invalid",
        updatedInput: { ...peerQuestionInput, answers: {} },
      });
    });
  }
});

test("consecutive question timing reads recover from native rejection without reusing a stale deadline", async () => {
  await withCommandTargets(async (target) => {
    let reads = 0;
    const native = {
      close: () => {},
      async getSettings() {
        assert.equal(this, native);
        reads++;
        if (reads === 1)
          return {
            sources: [{ source: "userSettings", settings: { askUserQuestionTimeout: "60s" } }],
          };
        if (reads === 2) throw new Error("INDEPENDENT-TIMING-READ-CAUSE");
        return {
          sources: [{ source: "flagSettings", settings: { askUserQuestionTimeout: "5m" } }],
        };
      },
    };
    target.query = native as unknown as SessionState["query"];
    const inbox = bridgeEventInbox(),
      controller = new AbortController();
    const operations: Array<ReturnType<typeof requestAskUserQuestionAnswers>> = [];
    const results: Array<Awaited<ReturnType<typeof requestAskUserQuestionAnswers>>> = [];
    const events = await captureBridgeEventsAsync(
      async () => {
        try {
          for (const row of [
            { id: "timing-first", timeout: 60000 },
            { id: "timing-failed-read", timeout: undefined },
            { id: "timing-recovered-read", timeout: 300000 },
          ]) {
            const pending = ownedQuestion(
              target,
              row.id,
              structuredClone(peerQuestionInput),
              controller.signal,
            );
            operations.push(pending);
            const request = await nextQuestion(inbox, target, row.id, 0);
            assert.equal(request.request.idle_timeout_ms, row.timeout);
            answerQuestion(target.sessionId, row.id, ["question_1"]);
            results.push(await boundedBridgeTask(pending));
          }
        } finally {
          controller.abort();
          await Promise.allSettled(operations);
          inbox.dispose();
        }
      },
      (event) => inbox.observe(event),
    );
    assert.equal(reads, 3);
    assert.deepEqual(
      results,
      ["timing-first", "timing-failed-read", "timing-recovered-read"].map((id) => ({
        behavior: "allow",
        toolUseID: id,
        updatedInput: { ...peerQuestionInput, answers: { "Choose peer route": "Peer South" } },
      })),
    );
    for (const id of ["timing-first", "timing-failed-read", "timing-recovered-read"])
      assert.equal(publishedTool(events, target.sessionId, id).status, "completed");
  });
});

test("created tools expose independent semantic categories while preserving supported and unknown tool identity and input", () => {
  for (const row of [
    { name: "Bash", input: { command: "printf fixture" }, kind: "execute" },
    { name: "PowerShell", input: { command: "Get-Item owned.txt" }, kind: "execute" },
    { name: "Read", input: { file_path: "owned.txt" }, kind: "read" },
    { name: "Write", input: { file_path: "owned.txt", content: "Requested source" }, kind: "edit" },
    {
      name: "Edit",
      input: { file_path: "owned.txt", old_string: "old", new_string: "new" },
      kind: "edit",
    },
    { name: "Delete", input: { file_path: "owned.txt" }, kind: "delete" },
    { name: "Move", input: { source: "before.txt", destination: "after.txt" }, kind: "move" },
    { name: "Grep", input: { pattern: "independent-pattern" }, kind: "search" },
    { name: "WebFetch", input: { url: "https://example.test/owned" }, kind: "fetch" },
    { name: "Agent", input: { prompt: "Independent agent work" }, kind: "think" },
    { name: "Task", input: { prompt: "Independent task work" }, kind: "think" },
    { name: "EnterPlanMode", input: {}, kind: "switch_mode" },
    { name: "ExitPlanMode", input: { plan: "Independent plan" }, kind: "switch_mode" },
    { name: "Monitor", input: { command: "tail owned.log" }, kind: "other" },
    { name: "TaskOutput", input: { task_id: "known-task" }, kind: "other" },
  ]) {
    const tool = createToolCall(`category-${row.name}`, row.name, row.input);
    assert.equal(tool.tool_call_id, `category-${row.name}`);
    assert.equal(tool.kind, row.kind, row.name);
    assert.deepEqual(tool.raw_input, row.input);
    assert.equal((tool.meta as { claudeCode: { toolName: string } }).claudeCode.toolName, row.name);
  }
  const unknown = createToolCall("unknown-native-tool", "FutureUnknownTool", {
    independent: "future payload",
  });
  assert.equal(unknown.tool_call_id, "unknown-native-tool");
  assert.deepEqual(unknown.raw_input, { independent: "future payload" });
  assert.equal(
    (unknown.meta as { claudeCode: { toolName: string } }).claudeCode.toolName,
    "FutureUnknownTool",
  );
  assert.ok(unknown.title.trim());
});

test("supported shell titles preserve literal command bytes while lookalike names remain ordinary tools", () => {
  for (const row of [
    { name: "Bash", command: 'printf "two  spaces" && echo literal' },
    { name: "PowerShell", command: "Write-Output 'two  spaces'; Get-Item ." },
  ]) {
    const tool = createToolCall(`shell-${row.name}`, row.name, { command: row.command });
    assert.equal(tool.title, row.command);
    assert.equal(tool.kind, "execute");
    assert.deepEqual(tool.raw_input, { command: row.command });
    const empty = createToolCall(`empty-${row.name}`, row.name, {});
    assert.ok(empty.title.trim());
    assert.equal(empty.kind, "execute");
    assert.equal(
      (empty.meta as { claudeCode: { toolName: string } }).claudeCode.toolName,
      row.name,
    );
  }
  for (const name of ["Shell", "bash", "powershell"]) {
    const tool = createToolCall(`lookalike-${name}`, name, {
      command: "MUST-NOT-BECOME-SHELL-TITLE",
    });
    assert.notEqual(tool.title, "MUST-NOT-BECOME-SHELL-TITLE");
    assert.notEqual(tool.kind, "execute");
    assert.equal((tool.meta as { claudeCode: { toolName: string } }).claudeCode.toolName, name);
  }
});

test("resource directory titles retain the selected server and exact URI with useful missing-field identity", () => {
  for (const row of [
    {
      name: "both",
      input: { server: "independent-server-alpha", uri: "mcp://independent/space%20path?a=1&b=2" },
      expected: ["independent-server-alpha", "mcp://independent/space%20path?a=1&b=2"],
    },
    {
      name: "other-server",
      input: { server: "independent-server-beta", uri: "file://different/resource/" },
      expected: ["independent-server-beta", "file://different/resource/"],
    },
    {
      name: "uri-only",
      input: { uri: "mcp://uri-only/exact%2Fbytes" },
      expected: ["mcp://uri-only/exact%2Fbytes"],
    },
    { name: "empty", input: {}, expected: [] },
  ]) {
    const tool = createToolCall(`resource-directory-${row.name}`, "ReadMcpResourceDir", row.input);
    assert.equal(tool.tool_call_id, `resource-directory-${row.name}`);
    assert.equal(tool.kind, "read");
    assert.deepEqual(tool.raw_input, row.input);
    assert.equal(
      (tool.meta as { claudeCode: { toolName: string } }).claudeCode.toolName,
      "ReadMcpResourceDir",
    );
    assert.ok(tool.title.trim());
    for (const text of row.expected) assert.ok(tool.title.includes(text));
    if (row.name === "empty") assert.match(tool.title, /resource|mcp/i);
  }
});

test("rate limit projection and SDK ingress preserve independent core, optional, credit and overage observations", () => {
  const rows: Array<{ name: string; input: unknown; expected: Record<string, unknown> | null }> = [
    {
      name: "rich-warning",
      input: {
        status: "allowed_warning",
        resetsAt: 1741280000,
        utilization: 0.92,
        rateLimitType: "five_hour",
        limitScope: "group_pool",
        overageEnabled: false,
        overageStatus: "rejected",
        overageResetsAt: 1741280600,
        overageDisabledReason: "out_of_credits",
        isUsingOverage: false,
        surpassedThreshold: 0.9,
        errorCode: "credits_required",
        canUserPurchaseCredits: true,
        hasChargeableSavedPaymentMethod: false,
      },
      expected: {
        type: "rate_limit_update",
        status: "allowed_warning",
        resets_at: 1741280000,
        utilization: 0.92,
        rate_limit_type: "five_hour",
        limit_scope: "group_pool",
        overage_enabled: false,
        overage_status: "rejected",
        overage_resets_at: 1741280600,
        overage_disabled_reason: "out_of_credits",
        is_using_overage: false,
        surpassed_threshold: 0.9,
        error_code: "credits_required",
        can_user_purchase_credits: true,
        has_chargeable_saved_payment_method: false,
      },
    },
    {
      name: "enabled-not-used",
      input: {
        status: "allowed",
        overageEnabled: true,
        isUsingOverage: false,
        overageStatus: "allowed",
      },
      expected: {
        type: "rate_limit_update",
        status: "allowed",
        overage_enabled: true,
        is_using_overage: false,
        overage_status: "allowed",
      },
    },
    {
      name: "disabled-but-used",
      input: {
        status: "rejected",
        overageEnabled: false,
        isUsingOverage: true,
        overageStatus: "rejected",
      },
      expected: {
        type: "rate_limit_update",
        status: "rejected",
        overage_enabled: false,
        is_using_overage: true,
        overage_status: "rejected",
      },
    },
    {
      name: "unknown-enablement",
      input: { status: "allowed", isUsingOverage: true, overageStatus: "allowed_warning" },
      expected: {
        type: "rate_limit_update",
        status: "allowed",
        is_using_overage: true,
        overage_status: "allowed_warning",
      },
    },
    {
      name: "null-enablement",
      input: { status: "allowed", overageEnabled: null, isUsingOverage: false },
      expected: { type: "rate_limit_update", status: "allowed", is_using_overage: false },
    },
    {
      name: "string-enablement",
      input: { status: "allowed", overageEnabled: "true", isUsingOverage: false },
      expected: { type: "rate_limit_update", status: "allowed", is_using_overage: false },
    },
    {
      name: "zero-and-fraction",
      input: {
        status: "allowed",
        resetsAt: 0,
        utilization: 0.375,
        overageResetsAt: 10.5,
        surpassedThreshold: 0,
        limitScope: "channel",
        rateLimitType: "future-native-window",
      },
      expected: {
        type: "rate_limit_update",
        status: "allowed",
        resets_at: 0,
        utilization: 0.375,
        overage_resets_at: 10.5,
        surpassed_threshold: 0,
        limit_scope: "channel",
        rate_limit_type: "future-native-window",
      },
    },
    {
      name: "invalid-optionals",
      input: {
        status: "allowed",
        resetsAt: Number.POSITIVE_INFINITY,
        utilization: Number.NaN,
        overageResetsAt: Number.NEGATIVE_INFINITY,
        surpassedThreshold: Number.NaN,
        overageStatus: "unknown",
        errorCode: "unknown",
        limitScope: "unknown",
        canUserPurchaseCredits: null,
        hasChargeableSavedPaymentMethod: "false",
      },
      expected: { type: "rate_limit_update", status: "allowed" },
    },
    {
      name: "missing-required",
      input: { overageStatus: "allowed", utilization: 0.5 },
      expected: null,
    },
    {
      name: "invalid-required",
      input: { status: "unknown", overageStatus: "allowed" },
      expected: null,
    },
    { name: "non-string-required", input: { status: 7 }, expected: null },
    { name: "non-record", input: null, expected: null },
  ];
  for (const row of rows) {
    assert.deepEqual(buildRateLimitUpdate(row.input), row.expected, row.name);
    const session = makeSessionState();
    session.sessionId = `rate-limit-${row.name}`;
    const events = captureBridgeEvents(() =>
      handleSdkMessage(session, {
        type: "rate_limit_event",
        uuid: `rate-source-${row.name}`,
        session_id: session.sessionId,
        rate_limit_info: row.input,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    assert.deepEqual(
      events,
      row.expected
        ? [{ event: "session_update", session_id: session.sessionId, update: row.expected }]
        : [],
      row.name,
    );
  }
});

test("buildRateLimitUpdate normalizes SDK overage boolean spellings", () => {
  const cases = [
    ["old spelling true", { isUsingOverage: true }, true],
    ["old spelling false", { isUsingOverage: false }, false],
    ["new spelling true", { overageInUse: true }, true],
    ["new spelling false", { overageInUse: false }, false],
    ["both spellings true", { isUsingOverage: true, overageInUse: true }, true],
    [
      "both spellings false",
      { isUsingOverage: false, overageInUse: false },
      false,
    ],
    [
      "conflicting spellings prefer new true",
      { isUsingOverage: false, overageInUse: true },
      true,
    ],
    [
      "conflicting spellings prefer new false",
      { isUsingOverage: true, overageInUse: false },
      false,
    ],
  ] as const;

  for (const [name, fields, expected] of cases) {
    const update = buildRateLimitUpdate({
      status: "allowed",
      ...fields,
    });
    assert.equal(update?.is_using_overage, expected, name);
  }
});

test("buildApiRetryUpdate maps SDK api_retry messages to wire shape", () => {
  assert.deepEqual(
    buildApiRetryUpdate({
      attempt: 2,
      max_retries: 4,
      retry_delay_ms: 1500,
      error_status: 529,
      error: "server_error",
    }),
    {
      type: "api_retry_update",
      attempt: 2,
      max_retries: 4,
      retry_delay_ms: 1500,
      error_status: 529,
      error: "server_error",
    },
  );
  for (const error of [
    "account_on_hold",
    "model_not_found",
    "oauth_org_not_allowed",
    "overloaded",
  ] as const) {
    assert.deepEqual(
      buildApiRetryUpdate({
        attempt: 2,
        max_retries: 4,
        retry_delay_ms: 1500,
        error_status: 529,
        error,
      }),
      {
        type: "api_retry_update",
        attempt: 2,
        max_retries: 4,
        retry_delay_ms: 1500,
        error_status: 529,
        error,
      },
    );
  }

  assert.deepEqual(
    buildApiRetryUpdate({
      attempt: 1,
      maxRetries: 4,
      retryDelayMs: 1000,
      errorStatus: null,
      error: "unexpected",
    }),
    {
      type: "api_retry_update",
      attempt: 1,
      max_retries: 4,
      retry_delay_ms: 1000,
      error_status: null,
      error: "unknown",
    },
  );
  assert.deepEqual(
    buildApiRetryUpdate({
      attempt: 1,
      max_retries: 10,
      retry_delay_ms: 549.8881698459426,
      error_status: null,
      error: "unexpected",
    }),
    {
      type: "api_retry_update",
      attempt: 1,
      max_retries: 10,
      retry_delay_ms: 549.8881698459426,
      error_status: null,
      error: "unknown",
    },
  );
  assert.equal(buildApiRetryUpdate({ attempt: 1 }), null);
  assert.equal(
    buildApiRetryUpdate({
      attempt: 1,
      max_retries: 10,
      retry_delay_ms: -1,
      error_status: null,
      error: "server_error",
    }),
    null,
  );
});

test("supported lifecycle SDK envelopes publish their own scoped payloads and reject malformed ingress", () => {
  const rows: Array<{
    name: string;
    message: Record<string, unknown>;
    expected: Record<string, unknown> | null;
  }> = [
    {
      name: "suggestion",
      message: { type: "prompt_suggestion", suggestion: "Independent suggestion\n exact" },
      expected: { type: "prompt_suggestion_update", suggestion: "Independent suggestion\n exact" },
    },
    {
      name: "api-retry",
      message: {
        type: "system",
        subtype: "api_retry",
        attempt: 2,
        max_retries: 5,
        retry_delay_ms: 1250.5,
        error_status: null,
        error: "server_error",
      },
      expected: {
        type: "api_retry_update",
        attempt: 2,
        max_retries: 5,
        retry_delay_ms: 1250.5,
        error_status: null,
        error: "server_error",
      },
    },
    {
      name: "malformed-retry",
      message: {
        type: "system",
        subtype: "api_retry",
        attempt: 2,
        max_retries: 5,
        retry_delay_ms: -1,
        error_status: null,
        error: "server_error",
      },
      expected: null,
    },
    {
      name: "idle-runtime",
      message: { type: "system", subtype: "session_state_changed", state: "idle" },
      expected: { type: "runtime_session_state_update", state: "idle" },
    },
    {
      name: "running-runtime",
      message: { type: "system", subtype: "session_state_changed", state: "running" },
      expected: { type: "runtime_session_state_update", state: "running" },
    },
    {
      name: "action-runtime",
      message: { type: "system", subtype: "session_state_changed", state: "requires_action" },
      expected: { type: "runtime_session_state_update", state: "requires_action" },
    },
    {
      name: "invalid-runtime",
      message: { type: "system", subtype: "session_state_changed", state: "unknown" },
      expected: null,
    },
    {
      name: "absent-runtime",
      message: { type: "system", subtype: "session_state_changed" },
      expected: null,
    },
    {
      name: "requesting",
      message: { type: "system", subtype: "status", status: "requesting" },
      expected: { type: "session_status_update", status: "requesting" },
    },
    {
      name: "idle",
      message: { type: "system", subtype: "status", status: null },
      expected: { type: "session_status_update", status: "idle" },
    },
  ];
  for (const row of rows) {
    const session = makeSessionState();
    session.sessionId = `lifecycle-${row.name}`;
    const events = captureBridgeEvents(() =>
      handleSdkMessage(session, {
        ...row.message,
        uuid: `lifecycle-source-${row.name}`,
        session_id: session.sessionId,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    assert.deepEqual(
      events,
      row.expected
        ? [{ event: "session_update", session_id: session.sessionId, update: row.expected }]
        : [],
      row.name,
    );
  }
});

test("each compaction workflow publishes one typed outcome before idle and validates required versus optional boundary counters", () => {
  for (const row of [
    {
      name: "success",
      result: "success",
      error: undefined,
      expected: { type: "compaction_update", phase: "finished", result: "success" },
    },
    {
      name: "known-failure",
      result: "failed",
      error: "too_few_groups",
      expected: {
        type: "compaction_update",
        phase: "finished",
        result: "failed",
        error_code: "too_few_groups",
        error: "too_few_groups",
      },
    },
    {
      name: "unknown-failure",
      result: "failed",
      error: "  INDEPENDENT-COMPACT-CAUSE  ",
      expected: {
        type: "compaction_update",
        phase: "finished",
        result: "failed",
        error_code: "unknown",
        error: "INDEPENDENT-COMPACT-CAUSE",
      },
    },
    {
      name: "missing-detail",
      result: "failed",
      error: undefined,
      expected: {
        type: "compaction_update",
        phase: "finished",
        result: "failed",
        error_code: "unknown",
      },
    },
  ]) {
    const session = makeSessionState();
    session.sessionId = `compaction-${row.name}`;
    const events = captureBridgeEvents(() => {
      handleSdkMessage(session, {
        type: "system",
        subtype: "status",
        status: "compacting",
        session_id: session.sessionId,
        uuid: "compaction-start",
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      handleSdkMessage(session, {
        type: "system",
        subtype: "compact_boundary",
        compact_metadata: {
          trigger: "auto",
          pre_tokens: 180000,
          post_tokens: 22000,
          duration_ms: 1250,
        },
        session_id: session.sessionId,
        uuid: "compaction-boundary",
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      handleSdkMessage(session, {
        type: "system",
        subtype: "status",
        status: null,
        compact_result: row.result,
        ...(row.error === undefined ? {} : { compact_error: row.error }),
        session_id: session.sessionId,
        uuid: "compaction-finish",
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    });
    assert.deepEqual(events, [
      {
        event: "session_update",
        session_id: session.sessionId,
        update: { type: "compaction_update", phase: "started" },
      },
      {
        event: "session_update",
        session_id: session.sessionId,
        update: {
          type: "compaction_update",
          phase: "boundary",
          trigger: "auto",
          pre_tokens: 180000,
          post_tokens: 22000,
          duration_ms: 1250,
        },
      },
      { event: "session_update", session_id: session.sessionId, update: row.expected },
      {
        event: "session_update",
        session_id: session.sessionId,
        update: { type: "session_status_update", status: "idle" },
      },
    ]);
  }
  for (const row of [
    {
      name: "manual-zero",
      metadata: { trigger: "manual", pre_tokens: 0, post_tokens: 0, duration_ms: 0 },
      expected: {
        type: "compaction_update",
        phase: "boundary",
        trigger: "manual",
        pre_tokens: 0,
        post_tokens: 0,
        duration_ms: 0,
      },
    },
    { name: "invalid-trigger", metadata: { trigger: "unknown", pre_tokens: 1 }, expected: null },
    { name: "missing-pre", metadata: { trigger: "auto" }, expected: null },
    { name: "negative-pre", metadata: { trigger: "auto", pre_tokens: -1 }, expected: null },
    { name: "fractional-pre", metadata: { trigger: "auto", pre_tokens: 1.5 }, expected: null },
    {
      name: "invalid-post",
      metadata: { trigger: "auto", pre_tokens: 17, post_tokens: -1, duration_ms: 5 },
      expected: {
        type: "compaction_update",
        phase: "boundary",
        trigger: "auto",
        pre_tokens: 17,
        duration_ms: 5,
      },
    },
    {
      name: "invalid-duration",
      metadata: {
        trigger: "auto",
        pre_tokens: 17,
        post_tokens: 7,
        duration_ms: Number.POSITIVE_INFINITY,
      },
      expected: {
        type: "compaction_update",
        phase: "boundary",
        trigger: "auto",
        pre_tokens: 17,
        post_tokens: 7,
      },
    },
  ]) {
    const session = makeSessionState();
    session.sessionId = `compaction-boundary-${row.name}`;
    const events = captureBridgeEvents(() =>
      handleSdkMessage(session, {
        type: "system",
        subtype: "compact_boundary",
        compact_metadata: row.metadata,
        uuid: "independent-boundary",
        session_id: session.sessionId,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    assert.deepEqual(
      events,
      row.expected
        ? [{ event: "session_update", session_id: session.sessionId, update: row.expected }]
        : [],
    );
    const idle = captureBridgeEvents(() =>
      handleSdkMessage(session, {
        type: "system",
        subtype: "status",
        status: null,
        uuid: "ordinary-idle",
        session_id: session.sessionId,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    assert.deepEqual(idle, [
      {
        event: "session_update",
        session_id: session.sessionId,
        update: { type: "session_status_update", status: "idle" },
      },
    ]);
  }
});

function commandPublications(events: Array<Record<string, unknown>>, sessionId: string) {
  return events.flatMap((event) => {
    const update = event.update as
      | {
          type?: string;
          commands: Array<Record<string, unknown>>;
          source: string;
          generation: number;
        }
      | undefined;
    return event.session_id === sessionId && update?.type === "available_commands_update"
      ? [update]
      : [];
  });
}
function commandInitQuery(
  session: SessionState,
  supported: () => Promise<import("@anthropic-ai/claude-agent-sdk").SlashCommand[]>,
) {
  const pending = new Set<Promise<unknown>>();
  function read<T>(operation: Promise<T>): Promise<T> {
    pending.add(operation);
    void operation.finally(() => pending.delete(operation));
    return operation;
  }
  const native = {
    close: () => {},
    supportedCommands: function () {
      assert.equal(this, native);
      return read(supported());
    },
    supportedAgents: function () {
      assert.equal(this, native);
      return read(Promise.resolve([]));
    },
    getSettings: function () {
      assert.equal(this, native);
      return read(
        Promise.resolve({
          applied: {
            model: "haiku",
            effort: "medium",
            ultracodeAvailable: false,
            ultracodeRequested: false,
            ultracode: false,
          },
        }),
      );
    },
  };
  session.query = native as unknown as SessionState["query"];
  return {
    async settle() {
      while (pending.size) await boundedBridgeTask(Promise.all([...pending]));
      await eventLoopCheckpoint();
      while (pending.size) await boundedBridgeTask(Promise.all([...pending]));
    },
  };
}
test("dynamic command authority replaces metadata, removes omitted commands and explicitly clears only its own session", async () => {
  await withCommandTargets(async (target, peer) => {
    const initial = await captureBridgeEventsAsync(async () => {
      updateAvailableCommands(target, "reload_plugins", [
        { name: "removed-plugin", description: "Old removed plugin" },
        {
          name: "retained-plugin",
          description: "Old retained description",
          input_hint: "<old>",
          aliases: ["old-alias"],
          builtin: true,
        },
      ]);
      handleSdkMessage(peer, {
        type: "system",
        subtype: "commands_changed",
        session_id: peer.sessionId,
        uuid: "peer-command-source",
        commands: [{ name: "peer-command", description: "Independent peer command" }],
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    });
    const before = commandPublications(initial, target.sessionId);
    assert.equal(before.length, 1);
    assert.deepEqual(before[0]?.commands, [
      { name: "removed-plugin", description: "Old removed plugin" },
      {
        name: "retained-plugin",
        description: "Old retained description",
        input_hint: "<old>",
        aliases: ["old-alias"],
        builtin: true,
      },
    ]);
    assert.equal(before[0]?.source, "reload_plugins");
    const changed = await captureBridgeEventsAsync(async () =>
      handleSdkMessage(target, {
        type: "system",
        subtype: "commands_changed",
        session_id: target.sessionId,
        uuid: "changed-command-source",
        commands: [
          {
            name: "retained-plugin",
            description: "New retained description",
            argumentHint: "<new>",
            aliases: ["new-alias", "new-alias", "retained-plugin"],
            builtin: false,
          },
          { name: "new-plugin", description: "Independent new plugin" },
        ],
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    const replacement = commandPublications(changed, target.sessionId);
    assert.equal(replacement.length, 1);
    assert.equal(replacement[0]?.source, "commands_changed");
    assert.deepEqual(replacement[0]?.commands, [
      {
        name: "retained-plugin",
        description: "New retained description",
        input_hint: "<new>",
        aliases: ["new-alias"],
        builtin: false,
      },
      { name: "new-plugin", description: "Independent new plugin" },
    ]);
    assert.ok((replacement[0]?.generation ?? 0) > (before[0]?.generation ?? 0));
    assert.deepEqual(commandPublications(changed, peer.sessionId), []);
    const cleared = await captureBridgeEventsAsync(async () =>
      handleSdkMessage(target, {
        type: "system",
        subtype: "commands_changed",
        session_id: target.sessionId,
        uuid: "empty-command-source",
        commands: [],
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    const empty = commandPublications(cleared, target.sessionId);
    assert.equal(empty.length, 1);
    assert.equal(empty[0]?.source, "commands_changed");
    assert.deepEqual(empty[0]?.commands, []);
    assert.ok((empty[0]?.generation ?? 0) > (replacement[0]?.generation ?? 0));
    assert.deepEqual(commandPublications(cleared, peer.sessionId), []);
    const stale = await captureBridgeEventsAsync(async () => {
      updateAvailableCommands(target, "supportedCommands", [
        { name: "must-not-resurrect", description: "Stale bootstrap" },
      ]);
    });
    assert.deepEqual(commandPublications(stale, target.sessionId), []);
    assert.deepEqual(commandPublications(stale, peer.sessionId), []);
    assert.deepEqual(commandPublications(initial, peer.sessionId)[0]?.commands, [
      { name: "peer-command", description: "Independent peer command" },
    ]);
  });
});

test("command inventory normalizes named no-argument hints while preserving exact real argument syntax", () => {
  const rows = [
    { name: "hint-absent", input: undefined, expected: undefined },
    { name: "hint-empty", input: "", expected: undefined },
    { name: "hint-spaces", input: "  ", expected: undefined },
    { name: "hint-none", input: "none", expected: undefined },
    { name: "hint-bracketed", input: " [None] ", expected: undefined },
    { name: "hint-lower-bracketed", input: "[none]", expected: undefined },
    { name: "hint-required", input: "<target>", expected: "<target>" },
    { name: "hint-optional", input: "[optional reason]", expected: "[optional reason]" },
    { name: "hint-alternatives", input: "[none|all]", expected: "[none|all]" },
  ];
  const session = makeSessionState();
  session.sessionId = "argument-hint-owner";
  const events = captureBridgeEvents(() =>
    handleSdkMessage(session, {
      type: "system",
      subtype: "commands_changed",
      uuid: "argument-hint-source",
      session_id: session.sessionId,
      commands: rows.map((row) => ({
        name: row.name,
        description: `Independent ${row.name}`,
        argumentHint: row.input,
      })),
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
  );
  const snapshots = commandPublications(events, session.sessionId);
  assert.equal(snapshots.length, 1);
  const commands = new Map(snapshots[0]?.commands.map((command) => [command.name, command]));
  assert.deepEqual(new Set(commands.keys()), new Set(rows.map((row) => row.name)));
  for (const row of rows)
    assert.deepEqual(commands.get(row.name), {
      name: row.name,
      description: `Independent ${row.name}`,
      ...(row.expected === undefined ? {} : { input_hint: row.expected }),
    });
});

test("identity replacement resets before replaying nonempty or empty command authority and cannot publish stale bootstrap work", async () => {
  for (const populated of [true, false]) {
    await withCommandTargets(async (target, peer) => {
      const payload = populated
        ? [
            {
              name: "current-plugin",
              description: "Independent current plugin",
              argumentHint: "<path>",
              aliases: ["current-short"],
              builtin: false,
            },
          ]
        : [];
      const expected = populated
        ? [
            {
              name: "current-plugin",
              description: "Independent current plugin",
              input_hint: "<path>",
              aliases: ["current-short"],
              builtin: false,
            },
          ]
        : [];
      const seeded = await captureBridgeEventsAsync(async () =>
        handleSdkMessage(target, {
          type: "system",
          subtype: "commands_changed",
          uuid: "seed-current-authority",
          session_id: target.sessionId,
          commands: payload,
        } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
      );
      const seed = commandPublications(seeded, target.sessionId)[0];
      assert.ok(seed);
      assert.deepEqual(seed.commands, expected);
      const stale = bridgeDeferred<import("@anthropic-ai/claude-agent-sdk").SlashCommand[]>(),
        entered = bridgeDeferred<void>();
      const native = commandInitQuery(target, () => {
        entered.resolve();
        return stale.promise;
      });
      const replacementId = crypto.randomUUID();
      const events = await captureBridgeEventsAsync(async () => {
        try {
          handleSdkMessage(target, {
            type: "system",
            subtype: "init",
            uuid: "native-replacement-init",
            session_id: replacementId,
            model: "haiku",
            slash_commands: ["must-not-publish-stale-init"],
            agents: [],
          } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
          await boundedBridgeTask(entered.promise);
          stale.resolve([
            {
              name: "must-not-publish-stale-native",
              description: "Stale native response",
              argumentHint: "",
            },
          ]);
          await native.settle();
        } finally {
          stale.resolve([]);
          await native.settle();
        }
      });
      const relevant = events.filter(
        (event) =>
          event.event === "session_replaced" ||
          (event.update as { type?: string } | undefined)?.type === "available_commands_update",
      );
      assert.deepEqual(
        relevant.map((event) => ({ event: event.event, session_id: event.session_id })),
        [
          { event: "session_replaced", session_id: replacementId },
          { event: "session_update", session_id: replacementId },
        ],
      );
      assert.deepEqual(commandPublications(events, replacementId), [
        {
          type: "available_commands_update",
          commands: expected,
          source: "commands_changed",
          generation: seed.generation,
        },
      ]);
      assert.deepEqual(commandPublications(events, peer.sessionId), []);
      assert.doesNotMatch(
        JSON.stringify(relevant),
        /must-not-publish-stale-init|must-not-publish-stale-native/,
      );
    });
  }
});

test("late supportedCommands callbacks cannot overwrite populated or empty dynamic authority while an independent bootstrap still publishes", async () => {
  for (const populated of [true, false]) {
    await withCommandTargets(async (target, peer) => {
      const seeded = await captureBridgeEventsAsync(async () => {
        updateAvailableCommands(target, "session_result_commands", [
          {
            name: "initial-full",
            description: "Independent initial metadata",
            input_hint: "<initial>",
          },
        ]);
      });
      const seed = commandPublications(seeded, target.sessionId)[0];
      assert.ok(seed);
      const stale = bridgeDeferred<import("@anthropic-ai/claude-agent-sdk").SlashCommand[]>(),
        entered = bridgeDeferred<void>();
      const targetNative = commandInitQuery(target, () => {
        entered.resolve();
        return stale.promise;
      });
      const peerNative = commandInitQuery(peer, async () => [
        {
          name: "valid-peer-bootstrap",
          description: "Independent peer bootstrap",
          argumentHint: "<peer>",
        },
      ]);
      const events = await captureBridgeEventsAsync(async () => {
        try {
          handleSdkMessage(target, {
            type: "system",
            subtype: "init",
            uuid: "target-bootstrap-init",
            session_id: target.sessionId,
            model: "haiku",
            slash_commands: ["stale-sparse-init"],
            agents: [],
          } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
          await boundedBridgeTask(entered.promise);
          handleSdkMessage(peer, {
            type: "system",
            subtype: "init",
            uuid: "peer-bootstrap-init",
            session_id: peer.sessionId,
            model: "haiku",
            agents: [],
          } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
          handleSdkMessage(target, {
            type: "system",
            subtype: "commands_changed",
            uuid: "winning-dynamic-authority",
            session_id: target.sessionId,
            commands: populated
              ? [
                  {
                    name: "winning-dynamic",
                    description: "Independent dynamic command",
                    argumentHint: "<dynamic>",
                  },
                ]
              : [],
          } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
          stale.resolve([
            {
              name: "must-not-publish-stale-callback",
              description: "Removed native command",
              argumentHint: "<stale>",
            },
          ]);
          await Promise.all([targetNative.settle(), peerNative.settle()]);
        } finally {
          stale.resolve([]);
          await Promise.all([targetNative.settle(), peerNative.settle()]);
        }
      });
      const targetUpdates = commandPublications(events, target.sessionId);
      assert.equal(targetUpdates.length, 1);
      assert.equal(targetUpdates[0]?.source, "commands_changed");
      assert.deepEqual(
        targetUpdates[0]?.commands,
        populated
          ? [
              {
                name: "winning-dynamic",
                description: "Independent dynamic command",
                input_hint: "<dynamic>",
              },
            ]
          : [],
      );
      assert.ok((targetUpdates[0]?.generation ?? 0) > seed.generation);
      const peerUpdates = commandPublications(events, peer.sessionId);
      assert.equal(peerUpdates.length, 1);
      assert.equal(peerUpdates[0]?.source, "supportedCommands");
      assert.deepEqual(peerUpdates[0]?.commands, [
        {
          name: "valid-peer-bootstrap",
          description: "Independent peer bootstrap",
          input_hint: "<peer>",
        },
      ]);
      assert.doesNotMatch(
        JSON.stringify([...targetUpdates, ...peerUpdates]),
        /must-not-publish-stale-callback|stale-sparse-init/,
      );
    });
  }
});

test("init plugin diagnostics bound every field and the accepted list while preserving independent failure attribution", async () => {
  const session = makeSessionState();
  session.sessionId = `plugin-diagnostics-${crypto.randomUUID()}`;
  const native = commandInitQuery(session, async () => []);
  const fields = [
    { key: "plugin", prefix: "PLUGIN[0]", fill: "P" },
    { key: "type", prefix: "CATEGORY[0]", fill: "T" },
    { key: "message", prefix: "MESSAGE[0]", fill: "M" },
    { key: "path", prefix: "C:/OWNED/PATH[0]", fill: "X" },
  ];
  const oversized = Object.fromEntries(
    fields.map((field) => [
      field.key,
      field.prefix + field.fill.repeat(1200) + `UNRETAINED-${field.key}-TAIL`,
    ]),
  );
  const errors = [
    oversized,
    ...Array.from({ length: 22 }, (_, index) => ({
      plugin: `PLUGIN[${index + 1}]`,
      type: `CATEGORY[${index + 1}]`,
      message: `MESSAGE[${index + 1}]`,
      path: `C:/OWNED/PATH[${index + 1}]`,
    })),
  ];
  const events = await captureBridgeEventsAsync(async () => {
    try {
      handleSdkMessage(session, {
        type: "system",
        subtype: "init",
        session_id: session.sessionId,
        uuid: "plugin-diagnostic-init",
        model: "haiku",
        plugin_errors: errors,
        slash_commands: [],
        agents: [],
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      await native.settle();
    } finally {
      await native.settle();
      session.input.close();
    }
  });
  const notices = noticePublications(events, session.sessionId).map(
    (event) => event.update as { severity: string; message: string },
  );
  assert.equal(notices.length, 21);
  assert.ok(notices.every((notice) => notice.severity === "warning"));
  const first = notices.find((notice) => notice.message.includes("PLUGIN[0]"));
  assert.ok(first);
  for (const field of fields) {
    const expected = field.prefix + field.fill.repeat(1024 - field.prefix.length);
    assert.ok(first.message.includes(expected), field.key);
    assert.ok(!first.message.includes(expected + field.fill), field.key);
    assert.ok(!first.message.includes(`UNRETAINED-${field.key}-TAIL`));
  }
  for (let index = 1; index < 20; index++) {
    const matching = notices.filter((notice) => notice.message.includes(`PLUGIN[${index}]`));
    assert.equal(matching.length, 1);
    assert.ok(matching[0]?.message.includes(`CATEGORY[${index}]`));
    assert.ok(matching[0]?.message.includes(`MESSAGE[${index}]`));
    assert.ok(matching[0]?.message.includes(`C:/OWNED/PATH[${index}]`));
  }
  for (const index of [20, 21, 22])
    assert.ok(notices.every((notice) => !notice.message.includes(`PLUGIN[${index}]`)));
  const omitted = notices.filter((notice) => /omitt/i.test(notice.message));
  assert.equal(omitted.length, 1);
  assert.match(omitted[0]?.message ?? "", /\b3\b/);
  const installation = captureBridgeEvents(() =>
    handleSdkMessage(session, {
      type: "system",
      subtype: "plugin_install",
      uuid: "plugin-install-failure",
      session_id: session.sessionId,
      status: "failed",
      name: "INDEPENDENT-INSTALL-PLUGIN",
      error: "INDEPENDENT-INSTALL-CAUSE",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
  );
  const failed = noticePublications(installation, session.sessionId);
  assert.equal(failed.length, 1);
  const warning = failed[0]?.update as { severity: string; message: string };
  assert.equal(warning.severity, "warning");
  assert.ok(warning.message.includes("INDEPENDENT-INSTALL-PLUGIN"));
  assert.ok(warning.message.includes("INDEPENDENT-INSTALL-CAUSE"));
});

for (const boundary of ["close", "identity-change", "conversation-reset"] as const) {
  test(`side-question ${boundary} aborts SDK work and suppresses late NDJSON results`, async () => {
    const session=makeSessionState();session.sessionId=`side-question-${boundary}-${crypto.randomUUID()}`;const originalId=session.sessionId;sessions.set(originalId,session);
    const native=commandInitQuery(session,async()=>[]),oldResult=bridgeDeferred<unknown>(),entered=bridgeDeferred<AbortSignal>(),inbox=bridgeEventInbox();
    const query=session.query as SessionState["query"]&{askSideQuestion?:(question:string,options:{signal:AbortSignal})=>Promise<unknown>};
    query.askSideQuestion=async function(question,options){assert.equal(this,query);if(question==="OWNED-OLD-QUESTION"){entered.resolve(options.signal);return oldResult.promise;}assert.equal(question,"OWNED-NEW-QUESTION");return {response:"OWNED-NEW-ANSWER",synthetic:false};};
    const controlQuery={close:()=>{},askSideQuestion:async(question:string)=>{assert.equal(question,"OWNED-NEW-QUESTION");return {response:"OWNED-NEW-ANSWER",synthetic:false};}} as unknown as SessionState["query"];
    const controlId=`fresh-close-control-${crypto.randomUUID()}`;let newOwner=originalId;
    const events=await captureBridgeEventsAsync(async()=>{try{
      dispatchSideQuestion(originalId,session.query,{btwId:"owned-old",question:"OWNED-OLD-QUESTION"});const signal=await boundedBridgeTask(entered.promise);assert.equal(signal.aborted,false);
      if(boundary==="close")beginSessionClose(session);else if(boundary==="identity-change")updateSessionId(session,`${originalId}-replacement`);else handleSdkMessage(session,{type:"conversation_reset",new_conversation_id:"owned-reset-conversation",uuid:"side-question-reset-source",session_id:session.sessionId} as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      assert.equal(signal.aborted,true);oldResult.resolve({response:"MUST-NOT-PUBLISH-LATE-ANSWER",synthetic:false});await boundedBridgeTask(oldResult.promise);await eventLoopCheckpoint();await native.settle();
      newOwner=boundary==="close"?controlId:session.sessionId;dispatchSideQuestion(newOwner,boundary==="close"?controlQuery:session.query,{btwId:"owned-new",question:"OWNED-NEW-QUESTION"});await boundedBridgeTask(inbox.next(event=>event.event==="btw_result"&&event.session_id===newOwner&&event.btw_id==="owned-new"));
    }finally{closeSideQuestions(originalId,session.query);closeSideQuestions(session.sessionId,session.query);closeSideQuestions(controlId,controlQuery);beginSessionClose(session);oldResult.resolve({response:"CLEANUP-OLD-ANSWER",synthetic:false});await boundedBridgeTask(oldResult.promise);await eventLoopCheckpoint();await native.settle();await boundedBridgeTask(closeSession(session));for(const id of [originalId,session.sessionId])if(sessions.get(id)===session)sessions.delete(id);inbox.dispose();}},event=>inbox.observe(event));
    assert.equal(events.some(event=>event.btw_id==="owned-old"),false);assert.doesNotMatch(JSON.stringify(events),/MUST-NOT-PUBLISH-LATE-ANSWER/);assert.deepEqual(events.filter(event=>event.btw_id==="owned-new"),[{event:"btw_result",session_id:newOwner,btw_id:"owned-new",question:"OWNED-NEW-QUESTION",answer:"OWNED-NEW-ANSWER",metadata:{synthetic:false}}]);
  });
}

test("conversation reset preserves future metadata under the current session and refuses missing identity while refreshes settle", async () => {
  await withCommandTargets(async (target, peer) => {
    const native = commandInitQuery(target, async () => []);
    const events = await captureBridgeEventsAsync(async () => {
      try {
        handleSdkMessage(target, {
          type: "conversation_reset",
          uuid: "reset-envelope-source",
          session_id: target.sessionId,
          new_conversation_id: "independent-new-conversation",
          trigger: "future-native-trigger",
          timestamp: "2026-10-01T12:34:56Z",
          user_message_uuid: "independent-initiating-user",
        } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
        await native.settle();
      } finally {
        await native.settle();
      }
    });
    assert.deepEqual(
      events.filter(
        (event) => (event.update as { type?: string } | undefined)?.type === "conversation_reset",
      ),
      [
        {
          event: "session_update",
          session_id: target.sessionId,
          update: {
            type: "conversation_reset",
            new_conversation_id: "independent-new-conversation",
            trigger: "future-native-trigger",
            timestamp: "2026-10-01T12:34:56Z",
            user_message_uuid: "independent-initiating-user",
          },
        },
      ],
    );
    assert.equal(
      events.some((event) => event.session_id === peer.sessionId),
      false,
    );
    for (const identity of [undefined, "", "   ", 7]) {
      const rejected = await captureBridgeEventsAsync(async () => {
        handleSdkMessage(target, {
          type: "conversation_reset",
          uuid: "invalid-reset-source",
          session_id: target.sessionId,
          new_conversation_id: identity,
          trigger: "future-native-trigger",
        } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
        await native.settle();
      });
      assert.equal(
        rejected.some(
          (event) => (event.update as { type?: string } | undefined)?.type === "conversation_reset",
        ),
        false,
      );
    }
  });
});

function noticePublications(events: Array<Record<string, unknown>>, sessionId: string) {
  return events.filter(
    (event) =>
      event.session_id === sessionId &&
      (event.update as { type?: string } | undefined)?.type === "system_notice_update",
  );
}
function informationalFrame(session: SessionState, fields: Record<string, unknown>) {
  return captureBridgeEvents(() =>
    handleSdkMessage(session, {
      type: "system",
      subtype: "informational",
      session_id: session.sessionId,
      ...fields,
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
  );
}
test("informational visibility publishes scoped supplied content and severity while ordinary info and blank content stay silent", () => {
  for (const row of [
    {
      name: "future-tag",
      level: "notice",
      content: "Independent future-tag notice",
      tag: "future-native-tag",
      prevent: false,
      severity: "info",
      text: "Independent future-tag notice",
    },
    {
      name: "notice",
      level: "notice",
      content: "  Independent notice  ",
      prevent: false,
      severity: "info",
      text: "Independent notice",
    },
    {
      name: "suggestion",
      level: "suggestion",
      content: "Try /compact",
      prevent: false,
      severity: "info",
      text: "Try /compact",
    },
    {
      name: "warning",
      level: "warning",
      content: "Independent warning",
      prevent: false,
      severity: "warning",
      text: "Independent warning",
    },
    {
      name: "ordinary-info",
      level: "info",
      content: "Log-only independent info",
      prevent: false,
      severity: undefined,
      text: undefined,
    },
    {
      name: "blocking-info",
      level: "info",
      content: "Independent blocking explanation",
      prevent: true,
      severity: "warning",
      text: "Independent blocking explanation",
    },
    {
      name: "blank-notice",
      level: "notice",
      content: " \n ",
      prevent: false,
      severity: undefined,
      text: undefined,
    },
  ]) {
    const session = makeSessionState();
    session.sessionId = `information-${row.name}`;
    const events = informationalFrame(session, {
      uuid: `information-source-${row.name}`,
      level: row.level,
      tag: "tag" in row ? row.tag : undefined,
      content: row.content,
      prevent_continuation: row.prevent,
      tool_use_id: "information-tool",
    });
    const notices = noticePublications(events, session.sessionId);
    if (row.severity === undefined) {
      assert.deepEqual(notices, []);
      continue;
    }
    assert.equal(notices.length, 1);
    const update = notices[0]?.update as { type: string; severity: string; message: string };
    assert.equal(update.type, "system_notice_update");
    assert.equal(update.severity, row.severity);
    assert.ok(row.text);
    if (row.level === "suggestion") {
      assert.ok(update.message.includes(row.text));
      assert.match(update.message, /suggest/i);
    } else assert.equal(update.message, row.text);
  }
});

test("informational deduplication distinguishes tool, level, content and session while unscoped repeats remain visible", () => {
  const target = makeSessionState(),
    peer = makeSessionState();
  target.sessionId = "dedup-owner";
  peer.sessionId = "dedup-peer";
  const expected = (sessionId: string, severity: string, message: string) => [
    {
      event: "session_update",
      session_id: sessionId,
      update: { type: "system_notice_update", severity, message },
    },
  ];
  assert.deepEqual(
    noticePublications(
      informationalFrame(target, {
        uuid: "first-notice-source",
        tool_use_id: "dedup-tool-first",
        level: "notice",
        content: "Progress",
      }),
      target.sessionId,
    ),
    expected(target.sessionId, "info", "Progress"),
  );
  assert.deepEqual(
    noticePublications(
      informationalFrame(peer, {
        uuid: "peer-notice-source",
        tool_use_id: "dedup-tool-first",
        level: "notice",
        content: "Progress",
      }),
      peer.sessionId,
    ),
    expected(peer.sessionId, "info", "Progress"),
  );
  assert.deepEqual(
    noticePublications(
      informationalFrame(target, {
        uuid: "normalized-repeat-source",
        tool_use_id: "dedup-tool-first",
        level: "notice",
        content: "  Progress  ",
      }),
      target.sessionId,
    ),
    [],
  );
  assert.deepEqual(
    noticePublications(
      informationalFrame(target, {
        uuid: "changed-content-source",
        tool_use_id: "dedup-tool-first",
        level: "notice",
        content: "Progress changed",
      }),
      target.sessionId,
    ),
    expected(target.sessionId, "info", "Progress changed"),
  );
  assert.deepEqual(
    noticePublications(
      informationalFrame(target, {
        uuid: "changed-tool-source",
        tool_use_id: "dedup-tool-second",
        level: "notice",
        content: "Progress",
      }),
      target.sessionId,
    ),
    expected(target.sessionId, "info", "Progress"),
  );
  assert.deepEqual(
    noticePublications(
      informationalFrame(target, {
        uuid: "changed-level-source",
        tool_use_id: "dedup-tool-first",
        level: "warning",
        content: "Progress",
      }),
      target.sessionId,
    ),
    expected(target.sessionId, "warning", "Progress"),
  );
  for (const uuid of ["unscoped-first", "unscoped-second"])
    assert.deepEqual(
      noticePublications(
        informationalFrame(target, { uuid, level: "notice", content: "Unscoped progress" }),
        target.sessionId,
      ),
      expected(target.sessionId, "info", "Unscoped progress"),
    );
});

test("a blocking informational escalation remains visible after matching silent info and deduplicates only its repeated warning", () => {
  for (const row of [
    { name: "absent", blocking: undefined },
    { name: "explicit-false", blocking: false },
  ]) {
    const session = makeSessionState(),
      peer = makeSessionState();
    session.sessionId = `blocking-${row.name}`;
    peer.sessionId = `blocking-peer-${row.name}`;
    const silent = informationalFrame(session, {
      uuid: "ordinary-info-source",
      tool_use_id: "escalating-tool",
      level: "info",
      content: "INDEPENDENT-BLOCKING-EXPLANATION",
      ...(row.blocking === undefined ? {} : { prevent_continuation: row.blocking }),
    });
    assert.deepEqual(noticePublications(silent, session.sessionId), []);
    const blocking = informationalFrame(session, {
      uuid: "blocking-info-source",
      tool_use_id: "escalating-tool",
      level: "info",
      content: "INDEPENDENT-BLOCKING-EXPLANATION",
      prevent_continuation: true,
    });
    assert.deepEqual(noticePublications(blocking, session.sessionId), [
      {
        event: "session_update",
        session_id: session.sessionId,
        update: {
          type: "system_notice_update",
          severity: "warning",
          message: "INDEPENDENT-BLOCKING-EXPLANATION",
        },
      },
    ]);
    const repeated = informationalFrame(session, {
      uuid: "repeated-blocking-source",
      tool_use_id: "escalating-tool",
      level: "info",
      content: "  INDEPENDENT-BLOCKING-EXPLANATION  ",
      prevent_continuation: true,
    });
    assert.deepEqual(noticePublications(repeated, session.sessionId), []);
    assert.deepEqual(
      noticePublications(
        informationalFrame(peer, {
          uuid: "peer-blocking-source",
          tool_use_id: "escalating-tool",
          level: "info",
          content: "INDEPENDENT-BLOCKING-EXPLANATION",
          prevent_continuation: true,
        }),
        peer.sessionId,
      ),
      [
        {
          event: "session_update",
          session_id: peer.sessionId,
          update: {
            type: "system_notice_update",
            severity: "warning",
            message: "INDEPENDENT-BLOCKING-EXPLANATION",
          },
        },
      ],
    );
  }
});

test("the actual query consumer publishes shutdown only at exhaustion and suppresses preconnect or activity-cancelled warnings", async () => {
  for (const row of [
    { name: "connected", connected: true, activity: false, warning: true },
    { name: "preconnect", connected: false, activity: false, warning: false },
    { name: "later-activity", connected: true, activity: true, warning: false },
  ]) {
    const session = makeSessionState();
    session.sessionId = `shutdown-consumer-${row.name}`;
    session.connected = row.connected;
    sessions.set(session.sessionId, session);
    const initialized = bridgeDeferred<unknown>(),
      afterShutdown = bridgeDeferred<void>(),
      finish = bridgeDeferred<void>();
    const observed: Array<Record<string, unknown>> = [];
    session.query = {
      initializationResult: () => initialized.promise,
      close: () => {
        finish.resolve();
      },
      async *[Symbol.asyncIterator]() {
        for (const uuid of ["shutdown-first", "shutdown-repeat"])
          yield {
            type: "system",
            subtype: "worker_shutting_down",
            reason: "INDEPENDENT-WORKER-CAUSE",
            uuid,
            session_id: session.sessionId,
          };
        afterShutdown.resolve();
        await finish.promise;
        if (row.activity)
          yield {
            type: "system",
            subtype: "informational",
            level: "notice",
            content: "ACTIVITY-AFTER-SHUTDOWN",
            uuid: "activity-after-shutdown",
            session_id: session.sessionId,
          };
      },
    } as unknown as SessionState["query"];
    const events = await captureBridgeEventsAsync(
      async () => {
        try {
          startSessionTasks(session);
          await boundedBridgeTask(afterShutdown.promise);
          assert.deepEqual(noticePublications(observed, session.sessionId), []);
          finish.resolve();
          await boundedBridgeTask(Promise.resolve(session.queryConsumerTask));
        } finally {
          beginSessionClose(session);
          finish.resolve();
          initialized.resolve({
            commands: [],
            agents: [],
            models: [],
            account: { apiKeySource: "fixture" },
          });
          await boundedBridgeTask(
            Promise.allSettled([session.initializationTask, session.queryConsumerTask]),
          );
          await boundedBridgeTask(closeSession(session));
          if (sessions.get(session.sessionId) === session) sessions.delete(session.sessionId);
        }
      },
      (event) => observed.push(event),
    );
    const notices = noticePublications(events, session.sessionId).map(
      (event) => event.update as { severity: string; message: string },
    );
    const shutdown = notices.filter((notice) =>
      notice.message.includes("INDEPENDENT-WORKER-CAUSE"),
    );
    assert.equal(shutdown.length, row.warning ? 1 : 0, row.name);
    if (row.warning) assert.equal(shutdown[0]?.severity, "warning");
    if (row.activity)
      assert.ok(
        notices.some(
          (notice) => notice.severity === "info" && notice.message === "ACTIVITY-AFTER-SHUTDOWN",
        ),
      );
  }
});

test("diagnostic-only SDK messages retain attributed real logs without user output beside a visible warning control", async () => {
  const { spawnSync } = await import("node:child_process");
  const moduleUrl = new URL("./bridge/message_handlers.js", import.meta.url).href;
  const cases = [
    {
      name: "plugin",
      message: {
        subtype: "plugin_install",
        status: "completed",
        name: "INDEPENDENT-INSTALLED-PLUGIN",
      },
    },
    {
      name: "permission",
      message: {
        subtype: "permission_denied",
        tool_name: "Bash",
        tool_use_id: "independent-denied-tool",
        agent_id: "independent-denied-agent",
        decision_reason_type: "rule",
        decision_reason: "INDEPENDENT-DENIAL-RULE",
        message: "INDEPENDENT-DENIAL-DETAIL",
      },
    },
    {
      name: "memory",
      message: {
        subtype: "memory_recall",
        mode: "select",
        memories: [{ id: "memory-first" }, { id: "memory-second" }],
      },
    },
    {
      name: "thinking",
      message: { subtype: "thinking_tokens", estimated_tokens: 120, estimated_tokens_delta: 20 },
    },
    {
      name: "mirror",
      message: {
        subtype: "mirror_error",
        error: "INDEPENDENT-MIRROR-CAUSE",
        key: {
          projectKey: "independent-project-key",
          sessionId: "independent-mirror-session",
          subpath: "owned/subpath",
        },
      },
    },
    { name: "future", message: { subtype: "future-native-telemetry", future_field: true } },
  ];
  const script = `import {handleSdkMessage} from ${JSON.stringify(moduleUrl)};for(const row of ${JSON.stringify(cases)}){const session={sessionId:'log-only-'+row.name,connected:true,informationalDedupKeys:new Set()};handleSdkMessage(session,{type:'system',uuid:'log-only-source-'+row.name,session_id:session.sessionId,...row.message});handleSdkMessage(session,{type:'system',subtype:'informational',level:'warning',content:'VISIBLE-CONTROL-'+row.name,uuid:'visible-source-'+row.name,session_id:session.sessionId});}`;
  const child = spawnSync(process.execPath, ["--input-type=module", "-e", script], {
    encoding: "utf8",
    windowsHide: true,
    timeout: 15000,
    env: { ...process.env, CLAUDE_RS_BRIDGE_DIAGNOSTICS: "1" },
  });
  assert.equal(child.error, undefined);
  assert.equal(child.status, 0, child.stderr);
  const protocol = child.stdout
    .split("\n")
    .filter((line) => line.trim())
    .map((line) => JSON.parse(line) as Record<string, unknown>);
  assert.deepEqual(
    protocol,
    cases.map((row) => ({
      event: "session_update",
      session_id: `log-only-${row.name}`,
      update: {
        type: "system_notice_update",
        severity: "warning",
        message: `VISIBLE-CONTROL-${row.name}`,
      },
    })),
  );
  const logs = child.stderr
    .split("\n")
    .filter((line) => line.trim())
    .map(
      (line) =>
        JSON.parse(line) as {
          schema: string;
          event_name: string;
          level: string;
          session_id: string;
          tool_call_id?: string;
          fields: Record<string, unknown>;
        },
    );
  const record = (event: string, sessionId: string) => {
    const row = logs.find((log) => log.event_name === event && log.session_id === sessionId);
    assert.ok(row, `${sessionId}:${event}`);
    assert.equal(row.schema, "claude-rs-log/v1");
    return row;
  };
  const plugin = record("sdk_plugin_install_received", "log-only-plugin");
  assert.equal(plugin.level, "info");
  assert.equal(plugin.fields.plugin_status, "completed");
  assert.equal(plugin.fields.plugin_name, "INDEPENDENT-INSTALLED-PLUGIN");
  const permission = record("sdk_permission_denied_received", "log-only-permission");
  assert.equal(permission.level, "info");
  assert.equal(permission.tool_call_id, "independent-denied-tool");
  assert.equal(permission.fields.tool_name, "Bash");
  assert.equal(permission.fields.agent_id, "independent-denied-agent");
  assert.equal(permission.fields.decision_reason_type, "rule");
  assert.equal(permission.fields.decision_reason, "INDEPENDENT-DENIAL-RULE");
  assert.equal(permission.fields.denial_message, "INDEPENDENT-DENIAL-DETAIL");
  const memory = record("sdk_system_message_log_only", "log-only-memory");
  assert.equal(memory.level, "debug");
  assert.equal(memory.fields.sdk_subtype, "memory_recall");
  assert.equal(memory.fields.memory_count, 2);
  const thinking = record("sdk_system_message_log_only", "log-only-thinking");
  assert.equal(thinking.level, "debug");
  assert.equal(thinking.fields.sdk_subtype, "thinking_tokens");
  assert.equal(thinking.fields.estimated_tokens, 120);
  assert.equal(thinking.fields.estimated_tokens_delta, 20);
  const mirror = record("sdk_mirror_error_received", "log-only-mirror");
  assert.equal(mirror.level, "warn");
  assert.equal(mirror.fields.error_message, "INDEPENDENT-MIRROR-CAUSE");
  assert.equal(mirror.fields.project_key, "independent-project-key");
  assert.equal(mirror.fields.mirror_session_id, "independent-mirror-session");
  assert.equal(mirror.fields.subpath, "owned/subpath");
});

test("peer input publishes decoded body and associated provenance for recognized and future modes without exposing its raw envelope", () => {
  for (const row of [
    { name: "bypass", mode: "bypass", expected: "bypass" },
    { name: "prompting", mode: "prompting", expected: "prompting" },
    { name: "absent", mode: undefined, expected: undefined },
    { name: "future", mode: "future-native-mode", expected: undefined },
  ]) {
    const session = makeSessionState();
    session.sessionId = `peer-origin-${row.name}`;
    const events = captureBridgeEvents(() =>
      handleSdkMessage(session, {
        type: "user",
        uuid: `peer-message-${row.name}`,
        session_id: session.sessionId,
        parent_tool_use_id: null,
        origin: {
          kind: "peer",
          from: "independent-reported-route",
          name: "Independent build session",
          fromSession: "independent-sender-session",
          body: "DECODED-PEER-BODY\n exact  ",
          verifiedPeerPid: 4242,
          ...(row.mode === undefined ? {} : { fromMode: row.mode }),
        },
        message: { role: "user", content: "RAW-PEER-ENVELOPE-MUST-NOT-RENDER" },
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    assert.deepEqual(events, [
      {
        event: "session_update",
        session_id: session.sessionId,
        update: {
          type: "external_message_update",
          content: "DECODED-PEER-BODY\n exact  ",
          source_message_uuid: `peer-message-${row.name}`,
          origin: {
            kind: "peer",
            from: "independent-reported-route",
            name: "Independent build session",
            from_session: "independent-sender-session",
            verified_peer_pid: 4242,
            ...(row.expected === undefined ? {} : { from_mode: row.expected }),
          },
        },
      },
    ]);
    assert.doesNotMatch(JSON.stringify(events), /RAW-PEER-ENVELOPE-MUST-NOT-RENDER/);
    const auto = captureBridgeEvents(() =>
      handleSdkMessage(session, {
        type: "user",
        uuid: "auto-continuation-source",
        session_id: session.sessionId,
        origin: { kind: "auto_continuation" },
        message: { role: "user", content: "AUTO-CONTINUATION-NOT-HUMAN-INPUT" },
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    assert.deepEqual(auto, []);
    const human = captureBridgeEvents(() =>
      handleSdkMessage(session, {
        type: "stream_event",
        uuid: "human-control-source",
        session_id: session.sessionId,
        parent_tool_use_id: null,
        user_message_uuid: "independent-human-submission",
        event: {
          type: "content_block_delta",
          delta: { type: "text_delta", text: "HUMAN-CONTROL-TEXT" },
        },
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    assert.deepEqual(
      human.filter((event) => event.event === "user_message_started"),
      [
        {
          event: "user_message_started",
          session_id: session.sessionId,
          message_uuid: "independent-human-submission",
          source: "stream_event",
        },
      ],
    );
    assert.ok(
      human.some(
        (event) =>
          event.session_id === session.sessionId &&
          (event.update as { content?: { text?: string } } | undefined)?.content?.text ===
            "HUMAN-CONTROL-TEXT",
      ),
    );
  }
});

test("handleSdkMessage distinguishes peer-send task notifications from human prompts", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "user",
      uuid: "task-peer-1",
      session_id: "session-1",
      parent_tool_use_id: null,
      origin: { kind: "task-notification", subkind: "peer-send-message" },
      message: {
        role: "user",
        content: [{ type: "text", text: "Peer task body" }],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.equal(events.length, 1);
  assert.deepEqual(events[0]?.update, {
    type: "external_message_update",
    content: "Peer task body",
    source_message_uuid: "task-peer-1",
    origin: { kind: "task-notification", subkind: "peer-send-message" },
  });
});

test("assistant correlation metadata and source UUID stay on their own root or child tool without leaking to another message", () => {
  const session = makeSessionState();
  session.sessionId = "assistant-correlation-owner";
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "parent-agent-source",
      session_id: session.sessionId,
      parent_tool_use_id: null,
      message: {
        content: [
          {
            type: "tool_use",
            id: "parent-agent-tool",
            name: "Agent",
            input: { prompt: "Parent independent work" },
          },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "root-owned-source",
      session_id: session.sessionId,
      parent_tool_use_id: null,
      request_id: "root-owned-request",
      subagent_type: "root-reviewer",
      task_description: "ROOT-OWNED-DESCRIPTION",
      message: {
        content: [
          {
            type: "tool_use",
            id: "root-owned-tool",
            name: "Bash",
            input: { command: "printf root" },
          },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "child-owned-source",
      session_id: session.sessionId,
      parent_tool_use_id: "parent-agent-tool",
      request_id: "child-owned-request",
      subagent_type: "child-reviewer",
      task_description: "CHILD-OWNED-DESCRIPTION",
      message: {
        content: [
          {
            type: "tool_use",
            id: "child-owned-tool",
            name: "Read",
            input: { file_path: "child-owned.txt" },
          },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "uncorrelated-source",
      session_id: session.sessionId,
      parent_tool_use_id: null,
      message: {
        content: [
          {
            type: "tool_use",
            id: "uncorrelated-tool",
            name: "Read",
            input: { file_path: "unrelated.txt" },
          },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });
  const root = publishedTool(events, session.sessionId, "root-owned-tool"),
    child = publishedTool(events, session.sessionId, "child-owned-tool"),
    unrelated = publishedTool(events, session.sessionId, "uncorrelated-tool");
  assert.equal(root.source_message_uuid, "root-owned-source");
  assert.equal(child.source_message_uuid, "child-owned-source");
  assert.equal(unrelated.source_message_uuid, "uncorrelated-source");
  assert.deepEqual((root.meta as { claudeCode: unknown }).claudeCode, {
    toolName: "Bash",
    parentToolUseId: null,
    requestId: "root-owned-request",
    subagentType: "root-reviewer",
    taskDescription: "ROOT-OWNED-DESCRIPTION",
  });
  assert.deepEqual((child.meta as { claudeCode: unknown }).claudeCode, {
    toolName: "Read",
    parentToolUseId: "parent-agent-tool",
    requestId: "child-owned-request",
    subagentType: "child-reviewer",
    taskDescription: "CHILD-OWNED-DESCRIPTION",
  });
  assert.deepEqual((unrelated.meta as { claudeCode: unknown }).claudeCode, {
    toolName: "Read",
    parentToolUseId: null,
  });
});

test("an unlinked ambient execution advances inventory through completion without fabricating tools or changing an unrelated invocation", () => {
  const session = makeSessionState();
  session.sessionId = "ambient-inventory-owner";
  const initial = taskOperationStart(session, "unrelated-visible-tool", "Read", {
    file_path: "unrelated.txt",
  });
  const stages = [
    {
      subtype: "task_started",
      message: {
        description: "AMBIENT-START-DESCRIPTION",
        task_type: "artifact_watch",
        ambient: true,
      },
      status: "in_progress",
      description: "AMBIENT-START-DESCRIPTION",
    },
    {
      subtype: "task_progress",
      message: { summary: "AMBIENT-PROGRESS-SUMMARY" },
      status: "in_progress",
      description: "AMBIENT-PROGRESS-SUMMARY",
    },
    {
      subtype: "task_notification",
      message: {
        status: "completed",
        summary: "AMBIENT-FINAL-SUMMARY",
        output_file: "C:/owned/ambient.output",
      },
      status: "completed",
      description: "AMBIENT-FINAL-SUMMARY",
    },
  ];
  let all = [...initial];
  for (const stage of stages) {
    const events = captureBridgeEvents(() =>
      handleSdkMessage(session, {
        type: "system",
        subtype: stage.subtype,
        task_id: "ambient-owned-task",
        uuid: `ambient-${stage.subtype}-source`,
        session_id: session.sessionId,
        ...stage.message,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    all = [...all, ...events];
    const rows = publishedTaskRows(events, session.sessionId, "ambient-owned-task");
    assert.equal(rows.length, 1);
    const task = rows[0];
    assert.ok(task);
    assert.equal(task.status, stage.status);
    assert.equal(task.description, stage.description);
    assert.equal(task.source_tool_call_id, undefined);
    assert.equal((task.metadata as Record<string, unknown>).ambient, true);
    assert.equal((task.metadata as Record<string, unknown>).task_type, "artifact_watch");
    if (stage.status === "completed") {
      assert.equal((task.metadata as Record<string, unknown>).terminal_status, "completed");
      assert.equal(
        (task.metadata as Record<string, unknown>).output_file,
        "C:/owned/ambient.output",
      );
    }
    assert.equal(
      events.some((event) =>
        ["tool_call", "tool_call_update"].includes((event.update as { type: string }).type),
      ),
      false,
    );
    assert.deepEqual(
      publishedTool(all, session.sessionId, "unrelated-visible-tool"),
      publishedTool(initial, session.sessionId, "unrelated-visible-tool"),
    );
  }
  const linked = taskOperationStart(session, "linked-ambient-control", "Agent", {
    prompt: "Real linked ambient work",
  });
  const progress = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "task_started",
      task_id: "linked-ambient-task",
      tool_use_id: "linked-ambient-control",
      ambient: true,
      task_type: "local_agent",
      description: "Linked ambient control",
      uuid: "linked-ambient-start",
      session_id: session.sessionId,
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "task_progress",
      task_id: "linked-ambient-task",
      summary: "LINKED-AMBIENT-PROGRESS",
      uuid: "linked-ambient-progress",
      session_id: session.sessionId,
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });
  assert.equal(
    publishedTool([...linked, ...progress], session.sessionId, "linked-ambient-control").raw_output,
    "LINKED-AMBIENT-PROGRESS",
  );
  assert.ok(
    publishedTaskRows(progress, session.sessionId, "linked-ambient-task").every(
      (task) => task.source_tool_call_id === "linked-ambient-control",
    ),
  );
});

async function dispatchControl(wire: Record<string, unknown>): Promise<void> {
  const parsed = parseCommandEnvelope(JSON.stringify(wire));
  await handleSessionControlCommand(
    parsed.command as Parameters<typeof handleSessionControlCommand>[0],
    parsed.requestId,
    promptControlDeps(),
  );
}
function configPublications(events: Array<Record<string, unknown>>, option: string) {
  return events.filter(
    (event) =>
      (event.update as { type?: string; option_id?: string } | undefined)?.type ===
        "config_option_update" && (event.update as { option_id?: string }).option_id === option,
  );
}
function controlModel(session: SessionState): void {
  session.availableModels = [
    {
      id: "haiku",
      display_name: "Independent applied model",
      supports_effort: true,
      supported_effort_levels: ["low", "medium", "high", "xhigh", "max"],
      supports_fast_mode: true,
    },
  ];
  refreshCurrentModel(session);
}
test("effort commands apply the target native flags and publish independently observed levels including reset, refusal and recovery", async () => {
  await withCommandTargets(async (target, peer) => {
    controlModel(target);
    target.ultracode = { available: true, requested: true, effective: true };
    let applied = "medium",
      reject = false;
    const calls: unknown[] = [];
    const native = {
      close: () => {},
      async applyFlagSettings(flags: { effortLevel: string | null; ultracode?: boolean }) {
        assert.equal(this, native);
        calls.push(flags);
        if (reject) throw new Error("INDEPENDENT-EFFORT-APPLY-CAUSE");
        applied = flags.effortLevel ?? "low";
      },
      async getSettings() {
        assert.equal(this, native);
        return {
          applied: {
            effort: applied,
            model: "haiku",
            ultracodeAvailable: true,
            ultracodeRequested: true,
            ultracode: true,
          },
        };
      },
    };
    target.query = native as unknown as SessionState["query"];
    for (const row of [
      { effort: "xhigh", observed: "xhigh" },
      { effort: "max", observed: "max" },
      { effort: null, observed: "low" },
    ]) {
      const events = await captureBridgeEventsAsync(() =>
        dispatchControl({
          command: "set_effort",
          session_id: target.sessionId,
          request_id: `effort-${row.effort}`,
          effort: row.effort,
        }),
      );
      assert.deepEqual(calls.at(-1), { effortLevel: row.effort, ultracode: true });
      assert.deepEqual(configPublications(events, "effortLevel"), [
        {
          event: "session_update",
          session_id: target.sessionId,
          update: { type: "config_option_update", option_id: "effortLevel", value: row.observed },
        },
      ]);
      assert.equal(
        events.some((event) => event.session_id === peer.sessionId),
        false,
      );
      assert.equal(
        events.some((event) => event.event === "slash_error"),
        false,
      );
    }
    const before = calls.length;
    target.currentModel = { ...target.currentModel!, supported_effort_levels: ["low"] };
    const refused = await captureBridgeEventsAsync(() =>
      dispatchControl({
        command: "set_effort",
        session_id: target.sessionId,
        request_id: "effort-unsupported",
        effort: "max",
      }),
    );
    assert.equal(calls.length, before);
    assert.ok(
      refused.some(
        (event) =>
          event.event === "slash_error" &&
          event.session_id === target.sessionId &&
          event.request_id === "effort-unsupported",
      ),
    );
    controlModel(target);
    reject = true;
    const failed = await captureBridgeEventsAsync(() =>
      dispatchControl({
        command: "set_effort",
        session_id: target.sessionId,
        request_id: "effort-failed",
        effort: "max",
      }),
    );
    assert.ok(
      failed.some(
        (event) =>
          event.event === "slash_error" &&
          event.session_id === target.sessionId &&
          event.request_id === "effort-failed" &&
          String(event.message).includes("INDEPENDENT-EFFORT-APPLY-CAUSE"),
      ),
    );
    assert.deepEqual(
      configPublications(failed, "effortLevel").map(
        (event) => (event.update as { value: unknown }).value,
      ),
      ["low"],
    );
    reject = false;
    const recovered = await captureBridgeEventsAsync(() =>
      dispatchControl({
        command: "set_effort",
        session_id: target.sessionId,
        request_id: "effort-recovered",
        effort: "high",
      }),
    );
    assert.deepEqual(
      configPublications(recovered, "effortLevel").map(
        (event) => (event.update as { value: unknown }).value,
      ),
      ["high"],
    );
  });
});

test("fast mode commands verify only after accepted native application and publish truthful success, failure and recovery states", async () => {
  for (const row of [
    {
      name: "on",
      enabled: true,
      response: { fast_mode_state: "on", fast_mode_disabled_reason: "pending" },
      expected: { state: "on", disabled_reason: "pending" },
    },
    {
      name: "cooldown",
      enabled: true,
      response: { fast_mode_state: "cooldown" },
      expected: { state: "cooldown" },
    },
    {
      name: "off",
      enabled: false,
      response: { fast_mode_state: "off" },
      expected: { state: "off" },
    },
    { name: "sparse-off", enabled: false, response: {}, expected: { state: "off" } },
    {
      name: "disagrees",
      enabled: true,
      response: { fast_mode_state: "off", fast_mode_disabled_reason: "model_not_allowed" },
      expected: { state: "off", disabled_reason: "model_not_allowed" },
      cause: "off",
    },
    {
      name: "apply-rejected",
      enabled: true,
      response: { fast_mode_state: "on" },
      expected: undefined,
      cause: "INDEPENDENT-FAST-APPLY-CAUSE",
    },
    {
      name: "read-rejected",
      enabled: true,
      response: { fast_mode_state: "on" },
      expected: { state: "unknown" },
      cause: "INDEPENDENT-FAST-READ-CAUSE",
    },
    {
      name: "missing-enabled",
      enabled: true,
      response: {},
      expected: { state: "unknown" },
      cause: "resulting state",
    },
  ])
    await withCommandTargets(async (target, peer) => {
      controlModel(target);
      observeFast(target, {
        fast_mode_state: "cooldown",
        fast_mode_disabled_reason: "prior-independent-reason",
      });
      const entered = bridgeDeferred<void>(),
        release = bridgeDeferred<void>();
      let recovery = false,
        accepted = false,
        reads = 0;
      const calls: unknown[] = [];
      const native = {
        close: () => {},
        async applyFlagSettings(flags: unknown) {
          assert.equal(this, native);
          calls.push(flags);
          if (!recovery) {
            entered.resolve();
            await release.promise;
            if (row.name === "apply-rejected") throw new Error("INDEPENDENT-FAST-APPLY-CAUSE");
          }
          accepted = true;
        },
        async reinitialize() {
          assert.equal(this, native);
          assert.equal(accepted, true, "verification follows accepted application");
          reads++;
          if (recovery)
            return {
              fast_mode_state: "on",
              fast_mode_disabled_reason: "recovered-independent-reason",
            };
          if (row.name === "read-rejected") throw new Error("INDEPENDENT-FAST-READ-CAUSE");
          return row.response;
        },
        async getSettings() {
          return { applied: { effort: "medium" } };
        },
      };
      target.query = native as unknown as SessionState["query"];
      const request = `fast-${row.name}`;
      const events = await captureBridgeEventsAsync(async () => {
        const changing = dispatchControl({
          command: "set_fast_mode",
          session_id: target.sessionId,
          request_id: request,
          enabled: row.enabled,
        });
        try {
          await boundedBridgeTask(entered.promise);
          assert.equal(reads, 0);
          assert.deepEqual(calls, [{ fastMode: row.enabled }]);
          release.resolve();
          await boundedBridgeTask(changing);
        } finally {
          release.resolve();
          await boundedBridgeTask(changing);
        }
      });
      assert.deepEqual(
        fastPublications(events).map((event) => ({
          session: event.session_id,
          snapshot: (() => {
            const update = event.update as {
              fast_mode_state: string;
              fast_mode_disabled_reason?: string;
            };
            return {
              state: update.fast_mode_state,
              ...(update.fast_mode_disabled_reason
                ? { disabled_reason: update.fast_mode_disabled_reason }
                : {}),
            };
          })(),
        })),
        row.expected ? [{ session: target.sessionId, snapshot: row.expected }] : [],
      );
      if (row.name === "apply-rejected") {
        assert.equal(reads, 0);
        assert.equal(target.fastModeState, "cooldown");
        assert.equal(target.fastModeDisabledReason, "prior-independent-reason");
      }
      if (row.cause)
        assert.ok(
          events.some(
            (event) =>
              event.event === "slash_error" &&
              event.session_id === target.sessionId &&
              event.request_id === request &&
              String(event.message).includes(row.cause),
          ),
        );
      else
        assert.equal(
          events.some((event) => event.event === "slash_error"),
          false,
        );
      assert.equal(
        events.some((event) => event.session_id === peer.sessionId),
        false,
      );
      recovery = true;
      const recovered = await captureBridgeEventsAsync(() =>
        dispatchControl({
          command: "set_fast_mode",
          session_id: target.sessionId,
          request_id: `${request}-recovered`,
          enabled: true,
        }),
      );
      assert.deepEqual(calls.at(-1), { fastMode: true });
      assert.deepEqual(
        fastPublications(recovered).map((event) => ({
          session: event.session_id,
          snapshot: (() => {
            const update = event.update as {
              fast_mode_state: string;
              fast_mode_disabled_reason?: string;
            };
            return {
              state: update.fast_mode_state,
              ...(update.fast_mode_disabled_reason
                ? { disabled_reason: update.fast_mode_disabled_reason }
                : {}),
            };
          })(),
        })),
        [
          {
            session: target.sessionId,
            snapshot: { state: "on", disabled_reason: "recovered-independent-reason" },
          },
        ],
      );
      assert.equal(
        recovered.some((event) => event.event === "slash_error"),
        false,
      );
    });
});

test("agent commands acknowledge only accepted target switch or reset and refresh real controls through rejection and recovery", async () => {
  await withCommandTargets(async (target, peer) => {
    controlModel(target);
    target.availableAgents = [
      { name: "reviewer", description: "Independent reviewer" },
      { name: "recovery", description: "Independent recovery" },
    ];
    const calls: unknown[] = [];
    let gate: ReturnType<typeof bridgeDeferred<void>> | undefined,
      entered: ReturnType<typeof bridgeDeferred<void>> | undefined,
      reject = false,
      accepted = false;
    const native = {
      close: () => {},
      async applyFlagSettings(flags: unknown) {
        assert.equal(this, native);
        calls.push(flags);
        entered?.resolve();
        if (gate) await gate.promise;
        if (reject) throw new Error("INDEPENDENT-AGENT-APPLY-CAUSE");
        accepted = true;
      },
      async getSettings() {
        assert.equal(this, native);
        return {
          applied: {
            model: "haiku",
            effort: "high",
            ultracodeAvailable: true,
            ultracodeRequested: false,
            ultracode: false,
          },
        };
      },
    };
    target.query = native as unknown as SessionState["query"];
    for (const agent of ["reviewer", null]) {
      let premature = false;
      accepted = false;
      gate = bridgeDeferred<void>();
      entered = bridgeDeferred<void>();
      let running: Promise<void> | undefined;
      const events = await captureBridgeEventsAsync(
        async () => {
          try {
            running = dispatchControl({
              command: "set_agent",
              session_id: target.sessionId,
              request_id: `agent-${agent}`,
              agent,
            });
            await boundedBridgeTask(entered!.promise);
            assert.deepEqual(calls.at(-1), { agent });
            gate!.resolve();
            await boundedBridgeTask(running);
          } finally {
            gate!.resolve();
            if (running) await boundedBridgeTask(running);
          }
        },
        (event) => {
          if (!gate) return;
          if ((event.update as { option_id?: string } | undefined)?.option_id === "agent")
            if (!accepted) premature = true;
        },
      );
      assert.equal(premature, false, "agent acknowledgement follows native acceptance");
      assert.deepEqual(configPublications(events, "agent"), [
        {
          event: "session_update",
          session_id: target.sessionId,
          update: { type: "config_option_update", option_id: "agent", value: agent },
        },
      ]);
      assert.ok(
        events.some(
          (event) =>
            event.session_id === target.sessionId &&
            (event.update as { type?: string; current_model?: { resolved_id: string } } | undefined)
              ?.type === "current_model_update" &&
            (event.update as { current_model: { resolved_id: string } }).current_model
              .resolved_id === "haiku",
        ),
      );
      assert.deepEqual(
        configPublications(events, "effortLevel").map(
          (event) => (event.update as { value: unknown }).value,
        ),
        ["high"],
      );
      assert.ok(
        events.some(
          (event) =>
            event.session_id === target.sessionId &&
            (event.update as { type?: string; ultracode?: unknown } | undefined)?.type ===
              "ultracode_update",
        ),
      );
      assert.equal(
        events.some((event) => event.session_id === peer.sessionId),
        false,
      );
    }
    gate = undefined;
    entered = undefined;
    const before = calls.length;
    const unavailable = await captureBridgeEventsAsync(() =>
      dispatchControl({
        command: "set_agent",
        session_id: target.sessionId,
        request_id: "agent-unavailable",
        agent: "missing-agent",
      }),
    );
    assert.equal(calls.length, before);
    assert.deepEqual(configPublications(unavailable, "agent"), []);
    assert.ok(
      unavailable.some(
        (event) =>
          event.event === "slash_error" &&
          event.request_id === "agent-unavailable" &&
          event.session_id === target.sessionId,
      ),
    );
    reject = true;
    const failed = await captureBridgeEventsAsync(() =>
      dispatchControl({
        command: "set_agent",
        session_id: target.sessionId,
        request_id: "agent-failed",
        agent: "reviewer",
      }),
    );
    assert.deepEqual(configPublications(failed, "agent"), []);
    assert.ok(
      failed.some(
        (event) =>
          event.event === "slash_error" &&
          event.session_id === target.sessionId &&
          event.request_id === "agent-failed" &&
          String(event.message).includes("INDEPENDENT-AGENT-APPLY-CAUSE"),
      ),
    );
    reject = false;
    const recovered = await captureBridgeEventsAsync(() =>
      dispatchControl({
        command: "set_agent",
        session_id: target.sessionId,
        request_id: "agent-recovered",
        agent: "recovery",
      }),
    );
    assert.deepEqual(calls.at(-1), { agent: "recovery" });
    assert.deepEqual(
      configPublications(recovered, "agent").map((event) => ({
        session: event.session_id,
        value: (event.update as { value: unknown }).value,
      })),
      [{ session: target.sessionId, value: "recovery" }],
    );
  });
});

test("in-flight agent commands cannot acknowledge an obsolete session lifetime and a replacement accepts its own independent agent", async () => {
  for (const lifetime of ["replacement", "close"])
    for (const outcome of ["success", "rejection"]) {
      await withCommandTargets(async (target, peer) => {
        const originalId = target.sessionId,
          request = `agent-lifetime-${lifetime}-${outcome}`;
        const fixture = commandInitQuery(target, async () => []);
        controlModel(target);
        target.availableAgents = [{ name: "obsolete-agent", description: "Old lifetime choice" }];
        const entered = bridgeDeferred<void>(),
          release = bridgeDeferred<void>();
        let recovering = false;
        const calls: unknown[] = [];
        Object.assign(target.query, {
          async applyFlagSettings(flags: unknown) {
            assert.equal(this, target.query);
            calls.push(flags);
            if (!recovering) {
              entered.resolve();
              await release.promise;
              if (outcome === "rejection") throw new Error("OBSOLETE-AGENT-NATIVE-CAUSE");
            }
          },
        });
        let pending: Promise<void> | undefined;
        const obsolete = await captureBridgeEventsAsync(async () => {
          try {
            pending = dispatchControl({
              command: "set_agent",
              session_id: originalId,
              request_id: request,
              agent: "obsolete-agent",
            });
            await boundedBridgeTask(entered.promise);
            assert.deepEqual(calls, [{ agent: "obsolete-agent" }]);
            if (lifetime === "replacement") {
              handleSdkMessage(target, {
                type: "system",
                subtype: "init",
                uuid: "replacement-agent-source",
                session_id: `replacement-${originalId}`,
                model: "haiku",
                agents: ["replacement-agent"],
                effort: "medium",
              } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
              await fixture.settle();
            } else await boundedBridgeTask(closeSession(target));
            release.resolve();
            await boundedBridgeTask(pending);
            await fixture.settle();
          } finally {
            release.resolve();
            if (pending) await boundedBridgeTask(pending);
            await fixture.settle();
          }
        });
        assert.deepEqual(
          configPublications(obsolete, "agent"),
          [],
          "old mutation cannot publish an agent acknowledgement after its lifetime ended",
        );
        for (const event of obsolete.filter((event) => event.event === "slash_error")) {
          assert.equal(event.session_id, originalId);
          assert.equal(event.request_id, request);
        }
        if (lifetime === "replacement") {
          recovering = true;
          target.availableAgents = [
            { name: "replacement-agent", description: "New lifetime choice" },
          ];
          const recovered = await captureBridgeEventsAsync(async () => {
            await dispatchControl({
              command: "set_agent",
              session_id: target.sessionId,
              request_id: "replacement-agent-request",
              agent: "replacement-agent",
            });
            await fixture.settle();
          });
          assert.deepEqual(calls.at(-1), { agent: "replacement-agent" });
          assert.deepEqual(configPublications(recovered, "agent"), [
            {
              event: "session_update",
              session_id: target.sessionId,
              update: {
                type: "config_option_update",
                option_id: "agent",
                value: "replacement-agent",
              },
            },
          ]);
          assert.equal(
            recovered.some(
              (event) => event.event === "slash_error" || event.session_id === peer.sessionId,
            ),
            false,
          );
        }
      });
    }
});

test("SDK effort observations and explicit null defeat a held older read while an uncontested control still publishes", async () => {
  const session = makeSessionState();
  session.sessionId = "effort-observation-owner";
  const fixture = commandInitQuery(session, async () => []);
  const native = session.query as unknown as { getSettings: () => Promise<unknown> };
  const original = native.getSettings;
  const entered = bridgeDeferred<void>(),
    release = bridgeDeferred<{ applied: { effort: string } }>();
  let held = true;
  native.getSettings = async () => {
    if (held) {
      held = false;
      entered.resolve();
      return release.promise;
    }
    return original.call(native);
  };
  let reading: Promise<boolean> | undefined;
  const events = await captureBridgeEventsAsync(async () => {
    try {
      reading = refreshSessionEffort(session);
      await boundedBridgeTask(entered.promise);
      handleSdkMessage(session, {
        type: "system",
        subtype: "init",
        session_id: session.sessionId,
        model: "haiku",
        effort: "xhigh",
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      await fixture.settle();
      release.resolve({ applied: { effort: "high" } });
      await boundedBridgeTask(reading);
      handleSdkMessage(session, {
        type: "system",
        subtype: "init",
        session_id: session.sessionId,
        model: "haiku",
        effort: null,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      await fixture.settle();
      await refreshSessionEffort(session);
    } finally {
      release.resolve({ applied: { effort: "high" } });
      if (reading) await boundedBridgeTask(reading);
      await fixture.settle();
      session.input.close();
    }
  });
  assert.deepEqual(
    configPublications(events, "effortLevel"),
    ["xhigh", null, "medium"].map((value) => ({
      event: "session_update",
      session_id: session.sessionId,
      update: { type: "config_option_update", option_id: "effortLevel", value },
    })),
  );
});

test("real initialization publishes provider-aware login hints only without first-party credential evidence", async () => {
  for (const row of [
    { name: "legacy-missing", account: {}, hint: true },
    { name: "first-party-missing", account: { apiProvider: "firstParty" }, hint: true },
    {
      name: "blank-credentials",
      account: { apiProvider: "firstParty", email: " \t", apiKeySource: " " },
      hint: true,
    },
    {
      name: "email",
      account: { apiProvider: "firstParty", email: " independent@example.test " },
      hint: false,
    },
    {
      name: "key",
      account: { apiProvider: "firstParty", apiKeySource: "independent-key-source" },
      hint: false,
    },
    { name: "bedrock", account: { apiProvider: "bedrock" }, hint: false },
    { name: "gateway", account: { apiProvider: "gateway" }, hint: false },
    {
      name: "future-external",
      account: { apiProvider: "future-independent-provider" },
      hint: false,
    },
  ]) {
    const session = makeSessionState();
    session.sessionId = `startup-account-${row.name}`;
    session.connected = false;
    const frames = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKMessage>();
    const fixture = commandInitQuery(session, async () => []);
    Object.assign(session.query, {
      initializationResult: async () => ({
        models: [],
        agents: [],
        commands: [],
        account: row.account,
      }),
      [Symbol.asyncIterator]: () => frames[Symbol.asyncIterator](),
      close: () => frames.close(),
    });
    const events = await captureBridgeEventsAsync(async () => {
      try {
        startSessionTasks(session);
        await boundedBridgeTask(Promise.resolve(session.initializationTask));
        await fixture.settle();
      } finally {
        frames.close();
        await boundedBridgeTask(
          Promise.allSettled([session.initializationTask, session.queryConsumerTask]),
        );
        await fixture.settle();
        session.input.close();
      }
    });
    assert.equal(
      events.filter(
        (event) => event.event === "connected" && event.session_id === session.sessionId,
      ).length,
      1,
      row.name,
    );
    const hints = events.filter((event) => event.event === "auth_required");
    assert.equal(hints.length, row.hint ? 1 : 0, row.name);
    for (const hint of hints) {
      assert.equal(hint.session_id, undefined);
      assert.ok(String(hint.message).trim().length > 0);
    }
  }
});

test("parsed status requests publish the target account DTO with independent normalized metadata and optional-field omission", async () => {
  for (const row of [
    {
      name: "complete",
      input: {
        email: " independent@example.test ",
        organization: " independent-org ",
        subscriptionType: " Independent Plan ",
        tokenSource: " independent-token ",
        apiKeySource: " independent-key ",
        apiProvider: "gateway",
      },
      expected: {
        email: "independent@example.test",
        organization: "independent-org",
        subscription_type: "Independent Plan",
        token_source: "independent-token",
        api_key_source: "independent-key",
        api_provider: "gateway",
      },
    },
    {
      name: "future",
      input: { apiProvider: " future-independent-provider ", email: "  ", apiKeySource: null },
      expected: { api_provider: "future-independent-provider" },
    },
    { name: "absent", input: {}, expected: {} },
  ])
    await withCommandTargets(async (target, peer) => {
      const native = {
        close: () => {},
        async accountInfo() {
          assert.equal(this, native);
          return row.input;
        },
      };
      target.query = native as unknown as SessionState["query"];
      const request = `status-${row.name}`;
      const events = await dispatchData(target, "get_status_snapshot", request);
      assert.deepEqual(events, [
        {
          event: "status_snapshot",
          session_id: target.sessionId,
          request_id: request,
          account: row.expected,
        },
      ]);
      assert.equal(
        events.some((event) => event.session_id === peer.sessionId),
        false,
      );
    });
});

test("settings errors retain valid supplied cause and path through direct and init ingress without inventing malformed siblings", async () => {
  const session = makeSessionState();
  session.sessionId = "settings-error-owner";
  const fixture = commandInitQuery(session, async () => []);
  const rows = [
    {
      input: {
        file: "C:/independent/settings.json",
        path: "permissions.allow",
        message: " INDEPENDENT-ARRAY-CAUSE ",
      },
      expected: {
        file: "C:/independent/settings.json",
        path: "permissions.allow",
        message: "INDEPENDENT-ARRAY-CAUSE",
      },
    },
    {
      input: { path: "", message: "INDEPENDENT-FILELESS-CAUSE" },
      expected: { path: "", message: "INDEPENDENT-FILELESS-CAUSE" },
    },
    { input: { file: "C:/invalid.json", path: "unusable", message: " " }, expected: null },
    { input: { path: "missing-message" }, expected: null },
    { input: { message: 7 }, expected: null },
  ];
  const events = await captureBridgeEventsAsync(async () => {
    for (const row of rows)
      handleSdkMessage(session, {
        type: "settings_parse_error",
        ...row.input,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "init",
      session_id: session.sessionId,
      model: "haiku",
      settings_errors: [rows[0]!.input, "malformed-root", rows[3]!.input, rows[1]!.input],
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    await fixture.settle();
  });
  const expected = [rows[0]!.expected, rows[1]!.expected, rows[0]!.expected, rows[1]!.expected];
  assert.deepEqual(
    events.filter(
      (event) => (event.update as { type?: string } | undefined)?.type === "settings_parse_error",
    ),
    expected.map((error) => ({
      event: "session_update",
      session_id: session.sessionId,
      update: { type: "settings_parse_error", ...error },
    })),
  );
  session.input.close();
});

test("advertised agent normalization publishes useful duplicate enrichment by identity and a real empty replacement without mutating inputs", () => {
  const session = makeSessionState();
  session.sessionId = "agent-normalization-owner";
  const input = Object.freeze([
    Object.freeze({ name: " reviewer ", description: "", model: "" }),
    Object.freeze({
      name: "reviewer",
      description: "Independent review description",
      model: "haiku",
    }),
    Object.freeze({
      name: "explore",
      description: "Independent explore description",
      model: "sonnet",
    }),
    Object.freeze({ name: "  ", description: "INVALID-NAME" }),
    Object.freeze({ name: 7 }),
    null,
  ]);
  const baseline = JSON.stringify(input);
  const events = captureBridgeEvents(() =>
    emitAvailableAgentsIfChanged(session, mapAvailableAgents(input)),
  );
  assert.equal(JSON.stringify(input), baseline);
  assert.equal(events.length, 1);
  assert.equal(events[0]?.session_id, session.sessionId);
  const update = events[0]?.update as {
    type: string;
    agents: Array<{ name: string; description: string; model?: string }>;
  };
  assert.equal(update.type, "available_agents_update");
  assert.deepEqual(Object.fromEntries(update.agents.map((agent) => [agent.name, agent])), {
    reviewer: { name: "reviewer", description: "Independent review description", model: "haiku" },
    explore: { name: "explore", description: "Independent explore description", model: "sonnet" },
  });
  assert.deepEqual(
    captureBridgeEvents(() => emitAvailableAgentsIfChanged(session, mapAvailableAgents([]))),
    [
      {
        event: "session_update",
        session_id: session.sessionId,
        update: { type: "available_agents_update", agents: [] },
      },
    ],
  );
  for (const invalid of [null, {}, 7, "invalid-root"]) {
    assert.deepEqual(mapAvailableAgents(invalid), []);
  }
});

test("agent refresh preserves newer populated or empty authority, retains verified membership on failure and recovers at actual agent admission", async () => {
  for (const replacement of [[{ name: "new-agent", description: "Independent new authority" }], []])
    await withCommandTargets(async (target) => {
      const fixture = commandInitQuery(target, async () => []);
      controlModel(target);
      captureBridgeEvents(() =>
        emitAvailableAgentsIfChanged(target, [
          { name: "old-agent", description: "Initially verified" },
        ]),
      );
      const stale = bridgeDeferred<unknown[]>();
      let answer: () => Promise<unknown[]> = () => stale.promise;
      const nativeCalls: unknown[] = [];
      Object.assign(target.query, {
        supportedAgents() {
          return answer();
        },
        async applyFlagSettings(flags: unknown) {
          nativeCalls.push(flags);
        },
      });
      const events = await captureBridgeEventsAsync(async () => {
        try {
          refreshAvailableAgents(target);
          answer = async () => replacement;
          refreshAvailableAgents(target);
          await fixture.settle();
          stale.resolve([{ name: "stale-agent", description: "Late obsolete read" }]);
          await stale.promise;
          await fixture.settle();
          assert.deepEqual(
            target.availableAgents?.map((agent) => ({
              name: agent.name,
              description: agent.description,
            })),
            replacement,
          );
          await dispatchControl({
            command: "set_agent",
            session_id: target.sessionId,
            request_id: "removed-agent-request",
            agent: "old-agent",
          });
          if (replacement.length)
            await dispatchControl({
              command: "set_agent",
              session_id: target.sessionId,
              request_id: "new-agent-request",
              agent: "new-agent",
            });
          answer = async () => {
            throw new Error("INDEPENDENT-AGENT-READ-CAUSE");
          };
          refreshAvailableAgents(target);
          await fixture.settle();
          assert.deepEqual(
            target.availableAgents?.map((agent) => ({
              name: agent.name,
              description: agent.description,
            })),
            replacement,
          );
          answer = async () => [
            { name: "recovered-agent", description: "Recovered independent authority" },
          ];
          refreshAvailableAgents(target);
          await fixture.settle();
          await dispatchControl({
            command: "set_agent",
            session_id: target.sessionId,
            request_id: "recovered-agent-request",
            agent: "recovered-agent",
          });
        } finally {
          stale.resolve([]);
          await stale.promise;
          await fixture.settle();
        }
      });
      assert.deepEqual(
        events
          .filter(
            (event) =>
              (event.update as { type?: string } | undefined)?.type === "available_agents_update",
          )
          .map((event) => ({
            session: event.session_id,
            agents: (event.update as { agents: unknown }).agents,
          })),
        [
          { session: target.sessionId, agents: replacement },
          {
            session: target.sessionId,
            agents: [{ name: "recovered-agent", description: "Recovered independent authority" }],
          },
        ],
      );
      assert.ok(
        events.some(
          (event) =>
            event.event === "slash_error" &&
            event.request_id === "removed-agent-request" &&
            event.session_id === target.sessionId,
        ),
      );
      assert.deepEqual(
        configPublications(events, "agent").map(
          (event) => (event.update as { value: unknown }).value,
        ),
        replacement.length ? ["new-agent", "recovered-agent"] : ["recovered-agent"],
      );
      assert.deepEqual(
        nativeCalls,
        replacement.length
          ? [{ agent: "new-agent" }, { agent: "recovered-agent" }]
          : [{ agent: "recovered-agent" }],
        "removed membership cannot reach native mutation",
      );
      assert.equal(
        events.some(
          (event) =>
            event.event === "slash_error" && event.request_id === "recovered-agent-request",
        ),
        false,
      );
    });
});

test("agent inventory reads released after real replacement or close cannot publish obsolete choices", async () => {
  for (const lifetime of ["replacement", "close"])
    await withCommandTargets(async (target) => {
      const fixture = commandInitQuery(target, async () => []);
      const stale = bridgeDeferred<unknown[]>();
      let current = false;
      Object.assign(target.query, {
        supportedAgents: () =>
          current
            ? Promise.resolve([
                { name: "replacement-agent", description: "Independent replacement inventory" },
              ])
            : stale.promise,
      });
      const events = await captureBridgeEventsAsync(async () => {
        try {
          refreshAvailableAgents(target);
          current = true;
          if (lifetime === "replacement") {
            handleSdkMessage(target, {
              type: "system",
              subtype: "init",
              session_id: `replaced-${target.sessionId}`,
              model: "haiku",
              agents: ["replacement-agent"],
              effort: "medium",
            } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
            await fixture.settle();
          } else await boundedBridgeTask(closeSession(target));
          stale.resolve([
            { name: "obsolete-inventory-agent", description: "Obsolete native read" },
          ]);
          await stale.promise;
          await fixture.settle();
        } finally {
          stale.resolve([]);
          await stale.promise;
          await fixture.settle();
        }
      });
      const inventories = events.filter(
        (event) =>
          (event.update as { type?: string } | undefined)?.type === "available_agents_update",
      );
      for (const event of inventories)
        assert.equal(
          (event.update as { agents: Array<{ name: string }> }).agents.some(
            (agent) => agent.name === "obsolete-inventory-agent",
          ),
          false,
        );
      assert.notEqual(
        target.availableAgents?.some((agent) => agent.name === "obsolete-inventory-agent"),
        true,
      );
      if (lifetime === "replacement")
        assert.ok(
          inventories.some(
            (event) =>
              event.session_id === target.sessionId &&
              (event.update as { agents: Array<{ name: string }> }).agents.some(
                (agent) => agent.name === "replacement-agent",
              ),
          ),
        );
      else assert.deepEqual(inventories, []);
    });
});

type PermissionAuditRow = {
  name: string;
  sessionId: string;
  toolId: string;
  input: Record<string, unknown>;
  request: {
    session_id: string;
    request: {
      tool_call: { tool_call_id: string; raw_input: unknown };
      options: Array<{ option_id: string; kind: string; name: string }>;
    };
  };
  result: {
    behavior: string;
    toolUseID: string;
    updatedInput?: unknown;
    updatedPermissions?: unknown;
    message?: string;
  };
  events: Array<Record<string, unknown>>;
  peer?: PermissionAuditRow;
  recovery?: PermissionAuditRow;
};
async function permissionAudit(mode: string): Promise<PermissionAuditRow[]> {
  const { spawnSync } = await import("node:child_process");
  const { fileURLToPath } = await import("node:url");
  const child = spawnSync(
    process.execPath,
    [
      fileURLToPath(new URL("../../tests/fixtures/bridge-tool-policy.mjs", import.meta.url)),
      "permissions",
      mode,
    ],
    {
      encoding: "utf8",
      windowsHide: true,
      timeout: 15000,
      cwd: fileURLToPath(new URL("../../", import.meta.url)),
    },
  );
  assert.equal(child.error, undefined);
  assert.equal(child.status, 0, child.stderr);
  const result = JSON.parse(child.stdout) as {
    rows: PermissionAuditRow[];
    allQueriesClosed: boolean;
  };
  assert.equal(result.allQueriesClosed, true);
  return result.rows;
}
function assertNativePermission(row: PermissionAuditRow, allowed: boolean) {
  assert.equal(row.request.session_id, row.sessionId);
  assert.equal(row.request.request.tool_call.tool_call_id, row.toolId);
  assert.equal(row.result.toolUseID, row.toolId);
  assert.equal(row.result.behavior, allowed ? "allow" : "deny");
  if (allowed) {
    assert.deepEqual(row.result.updatedInput, row.input);
    assert.deepEqual(row.request.request.tool_call.raw_input, row.input);
  } else {
    assert.equal(row.result.updatedPermissions, undefined);
    assert.equal(row.result.updatedInput, undefined);
    assert.ok(row.result.message);
  }
  const frames = publishedToolFrames(row.events, row.sessionId, row.toolId);
  assert.ok(frames.some((frame) => frame.status === (allowed ? "in_progress" : "failed")));
  if (!allowed)
    assert.equal(
      frames.some((frame) => frame.status === "in_progress"),
      false,
    );
}
test("installed ordinary permission callbacks settle only their addressed native caller for allow-once, deny and cancel", async () => {
  const rows = await permissionAudit("outcomes");
  assert.deepEqual(
    rows.map((row) => row.name),
    ["once", "deny", "cancel"],
  );
  for (const row of rows) {
    assert.deepEqual(row.input, {
      command: `printf '${row.name} independent bytes'`,
      cwd: "/owned/project",
    });
    assertNativePermission(row, row.name === "once");
    assert.equal(row.result.updatedPermissions, undefined);
    assert.ok(row.peer);
    assert.notEqual(row.peer.sessionId, row.sessionId);
    assertNativePermission(row.peer, true);
    assert.equal(row.peer.result.updatedPermissions, undefined);
  }
});

test("installed ordinary permission requests advertise their actual SDK suggestion scopes and suppression with usable once and deny choices", async () => {
  const rows = await permissionAudit("choices");
  const expected: Record<string, string[]> = {
    session: ["allow_once", "allow_session", "reject_once"],
    persistent: ["allow_once", "allow_always", "reject_once"],
    "suppressed-persistent": ["allow_once", "reject_once"],
    "suppressed-mixed": ["allow_once", "allow_session", "reject_once"],
    absent: ["allow_once", "allow_always", "reject_once"],
  };
  assert.equal(rows.length, Object.keys(expected).length);
  for (const row of rows) {
    const choices = row.request.request.options;
    assert.deepEqual(
      choices.map((option) => option.option_id).sort(),
      expected[row.name]?.slice().sort(),
      row.name,
    );
    for (const option of choices) {
      assert.equal(option.kind, option.option_id);
      if (option.option_id === "allow_session") assert.match(option.name, /session/i);
      if (option.option_id === "allow_always")
        assert.match(option.name, /always|persist|permanent/i);
    }
    assertNativePermission(row, true);
    assert.equal(row.result.updatedPermissions, undefined);
  }
});

test("offered session and persistent grants preserve independent SDK multi-tool rules and exact input without scope leakage", async () => {
  const sessionRule = {
    type: "addRules",
    rules: [{ toolName: "Write", ruleContent: "/owned/project/**" }],
    behavior: "allow",
    destination: "session",
  };
  const persistentRule = {
    type: "addRules",
    rules: [
      { toolName: "Bash", ruleContent: "git status:*" },
      { toolName: "Read", ruleContent: "/owned/shared/**" },
    ],
    behavior: "allow",
    destination: "projectSettings",
  };
  const expected: Record<string, unknown> = {
    persistent: [persistentRule],
    session: [sessionRule],
    "mixed-session": [sessionRule],
    "mixed-persistent": [persistentRule],
    "reachable-fallback": [
      {
        type: "addRules",
        rules: [{ toolName: "Bash" }],
        behavior: "allow",
        destination: "localSettings",
      },
    ],
  };
  const rows = await permissionAudit("grants");
  assert.deepEqual(rows.map((row) => row.name).sort(), Object.keys(expected).sort());
  for (const row of rows) {
    assertNativePermission(row, true);
    assert.deepEqual(row.input, {
      command: `printf '${row.name} independent bytes'`,
      cwd: "/owned/project",
    });
    assert.deepEqual(row.result.updatedPermissions, expected[row.name], row.name);
  }
});

test("unoffered ordinary permission choices deny the original native request without granting rules or settling a peer and allow a fresh valid recovery", async () => {
  const rows = await permissionAudit("invalid");
  const selected: Record<string, string> = {
    "suppressed-persistent": "allow_always",
    "session-only-persistent": "allow_always",
    "absent-session": "allow_session",
    "unknown-choice": "unknown-independent-option",
  };
  assert.deepEqual(rows.map((row) => row.name).sort(), Object.keys(selected).sort());
  for (const row of rows) {
    assert.equal(
      row.request.request.options.some((option) => option.option_id === selected[row.name]),
      false,
    );
    assertNativePermission(row, false);
    assert.ok(row.peer);
    assertNativePermission(row.peer, true);
    assert.equal(row.peer.result.updatedPermissions, undefined);
    assert.ok(row.recovery);
    assertNativePermission(row.recovery, true);
    assert.equal(row.recovery.result.updatedPermissions, undefined);
  }
});

test("native result authentication failures preserve exact login guidance while MCP and neutral failures remain ordinary turn errors", async () => {
  for (const row of [
    { name: "revoked", detail: "Failed to authenticate: OAuth token revoked", auth: true },
    { name: "login", detail: "Please run /login to authenticate", auth: true },
    { name: "mcp", detail: "MCP OAuth token revoked", auth: false },
    { name: "neutral", detail: "INDEPENDENT-ORDINARY-FAILURE", auth: false },
  ]) {
    const session = makeSessionState();
    session.sessionId = `auth-result-${row.name}`;
    const events = await captureBridgeEventsAsync(async () => {
      for (const turn of ["first", "second"]) {
        activityStream(session, {
          type: "message_start",
          message: { id: `auth-${row.name}-${turn}` },
        });
        handleSdkMessage(session, {
          type: "result",
          subtype: "error_during_execution",
          uuid: `auth-${turn}-result`,
          session_id: session.sessionId,
          errors: [row.detail],
        } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      }
    });
    const hints = events.filter((event) => event.event === "auth_required");
    assert.equal(hints.length, row.auth ? 1 : 0, row.name);
    if (row.auth) assert.equal(hints[0]?.method_description, row.detail);
    const errors = events.filter((event) => event.event === "turn_error");
    assert.equal(errors.length, 2);
    for (const error of errors) {
      assert.equal(error.session_id, session.sessionId);
      assert.equal(error.error_kind, row.auth ? "auth_required" : "other");
      assert.equal(error.message, row.detail);
    }
    assert.equal(
      events.some((event) => event.event === "turn_complete"),
      false,
    );
    session.input.close();
  }
});

function deferredNotification(toolUseId: string, status = "completed", output = "Fetched page") {
  return {
    type: "user", origin: { kind: "task-notification" }, uuid: "deferred-result",
    message: { role: "user", content: `<task-notification>\n<tool-use-id>${toolUseId}</tool-use-id>\n<task-type>tool_call</task-type>\n<status>${status}</status>\n<summary>SDK continuation instructions</summary>\n<result>\n${output}\n</result>\n</task-notification>` },
  };
}

test("detached WebFetch and WebSearch calls retain their own wire state through later turns until one authoritative notification settles each", async () => {
  for (const name of ["WebFetch", "WebSearch"])
    for (const status of ["completed", "failed"]) {
      const session = makeSessionState();
      session.sessionId = `web-${name}-${status}`;
      const emit = (message: unknown) =>
        handleSdkMessage(session, {
          session_id: session.sessionId,
          ...(message as object),
        } as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      const events = await captureBridgeEventsAsync(async () => {
        for (const id of ["web-target", "web-sibling"]) {
          emit({
            type: "assistant",
            uuid: `${id}-source`,
            message: {
              role: "assistant",
              content: [
                { type: "tool_use", id, name, input: { url: `https://example.test/${id}` } },
              ],
            },
          });
          emit({
            type: "user",
            uuid: `${id}-detached`,
            tool_use_result: { detachedToolCall: true },
            message: {
              role: "user",
              content: [
                {
                  type: "tool_result",
                  tool_use_id: id,
                  content: `SDK-CONTINUATION-INSTRUCTIONS-${id}`,
                },
              ],
            },
          });
        }
        emit({
          type: "tool_progress",
          uuid: "web-progress",
          tool_use_id: "web-target",
          tool_name: name,
          elapsed_time_seconds: 4,
        });
        emit({
          type: "tool_use_summary",
          uuid: "web-summary",
          preceding_tool_use_ids: ["web-target"],
          summary: "NONAUTHORITATIVE-SUMMARY",
        });
        emit({ type: "result", uuid: "parent-turn-result", subtype: "success" });
        activityStream(session, {
          type: "message_start",
          message: { id: "independent-later-turn" },
        });
        emit({
          type: "result",
          uuid: "later-turn-result",
          subtype: "error_during_execution",
          errors: ["INDEPENDENT-LATER-TURN-FAILURE"],
        });
        emit({
          ...deferredNotification("web-target", status, "INDEPENDENT-WEB-FINAL-RESULT"),
          origin: { kind: "peer" },
        });
      });
      for (const id of ["web-target", "web-sibling"]) {
        const tool = publishedTool(events, session.sessionId, id);
        assert.equal(tool.status, "detached");
        assert.equal(tool.raw_output, "");
        const frames = publishedToolFrames(events, session.sessionId, id);
        const detachedIndex = frames.findIndex((frame) => frame.status === "detached");
        assert.ok(detachedIndex >= 0);
        for (const frame of frames.slice(detachedIndex)) {
          if (frame.status !== undefined) assert.equal(frame.status, "detached");
          assert.doesNotMatch(
            JSON.stringify(frame),
            /SDK-CONTINUATION-INSTRUCTIONS|NONAUTHORITATIVE-SUMMARY/,
          );
        }
      }
      const settled = await captureBridgeEventsAsync(async () => {
        const notification = deferredNotification(
          "web-target",
          status,
          "INDEPENDENT-WEB-FINAL-RESULT",
        );
        emit(notification);
        emit(notification);
      });
      const all = [...events, ...settled];
      const target = publishedTool(all, session.sessionId, "web-target");
      assert.equal(target.status, status);
      assert.equal(target.raw_output, "INDEPENDENT-WEB-FINAL-RESULT");
      assert.deepEqual(publishedToolFrames(settled, session.sessionId, "web-sibling"), []);
      assert.equal(publishedTool(all, session.sessionId, "web-sibling").status, "detached");
      assert.equal(
        publishedToolFrames(settled, session.sessionId, "web-target").filter(
          (frame) => frame.raw_output === "INDEPENDENT-WEB-FINAL-RESULT",
        ).length,
        1,
      );
      session.input.close();
    }
});

test("close publishes killed for a natively detached web call without changing a peer or a genuinely completed tool and rejects late notification", () => {
  const session = makeSessionState(),
    peer = makeSessionState();
  session.sessionId = "web-close-owner";
  peer.sessionId = "web-close-peer";
  const launch = (owner: SessionState, id: string) => {
    handleSdkMessage(owner, {
      type: "assistant",
      uuid: `${id}-source`,
      message: {
        role: "assistant",
        content: [
          { type: "tool_use", id, name: "WebFetch", input: { url: `https://example.test/${id}` } },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(owner, {
      type: "user",
      uuid: `${id}-ack`,
      tool_use_result: { detachedToolCall: true },
      message: {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: id, content: "SDK-CONTINUATION-NOT-OUTPUT" }],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  };
  const initial = captureBridgeEvents(() => {
    launch(session, "closing-web");
    launch(session, "completed-web");
    launch(peer, "peer-web");
    handleSdkMessage(
      session,
      deferredNotification(
        "completed-web",
        "completed",
        "INDEPENDENT-COMPLETED-WEB",
      ) as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage,
    );
  });
  const closed = captureBridgeEvents(() => {
    beginSessionClose(session);
    handleSdkMessage(
      session,
      deferredNotification(
        "closing-web",
        "completed",
        "LATE-WEB-RESULT-MUST-NOT-REOPEN",
      ) as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage,
    );
  });
  assert.equal(
    publishedTool([...initial, ...closed], session.sessionId, "closing-web").status,
    "killed",
  );
  assert.ok(
    publishedToolFrames(closed, session.sessionId, "closing-web").some(
      (frame) => frame.status === "killed",
    ),
  );
  assert.equal(
    publishedToolFrames(closed, session.sessionId, "closing-web").some(
      (frame) =>
        frame.status === "completed" || frame.raw_output === "LATE-WEB-RESULT-MUST-NOT-REOPEN",
    ),
    false,
  );
  assert.deepEqual(publishedToolFrames(closed, peer.sessionId, "peer-web"), []);
  assert.equal(publishedTool(initial, peer.sessionId, "peer-web").status, "detached");
  assert.deepEqual(publishedToolFrames(closed, session.sessionId, "completed-web"), []);
  assert.equal(
    publishedTool([...initial, ...closed], session.sessionId, "completed-web").raw_output,
    "INDEPENDENT-COMPLETED-WEB",
  );
  peer.input.close();
});

test("live Write results use confirmed source instead of the requested preview", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "write-source",
      message: {
        content: [
          {
            type: "tool_use",
            id: "write-confirmed",
            name: "Write",
            input: { file_path: "requested.ts", content: "REQUESTED-A\n" },
          },
          {
            type: "tool_use",
            id: "write-sibling",
            name: "Write",
            input: { file_path: "sibling.ts", content: "SIBLING-PREVIEW" },
          },
          {
            type: "tool_use",
            id: "write-unavailable",
            name: "Write",
            input: { file_path: "large.ts", content: "REQUESTED-LARGE" },
          },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "user",
      uuid: "write-result",
      tool_use_result: {
        type: "update",
        filePath: "actual/confirmed.ts",
        originalFile: "ORIGINAL-B\r\n",
        content: "CONFIRMED-C\r\n\tend  \n",
        userModified: true,
        structuredPatch: [],
        gitDiff: { repository: "actual/repository" },
      },
      message: {
        content: [
          {
            type: "tool_result",
            tool_use_id: "write-confirmed",
            content: "Readable write confirmation",
          },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "user",
      uuid: "unavailable-result",
      tool_use_result: {
        type: "update",
        filePath: "actual/large.ts",
        originalFile: null,
        content: "CONFIRMED-LARGE",
        userModified: true,
        structuredPatch: [],
      },
      message: {
        content: [
          { type: "tool_result", tool_use_id: "write-unavailable", content: "Large file updated" },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });
  const patches = events.flatMap((event) => {
    const update = event.update as
      | { tool_call_update?: { tool_call_id: string; fields: Record<string, unknown> } }
      | undefined;
    return update?.tool_call_update ? [update.tool_call_update] : [];
  });
  const target = Object.assign(
    {},
    ...patches
      .filter((patch) => patch.tool_call_id === "write-confirmed")
      .map((patch) => patch.fields),
  );
  assert.equal(target.status, "completed");
  assert.deepEqual(target.content, [
    {
      type: "diff",
      old_path: "actual/confirmed.ts",
      new_path: "actual/confirmed.ts",
      old: "ORIGINAL-B\r\n",
      new: "CONFIRMED-C\r\n\tend  \n",
      repository: "actual/repository",
    },
  ]);
  assert.equal(
    patches.some((patch) => patch.tool_call_id === "write-sibling"),
    false,
  );
  const unavailable = Object.assign(
    {},
    ...patches
      .filter((patch) => patch.tool_call_id === "write-unavailable")
      .map((patch) => patch.fields),
  );
  assert.equal(unavailable.status, "completed");
  assert.match(String(unavailable.raw_output), /actual\/large\.ts/);
  assert.match(String(unavailable.raw_output), /previous content.*unavailable/);
  assert.doesNotMatch(String(unavailable.raw_output), /Created|REQUESTED-LARGE/);
  assert.deepEqual(unavailable.content, [
    { type: "content", content: { type: "text", text: unavailable.raw_output } },
  ]);
});

test("live Read results publish exact source bytes and their own location while empty reads retain the SDK explanation", () => {
  const session = makeSessionState(),
    source = "line one\r\n\tline two  \r\n";
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "read-source",
      message: {
        content: [
          {
            type: "tool_use",
            id: "read-target",
            name: "Read",
            input: { file_path: "requested.txt", offset: 63 },
          },
          { type: "tool_use", id: "read-empty", name: "Read", input: { file_path: "empty.txt" } },
          {
            type: "tool_use",
            id: "read-sibling",
            name: "Read",
            input: { file_path: "sibling.txt" },
          },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "user",
      uuid: "read-result",
      tool_use_result: {
        type: "text",
        file: {
          filePath: "C:/actual/source.txt",
          content: source,
          numLines: 2,
          startLine: 63,
          totalLines: 201,
        },
      },
      message: {
        content: [
          {
            type: "tool_result",
            tool_use_id: "read-target",
            content: "63\tWRONG-NUMBERED-BODY\n<system-reminder>PRIVATE-REMINDER</system-reminder>",
          },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "user",
      uuid: "empty-result",
      tool_use_result: {
        type: "text",
        file: { filePath: "empty.txt", content: "", numLines: 0, startLine: 1, totalLines: 0 },
      },
      message: {
        content: [
          {
            type: "tool_result",
            tool_use_id: "read-empty",
            content: "SDK confirms this file has no content.",
          },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });
  const target = publishedTool(events, session.sessionId, "read-target");
  assert.equal(target.status, "completed");
  assert.equal(target.source_message_uuid, "read-result");
  assert.equal(target.raw_output, source);
  assert.deepEqual(target.content, [{ type: "content", content: { type: "text", text: source } }]);
  assert.deepEqual((target.output_metadata as { read?: unknown })?.read, {
    file_path: "C:/actual/source.txt",
    start_line: 63,
  });
  const empty = publishedTool(events, session.sessionId, "read-empty");
  assert.equal(empty.status, "completed");
  assert.equal(empty.raw_output, "SDK confirms this file has no content.");
  assert.deepEqual(empty.content, [
    { type: "content", content: { type: "text", text: "SDK confirms this file has no content." } },
  ]);
  assert.equal((empty.output_metadata as { read?: unknown })?.read, undefined);
  const sibling = publishedTool(events, session.sessionId, "read-sibling");
  assert.equal(sibling.status, "in_progress");
  assert.equal(sibling.raw_output, undefined);
  assert.doesNotMatch(JSON.stringify(events), /WRONG-NUMBERED-BODY|PRIVATE-REMINDER/);
});

test("SDK history omits live-only result flags and meta task notifications", async () => {
  const sessionId = "11111111-1111-4111-8111-111111111111";
  const entry = (type: string, message: unknown, extra = {}) => ({ type, message, ...extra });
  const records = [
    entry("assistant", {
      role: "assistant",
      content: [{ type: "tool_use", id: "history-web", name: "WebFetch", input: {} }],
    }),
    entry(
      "user",
      {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: "history-web", content: "Do not wait" }],
      },
      { tool_use_result: { detachedToolCall: true } },
    ),
    entry("user", deferredNotification("history-web").message, {
      isMeta: true,
      origin: { kind: "task-notification" },
    }),
    entry("assistant", {
      role: "assistant",
      content: [{ type: "tool_use", id: "history-mcp", name: "mcp__docs__read", input: {} }],
    }),
    entry(
      "user",
      {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: "history-mcp", content: "Readable" }],
      },
      { tool_use_result: { structuredContentOmitted: true } },
    ),
    entry("assistant", { role: "assistant", content: [{ type: "text", text: "Done" }] }),
  ].map((record, index) => ({
    ...record,
    uuid: `22222222-2222-4222-8222-${String(index + 1).padStart(12, "0")}`,
    parentUuid: index === 0 ? null : `22222222-2222-4222-8222-${String(index).padStart(12, "0")}`,
    sessionId,
    isSidechain: false,
    timestamp: `2026-10-03T10:00:0${index}.000Z`,
  }));
  const messages = await getSessionMessages(sessionId, {
    dir: process.cwd(),
    sessionStore: {
      load: async () => records,
      append: async () => assert.fail("history reads must not write"),
    },
  });
  assert.deepEqual(
    messages.map((message) => message.uuid),
    [
      "22222222-2222-4222-8222-000000000001",
      "22222222-2222-4222-8222-000000000002",
      "22222222-2222-4222-8222-000000000004",
      "22222222-2222-4222-8222-000000000005",
      "22222222-2222-4222-8222-000000000006",
    ],
  );
  assert.ok(messages.every((message) => !Object.hasOwn(message, "tool_use_result")));
  const updates = mapSessionMessagesToUpdates(messages);
  const results = updates.filter((update) => update.type === "tool_call_update");
  const byId = Object.fromEntries(
    results.map((update) => [update.tool_call_update.tool_call_id, update.tool_call_update.fields]),
  );
  for (const [id, text] of [
    ["history-web", "Do not wait"],
    ["history-mcp", "Readable"],
  ]) {
    assert.equal(byId[id!]?.status, "completed");
    assert.equal(byId[id!]?.raw_output, text);
    assert.equal(byId[id!]?.output_metadata, undefined);
  }
  assert.equal(
    updates.some((update) => update.type === "user_message_chunk"),
    false,
  );
  assert.deepEqual(
    updates
      .filter((update) => update.type === "agent_message_chunk")
      .map((update) => update.content),
    [{ type: "text", text: "Done" }],
  );
});

test("rich native initialization command metadata survives sparse init before any dynamic authority then accepts replacement and clear", async () => {
  const session = makeSessionState();
  session.sessionId = "startup-command-owner";
  const fixture = commandInitQuery(session, async () => []);
  const frames = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKMessage>();
  Object.assign(session.query, {
    initializationResult: async () => ({
      models: [],
      agents: [],
      account: { apiKeySource: "fixture" },
      commands: [
        {
          name: "startup",
          description: "INDEPENDENT-STARTUP-DESCRIPTION",
          argumentHint: "<independent-argument>",
          aliases: ["independent-start"],
          builtin: true,
        },
      ],
    }),
    [Symbol.asyncIterator]: () => frames[Symbol.asyncIterator](),
    close: () => frames.close(),
  });
  let before: Array<Record<string, unknown>> = [];
  const events = await captureBridgeEventsAsync(async () => {
    try {
      startSessionTasks(session);
      await boundedBridgeTask(Promise.resolve(session.initializationTask));
      handleSdkMessage(session, {
        type: "system",
        subtype: "init",
        session_id: session.sessionId,
        model: "haiku",
        effort: "medium",
        slash_commands: ["startup"],
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      await fixture.settle();
    } finally {
      frames.close();
      await boundedBridgeTask(
        Promise.allSettled([session.initializationTask, session.queryConsumerTask]),
      );
      await fixture.settle();
      session.input.close();
    }
  });
  before = events;
  const snapshots = commandPublications(before, session.sessionId);
  assert.ok(snapshots.length > 0);
  for (const snapshot of snapshots) {
    assert.deepEqual(snapshot.commands, [
      {
        name: "startup",
        description: "INDEPENDENT-STARTUP-DESCRIPTION",
        input_hint: "<independent-argument>",
        aliases: ["independent-start"],
        builtin: true,
      },
    ]);
  }
  const later = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "commands_changed",
      commands: [{ name: "later", description: "INDEPENDENT-DYNAMIC-DESCRIPTION" }],
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "commands_changed",
      commands: [],
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });
  const dynamic = commandPublications(later, session.sessionId);
  assert.equal(dynamic.length, 2);
  assert.deepEqual(dynamic[0]?.commands, [
    { name: "later", description: "INDEPENDENT-DYNAMIC-DESCRIPTION" },
  ]);
  assert.deepEqual(dynamic[1]?.commands, []);
  assert.equal(dynamic[0]?.source, "commands_changed");
  assert.ok(dynamic[0]!.generation! > snapshots.at(-1)!.generation!);
  assert.ok(dynamic[1]!.generation! > dynamic[0]!.generation!);
});

test("a cut-short native message publishes its exact partial text once and only the actual result completes its turn", async () => {
  const session = makeSessionState();
  session.sessionId = "partial-stream-owner";
  const before = await captureBridgeEventsAsync(async () => {
    activityStream(session, {
      type: "message_start",
      message: { id: "partial-independent-response" },
    });
    activityStream(session, {
      type: "content_block_start",
      index: 0,
      content_block: { type: "text", text: "" },
    });
    activityStream(session, {
      type: "content_block_delta",
      index: 0,
      delta: { type: "text_delta", text: "INDEPENDENT PARTIAL\n exact  " },
    });
    activityStream(session, { type: "content_block_stop", index: 0 });
    activityStream(session, {
      type: "message_delta",
      delta: { stop_reason: "max_tokens" },
      usage: { output_tokens: 7 },
    });
    activityStream(session, { type: "message_stop" });
  });
  assert.equal(
    before.some((event) => event.event === "turn_complete" || event.event === "turn_error"),
    false,
  );
  const final = await captureBridgeEventsAsync(async () =>
    handleSdkMessage(session, {
      type: "result",
      subtype: "success",
      uuid: "partial-real-result",
      session_id: session.sessionId,
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
  );
  const all = [...before, ...final];
  assert.deepEqual(
    all
      .filter(
        (event) => (event.update as { type?: string } | undefined)?.type === "agent_message_chunk",
      )
      .map((event) => ({
        session: event.session_id,
        content: (event.update as { content: unknown }).content,
      })),
    [
      {
        session: session.sessionId,
        content: { type: "text", text: "INDEPENDENT PARTIAL\n exact  " },
      },
    ],
  );
  assert.deepEqual(
    all.filter((event) => event.event === "turn_complete"),
    [{ event: "turn_complete", session_id: session.sessionId }],
  );
  session.input.close();
});

test("SDK package resolution admits compatible initialization and rejects mismatched or unavailable versions before native work while shutdown remains usable", async () => {
  const { spawnSync } = await import("node:child_process");
  const { readFileSync } = await import("node:fs");
  const pin = (
    JSON.parse(readFileSync(new URL("../../package.json", import.meta.url), "utf8")) as {
      dependencies: Record<string, string>;
    }
  ).dependencies["@anthropic-ai/claude-agent-sdk"];
  assert.ok(pin);
  const bridgeUrl = new URL("./bridge.js", import.meta.url).href,
    eventsUrl = new URL("./bridge/events.js", import.meta.url).href,
    schedulerUrl = new URL("./bridge/command_scheduler.js", import.meta.url).href,
    lifecycleUrl = new URL("./bridge/session_lifecycle.js", import.meta.url).href;
  for (const row of [
    { name: "compatible", version: pin },
    { name: "mismatch", version: "0.0.1-independent" },
    { name: "unavailable", version: null },
  ]) {
    const fsSource = `export * from "node:fs";export {default} from "node:fs";import {readFileSync as nativeRead} from "node:fs";export const readFileSync=(path,...args)=>{if(String(path).replaceAll('\\\\','/').endsWith('/@anthropic-ai/claude-agent-sdk/package.json')){globalThis.versionReads++;if(globalThis.observedVersion===null)throw new Error('INDEPENDENT-UNAVAILABLE-METADATA');return JSON.stringify({version:globalThis.observedVersion});}return nativeRead(path,...args);};`;
    const sdkSource = `export * from ${JSON.stringify(import.meta.resolve("@anthropic-ai/claude-agent-sdk"))};export const query=()=>{globalThis.nativeQueries++;throw new Error('REJECTED-CREATE-MUST-NOT-QUERY');};export const listSessions=async()=>[];`;
    const script = `import {registerHooks} from 'node:module';globalThis.observedVersion=${JSON.stringify(row.version)};globalThis.versionReads=0;globalThis.nativeQueries=0;registerHooks({resolve(specifier,context,next){if(specifier==='node:fs'&&context.parentURL!=='fixture:version-fs')return {url:'fixture:version-fs',shortCircuit:true};if(specifier==='@anthropic-ai/claude-agent-sdk'&&!context.conditions.includes('require'))return {url:'fixture:version-sdk',shortCircuit:true};return next(specifier,context);},load(url,context,next){if(url==='fixture:version-fs')return {format:'module',source:${JSON.stringify(fsSource)},shortCircuit:true};if(url==='fixture:version-sdk')return {format:'module',source:${JSON.stringify(sdkSource)},shortCircuit:true};return next(url,context);}});const bridge=await import(${JSON.stringify(bridgeUrl)});const protocol=await import(${JSON.stringify(eventsUrl)});const {BridgeCommandScheduler}=await import(${JSON.stringify(schedulerUrl)});const lifecycle=await import(${JSON.stringify(lifecycleUrl)});const events=[];const restore=protocol.replaceProtocolEventWriter(line=>events.push(JSON.parse(line)));const scheduler=new BridgeCommandScheduler();const {writeSync}=await import("node:fs");let dispatching;process.on("exit",code=>writeSync(1,JSON.stringify({events,reads:globalThis.versionReads,nativeQueries:globalThis.nativeQueries,sessions:lifecycle.sessions.size,exitDuring:dispatching,exitCode:code})));try{for(const wire of [{command:'initialize',request_id:'version-init',cwd:process.cwd()},...(${JSON.stringify(row.name)}==='compatible'?[]:[{command:'create_session',request_id:'version-create',cwd:process.cwd()}]),{command:'shutdown',request_id:'version-shutdown'}]){dispatching=wire.command;await bridge.scheduleBridgeCommand(scheduler,bridge.parseCommandEnvelope(JSON.stringify(wire)));dispatching=undefined;}}finally{restore();await lifecycle.closeAllSessions();}`;
    const child = spawnSync(process.execPath, ["--input-type=module", "-e", script], {
      encoding: "utf8",
      windowsHide: true,
      timeout: 15000,
    });
    assert.equal(child.error, undefined);
    assert.equal(child.status, 0, child.stderr);
    const result = JSON.parse(child.stdout) as {
      events: Array<Record<string, unknown>>;
      reads: number;
      nativeQueries: number;
      sessions: number;
      exitDuring?: string;
      exitCode: number;
    };
    assert.ok(result.reads > 0, "actual installed package metadata resolution was exercised");
    assert.equal(result.nativeQueries, 0);
    assert.equal(result.sessions, 0);
    const initialized = result.events.filter((event) => event.event === "initialized");
    const failures = result.events.filter((event) => event.event === "connection_failed");
    if (row.name === "compatible") {
      assert.equal(initialized.length, 1);
      assert.equal(initialized[0]?.request_id, "version-init");
      assert.deepEqual(failures, []);
    } else {
      assert.deepEqual(initialized, []);
      assert.deepEqual(
        failures.map((event) => event.request_id),
        ["version-init", "version-create"],
      );
      for (const failure of failures) {
        assert.ok(String(failure.message).includes(pin));
        assert.ok(String(failure.message).includes(row.version ?? "unable to resolve"));
      }
    }
    assert.equal(result.exitDuring, "shutdown");
    assert.equal(result.exitCode, 0);
  }
});

test("the pinned SDK forwards named options unchanged through the host to a real child", { timeout: 15000 }, async (context) => {
  const { fileURLToPath } = await import("node:url");
  const fixture = fileURLToPath(new URL("../../tests/fixtures/sdk-compatibility-peer.js", import.meta.url));
  for (const special of [false, true]) {
    const session = makeSessionState();
    const host = buildQueryOptions({ resolvedSettings: {}, cwd: process.cwd(), launchSettings: {},
      provisionalSessionId: session.sessionId, input: session.input, canUseTool: async () => ({behavior:"deny",message:"unused"}),
      claudeCodeExecutable: fixture, enableSdkDebug:false, enableSpawnDebug:false, sessionIdForLogs:()=>session.sessionId });
    let generated: string[] | undefined;
    const sdkQuery = query({prompt:session.input,options:{
      ...host, executable:"node", persistSession:false, settings:{language:"value with = signs"},
      ...(special ? {model:"-model with = signs",debugFile:"-debug path=example.log",permissionMode:"plan" as const,
        settings:{language:""},settingSources:[]} : {}),
      spawnClaudeCodeProcess(options) {
        generated = [...options.args];
        return host.spawnClaudeCodeProcess(options);
      },
    }});
    context.after(()=>sdkQuery.close());
    try {
      const commands = await sdkQuery.supportedCommands();
      const report = commands.find(command=>command.name === "received-argv");
      assert.ok(report);
      assert.ok(generated);
      assert.equal(generated[0],fixture);
      assert.deepEqual(JSON.parse(report.description),generated.slice(1));
      if (special) {
        assert.ok(generated.includes("--model=-model with = signs"));
        assert.ok(generated.includes("--debug-file=-debug path=example.log"));
        assert.ok(generated.includes("--permission-mode=plan"));
        assert.ok(generated.includes("--setting-sources="));
      } else {
        assert.ok(!generated.some(arg=>arg.startsWith("--model=") || arg.startsWith("--debug-file=")));
        assert.ok(generated.includes("--setting-sources=user,project,local"));
      }
      assert.equal(generated[generated.indexOf("--settings")+1],special ? '{"language":""}' : '{"language":"value with = signs"}');
    } finally {sdkQuery.close();}
  }
});

test("usage attribution preserves both windows and future keys in the emitted response", async () => {
  const { readFileSync } = await import("node:fs");
  const raw = JSON.parse(
    readFileSync(
      new URL("../../tests/fixtures/sdk-usage-attribution.json", import.meta.url),
      "utf8",
    ),
  );
  await withCommandTargets(async (session, sibling) => {
    const native = {
      close: () => {},
      async usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET() {
        assert.equal(this, native);
        return raw;
      },
    };
    session.query = native as unknown as SessionState["query"];
    const events = await dispatchData(session, "get_usage", "usage-request");
    assert.equal(
      events.some((event) => event.session_id === sibling.sessionId),
      false,
    );
    assert.equal(events.length, 1);
    assert.equal(events[0].request_id, "usage-request");
    assert.equal(events[0].event, "usage_snapshot");
    assert.equal(events[0].session_id, session.sessionId);
    const snapshot = events[0].snapshot as import("./types.js").StructuredUsageSnapshot;
    assert.deepEqual(snapshot.activity_day?.behaviors, [
      { key: "max_effort", pct: 71, count: 7 },
      { key: "workflow", pct: 62, count: 6 },
      { key: "always_on_context", pct: 53, count: 5 },
      { key: "large_tool_results", pct: 44, count: 4 },
      { key: "future_behavior", pct: 35, count: 3 },
      { key: "sixth_behavior", pct: 26, count: 2 },
    ]);
    assert.deepEqual(snapshot.activity_week?.behaviors, [
      { key: "max_effort", pct: 81, count: 17 },
      { key: "workflow", pct: 72, count: 16 },
      { key: "always_on_context", pct: 63, count: 15 },
      { key: "large_tool_results", pct: 54, count: 14 },
      { key: "future_behavior", pct: 45, count: 13 },
      { key: "sixth_behavior", pct: 36, count: 12 },
    ]);
    assert.equal(snapshot.activity_day?.request_count, 20);
    assert.equal(snapshot.activity_week?.request_count, 100);
  });
});
test("pinned SDK preserves supplied citations and the host delivers streamed and saved text once", {timeout:15000}, async (context) => {
  // This peer checks transport and host consumers. settings_runtime.test.ts
  // separately exercises the native producer with actual API citation deltas.
  const {fileURLToPath} = await import("node:url");
  const fixture = fileURLToPath(new URL("../../tests/fixtures/sdk-compatibility-peer.js",import.meta.url));
  const session = makeSessionState();
  const host = buildQueryOptions({resolvedSettings:{},cwd:process.cwd(),launchSettings:{},provisionalSessionId:session.sessionId,
    input:session.input,canUseTool:async()=>({behavior:"deny",message:"unused"}),claudeCodeExecutable:fixture,
    enableSdkDebug:false,enableSpawnDebug:false,sessionIdForLogs:()=>session.sessionId});
  const sdkQuery = query({prompt:"COMPAT-CITATIONS",options:{...host,executable:"node",persistSession:false}});
  context.after(()=>sdkQuery.close());
  const completed: SessionMessage[] = [];
  let citationDeltaSeen = false;
  try {
    const events = await captureBridgeEventsAsync(async()=>{
      for await (const message of sdkQuery) {
        if (message.type === "stream_event" && message.event.type === "content_block_delta" && message.event.delta.type === "citations_delta") citationDeltaSeen = true;
        if (message.type === "assistant") {
          assert.deepEqual(message.message.content,[{type:"text",text:"Evidence: [source](https://example.test/source).",citations:[{
            type:"web_search_result_location",url:"https://example.test/citation-only",title:"Metadata source",encrypted_index:"fixture-only",cited_text:"Evidence",
          }]}]);
          completed.push({...message,parent_agent_id:null});
        }
        handleSdkMessage(session,message);
        if (message.type === "result") break;
      }
    });
    assert.ok(citationDeltaSeen);
    assert.equal(completed.length,1);
    const live = events.flatMap(event=>{const update=event.update as import("./types.js").SessionUpdate|undefined;return update?.type === "agent_message_chunk" ? [update.content] : [];});
    assert.deepEqual(live,[{type:"text",text:"Evidence: "},{type:"text",text:"[source](https://example.test/source)."}]);
    assert.equal(events.filter(event=>event.event === "turn_complete").length,1);
    const saved = await getSessionMessages("11111111-1111-4111-8111-111111111111",{dir:process.cwd(),sessionStore:{
      load:async()=>[{type:"assistant",uuid:"22222222-2222-4222-8222-222222222222",parentUuid:null,
        sessionId:"11111111-1111-4111-8111-111111111111",isSidechain:false,timestamp:"2026-10-18T12:00:00Z",message:completed[0].message}],
      append:async()=>assert.fail("history reads must not write"),
    }});
    assert.equal(saved.length,1);
    assert.deepEqual(saved[0].message,completed[0].message);
    const replay = mapSessionMessagesToUpdates(saved).filter(update=>update.type === "agent_message_chunk");
    assert.deepEqual(replay.map(update=>update.content),[{type:"text",text:"Evidence: [source](https://example.test/source)."}]);
    // Completed-only replies also keep the same visible text without needing deltas.
    const fallback = captureBridgeEvents(()=>handleSdkMessage(makeSessionState(),completed[0] as import("@anthropic-ai/claude-agent-sdk").SDKMessage));
    assert.deepEqual(fallback.flatMap(event=>{const update=event.update as import("./types.js").SessionUpdate|undefined;return update?.type === "agent_message_chunk" ? [update.content] : [];}),replay.map(update=>update.content));
  } finally {sdkQuery.close();}
});

function savedRecord(type: string, uuid: string, message: unknown): SessionMessage {
  return {
    type,
    uuid,
    session_id: "independent-saved-session",
    parent_tool_use_id: null,
    parent_agent_id: null,
    message,
  } as unknown as SessionMessage;
}
test("mixed saved messages preserve exact visible bytes, source UUIDs and tool association around hidden thinking, system and local-command records", () => {
  const updates = mapSessionMessagesToUpdates([
    savedRecord("user", "saved-visible-user", {
      role: "user",
      content: [{ type: "text", text: "INDEPENDENT USER\n exact  " }],
    }),
    savedRecord("assistant", "saved-hidden-thinking", {
      role: "assistant",
      content: [
        { type: "thinking", thinking: "HIDDEN-THINKING-SENTINEL", signature: "opaque-signature" },
      ],
    }),
    savedRecord("system", "saved-hidden-system", {
      type: "system",
      subtype: "future-internal-telemetry",
      content: "HIDDEN-SYSTEM-SENTINEL",
    }),
    savedRecord("system", "saved-context-command", {
      type: "system",
      subtype: "local_command",
      command: "/context",
      content: "HIDDEN-LOCAL-CONTEXT-SENTINEL",
    }),
    savedRecord("assistant", "saved-visible-assistant", {
      role: "assistant",
      content: [
        {
          type: "tool_use",
          id: "saved-independent-bash",
          name: "Bash",
          input: { command: "printf 'INDEPENDENT COMMAND'" },
        },
        { type: "text", text: "INDEPENDENT ASSISTANT\n exact  " },
      ],
    }),
    savedRecord("user", "saved-visible-result", {
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: "saved-independent-bash",
          content: "INDEPENDENT RESULT\r\n\tend  ",
          is_error: false,
        },
      ],
    }),
  ]);
  assert.deepEqual(
    updates
      .filter((update) => update.type === "user_message_chunk")
      .map((update) => ({ source: update.source_message_uuid, content: update.content })),
    [
      {
        source: "saved-visible-user",
        content: { type: "text", text: "INDEPENDENT USER\n exact  " },
      },
    ],
  );
  assert.deepEqual(
    updates
      .filter((update) => update.type === "agent_message_chunk")
      .map((update) => ({ source: update.source_message_uuid, content: update.content })),
    [
      {
        source: "saved-visible-assistant",
        content: { type: "text", text: "INDEPENDENT ASSISTANT\n exact  " },
      },
    ],
  );
  const calls = updates.filter((update) => update.type === "tool_call");
  assert.deepEqual(
    calls.map((update) => ({
      id: update.tool_call.tool_call_id,
      source: update.tool_call.source_message_uuid,
      input: update.tool_call.raw_input,
    })),
    [
      {
        id: "saved-independent-bash",
        source: "saved-visible-assistant",
        input: { command: "printf 'INDEPENDENT COMMAND'" },
      },
    ],
  );
  const results = updates.filter((update) => update.type === "tool_call_update");
  assert.deepEqual(
    results.map((update) => ({
      id: update.tool_call_update.tool_call_id,
      source: update.tool_call_update.source_message_uuid,
      status: update.tool_call_update.fields.status,
      raw: update.tool_call_update.fields.raw_output,
      content: update.tool_call_update.fields.content,
    })),
    [
      {
        id: "saved-independent-bash",
        source: "saved-visible-result",
        status: "completed",
        raw: "INDEPENDENT RESULT\r\n\tend  ",
        content: [
          { type: "content", content: { type: "text", text: "INDEPENDENT RESULT\r\n\tend  " } },
        ],
      },
    ],
  );
  assert.doesNotMatch(
    JSON.stringify(updates),
    /HIDDEN-THINKING-SENTINEL|HIDDEN-SYSTEM-SENTINEL|HIDDEN-LOCAL-CONTEXT-SENTINEL/,
  );
});

test("saved ToolSearch calls and result sentinels stay absent from every replay channel while a visible Bash sibling keeps exact input and output", () => {
  const updates = mapSessionMessagesToUpdates([
    savedRecord("assistant", "saved-search-and-bash", {
      role: "assistant",
      content: [
        {
          type: "server_tool_use",
          id: "hidden-saved-tool-search",
          name: "ToolSearch",
          input: { query: "HIDDEN-SEARCH-INPUT-SENTINEL" },
        },
        {
          type: "tool_use",
          id: "visible-saved-bash",
          name: "Bash",
          input: { command: "printf 'VISIBLE-SAVED-COMMAND'" },
        },
        { type: "text", text: "VISIBLE-SAVED-ASSISTANT" },
      ],
    }),
    savedRecord("user", "saved-search-and-bash-results", {
      role: "user",
      content: [
        {
          type: "tool_search_tool_result",
          tool_use_id: "hidden-saved-tool-search",
          content: "HIDDEN-SEARCH-RESULT-SENTINEL",
          is_error: false,
        },
        {
          type: "tool_result",
          tool_use_id: "visible-saved-bash",
          content: "VISIBLE-SAVED-OUTPUT\n",
          is_error: false,
        },
      ],
    }),
  ]);
  assert.doesNotMatch(
    JSON.stringify(updates),
    /hidden-saved-tool-search|HIDDEN-SEARCH-INPUT-SENTINEL|HIDDEN-SEARCH-RESULT-SENTINEL/,
  );
  assert.deepEqual(
    updates
      .filter((update) => update.type === "tool_call")
      .map((update) => ({ id: update.tool_call.tool_call_id, input: update.tool_call.raw_input })),
    [{ id: "visible-saved-bash", input: { command: "printf 'VISIBLE-SAVED-COMMAND'" } }],
  );
  assert.deepEqual(
    updates
      .filter((update) => update.type === "tool_call_update")
      .map((update) => ({
        id: update.tool_call_update.tool_call_id,
        status: update.tool_call_update.fields.status,
        raw: update.tool_call_update.fields.raw_output,
      })),
    [{ id: "visible-saved-bash", status: "completed", raw: "VISIBLE-SAVED-OUTPUT\n" }],
  );
  assert.deepEqual(
    updates
      .filter((update) => update.type === "agent_message_chunk")
      .map((update) => update.content),
    [{ type: "text", text: "VISIBLE-SAVED-ASSISTANT" }],
  );
});

test("mapSessionMessagesToUpdates preserves parallel tool results", () => {
  const updates = mapSessionMessagesToUpdates([
    {
      type: "assistant",
      uuid: "a1",
      session_id: "s1",
      parent_tool_use_id: null,
      parent_agent_id: null,
      message: {
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "tool-a",
            name: "Bash",
            input: { command: "echo a" },
          },
          {
            type: "tool_use",
            id: "tool-b",
            name: "Bash",
            input: { command: "echo b" },
          },
        ],
      },
    },
    {
      type: "user",
      uuid: "u1",
      session_id: "s1",
      parent_tool_use_id: null,
      parent_agent_id: null,
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "tool-b",
            content: "result b",
            is_error: false,
          },
          {
            type: "tool_result",
            tool_use_id: "tool-a",
            content: "result a",
            is_error: false,
          },
        ],
      },
    },
  ]);

  const toolCalls = updates.filter((update) => update.type === "tool_call");
  const toolUpdates = updates.filter(
    (update) => update.type === "tool_call_update",
  );

  assert.deepEqual(
    toolCalls.map((update) => update.tool_call.tool_call_id),
    ["tool-a", "tool-b"],
  );
  assert.deepEqual(
    toolUpdates.map((update) => update.tool_call_update.tool_call_id),
    ["tool-b", "tool-a"],
  );
  assert.deepEqual(
    toolUpdates.map((update) => update.tool_call_update.fields.raw_output),
    ["result b", "result a"],
  );
});

test("live non-execution metadata joins independently permuted result identities without corrupting malformed, unmatched or future-kind siblings", () => {
  const session = makeSessionState();
  session.sessionId = "non-execution-owner";
  const cases = [
    { id: "reject", kind: "user-rejected", status: "failed", error: false },
    { id: "permission", kind: "permission-rule", status: "failed", error: false },
    { id: "unavailable", kind: "automode-unavailable", status: "failed", error: false },
    { id: "parsing", kind: "automode-parsing-error", status: "failed", error: false },
    { id: "blocked", kind: "automode-blocked", status: "failed", error: false },
    { id: "cancel", kind: "cancelled", status: "killed", error: false },
    { id: "interrupt", kind: "interrupted", status: "killed", error: false },
    { id: "future-success", kind: "future-independent-kind", status: "completed", error: false },
    { id: "future-error", kind: "future-independent-kind", status: "failed", error: true },
    { id: "plain", kind: undefined, status: "completed", error: false },
  ];
  const results = [
    cases[4]!,
    cases[0]!,
    cases[8]!,
    cases[2]!,
    cases[6]!,
    cases[9]!,
    cases[1]!,
    cases[7]!,
    cases[5]!,
    cases[3]!,
  ];
  const metadata = [
    cases[7]!,
    cases[5]!,
    cases[2]!,
    cases[8]!,
    cases[0]!,
    cases[3]!,
    cases[6]!,
    cases[1]!,
    cases[4]!,
  ];
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "nonexecution-launch-source",
      session_id: session.sessionId,
      message: {
        role: "assistant",
        content: cases.map((row) => ({
          type: "tool_use",
          id: row.id,
          name: "Bash",
          input: { command: `printf '${row.id}'` },
        })),
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "user",
      uuid: "nonexecution-result-source",
      session_id: session.sessionId,
      message: {
        role: "user",
        content: results.map((row) => ({
          type: "tool_result",
          tool_use_id: row.id,
          content: `INDEPENDENT-BODY-${row.id}`,
          is_error: row.error,
        })),
      },
      tool_result_meta: [
        ...metadata.map((row) => ({
          id: row.id,
          non_execution_kind: row.kind,
          ...(row.id === "reject" ? { user_feedback: "INDEPENDENT-SAFER-COMMAND-FEEDBACK" } : {}),
        })),
        { id: "different-unlaunched-tool", non_execution_kind: "cancelled" },
        { id: "plain", non_execution_kind: "" },
        { id: "plain", non_execution_kind: 7 },
        { id: "", non_execution_kind: "cancelled" },
        null,
      ],
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });
  for (const row of cases) {
    const tool = publishedTool(events, session.sessionId, row.id);
    assert.equal(tool.status, row.status, row.id);
    assert.equal(tool.raw_output, `INDEPENDENT-BODY-${row.id}`);
    assert.equal(
      (tool.output_metadata as { non_execution?: { kind?: string } } | undefined)?.non_execution
        ?.kind,
      row.kind,
      row.id,
    );
    assert.equal(tool.source_message_uuid, "nonexecution-result-source");
  }
  const rejected = publishedTool(events, session.sessionId, "reject");
  assert.equal(
    (rejected.output_metadata as { non_execution: { user_feedback: string } }).non_execution
      .user_feedback,
    "INDEPENDENT-SAFER-COMMAND-FEEDBACK",
  );
  assert.deepEqual(publishedToolFrames(events, session.sessionId, "different-unlaunched-tool"), []);
});

test("saved task lifecycle projection associates inferred and explicit metadata by task ID and interrupts only unfinished work", () => {
  const system = (id: string, message: Record<string, unknown>) =>
    savedRecord("system", id, { type: "system", ...message });
  const updates = mapSessionMessagesToUpdates([
    savedRecord("assistant", "saved-agent-launch", {
      role: "assistant",
      content: [
        {
          type: "tool_use",
          id: "inferred-agent-tool",
          name: "Agent",
          input: { prompt: "INDEPENDENT-INFERRED-PROMPT", subagent_type: "inferred-launch-type" },
        },
        {
          type: "tool_use",
          id: "explicit-agent-tool",
          name: "Agent",
          input: { prompt: "MUST-NOT-OVERRIDE-EXPLICIT-PROMPT" },
        },
      ],
    }),
    system("inferred-start", {
      subtype: "task_started",
      task_id: "inferred-task",
      tool_use_id: "inferred-agent-tool",
      description: "INDEPENDENT-INFERRED-SUBJECT",
      subagent_type: "general-purpose",
      is_backgrounded: false,
      spawn_depth: 2,
    }),
    system("inferred-update", {
      subtype: "task_updated",
      task_id: "inferred-task",
      patch: {
        status: "running",
        description: "INDEPENDENT-LATER-DESCRIPTION",
        is_backgrounded: true,
      },
    }),
    system("explicit-start", {
      subtype: "task_started",
      task_id: "explicit-task",
      tool_use_id: "explicit-agent-tool",
      description: "INDEPENDENT-EXPLICIT-DESCRIPTION",
      task_type: "remote_agent",
      prompt: "INDEPENDENT-EXPLICIT-PROMPT",
      subagent_type: "explicit-record-type",
    }),
    system("remote-start", {
      subtype: "task_started",
      task_id: "unlinked-remote",
      task_type: "remote_agent",
      task_description: "INDEPENDENT-REMOTE-SUBJECT",
      description: "INDEPENDENT-REMOTE-DESCRIPTION",
      prompt: "INDEPENDENT-REMOTE-PROMPT",
    }),
    system("mcp-start", {
      subtype: "task_started",
      task_id: "unlinked-mcp",
      task_type: "mcp",
      workflow_name: "independent-docs-workflow",
      description: "INDEPENDENT-MCP-DESCRIPTION",
    }),
    system("terminal-start", {
      subtype: "task_started",
      task_id: "known-terminal-task",
      description: "INDEPENDENT-TERMINAL-SUBJECT",
    }),
    system("terminal-end", {
      subtype: "task_notification",
      task_id: "known-terminal-task",
      status: "completed",
      summary: "INDEPENDENT-SAVED-TERMINAL-SUMMARY",
      output_file: "C:/owned/known-terminal.output",
    }),
  ]);
  const rows = (id: string) =>
    updates.flatMap((update) =>
      update.type === "task_state_update" ? update.tasks.filter((task) => task.task_id === id) : [],
    );
  const local = rows("inferred-task");
  assert.ok(
    local.some(
      (task) =>
        task.status === "in_progress" &&
        task.description === "INDEPENDENT-LATER-DESCRIPTION" &&
        (task.metadata as Record<string, unknown> | undefined)?.is_backgrounded === true,
    ),
  );
  const finalLocal = local.at(-1)!;
  assert.equal(finalLocal.source_tool_call_id, "inferred-agent-tool");
  assert.equal(
    (finalLocal.metadata as Record<string, unknown> | undefined)?.task_type,
    "local_agent",
  );
  assert.equal((finalLocal.metadata as Record<string, unknown> | undefined)?.spawn_depth, 2);
  const explicit = rows("explicit-task").at(-1)!;
  assert.equal(explicit.source_tool_call_id, "explicit-agent-tool");
  assert.equal(
    (explicit.metadata as Record<string, unknown> | undefined)?.task_type,
    "remote_agent",
  );
  assert.equal(
    (explicit.metadata as Record<string, unknown> | undefined)?.prompt,
    "INDEPENDENT-EXPLICIT-PROMPT",
  );
  assert.equal(
    (explicit.metadata as Record<string, unknown> | undefined)?.subagent_type,
    "explicit-record-type",
  );
  for (const [id, type, subject, description] of [
    [
      "unlinked-remote",
      "remote_agent",
      "INDEPENDENT-REMOTE-SUBJECT",
      "INDEPENDENT-REMOTE-DESCRIPTION",
    ],
    ["unlinked-mcp", "mcp", "independent-docs-workflow", "INDEPENDENT-MCP-DESCRIPTION"],
  ]) {
    const task = rows(id!).at(-1)!;
    assert.equal((task.metadata as Record<string, unknown> | undefined)?.task_type, type);
    assert.equal(task.subject, subject);
    assert.equal(task.description, description);
    assert.equal(task.source_tool_call_id, undefined);
  }
  for (const id of ["inferred-task", "explicit-task", "unlinked-remote", "unlinked-mcp"]) {
    const task = rows(id).at(-1)!;
    assert.equal(task.status, "completed");
    assert.equal(
      (task.metadata as Record<string, unknown> | undefined)?.terminal_status,
      "interrupted",
    );
    assert.equal((task.metadata as Record<string, unknown> | undefined)?.output_file, undefined);
  }
  const terminal = rows("known-terminal-task").at(-1)!;
  assert.equal(terminal.status, "completed");
  assert.equal(
    (terminal.metadata as Record<string, unknown> | undefined)?.terminal_status,
    "completed",
  );
  assert.equal(terminal.description, "INDEPENDENT-SAVED-TERMINAL-SUMMARY");
  assert.equal(
    (terminal.metadata as Record<string, unknown> | undefined)?.output_file,
    "C:/owned/known-terminal.output",
  );
});

test("native background inventory replaces only its own tasks and publishes ambient true-to-false transitions while ordinary tasks remain usable", () => {
  const session = makeSessionState();
  session.sessionId = "background-replacement-owner";
  taskOperationStart(session, "ordinary-create-tool", "TaskCreate", {
    subject: "INDEPENDENT-ORDINARY-SUBJECT",
    description: "INDEPENDENT-ORDINARY-DESCRIPTION",
  });
  const ordinary = taskOperationResult(session, "ordinary-create-tool", {
    task: { id: "ordinary-retained-task", subject: "INDEPENDENT-ORDINARY-SUBJECT" },
  });
  const snapshot = (uuid: string, tasks: unknown[]) =>
    captureBridgeEvents(() =>
      handleSdkMessage(session, {
        type: "system",
        subtype: "background_tasks_changed",
        uuid,
        session_id: session.sessionId,
        tasks,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
  const first = snapshot("background-initial", [
    {
      task_id: "obsolete-background",
      description: "INDEPENDENT-OLD-BACKGROUND",
      task_type: "workflow_agent",
      ambient: false,
    },
    {
      task_id: "retained-background",
      description: "INDEPENDENT-AMBIENT-TRUE",
      task_type: "artifact_watch",
      ambient: true,
    },
  ]);
  const replacement = snapshot("background-replacement", [
    {
      task_id: "retained-background",
      description: "INDEPENDENT-AMBIENT-FALSE",
      task_type: "artifact_watch",
      ambient: false,
    },
    {
      task_id: "new-background",
      description: "INDEPENDENT-NEW-BACKGROUND",
      task_type: "workflow_agent",
      ambient: true,
    },
  ]);
  const changed = taskStateEvents(replacement, session.sessionId);
  assert.equal(changed.length, 1);
  assert.deepEqual(changed[0]?.removed_task_ids, ["obsolete-background"]);
  assert.equal(changed[0]?.is_complete_snapshot, true);
  assert.equal(changed[0]?.source, "background_tasks");
  const byId = Object.fromEntries(
    (changed[0]!.tasks as Array<Record<string, unknown>>).map((task) => [task.task_id, task]),
  );
  assert.deepEqual(Object.keys(byId).sort(), ["new-background", "retained-background"]);
  assert.equal(byId["retained-background"]?.description, "INDEPENDENT-AMBIENT-FALSE");
  assert.equal(byId["retained-background"]?.status, "in_progress");
  const retainedMetadata = byId["retained-background"]?.metadata as Record<string, unknown> | undefined;
  const newMetadata = byId["new-background"]?.metadata as Record<string, unknown> | undefined;
  assert.ok(retainedMetadata);
  assert.ok(newMetadata);
  assert.equal(retainedMetadata.ambient, false);
  assert.equal(retainedMetadata.sdk_background_task, true);
  assert.equal(newMetadata.ambient, true);
  assert.equal(
    (
      publishedTaskRows(first, session.sessionId, "retained-background")[0]?.metadata as
        | Record<string, unknown>
        | undefined
    )?.ambient,
    true,
  );
  const empty = snapshot("background-clear", []);
  const removedIds = taskStateEvents(empty, session.sessionId)[0]?.removed_task_ids;
  assert.ok(Array.isArray(removedIds));
  assert.deepEqual(removedIds.slice().sort(), ["new-background", "retained-background"]);
  assert.equal(
    [...replacement, ...empty].some((event) =>
      (event.update as { removed_task_ids?: string[] } | undefined)?.removed_task_ids?.includes(
        "ordinary-retained-task",
      ),
    ),
    false,
  );
  // A supported TaskGet read demonstrates retained ordinary ownership after both replacements.
  const start = taskOperationStart(session, "ordinary-get-tool", "TaskGet", {
    taskId: "ordinary-retained-task",
  });
  const detail = taskOperationResult(session, "ordinary-get-tool", {
    task: {
      id: "ordinary-retained-task",
      subject: "INDEPENDENT-ORDINARY-SUBJECT",
      status: "pending",
    },
  });
  assert.equal(
    publishedTaskRows(detail, session.sessionId, "ordinary-retained-task")[0]?.subject,
    "INDEPENDENT-ORDINARY-SUBJECT",
  );
  assert.equal(
    publishedTaskRows(detail, session.sessionId, "ordinary-retained-task")[0]?.description,
    "INDEPENDENT-ORDINARY-DESCRIPTION",
  );
  assert.ok(ordinary.length && start.length);
});

test("native terminal result ingress publishes one scoped outcome with literal supported reasons while successful telemetry stays nonsemantic", async () => {
  const rows = [
    {
      name: "completed",
      message: { subtype: "success", terminal_reason: "completed" },
      expected: { event: "turn_complete", terminal_reason: "completed" },
    },
    {
      name: "malformed-tool",
      message: { subtype: "success", terminal_reason: "malformed_tool_use_exhausted" },
      expected: { event: "turn_complete", terminal_reason: "malformed_tool_use_exhausted" },
    },
    {
      name: "budget",
      message: { subtype: "success", terminal_reason: "budget_exhausted" },
      expected: { event: "turn_complete", terminal_reason: "budget_exhausted" },
    },
    {
      name: "structured",
      message: { subtype: "success", terminal_reason: "structured_output_retry_exhausted" },
      expected: { event: "turn_complete", terminal_reason: "structured_output_retry_exhausted" },
    },
    {
      name: "deferred-unavailable",
      message: { subtype: "success", terminal_reason: "tool_deferred_unavailable" },
      expected: { event: "turn_complete", terminal_reason: "tool_deferred_unavailable" },
    },
    {
      name: "setup",
      message: { subtype: "success", terminal_reason: "turn_setup_failed" },
      expected: { event: "turn_complete", terminal_reason: "turn_setup_failed" },
    },
    {
      name: "telemetry",
      message: {
        subtype: "success",
        ttft_stream_ms: 42,
        time_to_request_ms: 12,
        time_to_request_from_spawn_ms: 7,
        warm_spare_claimed: true,
      },
      expected: { event: "turn_complete" },
    },
    {
      name: "max-turns",
      message: {
        subtype: "error_max_turns",
        terminal_reason: "max_turns",
        errors: ["INDEPENDENT-MAX-TURNS-CAUSE"],
      },
      expected: {
        event: "turn_error",
        message: "INDEPENDENT-MAX-TURNS-CAUSE",
        error_kind: "plan_limit",
        sdk_result_subtype: "error_max_turns",
        terminal_reason: "max_turns",
      },
    },
  ];
  for (const row of rows) {
    const session = makeSessionState();
    session.sessionId = `terminal-${row.name}`;
    const events = await captureBridgeEventsAsync(async () =>
      handleSdkMessage(session, {
        type: "result",
        uuid: `terminal-${row.name}-source`,
        session_id: session.sessionId,
        ...row.message,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage),
    );
    assert.deepEqual(
      events.filter((event) => event.event === "turn_complete" || event.event === "turn_error"),
      [{ session_id: session.sessionId, ...row.expected }],
      row.name,
    );
    session.input.close();
  }
});

function guardedCandidate() {
  const session = makeSessionState();
  session.sessionId = `guard-candidate-${crypto.randomUUID()}`;
  session.connected = false;
  session.deferConnect = true;
  session.resumeDropsTurn = "independent-dropped-user-message";
  const fixture = commandInitQuery(session, async () => []);
  const frames = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKMessage>();
  const fenceEntered = bridgeDeferred<void>(),
    releaseFence = bridgeDeferred<void>(),
    refusalConsumed = bridgeDeferred<void>();
  let rejectFence = false;
  const calls: unknown[] = [];
  Object.assign(session.query, {
    initializationResult: async () => ({
      models: [],
      agents: [],
      commands: [],
      account: { apiKeySource: "fixture" },
    }),
    async getContextUsage(options: unknown) {
      calls.push(options);
      fenceEntered.resolve();
      await releaseFence.promise;
      if (rejectFence) throw new Error("INDEPENDENT-GUARD-FENCE-CAUSE");
      return { percentage: 1 };
    },
    close: () => frames.close(),
    async *[Symbol.asyncIterator]() {
      for await (const frame of frames) {
        yield frame;
        if (String(frame.uuid) === "guard-native-refusal") refusalConsumed.resolve();
      }
    },
  });
  sessions.set(session.sessionId, session);
  return {
    session,
    fixture,
    frames,
    fenceEntered,
    releaseFence,
    refusalConsumed,
    calls,
    rejectFence: () => {
      rejectFence = true;
    },
    async cleanup() {
      releaseFence.resolve();
      frames.close();
      await boundedBridgeTask(
        Promise.allSettled([session.initializationTask, session.queryConsumerTask]),
      );
      await fixture.settle();
      await closeSession(session);
      if (sessions.get(session.sessionId) === session) sessions.delete(session.sessionId);
    },
  };
}
test("guarded resume blocks commit on its actual summary fence, consumes queued native refusal and admits only a successfully verified candidate", async () => {
  for (const outcome of ["success", "native-refusal", "fence-rejection"])
    await withCommandTargets(async (source, peer) => {
      const candidate = guardedCandidate();
      candidate.session.connectEvent = "session_replaced";
      candidate.session.sessionsToCloseAfterConnect = [source];
      const candidateId = candidate.session.sessionId;
      let admission: Promise<{ error?: unknown }> | undefined;
      const events = await captureBridgeEventsAsync(async () => {
        try {
          startSessionTasks(candidate.session);
          admission = awaitSessionInitialization(candidate.session).then(
            () => ({}),
            (error) => ({ error }),
          );
          await boundedBridgeTask(candidate.fenceEntered.promise);
          assert.deepEqual(candidate.calls, [{ detail: "summary" }]);
          assert.throws(() => commitDeferredSession(candidate.session), /not ready/);
          assert.equal(sessions.get(source.sessionId), source);
          if (outcome === "native-refusal") {
            candidate.frames.enqueue({
              type: "result",
              subtype: "error_during_execution",
              uuid: "guard-native-refusal",
              errors: ["Resume rejected by --resume-drops-turn: INDEPENDENT-SOURCE-CHANGED"],
            } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
            await boundedBridgeTask(candidate.refusalConsumed.promise);
          } else if (outcome === "fence-rejection") candidate.rejectFence();
          candidate.releaseFence.resolve();
          const result = await boundedBridgeTask(admission);
          await candidate.fixture.settle();
          if (outcome === "success") {
            assert.equal(result.error, undefined);
            commitDeferredSession(candidate.session);
            assert.equal(sessions.has(source.sessionId), false);
          } else {
            assert.ok(result.error);
            assert.match(
              String(result.error),
              outcome === "native-refusal"
                ? /INDEPENDENT-SOURCE-CHANGED/
                : /INDEPENDENT-GUARD-FENCE-CAUSE/,
            );
            assert.throws(() => commitDeferredSession(candidate.session), /not ready/);
            assert.equal(sessions.get(source.sessionId), source);
          }
          emitSessionUpdate(peer.sessionId, { type: "session_status_update", status: "idle" });
        } finally {
          candidate.releaseFence.resolve();
          if (admission) await boundedBridgeTask(admission);
          await candidate.cleanup();
        }
      });
      const published = events.filter(
        (event) =>
          (event.event === "connected" || event.event === "session_replaced") &&
          event.session_id === candidateId,
      );
      assert.equal(published.length, outcome === "success" ? 1 : 0, outcome);
      assert.equal(
        events.some((event) => event.event === "turn_error" && event.session_id === candidateId),
        false,
      );
      assert.ok(
        events.some(
          (event) =>
            event.session_id === peer.sessionId &&
            (event.update as { type?: string } | undefined)?.type === "session_status_update",
        ),
      );
    });
});

test("a genuinely fenced deferred candidate commits its final identity before each correlated buffered payload and auth hint exactly once while a peer remains live", async () => {
  await withCommandTargets(async (_source, peer) => {
    const candidate = guardedCandidate(),
      provisionalId = candidate.session.sessionId,
      finalId = `final-${provisionalId}`;
    const premature: Array<Record<string, unknown>> = [];
    let committing = false,
      admission: Promise<void> | undefined;
    const events = await captureBridgeEventsAsync(
      async () => {
        try {
          startSessionTasks(candidate.session);
          admission = awaitSessionInitialization(candidate.session);
          await boundedBridgeTask(candidate.fenceEntered.promise);
          emitSessionUpdate(
            provisionalId,
            { type: "session_status_update", status: "idle" },
            "buffered-status-request",
          );
          writeEvent(
            { event: "runtime_reload_completed", session_id: provisionalId, error_count: 3 },
            "buffered-reload-request",
          );
          emitAuthRequired(candidate.session, "INDEPENDENT-DEFERRED-LOGIN-GUIDANCE");
          emitSessionUpdate(
            peer.sessionId,
            { type: "session_status_update", status: "requesting" },
            "peer-live-request",
          );
          updateSessionId(candidate.session, finalId);
          candidate.releaseFence.resolve();
          await boundedBridgeTask(admission);
          await candidate.fixture.settle();
          committing = true;
          commitDeferredSession(candidate.session);
          assert.throws(() => commitDeferredSession(candidate.session), /not ready/);
          await candidate.fixture.settle();
        } finally {
          candidate.releaseFence.resolve();
          if (admission) await boundedBridgeTask(admission);
          await candidate.cleanup();
        }
      },
      (event) => {
        if (!committing) premature.push(event);
      },
    );
    assert.equal(
      premature.some(
        (event) =>
          event.session_id === provisionalId ||
          event.session_id === finalId ||
          event.event === "auth_required",
      ),
      false,
    );
    assert.ok(
      premature.some(
        (event) => event.request_id === "peer-live-request" && event.session_id === peer.sessionId,
      ),
    );
    const connection = events.findIndex(
      (event) => event.event === "connected" && event.session_id === finalId,
    );
    assert.ok(connection >= 0);
    const expected = [
      {
        request_id: "buffered-status-request",
        event: "session_update",
        session_id: finalId,
        update: { type: "session_status_update", status: "idle" },
      },
      {
        request_id: "buffered-reload-request",
        event: "runtime_reload_completed",
        session_id: finalId,
        error_count: 3,
      },
    ];
    for (const payload of expected) {
      const matching = events.filter((event) => event.request_id === payload.request_id);
      assert.deepEqual(matching, [payload]);
      assert.ok(events.indexOf(matching[0]!) > connection);
    }
    const hints = events.filter((event) => event.event === "auth_required");
    assert.equal(hints.length, 1);
    assert.equal(hints[0]?.method_description, "INDEPENDENT-DEFERRED-LOGIN-GUIDANCE");
    assert.ok(events.indexOf(hints[0]!) > connection);
    assert.equal(
      events.some((event) => event.session_id === provisionalId),
      false,
    );
  });
});

test("native assistant error authority and independently specified API status shapes survive conflicting result signals on the intended session", async () => {
  const cases = [
    { code: "account_on_hold", kind: "account_access" },
    { code: "cloud_credential_error", kind: "account_access" },
    { code: "model_not_found", kind: "model_unavailable" },
    { code: "oauth_org_not_allowed", kind: "account_access" },
    { code: "overloaded", kind: "transient_service" },
    { code: "verification_required", kind: "auth_required" },
    { code: "server_error", kind: "transient_service" },
    { code: "authentication_failed", kind: "auth_required" },
    { code: "billing_error", kind: "plan_limit" },
    { code: "rate_limit", kind: "plan_limit" },
    { code: "invalid_request", kind: "other" },
    { code: "unknown", kind: "plan_limit" },
    { code: "max_output_tokens", kind: "plan_limit" },
    { code: "future-unrecognized-code", kind: "plan_limit" },
  ];
  for (const row of cases) {
    const session = makeSessionState();
    session.sessionId = `native-error-${row.code}`;
    const cause = "INDEPENDENT CAUSE; Please run /login";
    const events = await captureBridgeEventsAsync(async () => {
      handleSdkMessage(session, {
        type: "assistant",
        uuid: `${row.code}-observation`,
        session_id: session.sessionId,
        error: row.code,
        message: { role: "assistant", content: [] },
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      handleSdkMessage(session, {
        type: "result",
        uuid: `${row.code}-result`,
        session_id: session.sessionId,
        subtype: "error_max_turns",
        errors: [cause],
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    });
    const expected: Record<string, unknown> = {
      event: "turn_error",
      session_id: session.sessionId,
      message: cause,
      error_kind: row.kind,
      sdk_result_subtype: "error_max_turns",
    };
    expected.assistant_error = row.code === "future-unrecognized-code" ? "unknown" : row.code;
    assert.deepEqual(
      events.filter((event) => event.event === "turn_error" || event.event === "turn_complete"),
      [expected],
      row.code,
    );
    session.input.close();
  }
  for (const row of [
    { name: "429", input: { api_error_status: 429 }, expected: { api_error_status: 429 } },
    { name: "529", input: { api_error_status: 529 }, expected: { api_error_status: 529 } },
    { name: "null", input: { api_error_status: null }, expected: {} },
    { name: "absent", input: {}, expected: {} },
  ]) {
    const session = makeSessionState();
    session.sessionId = `api-status-${row.name}`;
    const events = await captureBridgeEventsAsync(async () => {
      handleSdkMessage(session, {
        type: "assistant",
        error: "overloaded",
        message: { role: "assistant", content: [] },
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      handleSdkMessage(session, {
        type: "result",
        subtype: "error_during_execution",
        errors: ["INDEPENDENT-STATUS-CAUSE"],
        ...row.input,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    });
    assert.deepEqual(
      events.filter((event) => event.event === "turn_error" || event.event === "turn_complete"),
      [
        {
          event: "turn_error",
          session_id: session.sessionId,
          message: "INDEPENDENT-STATUS-CAUSE",
          error_kind: "transient_service",
          sdk_result_subtype: "error_during_execution",
          assistant_error: "overloaded",
          ...row.expected,
        },
      ],
    );
    session.input.close();
  }
});

test("mapSdkSessions normalizes and sorts sessions", () => {
  const mapped = mapSdkSessions([
    {
      sessionId: "older",
      summary: " Older summary ",
      lastModified: 100,
      fileSize: 10,
      cwd: "C:/work",
    },
    {
      sessionId: "latest",
      summary: "",
      lastModified: 200,
      fileSize: 20,
      customTitle: "Custom title",
      gitBranch: "main",
      firstPrompt: "hello",
    },
  ]);

  assert.deepEqual(mapped, [
    {
      session_id: "latest",
      summary: "Custom title",
      last_modified_ms: 200,
      file_size_bytes: 20,
      git_branch: "main",
      custom_title: "Custom title",
      first_prompt: "hello",
    },
    {
      session_id: "older",
      summary: "Older summary",
      last_modified_ms: 100,
      file_size_bytes: 10,
      cwd: "C:/work",
    },
  ]);
});

test("mapAvailableModels preserves optional fast and auto mode metadata", () => {
  const mapped = mapAvailableModels([
    {
      value: "sonnet",
      resolvedModel: "claude-sonnet-5",
      displayName: "Claude Sonnet",
      description: "Balanced model",
      supportsEffort: true,
      supportedEffortLevels: ["low", "medium", "high", "xhigh", "max"],
      supportsAdaptiveThinking: true,
      supportsFastMode: true,
      supportsAutoMode: false,
    },
    {
      value: "haiku",
      displayName: "Claude Haiku",
      description: "Fast model",
      supportsEffort: false,
    },
  ]);

  assert.deepEqual(mapped, [
    {
      id: "sonnet",
      resolved_model: "claude-sonnet-5",
      display_name: "Claude Sonnet",
      description: "Balanced model",
      supports_effort: true,
      supported_effort_levels: ["low", "medium", "high", "xhigh", "max"],
      supports_adaptive_thinking: true,
      supports_fast_mode: true,
      supports_auto_mode: false,
    },
    {
      id: "haiku",
      display_name: "Claude Haiku",
      description: "Fast model",
      supports_effort: false,
      supported_effort_levels: [],
    },
  ]);
});

test("actual initialization publishes Fable, dated and opaque managed model identities with the native roster capabilities", async () => {
  const session = makeSessionState();
  session.sessionId = "native-model-roster-owner";
  session.connected = false;
  const fixture = commandInitQuery(session, async () => []);
  const frames = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKMessage>();
  Object.assign(session.query, {
    initializationResult: async () => ({
      models: [
        {
          value: "fable",
          resolvedModel: "claude-fable-5",
          displayName: "Independent Fable alias",
          description: "INDEPENDENT-ALIAS-DESCRIPTION",
          supportsEffort: true,
          supportedEffortLevels: ["high", "max"],
          supportsFastMode: false,
        },
        {
          value: "claude-fable-5-20260612",
          displayName: "Independent dated Fable",
          description: "INDEPENDENT-DATED-DESCRIPTION",
          supportsEffort: false,
        },
        {
          value: "managed/future-id",
          displayName: "Independent managed model",
          description: "INDEPENDENT-MANAGED-DESCRIPTION",
          supportsEffort: true,
          supportedEffortLevels: ["low"],
          supportsAutoMode: true,
        },
      ],
      agents: [],
      commands: [],
      account: { apiKeySource: "fixture" },
    }),
    close: () => frames.close(),
    [Symbol.asyncIterator]: () => frames[Symbol.asyncIterator](),
  });
  const events = await captureBridgeEventsAsync(async () => {
    try {
      startSessionTasks(session);
      await boundedBridgeTask(Promise.resolve(session.initializationTask));
      await fixture.settle();
    } finally {
      frames.close();
      await boundedBridgeTask(
        Promise.allSettled([session.initializationTask, session.queryConsumerTask]),
      );
      await fixture.settle();
      session.input.close();
    }
  });
  const connections = events.filter(
    (event) => event.event === "connected" && event.session_id === session.sessionId,
  );
  assert.equal(connections.length, 1);
  assert.deepEqual(connections[0]?.available_models, [
    {
      id: "fable",
      resolved_model: "claude-fable-5",
      display_name: "Independent Fable alias",
      description: "INDEPENDENT-ALIAS-DESCRIPTION",
      supports_effort: true,
      supported_effort_levels: ["high", "max"],
      supports_fast_mode: false,
    },
    {
      id: "claude-fable-5-20260612",
      display_name: "Independent dated Fable",
      description: "INDEPENDENT-DATED-DESCRIPTION",
      supports_effort: false,
      supported_effort_levels: [],
    },
    {
      id: "managed/future-id",
      display_name: "Independent managed model",
      description: "INDEPENDENT-MANAGED-DESCRIPTION",
      supports_effort: true,
      supported_effort_levels: ["low"],
      supports_auto_mode: true,
    },
  ]);
});

test("runtime model display retains context and version while removing only the release-date suffix", () => {
  for (const row of [
    { id: "claude-opus-4-7[1m]", short: "Opus 4.7 [1M]", long: "Opus 4.7 [1M]" },
    { id: "claude-opus-4-5-20251101", short: "Opus 4.5", long: "Opus 4.5" },
    { id: "claude-sonnet-4-7-20261001[1m]", short: "Sonnet 4.7 [1M]", long: "Sonnet 4.7 [1M]" },
  ]) {
    const session = makeSessionState();
    session.resolvedRuntimeModelId = row.id;
    const current = resolveCurrentModel(session);
    assert.equal(current.resolved_id, row.id);
    assert.equal(current.display_name_short, row.short);
    assert.equal(current.display_name_long, row.long);
  }
});

test("resolveCurrentModel does not inherit standard Opus capabilities for 1M when sibling variants exist", () => {
  const session = makeSessionState();
  session.requestedModelId = "claude-opus-4-7";
  session.resolvedRuntimeModelId = "claude-opus-4-7[1m]";
  session.availableModels = [
    {
      id: "claude-opus-4-7",
      display_name: "Claude Opus",
      supports_effort: true,
      supported_effort_levels: ["low", "medium", "high"],
    },
    {
      id: "claude-opus-4-7[1m]",
      display_name: "Claude Opus 1M",
      supports_effort: false,
      supported_effort_levels: [],
    },
  ];

  const currentModel = resolveCurrentModel(session);

  assert.equal(currentModel.catalog_id, "claude-opus-4-7[1m]");
  assert.equal(currentModel.supports_effort, false);
});

test("resolveCurrentModel avoids suffix-insensitive fallback when sibling variants make it ambiguous", () => {
  const session = makeSessionState();
  session.requestedModelId = "claude-opus-4-7";
  session.resolvedRuntimeModelId = "claude-opus-4-7[1m]";
  session.availableModels = [
    {
      id: "claude-opus-4-7",
      display_name: "Claude Opus",
      supports_effort: true,
      supported_effort_levels: ["low", "medium", "high"],
    },
    {
      id: "claude-opus-4-7-alt[1m]",
      display_name: "Claude Opus Alt 1M",
      supports_effort: false,
      supported_effort_levels: [],
    },
  ];

  const currentModel = resolveCurrentModel(session);

  assert.equal(currentModel.catalog_id, undefined);
  assert.equal(currentModel.supports_effort, false);
});

test("a parsed no-op model command waits for native acceptance and applied verification then publishes the same verified identity to its target", async () => {
  await withCommandTargets(async (target, peer) => {
    controlModel(target);
    target.requestedModelId = "haiku";
    refreshCurrentModel(target);
    const calls: string[] = [];
    const entered = bridgeDeferred<void>(),
      accept = bridgeDeferred<void>(),
      readEntered = bridgeDeferred<void>(),
      verify = bridgeDeferred<void>();
    let verified = false,
      premature = false;
    let pending: Promise<void> | undefined;
    const native = {
      close: () => {},
      async setModel(model: string) {
        calls.push(model);
        entered.resolve();
        await accept.promise;
      },
      async getSettings() {
        readEntered.resolve();
        await verify.promise;
        verified = true;
        return {
          applied: {
            model: "haiku",
            effort: "medium",
            ultracodeAvailable: false,
            ultracodeRequested: false,
            ultracode: false,
          },
        };
      },
    };
    target.query = native as unknown as SessionState["query"];
    const events = await captureBridgeEventsAsync(
      async () => {
        try {
          pending = dispatchControl({
            command: "set_model",
            session_id: target.sessionId,
            request_id: "no-op-model-command",
            model: "haiku",
          });
          await boundedBridgeTask(entered.promise);
          assert.deepEqual(calls, ["haiku"]);
          accept.resolve();
          await boundedBridgeTask(readEntered.promise);
          verify.resolve();
          await boundedBridgeTask(pending);
        } finally {
          accept.resolve();
          verify.resolve();
          if (pending) await boundedBridgeTask(pending);
        }
      },
      (event) => {
        if (
          (event.update as { type?: string } | undefined)?.type === "current_model_update" &&
          !verified
        )
          premature = true;
      },
    );
    assert.equal(premature, false);
    const models = events.filter(
      (event) => (event.update as { type?: string } | undefined)?.type === "current_model_update",
    );
    assert.equal(models.length, 1);
    assert.equal(models[0]?.session_id, target.sessionId);
    const publication = models[0];
    assert.ok(publication);
    const model = (publication.update as { current_model: Record<string, unknown> }).current_model;
    assert.equal(model.resolved_id, "haiku");
    assert.equal(model.catalog_id, "haiku");
    assert.equal(model.is_authoritative, true);
    assert.equal(model.supports_fast_mode, true);
    assert.deepEqual(model.supported_effort_levels, ["low", "medium", "high", "xhigh", "max"]);
    assert.equal(
      events.some((event) => event.session_id === peer.sessionId || event.event === "slash_error"),
      false,
    );
  });
});

test("late native initialization metadata enriches the already connected runtime model without replacing its observed identity", async () => {
  const session = makeSessionState();
  session.sessionId = "late-catalog-owner";
  session.connected = false;
  session.resolvedRuntimeModelId = undefined;
  const fixture = commandInitQuery(session, async () => []);
  const initialized = bridgeDeferred<unknown>(),
    frames = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKMessage>();
  Object.assign(session.query, {
    initializationResult: () => initialized.promise,
    getSettings: async () => ({
      applied: {
        model: "sonnet",
        effort: "high",
        ultracodeAvailable: false,
        ultracodeRequested: false,
        ultracode: false,
      },
    }),
    close: () => frames.close(),
    [Symbol.asyncIterator]: () => frames[Symbol.asyncIterator](),
  });
  const metadata = {
    models: [
      {
        value: "haiku",
        displayName: "Wrong catalog",
        supportsEffort: false,
        supportsAutoMode: false,
      },
      {
        value: "sonnet",
        displayName: "Independent enriched Sonnet",
        supportsEffort: true,
        supportedEffortLevels: ["low", "high"],
        supportsAutoMode: true,
      },
    ],
    agents: [],
    commands: [],
    account: { apiKeySource: "fixture" },
  };
  const events = await captureBridgeEventsAsync(async () => {
    try {
      startSessionTasks(session);
      handleSdkMessage(session, {
        type: "system",
        subtype: "init",
        session_id: session.sessionId,
        model: "sonnet",
        effort: "high",
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      await fixture.settle();
      initialized.resolve(metadata);
      await boundedBridgeTask(Promise.resolve(session.initializationTask));
      await fixture.settle();
    } finally {
      initialized.resolve(metadata);
      frames.close();
      await boundedBridgeTask(
        Promise.allSettled([session.initializationTask, session.queryConsumerTask]),
      );
      await fixture.settle();
      session.input.close();
    }
  });
  const connected = events.findIndex(
    (event) => event.event === "connected" && event.session_id === session.sessionId,
  );
  assert.ok(connected >= 0);
  const initial = events[connected]?.current_model as Record<string, unknown>;
  assert.equal(initial.resolved_id, "sonnet");
  assert.equal(initial.supports_effort, false);
  const enriched = events.filter(
    (event) =>
      event.session_id === session.sessionId &&
      (event.update as { type?: string } | undefined)?.type === "current_model_update",
  );
  assert.ok(enriched.length > 0);
  const latest = enriched.at(-1)!;
  assert.ok(events.indexOf(latest) > connected);
  assert.deepEqual((latest.update as { current_model: unknown }).current_model, {
    resolved_id: "sonnet",
    catalog_id: "sonnet",
    display_name_short: "Sonnet",
    display_name_long: "Independent enriched Sonnet",
    supports_effort: true,
    supported_effort_levels: ["low", "high"],
    supports_auto_mode: true,
    is_authoritative: true,
  });
});

test("observed runtime identity selects its own catalog capabilities instead of a conflicting requested alias and retains dated Fable or Opus display", () => {
  for (const row of [
    {
      runtime: "sonnet",
      catalog: "sonnet",
      resolved: undefined,
      short: "Sonnet",
      long: "Independent Sonnet catalog",
    },
    {
      runtime: "claude-fable-5-20260612",
      catalog: "fable",
      resolved: "claude-fable-5",
      short: "Fable 5",
      long: "Independent Fable catalog",
    },
    {
      runtime: "claude-opus-4-7-20260101",
      catalog: "opus",
      resolved: undefined,
      short: "Opus 4.7",
      long: "Independent Opus catalog",
    },
  ]) {
    const session = makeSessionState();
    session.requestedModelId = "haiku";
    session.resolvedRuntimeModelId = row.runtime;
    session.availableModels = [
      {
        id: "haiku",
        display_name: "Wrong requested catalog",
        supports_effort: false,
        supported_effort_levels: [],
        supports_auto_mode: false,
        supports_fast_mode: false,
      },
      {
        id: row.catalog,
        ...(row.resolved ? { resolved_model: row.resolved } : {}),
        display_name: row.long,
        supports_effort: true,
        supported_effort_levels: ["high", "xhigh"],
        supports_auto_mode: true,
        supports_fast_mode: true,
      },
    ];
    const current = resolveCurrentModel(session);
    assert.deepEqual(current, {
      requested_id: "haiku",
      resolved_id: row.runtime,
      display_name_short: row.short,
      display_name_long: row.long,
      catalog_id: row.catalog,
      supports_effort: true,
      supported_effort_levels: ["high", "xhigh"],
      supports_auto_mode: true,
      supports_fast_mode: true,
      is_authoritative: true,
    });
  }
});

function userDialogHandlerForTest(): NonNullable<Options["onUserDialog"]> {
  const input = new AsyncQueue<
    import("@anthropic-ai/claude-agent-sdk").SDKUserMessage
  >();
  const options = buildQueryOptions({
    resolvedSettings: {},
    cwd: "C:/work",
    launchSettings: {},
    provisionalSessionId: "session-dialog",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-dialog",
  });
  assert.deepEqual(options.supportedDialogKinds, ["refusal_fallback_prompt"]);
  const handler = options.onUserDialog;
  assert.ok(handler, "expected buildQueryOptions to declare onUserDialog");
  return handler;
}

function registerDialogSession(): SessionState {
  const session = makeSessionState();
  session.sessionId = "session-dialog";
  sessions.set("session-dialog", session);
  return session;
}

test("accepted model mutations invalidate unverifiable old identity, retain correlation and recover using the actual runtime while rejected mutations preserve prior knowledge", async () => {
  for (const failure of ["read-rejection", "missing-model", "apply-rejection"])
    await withCommandTargets(async (target, peer) => {
      target.requestedModelId = "old-alias";
      target.resolvedRuntimeModelId = "old-runtime";
      target.availableModels = [
        {
          id: "old-alias",
          resolved_model: "old-runtime",
          display_name: "Independent old model",
          supports_effort: true,
          supported_effort_levels: ["low"],
          supports_fast_mode: false,
        },
        {
          id: "new-alias",
          display_name: "Requested but unverified",
          supports_effort: false,
          supported_effort_levels: [],
        },
        {
          id: "runtime-catalog",
          resolved_model: "claude-sonnet-4-7",
          display_name: "Independent actual runtime",
          supports_effort: true,
          supported_effort_levels: ["high", "max"],
          supports_fast_mode: true,
        },
      ];
      refreshCurrentModel(target);
      const calls: string[] = [];
      let recover = false;
      target.query = {
        close: () => {},
        async setModel(model: string) {
          calls.push(model);
          if (!recover && failure === "apply-rejection")
            throw new Error("INDEPENDENT-MODEL-APPLY-CAUSE");
        },
        async getSettings() {
          if (!recover && failure === "read-rejection")
            throw new Error("INDEPENDENT-MODEL-READ-CAUSE");
          return {
            applied: {
              ...(recover
                ? { model: "claude-sonnet-4-7" }
                : failure === "apply-rejection"
                  ? { model: "old-runtime" }
                  : {}),
              effort: "high",
              ultracodeAvailable: false,
              ultracodeRequested: false,
              ultracode: false,
            },
          };
        },
      } as unknown as SessionState["query"];
      const events = await captureBridgeEventsAsync(() =>
        dispatchControl({
          command: "set_model",
          session_id: target.sessionId,
          request_id: `model-${failure}`,
          model: "new-alias",
        }),
      );
      assert.deepEqual(calls, ["new-alias"]);
      assert.ok(
        events.some(
          (event) =>
            event.event === "slash_error" &&
            event.session_id === target.sessionId &&
            event.request_id === `model-${failure}`,
        ),
      );
      const publications = events.filter(
        (event) => (event.update as { type?: string } | undefined)?.type === "current_model_update",
      );
      if (failure === "apply-rejection") {
        assert.deepEqual(publications, []);
        assert.equal(target.currentModel?.resolved_id, "old-runtime");
        assert.equal(target.currentModel?.is_authoritative, true);
      } else {
        assert.equal(publications.length, 1);
        assert.equal(publications[0]?.session_id, target.sessionId);
        const model = (publications[0]!.update as { current_model: Record<string, unknown> })
          .current_model;
        assert.equal(model.is_authoritative, false);
        assert.notEqual(model.resolved_id, "old-runtime");
        assert.notEqual(model.resolved_id, "new-alias");
        assert.equal(model.catalog_id, undefined);
        assert.equal(model.supports_effort, false);
        assert.deepEqual(model.supported_effort_levels, []);
        assert.equal(model.supports_fast_mode, undefined);
      }
      recover = true;
      const recovered = await captureBridgeEventsAsync(() =>
        dispatchControl({
          command: "set_model",
          session_id: target.sessionId,
          request_id: "model-recovered",
          model: "new-alias",
        }),
      );
      const modelEvents = recovered.filter(
        (event) => (event.update as { type?: string } | undefined)?.type === "current_model_update",
      );
      assert.equal(modelEvents.length, 1);
      assert.equal(modelEvents[0]?.session_id, target.sessionId);
      const current = (modelEvents[0]!.update as { current_model: Record<string, unknown> })
        .current_model;
      assert.equal(current.resolved_id, "claude-sonnet-4-7");
      assert.equal(current.requested_id, "new-alias");
      assert.equal(current.catalog_id, "runtime-catalog");
      assert.equal(current.is_authoritative, true);
      assert.equal(current.supports_fast_mode, true);
      assert.deepEqual(current.supported_effort_levels, ["high", "max"]);
      assert.equal(
        [...events, ...recovered].some((event) => event.session_id === peer.sessionId),
        false,
      );
      assert.equal(
        recovered.some((event) => event.event === "slash_error"),
        false,
      );
    });
});

function interactionCallbacks(session: SessionState) {
  const options = buildQueryOptions({
    resolvedSettings: {},
    cwd: "C:/work",
    launchSettings: {},
    provisionalSessionId: session.sessionId,
    input: session.input,
    canUseTool: async () => ({ behavior: "deny", message: "unused" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => session.sessionId,
  });
  assert.ok(options.onUserDialog);
  assert.ok(options.onElicitation);
  return { dialog: options.onUserDialog, elicitation: options.onElicitation };
}
function dispatchInteraction(wire: Record<string, unknown>) {
  const parsed = parseCommandEnvelope(JSON.stringify(wire));
  assert.ok(
    [
      "permission_response",
      "question_response",
      "user_dialog_response",
      "elicitation_response",
    ].includes(parsed.command.command),
  );
  handleInteractionCommand(parsed.command as Parameters<typeof handleInteractionCommand>[0]);
}
function interactionRequest(events: Array<Record<string, unknown>>, kind: string, id: string) {
  const found = events.filter(
    (event) =>
      event.event === kind &&
      (event.request as { request_id?: string } | undefined)?.request_id === id,
  );
  assert.equal(found.length, 1);
  return found[0]!;
}
const nativeDialog = {
  dialogKind: "refusal_fallback_prompt",
  payload: {
    originalModel: "claude-opus-4-8",
    fallbackModel: "claude-sonnet-4-6",
    guidanceText: "INDEPENDENT-REFUSAL-GUIDANCE",
  },
};
const typedForm = {
  serverName: "independent-forms",
  message: "Independent typed form",
  mode: "form" as const,
  requestedSchema: {
    type: "object",
    properties: {
      zero: { type: "number" },
      no: { type: "boolean" },
      blank: { type: "string" },
      tags: { type: "array", items: { type: "string" } },
    },
  },
};
const typedAnswers = { zero: 0, no: false, blank: "", tags: ["independent-a", "independent-b"] };
function answerElicitation(request: Record<string, unknown>, action: string, content?: unknown) {
  dispatchInteraction({
    command: "elicitation_response",
    session_id: request.session_id,
    elicitation_request_id: (request.request as { request_id: string }).request_id,
    action,
    ...(content === undefined ? {} : { content }),
  });
}
test("published native dialogs accept parsed retry, edit and host cancellation without invalid or duplicate responses disturbing a sibling", async () => {
  await withCommandTargets(async (target, peer) => {
    const { dialog } = interactionCallbacks(target);
    const abort = new AbortController();
    const observed: Array<Record<string, unknown>> = [];
    await captureBridgeEventsAsync(
      async () => {
        try {
          for (const choice of ["retry_fallback", "edit_prompt", "cancelled"]) {
            let settled = false,
              siblingSettled = false;
            const pending = dialog(nativeDialog, {
              requestId: `dialog-${choice}`,
              signal: abort.signal,
            }).then((value) => {
              settled = true;
              return value;
            });
            const sibling = dialog(
              {
                ...nativeDialog,
                payload: { ...nativeDialog.payload, guidanceText: "SIBLING-GUIDANCE" },
              },
              { requestId: `sibling-${choice}`, signal: abort.signal },
            ).then((value) => {
              siblingSettled = true;
              return value;
            });
            const event = interactionRequest(observed, "user_dialog_request", `dialog-${choice}`),
              other = interactionRequest(observed, "user_dialog_request", `sibling-${choice}`);
            assert.equal(event.session_id, target.sessionId);
            const request = event.request as {
              request_id: string;
              dialog_kind: string;
              payload: unknown;
              options: Array<{ option_id: string; label: string }>;
            };
            assert.equal(request.dialog_kind, "refusal_fallback_prompt");
            assert.deepEqual(request.payload, {
              original_model: "claude-opus-4-8",
              fallback_model: "claude-sonnet-4-6",
              guidance_text: "INDEPENDENT-REFUSAL-GUIDANCE",
            });
            assert.deepEqual(request.options.map((option) => option.option_id).sort(), [
              "edit_prompt",
              "retry_fallback",
            ]);
            assert.ok(
              request.options
                .find((option) => option.option_id === "retry_fallback")
                ?.label.includes("claude-sonnet-4-6"),
            );
            assert.ok(
              request.options
                .find((option) => option.option_id === "edit_prompt")
                ?.label.includes("claude-opus-4-8"),
            );
            const wire = {
              command: "user_dialog_response",
              session_id: event.session_id,
              request_id: request.request_id,
              outcome:
                choice === "cancelled"
                  ? { outcome: "cancelled" }
                  : { outcome: "selected", option_id: choice },
            };
            assert.throws(() =>
              parseCommandEnvelope(
                JSON.stringify({
                  ...wire,
                  outcome: { outcome: "selected", option_id: "unsupported-choice" },
                }),
              ),
            );
            await Promise.resolve();
            assert.equal(settled, false);
            assert.equal(siblingSettled, false);
            dispatchInteraction(wire);
            assert.deepEqual(
              await boundedBridgeTask(pending),
              choice === "cancelled"
                ? { behavior: "cancelled" }
                : { behavior: "completed", result: choice },
            );
            const count = observed.length;
            dispatchInteraction(wire);
            await Promise.resolve();
            assert.equal(observed.length, count);
            assert.equal(siblingSettled, false);
            dispatchInteraction({
              command: "user_dialog_response",
              session_id: other.session_id,
              request_id: (other.request as { request_id: string }).request_id,
              outcome: { outcome: "selected", option_id: "edit_prompt" },
            });
            assert.deepEqual(await boundedBridgeTask(sibling), {
              behavior: "completed",
              result: "edit_prompt",
            });
          }
          assert.equal(
            observed.some((event) => event.session_id === peer.sessionId),
            false,
          );
        } finally {
          abort.abort();
        }
      },
      (event) => observed.push(event),
    );
  });
});

test("native dialog abort cancels the published interaction exactly once while a sibling remains answerable", async () => {
  await withCommandTargets(async (target) => {
    const { dialog } = interactionCallbacks(target),
      abort = new AbortController(),
      otherAbort = new AbortController();
    const observed: Array<Record<string, unknown>> = [];
    const events = await captureBridgeEventsAsync(
      async () => {
        try {
          const pending = dialog(nativeDialog, { requestId: "native-abort", signal: abort.signal });
          let otherSettled = false;
          const other = dialog(nativeDialog, {
            requestId: "native-survivor",
            signal: otherAbort.signal,
          }).then((value) => {
            otherSettled = true;
            return value;
          });
          const published = interactionRequest(observed, "user_dialog_request", "native-abort"),
            survivor = interactionRequest(observed, "user_dialog_request", "native-survivor");
          assert.equal(published.session_id, target.sessionId);
          abort.abort();
          assert.deepEqual(await boundedBridgeTask(pending), { behavior: "cancelled" });
          const afterAbort = observed.length;
          dispatchInteraction({
            command: "user_dialog_response",
            session_id: published.session_id,
            request_id: (published.request as { request_id: string }).request_id,
            outcome: { outcome: "selected", option_id: "retry_fallback" },
          });
          await Promise.resolve();
          assert.equal(observed.length, afterAbort);
          assert.equal(otherSettled, false);
          dispatchInteraction({
            command: "user_dialog_response",
            session_id: survivor.session_id,
            request_id: (survivor.request as { request_id: string }).request_id,
            outcome: { outcome: "selected", option_id: "retry_fallback" },
          });
          assert.deepEqual(await boundedBridgeTask(other), {
            behavior: "completed",
            result: "retry_fallback",
          });
        } finally {
          abort.abort();
          otherAbort.abort();
        }
      },
      (event) => observed.push(event),
    );
    assert.deepEqual(
      events.filter((event) => event.event === "interaction_cancelled"),
      [
        {
          event: "interaction_cancelled",
          session_id: target.sessionId,
          interaction_id: "native-abort",
        },
      ],
    );
  });
});

test("onUserDialog fails closed on an unknown dialog kind without emitting", async () => {
  const handler = userDialogHandlerForTest();
  const session = registerDialogSession();
  try {
    const events = await captureBridgeEventsAsync(async () => {
      const result = await handler(
        { dialogKind: "some_future_dialog_kind", payload: { anything: true } },
        { signal: new AbortController().signal, requestId: "dialog-unknown-1" },
      );
      assert.equal(result, null);
    });

    assert.equal(
      events.find((event) => event.event === "user_dialog_request"),
      undefined,
    );
    assert.equal(session.pendingUserDialogs.size, 0);
  } finally {
    sessions.delete("session-dialog");
  }
});

test("Ultracode commands validate explicit boolean payloads", () => {
  for (const enabled of [true, false]) {
    const command = { command: "set_ultracode", session_id: "session-1", enabled };
    assert.deepEqual(parseCommandEnvelope(JSON.stringify(command)).command, command);
  }
  for (const enabled of [undefined, "on", 1, null]) {
    assert.throws(() => parseCommandEnvelope(JSON.stringify({ command: "set_ultracode", session_id: "session-1", enabled })), /enabled must be a boolean/);
  }
  const refresh = { command: "refresh_ultracode", session_id: "session-1" };
  assert.deepEqual(parseCommandEnvelope(JSON.stringify(refresh)).command, refresh);
});

test("query options forward explicit checkpoint overrides and saved defaults while preserving host restrictions and supported inbound policy", () => {
  const cases = [
    {
      name: "explicit-enable",
      saved: {
        fileCheckpointingEnabled: false,
        feedbackDrafts: "on",
        crossSessionInbound: "refuse",
      },
      launch: {
        settings: {
          fileCheckpointingEnabled: true,
          feedbackDrafts: "on",
          crossSessionInbound: "accept",
        },
      },
      checkpoint: true,
      inbound: "accept",
    },
    {
      name: "explicit-disable",
      saved: { fileCheckpointingEnabled: true },
      launch: { settings: { fileCheckpointingEnabled: false, crossSessionInbound: "refuse" } },
      checkpoint: false,
      inbound: "refuse",
    },
    {
      name: "saved-disabled",
      saved: { fileCheckpointingEnabled: false, crossSessionInbound: "accept" },
      launch: {},
      checkpoint: false,
      inbound: undefined,
    },
    { name: "default-opt-in", saved: {}, launch: {}, checkpoint: true, inbound: "refuse" },
  ] as const;
  for (const row of cases) {
    const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
    try {
      const options = buildQueryOptions({
        cwd: "C:/work",
        resolvedSettings: row.saved as Parameters<typeof buildQueryOptions>[0]["resolvedSettings"],
        launchSettings: row.launch,
        provisionalSessionId: row.name,
        input,
        canUseTool: async () => ({ behavior: "deny", message: "unused" }),
        enableSdkDebug: false,
        enableSpawnDebug: false,
        sessionIdForLogs: () => row.name,
      });
      assert.equal(options.enableFileCheckpointing, row.checkpoint, row.name);
      assert.equal(options.includeHookEvents, true);
      assert.equal(options.promptSuggestions, true);
      assert.equal(options.agentProgressSummaries, true);
      for (const tool of ["ProposeSkills", "ProposeGoal"])
        assert.ok(options.disallowedTools?.includes(tool));
      const settings = options.settings as Record<string, unknown>;
      assert.equal(settings.feedbackDrafts, "off");
      assert.equal(settings.crossSessionInbound, row.inbound);
      const prompt = options.systemPrompt as {
        type: string;
        preset: string;
        snapshot: boolean;
        append: string;
      };
      assert.equal(prompt.type, "preset");
      assert.equal(prompt.preset, "claude_code");
      assert.equal(prompt.snapshot, true);
      assert.ok(prompt.append.trim().length > 0);
    } finally {
      input.close();
    }
  }
});

test("an inherited model becomes authoritative only when native init publishes its runtime identity and permission mode at connection", async () => {
  const session = makeSessionState();
  session.sessionId = "inherited-native-model";
  session.connected = false;
  session.requestedModelId = undefined;
  session.resolvedRuntimeModelId = undefined;
  session.currentModel = undefined;
  const fixture = commandInitQuery(session, async () => []);
  assert.equal(resolveCurrentModel(session).is_authoritative, false);
  const events = await captureBridgeEventsAsync(async () => {
    try {
      handleSdkMessage(session, {
        type: "system",
        subtype: "init",
        session_id: session.sessionId,
        model: "claude-haiku-4-5",
        permissionMode: "plan",
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      await fixture.settle();
    } finally {
      session.input.close();
    }
  });
  const connections = events.filter((event) => event.event === "connected");
  assert.equal(connections.length, 1);
  const connection = connections[0]!;
  assert.equal(connection.session_id, session.sessionId);
  const model = connection.current_model as Record<string, unknown>;
  assert.equal(model.resolved_id, "claude-haiku-4-5");
  assert.equal(model.is_authoritative, true);
  assert.equal(model.requested_id, undefined);
  assert.equal((connection.mode as { current_mode_id: string }).current_mode_id, "plan");
});

test("a parsed model command acknowledges newer native identity and capabilities throughout stale applied-read success or rejection", async () => {
  for (const rejected of [false, true])
    await withCommandTargets(async (target) => {
      target.availableModels = [
        {
          id: "opus",
          display_name: "Old Opus",
          supports_effort: false,
          supported_effort_levels: [],
          supports_fast_mode: false,
        },
        {
          id: "sonnet",
          display_name: "New Sonnet",
          supports_effort: true,
          supported_effort_levels: ["high", "max"],
          supports_fast_mode: true,
        },
      ];
      target.resolvedRuntimeModelId = "opus";
      refreshCurrentModel(target);
      const fixture = commandInitQuery(target, async () => []),
        entered = bridgeDeferred<void>(),
        release = bridgeDeferred<void>();
      const calls: string[] = [];
      let reads = 0,
        pending: Promise<void> | undefined,
        afterObservation = 0;
      const observed: Array<Record<string, unknown>> = [];
      Object.assign(target.query, {
        setModel: async (model: string) => {
          calls.push(model);
        },
        getSettings: async () => {
          if (++reads === 1) {
            entered.resolve();
            await release.promise;
            if (rejected) throw new Error("STALE-MODEL-READ");
            return { applied: { model: "opus" } };
          }
          return {
            applied: {
              model: "sonnet",
              effort: "high",
              ultracodeAvailable: false,
              ultracodeRequested: false,
              ultracode: false,
            },
          };
        },
      });
      const events = await captureBridgeEventsAsync(
        async () => {
          try {
            pending = dispatchControl({
              command: "set_model",
              session_id: target.sessionId,
              request_id: "stale-model-command",
              model: "opus",
            });
            await boundedBridgeTask(entered.promise);
            assert.deepEqual(calls, ["opus"]);
            handleSdkMessage(target, {
              type: "system",
              subtype: "init",
              session_id: target.sessionId,
              model: "sonnet",
            } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
            afterObservation = observed.length;
            release.resolve();
            await boundedBridgeTask(pending);
            await fixture.settle();
          } finally {
            release.resolve();
            if (pending) await boundedBridgeTask(pending);
            await fixture.settle();
          }
        },
        (event) => observed.push(event),
      );
      const all = events.filter(
        (event) => (event.update as { type?: string } | undefined)?.type === "current_model_update",
      );
      assert.ok(all.length >= 2);
      const completion = events
        .slice(afterObservation)
        .filter(
          (event) =>
            (event.update as { type?: string } | undefined)?.type === "current_model_update",
        );
      assert.ok(completion.length > 0);
      for (const event of all) {
        assert.equal(event.session_id, target.sessionId);
        const model = (event.update as { current_model: Record<string, unknown> }).current_model;
        assert.equal(model.resolved_id, "sonnet");
        assert.equal(model.catalog_id, "sonnet");
        assert.equal(model.is_authoritative, true);
        assert.equal(model.supports_fast_mode, true);
        assert.deepEqual(model.supported_effort_levels, ["high", "max"]);
      }
      assert.equal(
        events.some((event) => event.event === "slash_error"),
        false,
      );
    });
});

test("fast command completion republishes only the newer native off state and reason after an older on verification resolves", async () => {
  await withCommandTargets(async (target) => {
    controlModel(target);
    const fixture = commandInitQuery(target, async () => []),
      entered = bridgeDeferred<void>(),
      release = bridgeDeferred<void>();
    const calls: unknown[] = [];
    let pending: Promise<void> | undefined,
      completionStart = 0;
    const observed: Array<Record<string, unknown>> = [];
    Object.assign(target.query, {
      applyFlagSettings: async (settings: unknown) => {
        calls.push(settings);
      },
      reinitialize: async () => {
        entered.resolve();
        await release.promise;
        return { fast_mode_state: "on" };
      },
    });
    const events = await captureBridgeEventsAsync(
      async () => {
        try {
          pending = dispatchControl({
            command: "set_fast_mode",
            session_id: target.sessionId,
            request_id: "fast-stale-verification",
            enabled: true,
          });
          await boundedBridgeTask(entered.promise);
          assert.deepEqual(calls, [{ fastMode: true }]);
          handleSdkMessage(target, {
            type: "system",
            subtype: "init",
            session_id: target.sessionId,
            model: "haiku",
            fast_mode_state: "off",
            fast_mode_disabled_reason: "NEWER-NATIVE-REASON",
          } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
          await fixture.settle();
          completionStart = observed.length;
          release.resolve();
          await boundedBridgeTask(pending);
          await fixture.settle();
        } finally {
          release.resolve();
          if (pending) await boundedBridgeTask(pending);
          await fixture.settle();
        }
      },
      (event) => observed.push(event),
    );
    const snapshots = events.filter(
      (event) => (event.update as { type?: string } | undefined)?.type === "fast_mode_update",
    );
    assert.ok(snapshots.length >= 2);
    for (const event of snapshots) {
      assert.equal(event.session_id, target.sessionId);
      assert.deepEqual(event.update, {
        type: "fast_mode_update",
        fast_mode_state: "off",
        fast_mode_disabled_reason: "NEWER-NATIVE-REASON",
      });
    }
    assert.ok(
      events
        .slice(completionStart)
        .some(
          (event) => (event.update as { type?: string } | undefined)?.type === "fast_mode_update",
        ),
    );
    assert.deepEqual(
      events
        .filter((event) => event.event === "slash_error")
        .map((event) => [event.session_id, event.request_id]),
      [[target.sessionId, "fast-stale-verification"]],
    );
  });
});

test("pending Ultracode command and native refresh reads cannot revive snapshots superseded by newer observations or session lifetime", async () => {
  const off = {
      applied: {
        model: "haiku",
        effort: "medium",
        ultracodeAvailable: true,
        ultracodeRequested: false,
        ultracode: false,
      },
    },
    on = {
      applied: {
        model: "haiku",
        effort: "medium",
        ultracodeAvailable: true,
        ultracodeRequested: true,
        ultracode: true,
      },
    };
  for (const row of [
    { branch: "command", transition: "new-off", reject: false },
    { branch: "command", transition: "new-unknown", reject: true },
    { branch: "refresh-command", transition: "replace", reject: true },
    { branch: "command", transition: "close", reject: false },
    { branch: "background", transition: "new-off", reject: true },
    { branch: "background", transition: "replace", reject: false },
    { branch: "background", transition: "close", reject: true },
  ] as const)
    await withCommandTargets(async (target, peer) => {
      const fixture = commandInitQuery(target, async () => []),
        entered = bridgeDeferred<void>(),
        release = bridgeDeferred<void>();
      const nativeReads: Promise<unknown>[] = [];
      const calls: unknown[] = [];
      let first = true,
        newUnknown = false,
        pending: Promise<void> | undefined,
        cutoff = 0;
      const observed: Array<Record<string, unknown>> = [];
      Object.assign(target.query, {
        applyFlagSettings: async (value: unknown) => {
          calls.push(value);
        },
        getSettings: () => {
          const read = (async () => {
            if (first) {
              first = false;
              entered.resolve();
              await release.promise;
              if (row.reject) throw new Error("OBSOLETE-ULTRACODE-READ");
              return on;
            }
            if (newUnknown) throw new Error("CURRENT-ULTRACODE-UNKNOWN");
            return off;
          })();
          nativeReads.push(read);
          return read;
        },
      });
      const reset = () =>
        handleSdkMessage(target, {
          type: "conversation_reset",
          new_conversation_id: `conversation-${crypto.randomUUID()}`,
        } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      const drain = async () => {
        await Promise.allSettled(nativeReads.slice(1));
        await fixture.settle();
      };
      const events = await captureBridgeEventsAsync(
        async () => {
          try {
            if (row.branch === "background") reset();
            else
              pending = dispatchControl({
                command: row.branch === "command" ? "set_ultracode" : "refresh_ultracode",
                session_id: target.sessionId,
                request_id: "obsolete-ultracode",
                ...(row.branch === "command" ? { enabled: true } : {}),
              });
            await boundedBridgeTask(entered.promise);
            if (row.branch === "command") assert.deepEqual(calls, [{ ultracode: true }]);
            if (row.transition === "replace")
              updateSessionId(target, `ultracode-replacement-${crypto.randomUUID()}`);
            else if (row.transition === "close") beginSessionClose(target);
            else {
              newUnknown = row.transition === "new-unknown";
              reset();
              await drain();
              const fresh = observed.filter(
                (event) =>
                  (event.update as { type?: string } | undefined)?.type === "ultracode_update",
              );
              assert.ok(fresh.length > 0);
              assert.deepEqual(fresh.at(-1)?.update, {
                type: "ultracode_update",
                ultracode: newUnknown
                  ? null
                  : { available: true, requested: false, effective: false },
              });
            }
            cutoff = observed.length;
            release.resolve();
            await Promise.allSettled(nativeReads);
            if (pending) await boundedBridgeTask(pending);
            await fixture.settle();
            assert.equal(
              observed
                .slice(cutoff)
                .some(
                  (event) =>
                    event.event === "slash_error" ||
                    (event.update as { type?: string } | undefined)?.type === "ultracode_update",
                ),
              false,
            );
            newUnknown = false;
            const live = row.transition === "close" ? peer : target;
            if (live === peer) Object.assign(peer.query, { getSettings: async () => off });
            const recoveredFrom = observed.length;
            await dispatchControl({
              command: "refresh_ultracode",
              session_id: live.sessionId,
              request_id: "current-ultracode",
            });
            const recovered = observed
              .slice(recoveredFrom)
              .filter(
                (event) =>
                  (event.update as { type?: string } | undefined)?.type === "ultracode_update",
              );
            assert.deepEqual(recovered, [
              {
                event: "session_update",
                session_id: live.sessionId,
                update: {
                  type: "ultracode_update",
                  ultracode: { available: true, requested: false, effective: false },
                },
              },
            ]);
          } finally {
            release.resolve();
            if (pending) await boundedBridgeTask(pending);
            await Promise.allSettled(nativeReads);
            await fixture.settle();
          }
        },
        (event) => observed.push(event),
      );
      assert.equal(
        events.some(
          (event) => event.event === "slash_error" && event.request_id === "obsolete-ultracode",
        ),
        false,
      );
    });
});

test("live root messages retain independent source clocks and publish each successful or failed turn duration before its terminal outcome", async () => {
  await withCommandTargets(async (session) => {
    const send = (value: Record<string, unknown>) =>
      handleSdkMessage(session, {
        session_id: session.sessionId,
        ...value,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    for (const row of [
      {
        id: "success",
        userTime: "2026-10-03T15:02:00Z",
        assistantTime: "2026-10-03T15:02:01Z",
        duration: 2000,
        api: 1250,
        terminal: "turn_complete",
        subtype: "success",
      },
      {
        id: "failure",
        userTime: "2026-10-03T15:05:10Z",
        assistantTime: "2026-10-03T15:05:20Z",
        duration: 2500,
        api: 500,
        terminal: "turn_error",
        subtype: "error_during_execution",
      },
    ]) {
      await dispatchParsedPrompt({
        command: "prompt",
        session_id: session.sessionId,
        request_id: `${row.id}-request`,
        message_uuid: `${row.id}-prompt`,
        chunks: [{ kind: "text", value: `Independent ${row.id} question` }],
      });
      const queued = await nextPromptItem(session.input[Symbol.asyncIterator]());
      assert.equal(queued.done, false);
      assert.equal(queued.value?.uuid, `${row.id}-prompt`);
      assert.deepEqual(queued.value?.message.content, [
        { type: "text", text: `Independent ${row.id} question` },
      ]);
      const events = captureBridgeEvents(() => {
        send({
          type: "user",
          uuid: `${row.id}-prompt`,
          timestamp: row.userTime,
          parent_tool_use_id: null,
          message: {
            role: "user",
            content: [{ type: "text", text: `Independent ${row.id} question` }],
          },
        });
        send({
          type: "stream_event",
          uuid: `${row.id}-answer`,
          parent_tool_use_id: null,
          event: {
            type: "message_start",
            message: { id: `native-${row.id}`, role: "assistant", content: [] },
          },
        });
        send({
          type: "stream_event",
          uuid: `${row.id}-answer`,
          parent_tool_use_id: null,
          event: {
            type: "content_block_delta",
            index: 0,
            delta: { type: "text_delta", text: `Independent ${row.id} answer` },
          },
        });
        send({
          type: "assistant",
          uuid: `${row.id}-answer`,
          timestamp: row.assistantTime,
          parent_tool_use_id: null,
          message: {
            role: "assistant",
            content: [{ type: "text", text: `Independent ${row.id} answer` }],
          },
        });
        send({
          type: "result",
          subtype: row.subtype,
          duration_ms: row.duration,
          duration_api_ms: row.api,
          ...(row.id === "failure" ? { errors: ["Independent native failure"] } : {}),
        });
      });
      const updates = events.filter((event) => event.event === "session_update");
      assert.ok(updates.every((event) => event.session_id === session.sessionId));
      assert.deepEqual(
        updates
          .filter((event) => (event.update as { type: string }).type === "message_metadata")
          .map((event) => event.update),
        [
          {
            type: "message_metadata",
            role: "user",
            timestamp: row.userTime,
            source_message_uuid: `${row.id}-prompt`,
          },
          {
            type: "message_metadata",
            role: "assistant",
            timestamp: row.assistantTime,
            source_message_uuid: `${row.id}-answer`,
          },
        ],
      );
      for (const [type, text] of [["agent_message_chunk", `Independent ${row.id} answer`]])
        assert.deepEqual(
          updates
            .filter((event) => (event.update as { type: string }).type === type)
            .map((event) => (event.update as { content: unknown }).content),
          [{ type: "text", text }],
        );
      const clocks = updates.filter(
        (event) => (event.update as { type: string }).type === "turn_timing",
      );
      assert.equal(clocks.length, 1);
      assert.deepEqual(clocks[0]?.update, {
        type: "turn_timing",
        duration_ms: row.duration,
        api_duration_ms: row.api,
      });
      const terminal = events.filter((event) => event.event === row.terminal);
      assert.equal(terminal.length, 1);
      assert.equal(terminal[0]?.session_id, session.sessionId);
      assert.ok(events.indexOf(clocks[0]!) < events.indexOf(terminal[0]!));
    }
    const controls = captureBridgeEvents(() => {
      send({
        type: "assistant",
        uuid: "child-clock",
        parent_tool_use_id: "child-tool",
        timestamp: "2026-10-03T16:00:00Z",
        message: { role: "assistant", content: [{ type: "text", text: "CHILD-CLOCK-CONTROL" }] },
      });
      for (const timestamp of [undefined, "invalid-clock"]) {
        send({
          type: "user",
          uuid: `invalid-${timestamp}`,
          timestamp,
          parent_tool_use_id: null,
          message: { role: "user", content: [{ type: "text", text: "Clockless question" }] },
        });
        send({
          type: "assistant",
          uuid: `answer-${timestamp}`,
          parent_tool_use_id: null,
          message: { role: "assistant", content: [{ type: "text", text: "Clockless answer" }] },
        });
        send({
          type: "result",
          subtype: "success",
          duration_ms: timestamp === undefined ? undefined : -1,
          duration_api_ms: "invalid",
        });
      }
    });
    assert.equal(
      controls.some((event) =>
        ["message_metadata", "turn_timing"].includes(
          (event.update as { type?: string } | undefined)?.type ?? "",
        ),
      ),
      false,
    );
    assert.equal(controls.filter((event) => event.event === "turn_complete").length, 2);
  });
});

test("historical message clocks retain exact user and nested assistant source identity after their own content", () => {
  const updates = mapSessionMessagesToUpdates([
    {
      type: "user",
      uuid: "old-prompt",
      session_id: "historical-clock-owner",
      parent_tool_use_id: null,
      timestamp: "2025-01-01T10:00:00Z",
      message: { role: "user", content: [{ type: "text", text: "Independent old question" }] },
    },
    {
      type: "assistant",
      uuid: "old-answer",
      session_id: "historical-clock-owner",
      parent_tool_use_id: null,
      message: {
        timestamp: "2025-01-01T10:00:02Z",
        role: "assistant",
        content: [{ type: "text", text: "Independent old answer" }],
      },
    },
  ] as unknown as SessionMessage[]);
  for (const row of [
    {
      role: "user",
      uuid: "old-prompt",
      type: "user_message_chunk",
      text: "Independent old question",
      timestamp: "2025-01-01T10:00:00Z",
    },
    {
      role: "assistant",
      uuid: "old-answer",
      type: "agent_message_chunk",
      text: "Independent old answer",
      timestamp: "2025-01-01T10:00:02Z",
    },
  ]) {
    const content = updates.filter((update) => update.type === row.type);
    assert.equal(content.length, 1);
    assert.deepEqual((content[0] as { content: unknown }).content, {
      type: "text",
      text: row.text,
    });
    const metadata = updates.filter(
      (update) => update.type === "message_metadata" && update.source_message_uuid === row.uuid,
    );
    assert.deepEqual(metadata, [
      {
        type: "message_metadata",
        role: row.role,
        timestamp: row.timestamp,
        source_message_uuid: row.uuid,
      },
    ]);
    assert.ok(updates.indexOf(content[0]!) < updates.indexOf(metadata[0]!));
  }
});

test("native Agent progress and final result preserve per-tool elapsed and model metadata without synthesizing turn duration", () => {
  const session = makeSessionState();
  session.sessionId = "independent-tool-clocks";
  const send = (value: Record<string, unknown>) =>
    handleSdkMessage(session, {
      session_id: session.sessionId,
      ...value,
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  const events = captureBridgeEvents(() => {
    send({
      type: "assistant",
      uuid: "timed-launch",
      message: {
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "timed-tool",
            name: "Agent",
            input: { description: "Independent review", prompt: "Review independently" },
          },
          {
            type: "tool_use",
            id: "sibling-timed-tool",
            name: "Agent",
            input: { description: "Sibling review", prompt: "Review sibling" },
          },
        ],
      },
    });
    send({
      type: "tool_progress",
      tool_use_id: "timed-tool",
      tool_name: "Agent",
      parent_tool_use_id: null,
      elapsed_time_seconds: 1.25,
    });
    send({
      type: "tool_progress",
      tool_use_id: "sibling-timed-tool",
      tool_name: "Agent",
      parent_tool_use_id: null,
      elapsed_time_seconds: 3.5,
    });
    for (const row of [
      { id: "sibling-timed-tool", model: "claude-haiku-4-5", text: "Independent sibling result" },
      { id: "timed-tool", model: "claude-sonnet-4-7", text: "Independent target result" },
    ])
      send({
        type: "user",
        uuid: `${row.id}-result`,
        message: {
          role: "user",
          content: [{ type: "tool_result", tool_use_id: row.id, content: row.text }],
        },
        toolUseResult: { resolvedModel: row.model, content: [{ type: "text", text: row.text }] },
      });
  });
  for (const row of [
    {
      id: "timed-tool",
      duration: 1250,
      model: "claude-sonnet-4-7",
      text: "Independent target result",
    },
    {
      id: "sibling-timed-tool",
      duration: 3500,
      model: "claude-haiku-4-5",
      text: "Independent sibling result",
    },
  ]) {
    const tool = publishedTool(events, session.sessionId, row.id);
    assert.equal(tool.status, "completed");
    assert.equal(tool.raw_output, row.text);
    assert.deepEqual(tool.output_metadata, {
      timing: { duration_ms: row.duration, source: "progress" },
      agent: { resolved_model: row.model },
    });
  }
  assert.equal(
    events.some((event) => (event.update as { type?: string } | undefined)?.type === "turn_timing"),
    false,
  );
  const terminal = captureBridgeEvents(() =>
    send({ type: "result", subtype: "success", duration_ms: 9000, duration_api_ms: 7000 }),
  );
  assert.deepEqual(
    terminal.filter(
      (event) => (event.update as { type?: string } | undefined)?.type === "turn_timing",
    ),
    [
      {
        event: "session_update",
        session_id: session.sessionId,
        update: { type: "turn_timing", duration_ms: 9000, api_duration_ms: 7000 },
      },
    ],
  );
});

test("native proactive tool results preserve exact output and delivery observations while failed, non-executed and non-proactive siblings never request alerts", () => {
  const previousTimezone = process.env.TZ;
  process.env.TZ = "UTC";
  try {
    for (const replay of [false, true]) {
      const session = makeSessionState();
      session.sessionId = `proactive-owner-${replay}`;
      const send = (value: Record<string, unknown>) =>
        handleSdkMessage(session, {
          session_id: session.sessionId,
          ...value,
        } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      const cases = [
        {
          id: "positive",
          intent: "proactive",
          error: false,
          status: "completed",
          text: "Independent ready notification",
        },
        {
          id: "ordinary",
          intent: "ordinary",
          error: false,
          status: "completed",
          text: "Independent ordinary output",
        },
        {
          id: "failed",
          intent: "proactive",
          error: true,
          status: "failed",
          text: "Independent failed push",
        },
        {
          id: "not-executed",
          intent: "proactive",
          error: false,
          status: "killed",
          text: "Independent cancelled push",
        },
      ];
      const events = captureBridgeEvents(() => {
        send({
          type: "assistant",
          uuid: "push-launch",
          message: {
            role: "assistant",
            content: cases.map((row) => ({
              type: "tool_use",
              id: row.id,
              name: "PushNotification",
              input: { status: row.intent, message: "Independent requested push" },
            })),
          },
        });
        for (const row of cases)
          send({
            type: "user",
            uuid: `${row.id}-result`,
            isReplay: replay,
            message: {
              role: "user",
              content: [
                {
                  type: "tool_result",
                  tool_use_id: row.id,
                  content: row.text,
                  is_error: row.error,
                },
              ],
            },
            tool_use_result: {
              data: {
                message: row.text,
                pushSent: true,
                localSent: false,
                disabledReason: "independent_no_transport",
                sentAt: "2026-10-04T10:00:00Z",
              },
            },
            ...(row.id === "not-executed"
              ? { tool_result_meta: [{ id: row.id, non_execution_kind: "cancelled" }] }
              : {}),
          });
      });
      for (const row of cases) {
        const tool = publishedTool(events, session.sessionId, row.id);
        assert.equal(tool.status, row.status);
        assert.equal(
          tool.raw_output,
          row.error
            ? row.text
            : `Result: ${row.text}\nPush sent: yes\nLocal sent: no\nSent at: 2026-10-04 10:00:00 local`,
        );
      }
      assert.deepEqual(
        events.filter(
          (event) =>
            (event.update as { type?: string } | undefined)?.type === "notification_update",
        ),
        [
          {
            event: "session_update",
            session_id: session.sessionId,
            update: {
              type: "notification_update",
              replay,
              notification: {
                origin: "model_tool",
                session_id: session.sessionId,
                tool_use_id: "positive",
                text: "Independent ready notification",
                push_sent: true,
                local_sent: false,
                disabled_reason: "independent_no_transport",
                sent_at: "2026-10-04T10:00:00Z",
              },
            },
          },
        ],
      );
    }
  } finally {
    if (previousTimezone === undefined) delete process.env.TZ;
    else process.env.TZ = previousTimezone;
  }
});

test("native notice publications preserve scoped identity and future presentation metadata while rejecting missing required payloads", () => {
  const session = makeSessionState();
  session.sessionId = "native-notice-owner";
  const base = {
    type: "system",
    subtype: "notification",
    session_id: session.sessionId,
    uuid: "notice-original",
    key: "replaceable",
    text: "Independent native notice",
    priority: "immediate",
    color: "yellow",
    timeout_ms: 5000,
  };
  const events = captureBridgeEvents(() => {
    for (const message of [
      base,
      {
        ...base,
        uuid: "notice-future",
        text: "Independent future notice",
        priority: "future-priority",
        timeout_ms: 0,
      },
      {
        ...base,
        uuid: "notice-invalid-hints",
        text: "Independent valid notice",
        color: 7,
        timeout_ms: -1,
      },
      { ...base, text: " " },
      { ...base, text: 7 },
      { ...base, uuid: undefined },
      { ...base, uuid: 9 },
      { ...base, session_id: undefined },
    ])
      handleSdkMessage(
        session,
        message as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage,
      );
  });
  assert.deepEqual(
    events.filter(
      (event) => (event.update as { type?: string } | undefined)?.type === "notification_update",
    ),
    [
      {
        uuid: "notice-original",
        text: "Independent native notice",
        priority: "immediate",
        color: "yellow",
        timeout_ms: 5000,
      },
      {
        uuid: "notice-future",
        text: "Independent future notice",
        priority: "future-priority",
        color: "yellow",
        timeout_ms: 0,
      },
      { uuid: "notice-invalid-hints", text: "Independent valid notice", priority: "immediate" },
    ].map((notice) => ({
      event: "session_update",
      session_id: session.sessionId,
      update: {
        type: "notification_update",
        replay: false,
        notification: {
          origin: "sdk_notice",
          session_id: session.sessionId,
          key: "replaceable",
          ...notice,
        },
      },
    })),
  );
});

test("historical SDK notices retain metadata in the replay projection", () => {
  const updates = mapSessionMessagesToUpdates([{ type: "system", uuid: "old-notice", session_id: "session-1", message: {
    type: "system", subtype: "notification", session_id: "session-1", uuid: "old-notice", key: "replaceable", text: "Historical notice", priority: "future-priority", timeout_ms: 0,
  } }] as unknown as SessionMessage[]);
  assert.deepEqual(updates, [{ type: "notification_update", replay: true, notification: {
    origin: "sdk_notice", session_id: "session-1", uuid: "old-notice", key: "replaceable", text: "Historical notice", priority: "future-priority", timeout_ms: 0,
  } }]);
});

for (const action of ["accept", "decline", "cancel"] as const) {
  test(`elicitation ${action} preserves published schema and parsed native answers without disturbing another callback`,async()=>{
    await withCommandTargets(async(target)=>{
      const {elicitation}=interactionCallbacks(target);const abort=new AbortController(),siblingAbort=new AbortController();const observed:Array<Record<string,unknown>>=[];
      const events=await captureBridgeEventsAsync(async()=>{try{
        let siblingSettled=false;const sibling=elicitation(typedForm,{requestId:"typed-sibling",signal:siblingAbort.signal}).then(value=>{siblingSettled=true;return value;});
        const pending=elicitation(typedForm,{requestId:"typed-target",signal:abort.signal});const published=interactionRequest(observed,"elicitation_request","typed-target");
        assert.deepEqual(published,{event:"elicitation_request",session_id:target.sessionId,request:{request_id:"typed-target",field_order:["zero","no","blank","tags"],server_name:"independent-forms",message:"Independent typed form",mode:"form",requested_schema:typedForm.requestedSchema}});
        answerElicitation(published,action,typedAnswers);assert.deepEqual(await boundedBridgeTask(pending),action==="accept"?{action:"accept",content:typedAnswers}:{action});
        const before=observed.length;answerElicitation(published,action,typedAnswers);abort.abort();await Promise.resolve();assert.equal(observed.length,before);assert.equal(siblingSettled,false);
        answerElicitation(interactionRequest(observed,"elicitation_request","typed-sibling"),"accept",typedAnswers);assert.deepEqual(await boundedBridgeTask(sibling),{action:"accept",content:typedAnswers});
        if(action==="accept")for(const content of [{},undefined]){const id=content===undefined?"omitted":"empty";const result=elicitation(typedForm,{requestId:id,signal:siblingAbort.signal});answerElicitation(interactionRequest(observed,"elicitation_request",id),"accept",content);assert.deepEqual(await boundedBridgeTask(result),content===undefined?{action:"accept"}:{action:"accept",content:{}});}
      }finally{abort.abort();siblingAbort.abort();}},event=>observed.push(event));
      const ids=action==="accept"?["typed-target","typed-sibling","empty","omitted"]:["typed-target","typed-sibling"];
      assert.deepEqual(events.filter(event=>event.event==="elicitation_settled"),ids.map(request_id=>({event:"elicitation_settled",session_id:target.sessionId,settlement:{request_id,session_closing:false}})));
    });
  });
}

test("elicitation isolates aborts and duplicate callback IDs and fences responses across real session identity replacement", async () => {
  await withCommandTargets(async (target) => {
    const { elicitation } = interactionCallbacks(target),
      a = new AbortController(),
      b = new AbortController(),
      replacementAbort = new AbortController();
    const observed: Array<Record<string, unknown>> = [];
    const oldId = target.sessionId;
    const events = await captureBridgeEventsAsync(
      async () => {
        try {
          let firstSettled = false;
          const first = elicitation(typedForm, {
            requestId: "same-callback",
            signal: a.signal,
          }).then((value) => {
            firstSettled = true;
            return value;
          });
          const second = elicitation(typedForm, { requestId: "abort-only-this", signal: b.signal });
          const firstEvent = interactionRequest(observed, "elicitation_request", "same-callback"),
            secondEvent = interactionRequest(observed, "elicitation_request", "abort-only-this");
          assert.deepEqual(
            await elicitation(typedForm, { requestId: "same-callback", signal: b.signal }),
            { action: "cancel" },
          );
          assert.equal(observed.filter((event) => event.event === "elicitation_request").length, 2);
          b.abort();
          assert.deepEqual(await boundedBridgeTask(second), { action: "cancel" });
          answerElicitation(secondEvent, "accept", typedAnswers);
          await Promise.resolve();
          assert.equal(firstSettled, false);
          answerElicitation(firstEvent, "accept", typedAnswers);
          assert.deepEqual(await boundedBridgeTask(first), {
            action: "accept",
            content: typedAnswers,
          });
          const old = elicitation(typedForm, {
            requestId: "reused-after-replacement",
            signal: a.signal,
          });
          const oldRequest = interactionRequest(
            observed,
            "elicitation_request",
            "reused-after-replacement",
          );
          updateSessionId(target, `elicitation-replacement-${crypto.randomUUID()}`);
          a.abort();
          assert.deepEqual(await boundedBridgeTask(old), { action: "cancel" });
          let freshSettled = false;
          const fresh = elicitation(typedForm, {
            requestId: "reused-after-replacement",
            signal: replacementAbort.signal,
          }).then((value) => {
            freshSettled = true;
            return value;
          });
          const newRequest = observed.find(
            (event) =>
              event.event === "elicitation_request" && event.session_id === target.sessionId,
          );
          assert.ok(newRequest);
          const before = observed.length;
          answerElicitation(oldRequest, "accept", { wrong: "stale" });
          await Promise.resolve();
          assert.equal(freshSettled, false);
          assert.equal(observed.length, before);
          answerElicitation(newRequest, "accept", typedAnswers);
          assert.deepEqual(await boundedBridgeTask(fresh), {
            action: "accept",
            content: typedAnswers,
          });
        } finally {
          a.abort();
          b.abort();
          replacementAbort.abort();
        }
      },
      (event) => observed.push(event),
    );
    assert.deepEqual(
      events.filter((event) => event.event === "elicitation_settled"),
      [
        [oldId, "abort-only-this"],
        [oldId, "same-callback"],
        [oldId, "reused-after-replacement"],
        [target.sessionId, "reused-after-replacement"],
      ].map(([session_id, request_id]) => ({
        event: "elicitation_settled",
        session_id,
        settlement: { request_id, session_closing: false },
      })),
    );
  });
});

test("native URL completion matches both server and elicitation identity while forms and the other server remain usable", async () => {
  await withCommandTargets(async (target) => {
    const { elicitation } = interactionCallbacks(target),
      abort = new AbortController();
    const observed: Array<Record<string, unknown>> = [];
    const events = await captureBridgeEventsAsync(
      async () => {
        try {
          let formSettled = false,
            oneSettled = false;
          const form = elicitation(typedForm, { requestId: "form", signal: abort.signal }).then(
            (value) => {
              formSettled = true;
              return value;
            },
          );
          const one = elicitation(
            {
              serverName: "one",
              message: "URL one",
              mode: "url",
              url: "https://example.test/one",
              elicitationId: "shared-native-id",
            },
            { requestId: "one-callback", signal: abort.signal },
          ).then((value) => {
            oneSettled = true;
            return value;
          });
          const two = elicitation(
            {
              serverName: "two",
              message: "URL two",
              mode: "url",
              url: "https://example.test/two",
              elicitationId: "shared-native-id",
            },
            { requestId: "two-callback", signal: abort.signal },
          );
          const complete = (server: string | undefined, id = "shared-native-id") =>
            handleSdkMessage(target, {
              type: "system",
              subtype: "elicitation_complete",
              session_id: target.sessionId,
              elicitation_id: id,
              ...(server ? { mcp_server_name: server } : {}),
            } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
          complete("two");
          assert.deepEqual(await boundedBridgeTask(two), { action: "accept" });
          const before = observed.length;
          complete("two");
          complete(undefined);
          complete("unknown");
          complete("one", "wrong-id");
          await Promise.resolve();
          assert.equal(formSettled, false);
          assert.equal(oneSettled, false);
          assert.equal(observed.length, before);
          answerElicitation(
            interactionRequest(observed, "elicitation_request", "form"),
            "accept",
            typedAnswers,
          );
          assert.deepEqual(await boundedBridgeTask(form), {
            action: "accept",
            content: typedAnswers,
          });
          complete("one");
          assert.deepEqual(await boundedBridgeTask(one), { action: "accept" });
        } finally {
          abort.abort();
        }
      },
      (event) => observed.push(event),
    );
    assert.deepEqual(
      events
        .filter((event) => event.event === "elicitation_settled")
        .map((event) => ({ session: event.session_id, ...(event.settlement as object) }))
        .sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b))),
      ["form", "one-callback", "two-callback"]
        .map((request_id) => ({ session: target.sessionId, request_id, session_closing: false }))
        .sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b))),
    );
  });
});

test("session close settles each admitted elicitation before waiting for its consumer and leaves a separate session answerable", async () => {
  await withCommandTargets(async (target, peer) => {
    target.query = { close: () => {} } as SessionState["query"];
    const gate = bridgeDeferred<void>();
    target.queryConsumerTask = gate.promise;
    const abort = new AbortController(),
      gone = new AbortController();
    gone.abort();
    const handler = interactionCallbacks(target).elicitation,
      peerHandler = interactionCallbacks(peer).elicitation;
    const observed: Array<Record<string, unknown>> = [];
    let closing: Promise<void> | undefined;
    const events = await captureBridgeEventsAsync(
      async () => {
        try {
          assert.deepEqual(
            await handler(typedForm, { requestId: "already-aborted", signal: gone.signal }),
            { action: "cancel" },
          );
          const first = handler(typedForm, { requestId: "close-first", signal: abort.signal }),
            second = handler(typedForm, { requestId: "close-second", signal: abort.signal }),
            other = peerHandler(typedForm, { requestId: "peer-survivor", signal: abort.signal });
          let closeFinished = false;
          closing = closeSession(target).then(() => {
            closeFinished = true;
          });
          assert.deepEqual(await boundedBridgeTask(first), { action: "cancel" });
          assert.deepEqual(await boundedBridgeTask(second), { action: "cancel" });
          assert.equal(closeFinished, false);
          assert.deepEqual(
            observed.filter((event) => event.event === "elicitation_settled"),
            ["close-first", "close-second"].map((request_id) => ({
              event: "elicitation_settled",
              session_id: target.sessionId,
              settlement: { request_id, session_closing: true },
            })),
          );
          for (const id of ["close-first", "close-second"])
            answerElicitation(
              interactionRequest(observed, "elicitation_request", id),
              "accept",
              typedAnswers,
            );
          answerElicitation(
            interactionRequest(observed, "elicitation_request", "peer-survivor"),
            "accept",
            typedAnswers,
          );
          assert.deepEqual(await boundedBridgeTask(other), {
            action: "accept",
            content: typedAnswers,
          });
          abort.abort();
          gate.resolve();
          await boundedBridgeTask(closing);
        } finally {
          abort.abort();
          gate.resolve();
          if (closing) await boundedBridgeTask(closing);
        }
      },
      (event) => observed.push(event),
    );
    assert.deepEqual(
      events
        .filter((event) => event.event === "elicitation_request")
        .map((event) => [event.session_id, (event.request as { request_id: string }).request_id]),
      [
        [target.sessionId, "close-first"],
        [target.sessionId, "close-second"],
        [peer.sessionId, "peer-survivor"],
      ],
    );
    assert.deepEqual(
      events.filter((event) => event.event === "elicitation_settled"),
      [
        ...["close-first", "close-second"].map((request_id) => ({
          event: "elicitation_settled",
          session_id: target.sessionId,
          settlement: { request_id, session_closing: true },
        })),
        {
          event: "elicitation_settled",
          session_id: peer.sessionId,
          settlement: { request_id: "peer-survivor", session_closing: false },
        },
      ],
    );
  });
});

test("pinned SDK 0.3.302 forwards public elicitation identities, schema order, responses and aborts", {
  timeout: 15000,
}, async () => {
  const { spawn } = await import("node:child_process"),
    { fileURLToPath } = await import("node:url");
  const fixture = fileURLToPath(
    new URL("../../tests/fixtures/sdk-elicitation-peer.cjs", import.meta.url),
  );
  await withCommandTargets(async (target) => {
    const handler = interactionCallbacks(target).elicitation,
      input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
    let child: ReturnType<typeof spawn> | undefined,
      sdkQuery: ReturnType<typeof query> | undefined,
      termination: Promise<void> | undefined;
    const accepted = bridgeDeferred<Record<string, unknown>>(),
      inbox = bridgeEventInbox();
    const events = await captureBridgeEventsAsync(
      async () => {
        try {
          sdkQuery = query({
            prompt: input,
            options: {
              onElicitation: handler,
              spawnClaudeCodeProcess() {
                child = spawn(process.execPath, [fixture], {
                  stdio: ["pipe", "pipe", "pipe", "ipc"],
                  windowsHide: true,
                });
                termination = new Promise<void>((resolve) => child!.once("close", () => resolve()));
                child.stderr?.resume();
                child.on("message", (message) => {
                  const value = message as { response?: { request_id?: string } };
                  if (value.response?.request_id === "sdk-form-a")
                    accepted.resolve(message as Record<string, unknown>);
                });
                return child as ReturnType<NonNullable<Options["spawnClaudeCodeProcess"]>>;
              },
            },
          });
          await boundedBridgeTask(sdkQuery.supportedCommands());
          const first = await boundedBridgeTask(
              inbox.next(
                (event) =>
                  event.event === "elicitation_request" &&
                  (event.request as { request_id: string }).request_id === "sdk-form-a",
              ),
            ),
            second = await boundedBridgeTask(
              inbox.next(
                (event) =>
                  event.event === "elicitation_request" &&
                  (event.request as { request_id: string }).request_id === "sdk-form-b",
              ),
            );
          for (const [event, id] of [
            [first, "sdk-form-a"],
            [second, "sdk-form-b"],
          ] as const)
            assert.deepEqual(event, {
              event: "elicitation_request",
              session_id: target.sessionId,
              request: {
                request_id: id,
                server_name: "characterization",
                mode: "form",
                message: id,
                requested_schema: {
                  type: "object",
                  properties: { zulu: { type: "boolean" }, alpha: { type: "string" } },
                },
                field_order: ["zulu", "alpha"],
              },
            });
          answerElicitation(first, "accept", { flag: false, count: 0, tags: ["a"], blank: "" });
          const result = await boundedBridgeTask(accepted.promise);
          assert.deepEqual((result.response as { response: unknown }).response, {
            action: "accept",
            content: { flag: false, count: 0, tags: ["a"], blank: "" },
          });
          assert.ok(child);
          child.send({ cancel: "sdk-form-b" });
          const aborted = await boundedBridgeTask(
            inbox.next(
              (event) =>
                event.event === "elicitation_settled" &&
                (event.settlement as { request_id: string }).request_id === "sdk-form-b",
            ),
          );
          assert.deepEqual(aborted, {
            event: "elicitation_settled",
            session_id: target.sessionId,
            settlement: { request_id: "sdk-form-b", session_closing: false },
          });
        } finally {
          input.close();
          sdkQuery?.close();
          if (child && child.exitCode === null && child.signalCode === null) child.kill();
          if (termination) await boundedBridgeTask(termination);
          inbox.dispose();
        }
      },
      (event) => inbox.observe(event),
    );
    assert.deepEqual(
      events.filter((event) => event.event === "elicitation_settled"),
      ["sdk-form-a", "sdk-form-b"].map((request_id) => ({
        event: "elicitation_settled",
        session_id: target.sessionId,
        settlement: { request_id, session_closing: false },
      })),
    );
  });
});

test("invalid JSON elicitation answer values cancel without partial acceptance and fresh valid forms recover independently", async () => {
  await withCommandTargets(async (target) => {
    const { elicitation } = interactionCallbacks(target),
      abort = new AbortController();
    const observed: Array<Record<string, unknown>> = [];
    const events = await captureBridgeEventsAsync(
      async () => {
        try {
          let siblingSettled = false;
          const sibling = elicitation(typedForm, {
            requestId: "invalid-survivor",
            signal: abort.signal,
          }).then((value) => {
            siblingSettled = true;
            return value;
          });
          for (const [index, invalid] of [
            null,
            { nested: "private-value" },
            ["valid", 7],
          ].entries()) {
            const id = `invalid-${index}`,
              pending = elicitation(typedForm, { requestId: id, signal: abort.signal });
            answerElicitation(interactionRequest(observed, "elicitation_request", id), "accept", {
              ...typedAnswers,
              private_answer: invalid,
            });
            assert.deepEqual(await boundedBridgeTask(pending), { action: "cancel" });
            assert.equal(siblingSettled, false);
            const recoveryId = `recovery-${index}`,
              recovery = elicitation(typedForm, { requestId: recoveryId, signal: abort.signal });
            answerElicitation(
              interactionRequest(observed, "elicitation_request", recoveryId),
              "accept",
              typedAnswers,
            );
            assert.deepEqual(await boundedBridgeTask(recovery), {
              action: "accept",
              content: typedAnswers,
            });
          }
          answerElicitation(
            interactionRequest(observed, "elicitation_request", "invalid-survivor"),
            "accept",
            typedAnswers,
          );
          assert.deepEqual(await boundedBridgeTask(sibling), {
            action: "accept",
            content: typedAnswers,
          });
        } finally {
          abort.abort();
        }
      },
      (event) => observed.push(event),
    );
    assert.deepEqual(
      events.filter((event) => event.event === "elicitation_settled"),
      [
        "invalid-0",
        "recovery-0",
        "invalid-1",
        "recovery-1",
        "invalid-2",
        "recovery-2",
        "invalid-survivor",
      ].map((request_id) => ({
        event: "elicitation_settled",
        session_id: target.sessionId,
        settlement: { request_id, session_closing: false },
      })),
    );
  });
  const { spawnSync } = await import("node:child_process");
  const lifecycle = new URL("./bridge/session_lifecycle.js", import.meta.url).href,
    logger = new URL("./bridge/logger.js", import.meta.url).href;
  const script = `const {sessions,handleElicitationResponse}=await import(${JSON.stringify(lifecycle)});const {logBridgeCommandReceived}=await import(${JSON.stringify(logger)});const command={command:"elicitation_response",session_id:"invalid-private-session",elicitation_request_id:"invalid-private-request",action:"accept",content:{password:{nested:"INVALID-DO-NOT-LOG"},valid_secret:"VALID-DO-NOT-LOG"}};sessions.set(command.session_id,{pendingElicitations:new Map([[command.elicitation_request_id,{serverName:"privacy-test",resolve(value){process.stdout.write(JSON.stringify(value));}}]])});logBridgeCommandReceived(command);handleElicitationResponse(command);`;
  const result = spawnSync(process.execPath, ["--input-type=module", "-e", script], {
    env: { ...process.env, CLAUDE_RS_BRIDGE_DIAGNOSTICS: "1" },
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), { action: "cancel" });
  assert.ok(result.stderr.includes("elicitation_response_applied"));
  for (const secret of [
    "password",
    "nested",
    "INVALID-DO-NOT-LOG",
    "VALID-DO-NOT-LOG",
    "valid_secret",
  ])
    assert.equal(result.stderr.includes(secret), false);
});

test("elicitation answer values never enter detailed bridge diagnostics", async () => {
  const { spawnSync } = await import("node:child_process");
  const lifecycle = new URL("./bridge/session_lifecycle.js", import.meta.url).href;
  const logger = new URL("./bridge/logger.js", import.meta.url).href;
  const script = `
    const { sessions, handleElicitationResponse } = await import(${JSON.stringify(lifecycle)});
    const { logBridgeCommandReceived } = await import(${JSON.stringify(logger)});
    const command = { command:"elicitation_response",session_id:"private-session",elicitation_request_id:"private-request",action:"accept",content:{password:"DO-NOT-LOG-THIS-ANSWER",private_array:["DO-NOT-LOG-THIS-CHOICE"]} };
    sessions.set("private-session",{pendingElicitations:new Map([["private-request",{serverName:"test",resolve(){}}]])});
    logBridgeCommandReceived(command);
    handleElicitationResponse(command);
  `;
  const result = spawnSync(process.execPath,["--input-type=module","-e",script],{env:{...process.env,CLAUDE_RS_BRIDGE_DIAGNOSTICS:"1"},encoding:"utf8"});
  assert.equal(result.status,0,result.stderr);
  assert.ok(result.stderr.includes("elicitation_response_applied"));
  assert.ok(!result.stderr.includes("DO-NOT-LOG-THIS"));
  assert.ok(!result.stderr.includes("password"));
});

test("Ultracode completion after the effort refresh cannot report obsolete inactive-request feedback", async () => {
	for (const transition of ["observation", "replacement", "close"])
		await withCommandTargets(async (target, peer) => {
			const fixture = commandInitQuery(target, async () => []);
			const entered = bridgeDeferred<void>(),
				release = bridgeDeferred<void>();
			const inactive = {
				applied: {
					model: "haiku",
					effort: "medium",
					ultracodeAvailable: false,
					ultracodeRequested: true,
					ultracode: false,
				},
			};
			const disabled = {
				applied: {
					model: "haiku",
					effort: "low",
					ultracodeAvailable: true,
					ultracodeRequested: false,
					ultracode: false,
				},
			};
			let reads = 0,
				pending: Promise<void> | undefined;
			const calls: unknown[] = [];
			Object.assign(target.query, {
				applyFlagSettings: async (flags: unknown) => {
					calls.push(flags);
				},
				getSettings: async () => {
					if (++reads === 1) return inactive;
					if (reads === 2) {
						entered.resolve();
						await release.promise;
						return inactive;
					}
					return disabled;
				},
			});
			const observed: Array<Record<string, unknown>> = [];
			const events = await captureBridgeEventsAsync(
				async () => {
					try {
						pending = dispatchControl({
							command: "set_ultracode",
							session_id: target.sessionId,
							request_id: "old-inactive-request",
							enabled: true,
						});
						await boundedBridgeTask(entered.promise);
						assert.deepEqual(calls, [{ ultracode: true }]);
						assert.ok(
							observed.some(
								(event) =>
									(
										event.update as
											| { ultracode?: { requested: boolean } }
											| undefined
									)?.ultracode?.requested === true,
							),
						);
						if (transition === "observation") {
							handleSdkMessage(target, {
								type: "conversation_reset",
								new_conversation_id: "new-ultracode-conversation",
							} as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
							await fixture.settle();
							assert.ok(
								observed.some(
									(event) =>
										(
											event.update as
												| { ultracode?: { requested: boolean } }
												| undefined
										)?.ultracode?.requested === false,
								),
							);
						} else if (transition === "replacement")
							updateSessionId(
								target,
								`effort-replacement-${crypto.randomUUID()}`,
							);
						else beginSessionClose(target);
						release.resolve();
						await boundedBridgeTask(pending);
						await fixture.settle();
					} finally {
						release.resolve();
						if (pending) await boundedBridgeTask(pending);
						await fixture.settle();
					}
				},
				(event) => observed.push(event),
			);
			assert.equal(
				events.some(
					(event) =>
						event.event === "slash_error" &&
						event.request_id === "old-inactive-request",
				),
				false,
			);
			const live = transition === "close" ? peer : target;
			Object.assign(live.query, {
				applyFlagSettings: async () => {},
				getSettings: async () => inactive,
			});
			const control = await captureBridgeEventsAsync(() =>
				dispatchControl({
					command: "set_ultracode",
					session_id: live.sessionId,
					request_id: "current-inactive-request",
					enabled: true,
				}),
			);
			assert.ok(
				control.some(
					(event) =>
						event.event === "slash_error" &&
						event.session_id === live.sessionId &&
						event.request_id === "current-inactive-request",
				),
			);
		});
});

test("confirmed native receipts and literal REPL channels survive live wire delivery and saved result projection", async () => {
	const { readFileSync } = await import("node:fs");
	const rows = JSON.parse(
		readFileSync(
			new URL(
				"../../tests/fixtures/sdk-confirmed-tool-results.json",
				import.meta.url,
			),
			"utf8",
		),
	) as Array<{
		id: string;
		tool: string;
		input: Record<string, unknown>;
		result: Record<string, unknown>;
		fields: Array<[string, string]>;
		text: string;
	}>;
	const session = makeSessionState();
	session.sessionId = "confirmed-receipt-wire";
	for (const row of rows) {
		const launch = {
			type: "assistant",
			uuid: `${row.id}-launch`,
			session_id: session.sessionId,
			parent_tool_use_id: null,
			message: {
				role: "assistant",
				content: [
					{ type: "tool_use", id: row.id, name: row.tool, input: row.input },
				],
			},
		};
		const completion = {
			type: "user",
			uuid: `${row.id}-result`,
			session_id: session.sessionId,
			parent_tool_use_id: null,
			message: {
				role: "user",
				content: [
					{
						type: "tool_result",
						tool_use_id: row.id,
						content: "MODEL-FACING-FALLBACK",
					},
				],
			},
			toolUseResult: row.result,
		};
		const events = captureBridgeEvents(() => {
			for (const frame of [launch, completion])
				handleSdkMessage(
					session,
					frame as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage,
				);
		});
		const live = publishedTool(events, session.sessionId, row.id);
		const replay = mapSessionMessagesToUpdates([
			launch,
			{
				...completion,
				toolUseResult: undefined,
				message: {
					role: "user",
					content: [
						{
							type: "tool_result",
							tool_use_id: row.id,
							content: JSON.stringify(row.result),
						},
					],
				},
			},
		] as unknown as SessionMessage[]);
		const saved = replay
			.filter(
				(update) =>
					update.type === "tool_call_update" &&
					update.tool_call_update.tool_call_id === row.id,
			)
			.at(-1);
		assert.ok(saved?.type === "tool_call_update");
		for (const output of [live, saved.tool_call_update.fields]) {
			assert.equal(output.status, "completed");
			assert.equal(output.raw_output, row.text);
			assert.deepEqual(
				(
					output.output_metadata as
						| import("./types.js").ToolOutputMetadata
						| undefined
				)?.presentation,
				{
					sections: [
						{
							fields: row.fields.map(([label, value]) => ({
								label,
								value: { type: "text", value },
							})),
						},
					],
				},
			);
			assert.doesNotMatch(
				JSON.stringify({
					content: output.content,
					raw_output: output.raw_output,
					metadata: output.output_metadata,
				}),
				/MODEL-FACING-FALLBACK|PRIVATE-IMAGE|PRIVATE-DOCUMENT|REQUESTED-CODE/,
			);
		}
	}
});
