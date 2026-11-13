import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

const loggerModuleUrl = new URL("./logger.js", import.meta.url).href;
const lifecycleModuleUrl = new URL("./session_lifecycle.js", import.meta.url).href;
const sharedModuleUrl = new URL("./shared.js", import.meta.url).href;
const childCwd = fileURLToPath(new URL(".", import.meta.url));
const childTimeoutMs = 15_000;
const maxOutputBytes = 1_048_576;
const privateHome = process.platform === "win32"
  ? "C:\\Users\\T173_HOME_PRIVATE_SENTINEL"
  : "/home/T173_HOME_PRIVATE_SENTINEL";

type DiagnosticRecord = Record<string, unknown> & {
  fields?: Record<string, unknown>;
};

function childEnvironment(mode: string | undefined): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...process.env, HOME: privateHome, USERPROFILE: privateHome };
  delete env.CLAUDE_RS_BRIDGE_DIAGNOSTICS;
  if (mode !== undefined) env.CLAUDE_RS_BRIDGE_DIAGNOSTICS = mode;
  return env;
}

/** Import-time modes need separate processes; close settles after both pipes drain. */
async function runDiagnosticChild(mode: string | undefined, script: string) {
  return await new Promise<{ stdout: string; stderr: string }>((resolve, reject) => {
    const child = spawn(process.execPath, ["--input-type=module", "-e", script], {
      cwd: childCwd,
      env: childEnvironment(mode),
      windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    let outputBytes = 0;
    let failure: Error | undefined;
    const deadline = setTimeout(() => {
      failure = new Error(`diagnostic child exceeded ${childTimeoutMs}ms deadline`);
      child.kill();
    }, childTimeoutMs);
    function capture(chunks: Buffer[], chunk: Buffer) {
      const remaining = maxOutputBytes - outputBytes;
      outputBytes += chunk.length;
      if (remaining > 0) chunks.push(chunk.subarray(0, remaining));
      if (outputBytes > maxOutputBytes && !failure) {
        failure = new Error(`diagnostic child exceeded ${maxOutputBytes} output bytes`);
        child.kill();
      }
    }
    child.stdout.on("data", (chunk: Buffer) => capture(stdout, chunk));
    child.stderr.on("data", (chunk: Buffer) => capture(stderr, chunk));
    child.on("error", (error) => { failure = error; });
    child.on("close", (code, signal) => {
      clearTimeout(deadline);
      const captured = {
        stdout: Buffer.concat(stdout).toString("utf8"),
        stderr: Buffer.concat(stderr).toString("utf8"),
      };
      if (failure || code !== 0 || signal !== null) {
        reject(new Error(
          `${failure?.message ?? "diagnostic child failed"}; exit=${code}; signal=${signal}\nstdout: ${captured.stdout}\nstderr: ${captured.stderr}`,
        ));
      } else {
        resolve(captured);
      }
    });
  });
}

function diagnosticRecords(stderr: string): DiagnosticRecord[] {
  return stderr.split("\n").filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as DiagnosticRecord);
}

const genericEvents = ["trace", "debug", "info", "warn", "error"].map((level, index) => {
  const credentialPrefix = `${level} safe cause ${"p".repeat(160)} password=`;
  return {
    level,
    eventName: `log1_${level}_identity`,
    fields: {
      cause: `${level} fixture EACCES`,
      detail: `${level} useful safe prefix ${"long-safe-detail ".repeat(150)}`,
      error_message: `${credentialPrefix}"FIRST T173_${level}_PRIVATE_FRAGMENT ${"secret tail ".repeat(50)}"`,
      count: index + 11,
      enabled: index % 2 === 0,
      empty: null,
    },
    redactedError: `${credentialPrefix}[redacted]`,
  };
});

test("default diagnostics write only warnings and errors, with bounded and redacted fields", async (t) => {
  for (const mode of [undefined, "0", "1"]) {
    await t.test(`LOG1 ${mode === undefined ? "unset" : mode === "0" ? "baseline" : "detailed"} mode`, async () => {
      const output = await runDiagnosticChild(mode, `
        const { bridgeLogger, LOG_TARGETS } = await import(${JSON.stringify(loggerModuleUrl)});
        for (const { level, eventName, fields } of ${JSON.stringify(genericEvents)}) {
          bridgeLogger[level]({ target: LOG_TARGETS.BRIDGE_SDK, eventName, message: level, fields });
        }
      `);
      assert.equal(output.stdout, "", "diagnostics must use stderr exclusively");
      const records = diagnosticRecords(output.stderr);
      const admitted = genericEvents.filter(({ level }) => mode === "1" || (mode === "0" && ["warn", "error"].includes(level)));
      assert.deepEqual(
        records.map((record) => record.event_name).sort(),
        admitted.map(({ eventName }) => eventName).sort(),
      );
      if (mode === undefined) assert.equal(output.stderr, "");
      for (const expected of admitted) {
        const record = records.find((candidate) => candidate.event_name === expected.eventName);
        assert.ok(record, `missing ${expected.eventName}`);
        assert.equal(record.level, expected.level, expected.eventName);
        assert.equal(record.target, "bridge.sdk", expected.eventName);
        assert.ok(record.fields, expected.eventName);
        if (mode === "1") {
          assert.deepEqual(record.fields, expected.fields, expected.eventName);
        } else {
          assert.equal(record.fields.cause, expected.fields.cause, expected.eventName);
          assert.equal(record.fields.error_message, expected.redactedError, expected.eventName);
          assert.equal(record.fields.count, expected.fields.count, expected.eventName);
          assert.equal(record.fields.enabled, expected.fields.enabled, expected.eventName);
          assert.equal(record.fields.empty, null, expected.eventName);
          assert.equal(typeof record.fields.detail, "string");
          const detail = record.fields.detail as string;
          assert.ok(detail.startsWith(`${expected.level} useful safe prefix long-safe-detail `), expected.eventName);
          assert.ok(detail.length <= 243, expected.eventName);
        }
      }
    });
  }
});

test("a turn error from an interrupted turn is not a warning in the default log", async (t) => {
  const cases = [
    { reason: "aborted_streaming", errorKind: "other", level: "debug" },
    { reason: "aborted_tools", errorKind: "other", level: "debug" },
    { reason: undefined, errorKind: "auth_required", level: "warn" },
    { reason: "model_error", errorKind: "other", level: "warn" },
    { reason: undefined, errorKind: undefined, level: "warn" },
  ].map((row, index) => ({ ...row, sessionId: `log2-session-${index}`, requestId: `log2-request-${index}` }));
  for (const mode of ["0", "1"]) {
    await t.test(`LOG2 ${mode === "0" ? "baseline" : "detailed"} mode`, async () => {
      const output = await runDiagnosticChild(mode, `
        const { logBridgeEventSent } = await import(${JSON.stringify(loggerModuleUrl)});
        for (const row of ${JSON.stringify(cases)}) {
          logBridgeEventSent({ event: "turn_error", session_id: row.sessionId,
            message: "supplied turn failure", ...(row.errorKind ? { error_kind: row.errorKind } : {}),
            ...(row.reason ? { terminal_reason: row.reason } : {}) }, row.requestId, 100);
        }
      `);
      assert.equal(output.stdout, "");
      const records = diagnosticRecords(output.stderr);
      const admitted = cases.filter((row) => mode === "1" || row.level === "warn");
      assert.deepEqual(records.map((record) => record.request_id).sort(), admitted.map((row) => row.requestId).sort());
      for (const expected of admitted) {
        const record = records.find((candidate) => candidate.request_id === expected.requestId);
        assert.ok(record, expected.requestId);
        assert.equal(record.event_name, "bridge_event_sent", expected.requestId);
        assert.equal(record.target, "bridge.protocol", expected.requestId);
        assert.equal(record.session_id, expected.sessionId, expected.requestId);
        assert.equal(record.level, expected.level, expected.requestId);
        assert.equal(record.fields?.bridge_event, "turn_error", expected.requestId);
        assert.equal(record.fields?.terminal_reason, expected.reason, `${expected.requestId} terminal reason`);
        assert.equal(record.error_kind, expected.errorKind, `${expected.requestId} error kind`);
        if (expected.reason === undefined) assert.ok(!Object.hasOwn(record.fields ?? {}, "terminal_reason"));
        if (expected.errorKind === undefined) assert.ok(!Object.hasOwn(record, "error_kind"));
      }
    });
  }
});

test("native SDK stderr through query options remains useful and private in enabled diagnostic modes", async (t) => {
  for (const mode of ["0", "1"]) {
    await t.test(`A162 ${mode === "0" ? "baseline" : "detailed"} mode`, async () => {
      const lines = [
        { marker: "native-warning-cause", cause: "EACCES", level: "warn", line: "warning native-warning-cause EACCES api_key=T173_WARNING_CREDENTIAL" },
        { marker: "native-error-cause", cause: "ECONNREFUSED", level: "error", line: "error native-error-cause ECONNREFUSED Bearer T173_ERROR_CREDENTIAL" },
        { marker: "native-panic-cause", cause: "worker lock poisoned", level: "error", line: `native-panic-cause panic worker lock poisoned ${"p".repeat(130)} password="FIRST T173_BOUNDARY_PRIVATE_FRAGMENT ${"secret tail ".repeat(50)}"` },
        { marker: "native-ordinary-cause", cause: "waiting for worker", level: "debug", line: "native-ordinary-cause waiting for worker token=T173_DEBUG_CREDENTIAL" },
      ];
      const output = await runDiagnosticChild(mode, `
        const { homedir } = await import("node:os");
        const { buildQueryOptions } = await import(${JSON.stringify(lifecycleModuleUrl)});
        const { AsyncQueue } = await import(${JSON.stringify(sharedModuleUrl)});
        const options = buildQueryOptions({ resolvedSettings: {}, cwd: process.cwd(),
          launchSettings: {}, provisionalSessionId: "a162-unused-session", input: new AsyncQueue(),
          canUseTool: async () => ({ behavior: "deny", message: "not invoked" }),
          enableSdkDebug: false, enableSpawnDebug: false, sessionIdForLogs: () => "a162-unused-session" });
        if (typeof options.stderr !== "function") throw new Error("query options lack stderr callback");
        for (const line of ["", " ", "\\t\\r\\n"]) options.stderr(line);
        for (const { line } of ${JSON.stringify(lines)}) options.stderr(line + " path=" + homedir() + "/T173-private-native-file");
      `);
      assert.equal(output.stdout, "", "native diagnostics must not contaminate protocol stdout");
      for (const secret of ["T173_WARNING_CREDENTIAL", "T173_ERROR_CREDENTIAL", "T173_BOUNDARY_PRIVATE_FRAGMENT", "T173_DEBUG_CREDENTIAL", "T173_HOME_PRIVATE_SENTINEL"]) {
        assert.ok(!`${output.stdout}${output.stderr}`.includes(secret), `complete output leaked ${secret}`);
      }
      const records = diagnosticRecords(output.stderr);
      const admitted = lines.filter((row) => mode === "1" || row.level !== "debug");
      assert.equal(records.length, admitted.length, "blank lines and filtered ordinary lines must remain silent");
      for (const expected of admitted) {
        const matches = records.filter((record) => typeof record.fields?.preview === "string" && record.fields.preview.includes(expected.marker));
        assert.equal(matches.length, 1, `${expected.marker} must identify one useful record`);
        const [record] = matches;
        assert.equal(record.event_name, "sdk_stderr_line", expected.marker);
        assert.equal(record.target, "bridge.sdk", expected.marker);
        assert.equal(record.level, expected.level, expected.marker);
        const preview = record.fields?.preview as string;
        assert.ok(preview.includes(expected.cause), expected.marker);
        assert.ok(preview.includes("[redacted]"), expected.marker);
        assert.ok(preview.length <= 243, expected.marker);
        if (expected.marker !== "native-panic-cause") {
          assert.ok(preview.includes("~/T173-private-native-file"), expected.marker);
        }
      }
    });
  }
});
