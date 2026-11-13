import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import test from "node:test";
import { query } from "@anthropic-ai/claude-agent-sdk";
import {
  authenticateMcpServer,
  clearMcpServerAuth,
  detectMcpAuthCapabilities,
  submitMcpOAuthCallbackUrl,
} from "./mcp_auth_adapter.js";
import { BridgeCommandScheduler } from "./command_scheduler.js";
import { parseCommandEnvelope } from "./commands.js";
import { scheduleBridgeCommand, resolveInstalledAgentSdkVersion } from "../bridge.js";
import { replaceProtocolEventWriter } from "./events.js";
import { sessions, type SessionState } from "./session_lifecycle.js";
import type { BridgeEvent } from "../types.js";
import type { McpServerStatus } from "@anthropic-ai/claude-agent-sdk";

test("MCP capabilities independently reflect callable methods without invoking them", () => {
  const callable = () =>
    assert.fail("capability inspection must not invoke the SDK");
  const cases = [
    {
      query: {},
      expected: {
        authenticate: false,
        clear_auth: false,
        submit_oauth_callback_url: false,
      },
    },
    {
      query: { mcpAuthenticate: callable },
      expected: {
        authenticate: true,
        clear_auth: false,
        submit_oauth_callback_url: false,
      },
    },
    {
      query: { mcpClearAuth: callable },
      expected: {
        authenticate: false,
        clear_auth: true,
        submit_oauth_callback_url: false,
      },
    },
    {
      query: { mcpSubmitOAuthCallbackUrl: callable },
      expected: {
        authenticate: false,
        clear_auth: false,
        submit_oauth_callback_url: true,
      },
    },
    {
      query: {
        mcpAuthenticate: "not-callable",
        mcpClearAuth: null,
        mcpSubmitOAuthCallbackUrl: true,
      },
      expected: {
        authenticate: false,
        clear_auth: false,
        submit_oauth_callback_url: false,
      },
    },
    {
      query: {
        mcpAuthenticate: callable,
        mcpClearAuth: 0,
        mcpSubmitOAuthCallbackUrl: callable,
      },
      expected: {
        authenticate: true,
        clear_auth: false,
        submit_oauth_callback_url: true,
      },
    },
    {
      query: {
        mcpAuthenticate: callable,
        mcpClearAuth: callable,
        mcpSubmitOAuthCallbackUrl: callable,
      },
      expected: {
        authenticate: true,
        clear_auth: true,
        submit_oauth_callback_url: true,
      },
    },
  ];
  for (const row of cases)
    assert.deepEqual(detectMcpAuthCapabilities(row.query), row.expected);
});
test("MCP auth adapter preserves receiver and forwards exact arguments", async () => {
  const calls: Array<{ method: string; args: string[] }> = [];
  const query = {
    marker: "query",
    async mcpAuthenticate(this: { marker: string }, serverName: string) {
      assert.equal(this.marker, "query");
      calls.push({ method: "authenticate", args: [serverName] });
      return {
        authUrl: "https://auth.example.test/start",
        requiresUserAction: true,
      };
    },
    async mcpClearAuth(this: { marker: string }, serverName: string) {
      assert.equal(this.marker, "query");
      calls.push({ method: "clear_auth", args: [serverName] });
    },
    async mcpSubmitOAuthCallbackUrl(
      this: { marker: string },
      serverName: string,
      callbackUrl: string,
    ) {
      assert.equal(this.marker, "query");
      calls.push({
        method: "submit_callback",
        args: [serverName, callbackUrl],
      });
    },
  };

  assert.deepEqual(await authenticateMcpServer(query, "docs"), {
    server_name: "docs",
    auth_url: "https://auth.example.test/start",
    requires_user_action: true,
  });
  await clearMcpServerAuth(query, "docs");
  await submitMcpOAuthCallbackUrl(
    query,
    "docs",
    "https://callback.example.test/code",
  );

  assert.deepEqual(calls, [
    { method: "authenticate", args: ["docs"] },
    { method: "clear_auth", args: ["docs"] },
    {
      method: "submit_callback",
      args: ["docs", "https://callback.example.test/code"],
    },
  ]);
});

