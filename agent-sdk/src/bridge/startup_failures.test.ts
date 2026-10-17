import assert from "node:assert/strict";
import { homedir } from "node:os";
import test from "node:test";
import { redactStartupDetail } from "./startup_failures.js";

// Mirrors `redacts_credential_shapes` in src/cli/redaction.rs, the reference.
const CASES: Array<[string, string]> = [
  ["GET /v1?token=abc&x=1", "GET /v1?token=[redacted]&x=1"],
  ["CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat01-abcDEF123", "CLAUDE_CODE_OAUTH_TOKEN=[redacted]"],
  ["x-api-key: sk-ant-api03-secretvalue", "x-api-key: [redacted]"],
  ["access_token=abc123 next", "access_token=[redacted] next"],
  ["fetch https://user:pass@example.com/repo", "fetch https://[redacted]@example.com/repo"],
  ["Authorization: Basic dXNlcjpwYXNz", "Authorization: [redacted]"],
  ["Authorization: Bearer abc.def", "Authorization: [redacted]"],
  ["Authorization: token ghp_abc", "Authorization: [redacted]"],
  ["max_tokens=4096 input_tokens: 120", "max_tokens=4096 input_tokens: 120"],
  ["refresh-token=abc next", "refresh-token=[redacted] next"],
  ["token: abc max_tokens=1", "token: [redacted] max_tokens=1"],
  ["accessToken: abc next", "accessToken: [redacted] next"],
  ["key sk-live_abc123 end", "key [redacted] end"],
  ["task-1 and risk-free are not keys", "task-1 and risk-free are not keys"],
];

for (const [input, expected] of CASES) {
  test(`redacts credential shape: ${input}`, () => {
    assert.equal(redactStartupDetail(input, "/nonexistent-home"), expected);
  });
}

// Mirrors `replaces_the_home_directory_prefix_with_a_tilde` in src/cli/redaction.rs.
const HOME_CASES: Array<[string, string, string]> = [
  ["/Users/alice", "open '/Users/alice/x'", "open '~/x'"],
  ["/Users/alice/", "open '/Users/alice/x'", "open '~/x'"],
  ["/Users/alice", "ENOENT /Users/alice/.claude/settings.json: missing", "ENOENT ~/.claude/settings.json: missing"],
  ["/Users/alice", "in /Users/alice.", "in ~."],
  ["/Users/alice", "at file:///Users/alice/a.js:1", "at file://~/a.js:1"],
  ["/Users/alice", "a: /Users/alice and /Users/alice/b", "a: ~ and ~/b"],
  ["/Users/alice", "open '/Users/alice2/x'", "open '/Users/alice2/x'"],
  ["/Users/alice", "open '/Users/alice.bak/x'", "open '/Users/alice.bak/x'"],
  ["/Users/alice", "open '/srv/Users/alice/x'", "open '/srv/Users/alice/x'"],
  ["/Users/alice", "open '/Users/bob/x'", "open '/Users/bob/x'"],
  ["C:\\Users\\alice", "open 'C:\\Users\\alice\\x'", "open '~\\x'"],
  ["C:\\Users\\alice", "open 'C:/Users/alice/x'", "open '~/x'"],
  ["C:\\Users\\alice", "open 'c:\\users\\ALICE\\x'", "open '~\\x'"],
  ["C:\\Users\\alice", "open 'C:\\Users\\alice2\\x'", "open 'C:\\Users\\alice2\\x'"],
  ["C:\\Users\\alice", '{"p":"C:\\\\Users\\\\alice\\\\x"}', '{"p":"~\\\\x"}'],
];

for (const [home, input, expected] of HOME_CASES) {
  test(`replaces the home directory: ${home} | ${input}`, () => {
    assert.equal(redactStartupDetail(input, home), expected);
  });
}

test("the platform home directory is the default", () => {
  assert.equal(redactStartupDetail(`open '${homedir()}/x'`), "open '~/x'");
});
