import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";
import type { BridgeEvent } from "../types.js";

const eventsModuleUrl = new URL("./events.js", import.meta.url).href;

const childDeadlineMs = 10_000;

function runWriterInChild(script: string, input: unknown): Promise<{
  code: number | null;
  stdout: Buffer;
  stderr: string;
}> {
  // Stdin carries large fixtures without depending on the host's command-line limit.
  const bootstrap = `
    const inputChunks = [];
    for await (const chunk of process.stdin) inputChunks.push(chunk);
    const input = JSON.parse(Buffer.concat(inputChunks).toString("utf8"));
    const events = await import(${JSON.stringify(eventsModuleUrl)});
    ${script}
  `;
  const env = { ...process.env };
  delete env.NODE_OPTIONS;
  delete env.NODE_DEBUG;
  delete env.CLAUDE_RS_BRIDGE_DIAGNOSTICS;
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ["--input-type=module", "-e", bootstrap], {
      cwd: fileURLToPath(new URL(".", import.meta.url)),
      env,
      windowsHide: true,
      stdio: ["pipe", "pipe", "pipe"],
    });
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    let stdoutBytes = 0;
    let stderrBytes = 0;
    let failure: Error | undefined;
    const stop = (error: Error) => {
      failure ??= error;
      // This helper owns this one Node child, which does not start subprocesses.
      child.kill("SIGKILL");
    };
    const deadline = setTimeout(() => {
      stop(new Error(`writer child exceeded ${childDeadlineMs}ms deadline`));
    }, childDeadlineMs);
    child.stdout.on("data", (chunk: Buffer) => {
      stdoutBytes += chunk.length;
      if (stdoutBytes <= 8 * 1024 * 1024) stdout.push(chunk);
      else stop(new Error("writer child exceeded stdout capture limit"));
    });
    child.stderr.on("data", (chunk: Buffer) => {
      stderrBytes += chunk.length;
      if (stderrBytes <= 128 * 1024) stderr.push(chunk);
      else stop(new Error("writer child exceeded stderr capture limit"));
    });
    child.on("error", (error) => stop(new Error("writer child spawn failed", { cause: error })));
    child.stdin.on("error", (error) => stop(new Error("writer child input failed", { cause: error })));
    child.stdout.on("error", stop);
    child.stderr.on("error", stop);
    child.on("close", (code, signal) => {
      clearTimeout(deadline);
      const diagnostics = Buffer.concat(stderr).toString("utf8");
      // Close follows process exit and complete pipe drainage, including on failure.
      if (failure) {
        reject(new Error(`${failure.message}; exit=${code}; signal=${signal}; stderr=${diagnostics}`, { cause: failure }));
      } else {
        resolve({ code, stdout: Buffer.concat(stdout), stderr: diagnostics });
      }
    });
    child.stdin.end(JSON.stringify(input));
  });
}

// Only the native syscall boundary is scripted. The imported production writer
// still owns UTF-8 encoding, byte offsets, retry and terminal-error propagation.
const nativeWriteScript = `
  const fs = (await import("node:fs")).default;
  const { syncBuiltinESMExports } = await import("node:module");
  const nativeWrite = fs.writeSync;
  const accepted = [];
  let calls = 0;
  let recovered = false;
  const sinkError = Object.assign(new Error("controlled native sink failure"), {
    code: input.terminal ? "EPIPE" : "EAGAIN",
  });
  fs.writeSync = (fd, buffer, offset, length) => {
    if (fd !== process.stdout.fd) return nativeWrite(fd, buffer, offset, length);
    calls += 1;
    if (calls === 1) {
      accepted.push(Buffer.from(buffer.subarray(offset, offset + input.prefixBytes)));
      return input.prefixBytes;
    }
    if (calls === 2) throw sinkError;
    // Fail promptly if a terminal error is incorrectly retried.
    if (input.terminal) throw new Error("retried terminal sink failure");
    recovered = true;
    accepted.push(Buffer.from(buffer.subarray(offset, offset + length)));
    return length;
  };
  syncBuiltinESMExports();
  let completed = false;
  let caught;
  try {
    events.writeProtocolEventToStdout(input.record);
    completed = true;
  } catch (error) {
    caught = error;
  } finally {
    fs.writeSync = nativeWrite;
    syncBuiltinESMExports();
  }
  // Report through stderr, never through the protocol sink under test.
  process.stderr.write(JSON.stringify({
    completed,
    recovered,
    originalError: caught === sinkError,
    code: caught?.code,
    error: caught?.message,
    accepted: Buffer.concat(accepted).toString("base64"),
  }));
`;

test("protocol events larger than the pipe buffer reach the reader as one complete line", async () => {
  const message = Array.from({ length: 32_768 }, (_, index) =>
    `segment ${index}: 🧭漢字 café e\u0301 العربية\n"quoted" \\ end\r\n`,
  ).join("");
  const event = { event: "connection_failed", message } satisfies BridgeEvent;
  const requestId = "large-event-request-evt1-東京";
  const { code, stdout, stderr } = await runWriterInChild(
    "events.writeEvent(input.event, input.requestId);",
    { event, requestId },
  );

  assert.equal(code, 0, stderr);
  assert.equal(stderr, "", "unexpected writer diagnostics");
  assert.equal(stdout.at(-1), 0x0a, "record must end with the NDJSON delimiter");
  assert.equal(stdout.indexOf(0x0a), stdout.length - 1, "exactly one record must be emitted");
  const decoded = new TextDecoder("utf-8", { fatal: true }).decode(stdout);
  assert.deepEqual(JSON.parse(decoded), { ...event, request_id: requestId });
});

// This independent record includes multibyte characters before and after the
// accepted prefix, which deliberately splits the first emoji's UTF-8 encoding.
const nativeRecord = '{"message":"🧭漢e\u0301\\n\\"quoted\\"\\\\tail","request_id":"native-evt1"}\n';
const prefixBytes = 14;

test("protocol writer preserves exact UTF-8 bytes across a short write and EAGAIN", async () => {
  const { code, stdout, stderr } = await runWriterInChild(nativeWriteScript, {
    record: nativeRecord,
    prefixBytes,
    terminal: false,
  });
  assert.equal(code, 0, stderr);
  assert.equal(stdout.length, 0, "native seam must capture protocol bytes");
  const report = JSON.parse(stderr);
  assert.equal(report.completed, true, report.error);
  assert.equal(report.recovered, true, "writer must retry the transient failure");
  assert.deepEqual(Buffer.from(report.accepted, "base64"), Buffer.from(nativeRecord, "utf8"));
});

test("protocol writer propagates the original terminal error after an accepted prefix", async () => {
  const { code, stdout, stderr } = await runWriterInChild(nativeWriteScript, {
    record: nativeRecord,
    prefixBytes,
    terminal: true,
  });
  assert.equal(code, 0, stderr);
  assert.equal(stdout.length, 0, "failure reporting must stay off the broken sink");
  const report = JSON.parse(stderr);
  assert.equal(report.completed, false, "failed delivery must not report success");
  assert.equal(report.originalError, true, report.error);
  assert.equal(report.code, "EPIPE");
  assert.deepEqual(Buffer.from(report.accepted, "base64"), Buffer.from(nativeRecord, "utf8").subarray(0, prefixBytes));
});