test("MCP auth rejects incompatible capabilities and responses while preserving legitimate no-redirect outcomes", async () => {
  for (const [operation, method] of [
    [() => authenticateMcpServer({}, "docs"), "mcpAuthenticate"],
    [() => clearMcpServerAuth({}, "docs"), "mcpClearAuth"],
    [
      () =>
        submitMcpOAuthCallbackUrl(
          {},
          "docs",
          "https://callback.example.test/code",
        ),
      "mcpSubmitOAuthCallbackUrl",
    ],
  ] as const) {
    await assert.rejects(
      operation,
      (error) =>
        error instanceof Error &&
        error.message.includes(method) &&
        /support/.test(error.message),
    );
  }
  for (const [response, cause] of [
    ["not-an-object", /response/],
    [42, /response/],
    [[], /response/],
    [{ authUrl: 42 }, /authUrl/],
    [{ authUrl: "" }, /authUrl/],
    [{ authUrl: " \n " }, /authUrl/],
    [
      { authUrl: "https://auth.example.test/start", requiresUserAction: "yes" },
      /requiresUserAction/,
    ],
    [
      { authUrl: "https://auth.example.test/start", requiresUserAction: null },
      /requiresUserAction/,
    ],
  ] as const) {
    await assert.rejects(
      authenticateMcpServer({ mcpAuthenticate: async () => response }, "docs"),
      (error) =>
        error instanceof Error &&
        /mcpAuthenticate/.test(error.message) &&
        cause.test(error.message),
    );
  }
  for (const response of [
    undefined,
    null,
    {},
    { authUrl: undefined },
    { authUrl: null },
  ]) {
    assert.equal(
      await authenticateMcpServer(
        { mcpAuthenticate: async () => response },
        "docs",
      ),
      null,
    );
  }
  const url = "https://auth.example.test/start?state=a%2Bb";
  for (const [response, expected] of [
    [{ authUrl: url }, false],
    [{ authUrl: url, requiresUserAction: false }, false],
    [{ authUrl: url, requiresUserAction: true }, true],
  ] as const) {
    assert.deepEqual(
      await authenticateMcpServer(
        { mcpAuthenticate: async () => response },
        "selected-docs",
      ),
      {
        server_name: "selected-docs",
        auth_url: url,
        requires_user_action: expected,
      },
    );
  }
});

