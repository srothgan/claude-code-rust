import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import test from "node:test";
import {
  authenticateMcpServer,
  clearMcpServerAuth,
  detectMcpAuthCapabilities,
  submitMcpOAuthCallbackUrl,
} from "./mcp_auth_adapter.js";

test("detectMcpAuthCapabilities reports each runtime method independently", () => {
  assert.deepEqual(
    detectMcpAuthCapabilities({
      mcpAuthenticate() {},
      mcpClearAuth: "not-a-function",
      mcpSubmitOAuthCallbackUrl() {},
    }),
    {
      authenticate: true,
      clear_auth: false,
      submit_oauth_callback_url: true,
    },
  );
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

test("MCP auth adapter rejects missing methods and incompatible responses", async () => {
  await assert.rejects(
    authenticateMcpServer({}, "docs"),
    /installed SDK does not support mcpAuthenticate/,
  );
  await assert.rejects(
    clearMcpServerAuth({}, "docs"),
    /installed SDK does not support mcpClearAuth/,
  );
  await assert.rejects(
    submitMcpOAuthCallbackUrl({}, "docs", "https://callback.example.test/code"),
    /installed SDK does not support mcpSubmitOAuthCallbackUrl/,
  );
  await assert.rejects(
    authenticateMcpServer(
      {
        async mcpAuthenticate() {
          return { authUrl: 42 };
        },
      },
      "docs",
    ),
    /invalid mcpAuthenticate authUrl/,
  );
  await assert.rejects(
    authenticateMcpServer(
      {
        async mcpAuthenticate() {
          return {
            authUrl: "https://auth.example.test/start",
            requiresUserAction: "yes",
          };
        },
      },
      "docs",
    ),
    /invalid mcpAuthenticate requiresUserAction/,
  );
});

function installedRuntimeMethodSource(
  source: string,
  methodName: string,
): string {
  const start = source.indexOf(`async ${methodName}(`);
  assert.notEqual(start, -1, `installed SDK runtime is missing ${methodName}`);
  const end = source.indexOf("}async ", start);
  assert.notEqual(
    end,
    -1,
    `could not isolate installed SDK runtime method ${methodName}`,
  );
  return source.slice(start, end + 1);
}

test("pinned SDK runtime retains the MCP auth control-request contract", async () => {
  const require = createRequire(import.meta.url);
  const sdkEntry = require.resolve("@anthropic-ai/claude-agent-sdk");
  const source = await readFile(sdkEntry, "utf8");

  const authenticate = installedRuntimeMethodSource(source, "mcpAuthenticate");
  assert.match(authenticate, /subtype:"mcp_authenticate"/);
  assert.match(authenticate, /serverName:/);
  assert.match(authenticate, /redirectUri:/);
  assert.match(authenticate, /\.response/);

  const clearAuth = installedRuntimeMethodSource(source, "mcpClearAuth");
  assert.match(clearAuth, /subtype:"mcp_clear_auth"/);
  assert.match(clearAuth, /serverName:/);
  assert.match(clearAuth, /\.response/);

  const submitCallback = installedRuntimeMethodSource(
    source,
    "mcpSubmitOAuthCallbackUrl",
  );
  assert.match(submitCallback, /subtype:"mcp_oauth_callback_url"/);
  assert.match(submitCallback, /serverName:/);
  assert.match(submitCallback, /callbackUrl:/);
  assert.match(submitCallback, /\.response/);
});
