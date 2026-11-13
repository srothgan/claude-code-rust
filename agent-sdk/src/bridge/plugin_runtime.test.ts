import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import type {
  McpServerStatus,
  SDKControlReloadPluginsResponse,
} from "@anthropic-ai/claude-agent-sdk";

type NativeMessage = {
  type: string;
  subtype?: string;
  session_id: string;
  content?: unknown;
  result?: string;
  is_error?: boolean;
};
type Observation = Partial<SDKControlReloadPluginsResponse> & {
  name: string;
  error?: string;
  messages?: NativeMessage[];
  servers?: McpServerStatus[];
};
type PeerEntry = {
  pid: number;
  kind: string;
  wire?: string;
  message?: { id?: string | number; method: string };
};

test("native plugin attachment, partial application, held Apply and explicit resume preserve SDK semantics", async (context) => {
  const env = { ...process.env };
  for (const key of Object.keys(env)) {
    if (/^(ANTHROPIC_|CLAUDE_|CLAUDECODE$)/.test(key)) delete env[key];
  }
  const fixture = fileURLToPath(
    new URL("../../../tests/fixtures/sdk-plugin-runtime.mjs", import.meta.url),
  );
  const { stdout } = await promisify(execFile)(process.execPath, [fixture], {
    env,
    encoding: "utf8",
    timeout: 60_000,
    windowsHide: true,
  });
  const result = JSON.parse(stdout) as {
    sdk: string;
    root: string;
    sessionId: string;
    results: Observation[];
    peer: PeerEntry[];
    requests: Array<{
      phase: string;
      method: string;
      route: string;
      messages: Array<{ role: string; content: unknown }>;
    }>;
  };
  context.diagnostic(`Installed SDK ${result.sdk}`);
  function observation(name: string): Observation {
    const matches = result.results.filter((entry) => entry.name === name);
    assert.equal(matches.length, 1, `${name}: named completed observation`);
    assert.equal(
      Object.hasOwn(matches[0], "error"),
      false,
      `${name}: an error is not empty inventory`,
    );
    return matches[0];
  }
  function reload(name: string) {
    const entry = observation(name);
    assert.ok(Array.isArray(entry.plugins), `${name}: returned plugins`);
    assert.ok(Array.isArray(entry.commands), `${name}: returned commands`);
    assert.ok(Array.isArray(entry.mcpServers), `${name}: returned MCP inventory`);
    assert.equal(
      entry.error_count,
      name === "partial" ? 1 : 0,
      `${name}: only the deliberately malformed fixture-b may fail`,
    );
    return entry as Observation & SDKControlReloadPluginsResponse;
  }
  function inventory(name: string, expected: string[]) {
    const entry = reload(name);
    const plugins = entry.plugins.filter((plugin) => plugin.name.startsWith("fixture-"));
    assert.deepEqual(
      plugins
        .map((plugin) => ({ name: plugin.name, path: path.normalize(plugin.path) }))
        .sort((left, right) => left.name.localeCompare(right.name)),
      expected
        .map((name) => ({ name, path: path.join(result.root, name) }))
        .sort((left, right) => left.name.localeCompare(right.name)),
      `${name}: exact fixture folders`,
    );
    for (const plugin of plugins) {
      if (plugin.version !== undefined) assert.equal(plugin.version, "1.2.3", name);
      if (plugin.source !== undefined) assert.equal(plugin.source, `${plugin.name}@inline`, name);
    }
    assert.deepEqual(
      entry.commands
        .filter((command) => command.name.startsWith("fixture-"))
        .map((command) => command.name)
        .sort(),
      expected.map((name) => `${name}:hello`).sort(),
      `${name}: authored command identities`,
    );
    return entry;
  }
  inventory("initialization", []);
  inventory("attach", ["fixture-a"]);
  inventory("partial", ["fixture-a"]);
  inventory("retry-repaired", ["fixture-a", "fixture-b"]);
  inventory("duplicate", ["fixture-a", "fixture-b"]);

  function completedTurn(name: string) {
    const messages = observation(name).messages;
    assert.ok(Array.isArray(messages), `${name}: actual native stream`);
    const init = messages.filter(
      (message) => message.type === "system" && message.subtype === "init",
    );
    assert.equal(init.length, 1, `${name}: initialized session`);
    assert.equal(init[0].session_id, result.sessionId, `${name}: original session target`);
    assert.deepEqual(
      messages.filter((message) => message.type === "assistant").map((message) => message.content),
      [[{ type: "text", text: "LOCAL-PROBE-OK" }]],
      name,
    );
    const terminal = messages.filter((message) => message.type === "result");
    assert.equal(terminal.length, 1, `${name}: terminal turn`);
    assert.equal(terminal[0].session_id, result.sessionId, name);
    assert.equal(terminal[0].subtype, "success", name);
    assert.equal(terminal[0].is_error, false, name);
    assert.equal(terminal[0].result, "LOCAL-PROBE-OK", name);
  }
  completedTurn("completed-turn");
  const held = inventory("held-mcp", ["fixture-a", "fixture-b"]);
  assert.equal(held.held, true);
  assert.ok(
    held.cache_impact?.mcp_servers_added.includes("plugin:fixture-mcp:fixture"),
    "hold identifies the requested MCP server",
  );
  assert.equal(
    held.mcpServers.some(
      (server) => server.name === "plugin:fixture-mcp:fixture" && server.status === "connected",
    ),
    false,
    "held inventory does not report an applied active server",
  );
  const applied = inventory("apply-mcp", ["fixture-a", "fixture-b", "fixture-mcp"]);
  assert.notEqual(applied.held, true, "explicit Apply is not held");
  const servers = observation("applied-status").servers;
  assert.ok(Array.isArray(servers), "actual native MCP status after Apply");
  const server = servers.find((server) => server.name === "plugin:fixture-mcp:fixture");
  assert.ok(server, "requested local MCP server");
  assert.equal(server.status, "connected", "Apply establishes real MCP readiness");
  assert.deepEqual(server.serverInfo, { name: "fixture-mcp-peer", version: "1.0.0" });
  if (server.source !== undefined) assert.equal(server.source, "plugin");
  assert.ok(
    server.tools?.some((tool) => tool.name === "inspect_fixture"),
    "native tool inventory contains the peer's tool",
  );
  if (server.config !== undefined)
    assert.deepEqual(server.config, {
      type: "stdio",
      command: process.execPath,
      args: [path.join(result.root, "mcp.mjs")],
    });

  const starts = result.peer.filter((entry) => entry.kind === "start");
  assert.ok(starts.length > 0, "actual MCP peer process started");
  for (const start of starts)
    assert.ok(
      result.peer.some((entry) => entry.kind === "exit" && entry.pid === start.pid),
      "owned peer exited before cleanup",
    );
  function responseTo(method: string) {
    const request = result.peer.find(
      (entry) => entry.kind === "request" && entry.message?.method === method,
    );
    assert.ok(request?.message, `${method}: peer received real JSON-RPC request`);
    const response = result.peer.find(
      (entry) =>
        entry.kind === "response" &&
        entry.pid === request.pid &&
        JSON.parse(entry.wire as string).id === request.message?.id,
    );
    assert.ok(response?.wire, `${method}: correlated peer response`);
    assert.equal(response.wire.endsWith("\n"), true, `${method}: newline-delimited transport`);
    assert.equal(response.wire.trim().split("\n").length, 1, `${method}: one complete frame`);
    const frame = JSON.parse(response.wire);
    assert.equal(frame.jsonrpc, "2.0", method);
    assert.equal(frame.error, undefined, method);
    return frame.result;
  }
  assert.deepEqual(responseTo("initialize"), {
    protocolVersion: "2025-03-26",
    capabilities: { tools: {} },
    serverInfo: { name: "fixture-mcp-peer", version: "1.0.0" },
  });
  assert.deepEqual(responseTo("tools/list"), {
    tools: [
      {
        name: "inspect_fixture",
        description: "Inert fixture",
        inputSchema: { type: "object", properties: {} },
      },
    ],
  });
  inventory("resume", []);
  inventory("resume-explicit", ["fixture-a", "fixture-b"]);
  completedTurn("resumed-turn");
  inventory("new", []);
  const original = result.requests.filter((request) => request.phase === "completed-turn");
  const resumed = result.requests.filter((request) => request.phase === "resumed-turn");
  assert.equal(original.length, 1);
  assert.equal(resumed.length, 1);
  for (const request of [...original, ...resumed]) {
    assert.equal(request.method, "POST");
    assert.equal(request.route, "/v1/messages");
  }
  const containsPrompt = (request: (typeof original)[number], text: string) =>
    request.messages.some(
      (message) => message.role === "user" && JSON.stringify(message.content).includes(text),
    );
  assert.ok(containsPrompt(original[0], "Original plugin turn: only reply LOCAL-PROBE-OK"));
  assert.ok(
    containsPrompt(resumed[0], "Original plugin turn: only reply LOCAL-PROBE-OK"),
    "explicit resume restores the original transcript",
  );
  assert.ok(containsPrompt(resumed[0], "Explicit resume turn: only reply LOCAL-PROBE-OK"));
});