async function bounded<T>(task: Promise<T>, description: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      task,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(
          () =>
            reject(
              new assert.AssertionError({
                message: description,
                actual: "pending",
                expected: "settled",
              }),
            ),
          5_000,
        );
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

test("installed SDK transports auth, clear and callback operations through the production adapter", {
  timeout: 20_000,
}, async (context) => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-mcp-auth-"));
  const project = join(directory, "project");
  const profile = join(directory, "profile");
  const peer = join(directory, "peer.cjs");
  const journal = join(directory, "requests.jsonl");
  const release = join(directory, "release-request");
  let child: ChildProcessWithoutNullStreams | undefined;
  let closed: Promise<void> | undefined;
  let sdk: ReturnType<typeof query> | undefined;
  let stderr = "";
  try {
    mkdirSync(project);
    mkdirSync(profile);
    writeFileSync(journal, "");
    writeFileSync(
      peer,
      `
      const fs = require("node:fs");
      const send = value => process.stdout.write(JSON.stringify(value) + "\\n");
      const pending = new Map();
      const input = require("node:readline").createInterface({ input: process.stdin });
      const response = (message, body) => send({ type: "control_response", response: { subtype: "success", request_id: message.request_id, response: body } });
      input.on("line", line => {
        const message = JSON.parse(line);
        if (message.type !== "control_request") return;
        fs.appendFileSync(${JSON.stringify(journal)}, JSON.stringify(message) + "\\n");
        if (message.request.subtype.startsWith("mcp_")) pending.set(message.request_id, message);
        else response(message, { commands: [], agents: [], models: [], account: {} });
      });
      const timer = setInterval(() => {
        if (!fs.existsSync(${JSON.stringify(release)})) return;
        const id = fs.readFileSync(${JSON.stringify(release)}, "utf8");
        const message = pending.get(id);
        if (!message) return;
        pending.delete(id);
        if (message.request.serverName === "reject-clear") send({ type: "control_response", response: { subtype: "error", request_id: id, error: "fixture credential removal rejected" } });
        else response(message, message.request.subtype === "mcp_authenticate" ? { authUrl: "https://auth.example.test/native?state=literal%2Bvalue", requiresUserAction: false } : { acknowledged: message.request.subtype });
      }, 5);
      input.on("close", () => { clearInterval(timer); process.exit(0); });
    `,
    );
    const env = Object.fromEntries(
      Object.entries(process.env).filter(
        ([key]) => !/^(CLAUDE|ANTHROPIC)/i.test(key),
      ),
    );
    env.CLAUDE_CONFIG_DIR = profile;
    context.diagnostic(
      `Installed SDK ${resolveInstalledAgentSdkVersion() ?? "unknown"}`,
    );
    sdk = query({
      prompt: "unused-local-control-input",
      options: {
        cwd: project,
        pathToClaudeCodeExecutable: peer,
        executable: "node",
        settingSources: [],
        persistSession: false,
        mcpServers: {},
        spawnClaudeCodeProcess(options) {
          assert.equal(
            options.command,
            peer,
            "spawn only this fixture-owned CLI",
          );
          const owned = spawn(process.execPath, [peer, ...options.args], {
            cwd: project,
            env,
            stdio: "pipe",
            windowsHide: true,
          });
          child = owned;
          owned.stderr.on("data", (chunk) => {
            stderr += chunk.toString();
          });
          closed = new Promise<void>((resolve) =>
            owned.once("close", () => resolve()),
          );
          return owned;
        },
      },
    });
    await bounded(
      sdk.initializationResult(),
      "the controlled native Query must initialize",
    );
    type Request = {
      request_id: string;
      request: { subtype: string; serverName?: string; callbackUrl?: string };
    };
    const requests = (): Request[] => {
      const content = readFileSync(journal, "utf8");
      return content.slice(0, content.lastIndexOf("\n") + 1)
        .split("\n")
        .filter(Boolean)
        .map((line) => JSON.parse(line));
    };
    async function control<T>(
      operation: Promise<T>,
      subtype: string,
      serverName: string,
      failure = false,
    ): Promise<T | undefined> {
      let settled = false;
      void operation.then(
        () => {
          settled = true;
        },
        () => {
          settled = true;
        },
      );
      const started = Date.now();
      let received: Request | undefined;
      while (!received) {
        received = requests().find(
          (message) =>
            message.request.subtype === subtype &&
            message.request.serverName === serverName,
        );
        assert.ok(
          Date.now() - started < 5_000,
          `missing ${subtype}/${serverName}: ${stderr}`,
        );
        if (!received)
          await new Promise<void>((resolve) => setTimeout(resolve, 10));
      }
      assert.equal(typeof received.request_id, "string");
      assert.ok(received.request_id.length > 0);
      await new Promise<void>((resolve) => setImmediate(resolve));
      assert.equal(
        settled,
        false,
        "the adapter must await the correlated native response",
      );
      writeFileSync(release, received.request_id);
      if (failure)
        await assert.rejects(
          bounded(operation, "native rejection must settle"),
          /fixture credential removal rejected/,
        );
      else
        return await bounded(
          operation,
          "the correlated native response must settle the adapter operation",
        );
    }
    assert.deepEqual(
      await control(
        authenticateMcpServer(sdk, "docs-auth"),
        "mcp_authenticate",
        "docs-auth",
      ),
      {
        server_name: "docs-auth",
        auth_url: "https://auth.example.test/native?state=literal%2Bvalue",
        requires_user_action: false,
      },
    );
    assert.equal(
      await control(
        clearMcpServerAuth(sdk, "docs-clear"),
        "mcp_clear_auth",
        "docs-clear",
      ),
      undefined,
    );
    const callback =
      "https://callback.example.test/complete?code=a%2Bb&state=literal+value";
    assert.equal(
      await control(
        submitMcpOAuthCallbackUrl(sdk, "docs-callback", callback),
        "mcp_oauth_callback_url",
        "docs-callback",
      ),
      undefined,
    );
    await control(
      clearMcpServerAuth(sdk, "reject-clear"),
      "mcp_clear_auth",
      "reject-clear",
      true,
    );
    assert.deepEqual(
      requests()
        .filter((message) => message.request.subtype.startsWith("mcp_"))
        .map(({ request }) => ({
          subtype: request.subtype,
          serverName: request.serverName,
          ...(request.callbackUrl === undefined
            ? {}
            : { callbackUrl: request.callbackUrl }),
        })),
      [
        { subtype: "mcp_authenticate", serverName: "docs-auth" },
        { subtype: "mcp_clear_auth", serverName: "docs-clear" },
        {
          subtype: "mcp_oauth_callback_url",
          serverName: "docs-callback",
          callbackUrl: callback,
        },
        { subtype: "mcp_clear_auth", serverName: "reject-clear" },
      ],
    );
  } finally {
    try {
      sdk?.close();
    } finally {
      try {
        if (child && child.exitCode === null && child.signalCode === null)
          child.kill();
        if (closed)
          await bounded(
            closed,
            `owned CLI must exit and drain output: ${stderr}`,
          );
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    }
  }
});
test("clear-auth forgets only confirmed removed credentials and later connection evidence restores eligibility", async (context) => {
  let now = 1_800_000_000_000;
  const clock = context.mock.method(Date, "now", () => now);
  try {
    for (const rejected of [false, true]) {
      now = 1_800_000_000_000;
      const id = rejected ? "clear-auth-rejected" : "clear-auth-confirmed";
      const otherId = `${id}-other-session`;
      const target = "docs-selected";
      const sibling = "search-retained";
      let statuses: McpServerStatus[] = [
        { name: target, status: "connected" },
        { name: sibling, status: "connected" },
      ];
      const clearCalls: string[] = [];
      const reconnects: string[] = [];
      let entered!: () => void;
      let release!: () => void;
      const started = new Promise<void>((resolve) => {
        entered = resolve;
      });
      const permission = new Promise<void>((resolve) => {
        release = resolve;
      });
      const native = {
        marker: "selected-query",
        async mcpServerStatus(this: { marker: string }) {
          assert.equal(this.marker, "selected-query");
          return statuses;
        },
        async mcpClearAuth(this: { marker: string }, serverName: string) {
          assert.equal(this.marker, "selected-query");
          clearCalls.push(serverName);
          entered();
          await permission;
          if (rejected) throw new Error("selected credential removal rejected");
        },
        async reconnectMcpServer(this: { marker: string }, serverName: string) {
          assert.equal(this.marker, "selected-query");
          reconnects.push(serverName);
          statuses = statuses.map((server) =>
            server.name === serverName
              ? { ...server, status: "connected" }
              : server,
          );
        },
      };
      const session = {
        sessionId: id,
        cwd: process.cwd(),
        connected: true,
        closing: false,
        query: native,
        knownConnectedMcpServers: new Set(),
        mcpStatusRevalidatedAt: new Map(),
        mcpAuthMonitors: new Map(),
      } as unknown as SessionState;
      const unrelated = {
        ...session,
        sessionId: otherId,
        knownConnectedMcpServers: new Set(),
        mcpStatusRevalidatedAt: new Map(),
        mcpAuthMonitors: new Map(),
        query: {
          mcpClearAuth: async () =>
            assert.fail("wrong session must not clear credentials"),
          mcpServerStatus: async () =>
            assert.fail("wrong session must not supply status"),
          reconnectMcpServer: async () =>
            assert.fail("wrong session must not reconnect"),
        },
      } as unknown as SessionState;
      const previous = new Map(
        [id, otherId].map((key) => [key, sessions.get(key)]),
      );
      sessions.set(id, session);
      sessions.set(otherId, unrelated);
      const scheduler = new BridgeCommandScheduler();
      const events: Array<BridgeEvent & { request_id?: string }> = [];
      const restore = replaceProtocolEventWriter((line) => {
        events.push(JSON.parse(line));
      });
      const dispatch = async (
        command: "mcp_status" | "mcp_clear_auth",
        requestId: string,
      ) => {
        const parsed = parseCommandEnvelope(
          JSON.stringify({
            command,
            session_id: id,
            request_id: requestId,
            ...(command === "mcp_clear_auth" ? { server_name: target } : {}),
          }),
        );
        const task = scheduleBridgeCommand(scheduler, parsed);
        assert.ok(task, "the parsed command must be admitted");
        await bounded(task, `registered ${command} command must complete`);
      };
      const snapshot = (requestId: string) => {
        const matching = events.filter(
          (event) =>
            event.event === "mcp_snapshot" && event.request_id === requestId,
        );
        assert.equal(
          matching.length,
          1,
          "one snapshot for the selected status request",
        );
        const event = matching[0];
        assert.ok(event.event === "mcp_snapshot");
        assert.equal(event.session_id, id);
        return event.servers;
      };
      try {
        await dispatch("mcp_status", "seed-connected");
        assert.equal(
          snapshot("seed-connected").find((server) => server.name === target)
            ?.status,
          "connected",
        );
        assert.equal(
          snapshot("seed-connected").find((server) => server.name === sibling)
            ?.status,
          "connected",
        );
        const offset = events.length;
        let settled = false;
        const clearing = dispatch("mcp_clear_auth", "selected-clear");
        void clearing.then(
          () => {
            settled = true;
          },
          () => {
            settled = true;
          },
        );
        await bounded(
          started,
          "registered clear must invoke the selected native Query",
        );
        await new Promise<void>((resolve) => setImmediate(resolve));
        assert.equal(
          settled,
          false,
          "clear must await confirmed native removal",
        );
        release();
        await clearing;
        assert.deepEqual(clearCalls, ["docs-selected"]);
        const responses = events.slice(offset);
        assert.equal(
          responses.some((event) => event.event === "slash_error"),
          false,
        );
        const errors = responses.filter(
          (event) => event.event === "mcp_operation_error",
        );
        assert.equal(errors.length, rejected ? 1 : 0);
        if (rejected) {
          const failure = errors[0];
          assert.ok(failure.event === "mcp_operation_error");
          assert.equal(failure.session_id, id);
          assert.equal(failure.request_id, "selected-clear");
          assert.equal(failure.error.server_name, "docs-selected");
          assert.equal(failure.error.operation, "clear-auth");
          assert.match(
            failure.error.message,
            /selected credential removal rejected/,
          );
        }
        statuses = [
          { name: target, status: "needs-auth" },
          { name: sibling, status: "needs-auth" },
        ];
        await dispatch("mcp_status", "after-clear");
        assert.deepEqual(
          [...reconnects].sort(),
          rejected ? ["docs-selected", "search-retained"] : ["search-retained"],
        );
        assert.equal(
          snapshot("after-clear").find((server) => server.name === target)
            ?.status,
          rejected ? "connected" : "needs-auth",
        );
        assert.equal(
          snapshot("after-clear").find((server) => server.name === sibling)
            ?.status,
          "connected",
        );
        if (!rejected) {
          // Actual independent connection evidence restores future eligibility.
          statuses = [
            { name: target, status: "connected" },
            { name: sibling, status: "connected" },
          ];
          await dispatch("mcp_status", "observed-connected-again");
          assert.equal(
            snapshot("observed-connected-again").find(
              (server) => server.name === target,
            )?.status,
            "connected",
          );
          now += 24 * 60 * 60 * 1_000;
          statuses = [
            { name: target, status: "needs-auth" },
            { name: sibling, status: "connected" },
          ];
          const before = reconnects.length;
          await dispatch("mcp_status", "later-revalidation");
          assert.deepEqual(reconnects.slice(before), ["docs-selected"]);
          assert.equal(
            snapshot("later-revalidation").find(
              (server) => server.name === target,
            )?.status,
            "connected",
          );
        }
        assert.equal(
          events.some(
            (event) => "session_id" in event && event.session_id === otherId,
          ),
          false,
        );
      } finally {
        release();
        try {
          await bounded(
            scheduler.whenIdle(),
            "owned command tasks must settle before fixture cleanup",
          );
        } finally {
          restore();
          for (const [key, original] of previous) {
            if (original) sessions.set(key, original);
            else sessions.delete(key);
          }
        }
      }
    }
  } finally {
    clock.mock.restore();
  }
});
