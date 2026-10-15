import assert from "node:assert/strict";
import test from "node:test";
import { buildRewindConversationPlan, rewindTargetsFromSessionMessages } from "../bridge.js";
import { mapSessionMessagesToUpdates } from "./history.js";

const rename = "<command-name>/rename</command-name>\n<command-message>rename</command-message>\n<command-args>review-444</command-args>";
const color = "<command-name>/color</command-name>\n<command-message>color</command-message>\n<command-args>blue</command-args>";

function user(uuid: string, content: unknown, extra = {}) {
  return { type: "user" as const, uuid, session_id: "fixture", parent_tool_use_id: null, parent_agent_id: null, message: { role: "user", content }, ...extra };
}

test("resume shows complete command envelopes as commands while fork offers only conversation prompts", () => {
  const messages = [
    user("before", "First prompt"),
    user("caveat", "<local-command-caveat>Internal context only</local-command-caveat>", { isMeta: true }),
    user("rename", rename),
    user("color", [{ type: "text", text: color.slice(0, color.indexOf("<command-message>")) }, { type: "text", text: color.slice(color.indexOf("<command-message>")) }]),
    user("combined", `${rename}${color}`),
    user("no-args", "<command-name>/help</command-name><command-message>help</command-message><command-args></command-args>"),
    user("literal-args", "<command-name>/rename</command-name><command-message>rename</command-message><command-args>review <example>444</example> & more</command-args>"),
    user("after", "Continue with <example>literal XML</example>"),
  ];
  const original = structuredClone(messages);
  const updates = mapSessionMessagesToUpdates(messages);
  assert.deepEqual(updates.filter(update => update.type === "user_message_chunk").map(update => [update.source_message_uuid, (update.content.type === "text" ? update.content.text : update.content)]), [
    ["before", "First prompt"],
    ["rename", "/rename review-444"],
    ["color", "/color blue"],
    ["combined", "/rename review-444\n/color blue"],
    ["no-args", "/help"],
    ["literal-args", "/rename review <example>444</example> & more"],
    ["after", "Continue with <example>literal XML</example>"],
  ]);
  assert.deepEqual(rewindTargetsFromSessionMessages(messages).map(target => [target.uuid, target.input_text]), [
    ["after", "Continue with <example>literal XML</example>"], ["before", "First prompt"],
  ]);
  const fork = buildRewindConversationPlan(messages, "after");
  assert.ok(fork);
  assert.equal(fork.resumeSessionAtUuid, "literal-args");
  assert.deepEqual(fork.retainedMessages.map(message => message.uuid), ["before", "caveat", "rename", "color", "combined", "no-args", "literal-args"]);
  assert.deepEqual(fork.retainedMessages[2].message, { role: "user", content: rename });
  assert.deepEqual(fork.retainedMessages[3].message, { role: "user", content: messages[3].message.content });
  assert.equal(buildRewindConversationPlan(messages, "rename"), null);
  assert.deepEqual(messages, original);
});

test("command output becomes a notice with literal content and never becomes a fork point", () => {
  const messages = [
    user("stdout", "<local-command-stdout>Saved <example>XML</example> & text\nSecond line</local-command-stdout>"),
    user("stderr", [{ type: "text", text: "<local-command-stderr>Command failed</local-command-stderr>" }]),
    user("empty", "<local-command-stdout></local-command-stdout>"),
    user("after", "Next prompt"),
  ];
  const updates = mapSessionMessagesToUpdates(messages);
  assert.deepEqual(updates.filter(update => update.type === "system_notice_update"), [
    { type: "system_notice_update", severity: "info", message: "Saved <example>XML</example> & text\nSecond line" },
    { type: "system_notice_update", severity: "error", message: "Command failed" },
  ]);
  assert.deepEqual(rewindTargetsFromSessionMessages(messages).map(target => target.uuid), ["after"]);
});

test("XML examples, unknown tags and incomplete command records remain readable and eligible for forks", () => {
  const samples = [
    "<example>Keep this XML</example>",
    `Explain this record:\n${rename}`,
    `\`\`\`xml\n${rename}\n\`\`\``,
    `${rename}\nKeep this trailing explanation`,
    "<command-name>/rename</command-name>",
    "<command-name>/rename</command-name><command-message>rename</command-message><command-args>unfinished",
    "<command-name>not a command</command-name><command-message>rename</command-message><command-args>text</command-args>",
    "<local-command-stdout>unfinished output",
    "<local-command-caveat>A user-authored example without SDK metadata</local-command-caveat>",
    "<unknown-internal-tag>Do not guess</unknown-internal-tag>",
  ];
  const messages = samples.map((text, index) => user(`sample-${index}`, text));
  messages.push(user("human-command-example", rename, { origin: { kind: "human" } }));
  messages.push(user("peer-command-example", color, { origin: { kind: "peer" } }));
  const expected = [...samples, rename, color];
  const updates = mapSessionMessagesToUpdates(messages);
  assert.deepEqual(updates.filter(update => update.type === "user_message_chunk").map(update => (update.content.type === "text" ? update.content.text : update.content)), expected);
  assert.deepEqual(rewindTargetsFromSessionMessages(messages).map(target => target.input_text), [...expected].reverse());
  const assistant = { ...user("assistant", rename), type: "assistant" as const, message: { role: "assistant", content: rename } };
  assert.deepEqual(mapSessionMessagesToUpdates([assistant]).filter(update => update.type === "agent_message_chunk").map(update => (update.content.type === "text" ? update.content.text : update.content)), [rename]);
});

test("resume restores internal task outcomes while fork retains human, peer and scheduled messages", () => {
  const notification = "<task-notification><task-id>task-1</task-id><status>completed</status><summary>Finished</summary></task-notification>";
  const messages = [
    user("internal", notification, { origin: { kind: "task-notification", producer: "session-task" } }),
    user("malformed-internal", "<task-notification>unfinished", { origin: { kind: "task-notification" } }),
    user("summary", "Internal summary", { isCompactSummary: true }),
    user("human", notification, { origin: { kind: "human" } }),
    user("peer", "Peer request", { origin: { kind: "task-notification", subkind: "peer-send-message" } }),
    user("scheduled", "Scheduled request", { origin: { kind: "task-notification", subkind: "scheduled-trigger" } }),
  ];
  const updates = mapSessionMessagesToUpdates(messages);
  assert.deepEqual(updates.filter(update => update.type === "task_state_update").flatMap(update => update.tasks).map(task => [task.task_id, task.status, task.description]), [["task-1", "completed", "Finished"]]);
  assert.deepEqual(rewindTargetsFromSessionMessages(messages).map(target => target.uuid), ["scheduled", "peer", "human"]);
});

test("script prompts round-trip through resume and fork without losing XML, whitespace or quoted SDK tags", () => {
  const scripts = [
    "  # keep leading indentation\n  printf 'keep trailing whitespace'  \n\n",
    "#!/bin/sh\nprintf '%s\\n' 'plain script without XML'\n",
    "#!/bin/sh\ncat <<'XML'\n<config enabled=\"true\">\n  <path>src/main.rs</path>\n</config>\nXML\n",
    "$document = @'\r\n<root>\r\n  <value>&amp; &#65; <![CDATA[<literal>]]></value>\r\n</root>\r\n'@\r\nWrite-Output $document\r\n",
    "const view = `<main><h1>Hello</h1><input disabled /></main>`;\nconsole.log(view);\n",
    "fn main() {\n    let example = r#\"<task-notification><status>completed</status></task-notification>\"#;\n    println!(\"{example}\");\n}\n",
    `#!/bin/sh\ncat <<'EXAMPLE'\n${rename}\nEXAMPLE\n`,
    "```python\nprint('<command-name>/rename</command-name>')\nprint('<unknown>keep me</unknown>')\n```",
    "# incomplete XML is intentional test input\nprintf '%s' '<root><unclosed>'",
  ];
  for (const script of scripts) {
    for (const content of [
      script,
      [{ type: "text", text: script }],
      [{ type: "text", text: script.slice(0, 9) }, { type: "text", text: script.slice(9) }],
    ]) {
      const messages = [user("script", content)];
      const before = structuredClone(messages);
      const updates = mapSessionMessagesToUpdates(messages);
      const chunks = updates.filter(update => update.type === "user_message_chunk");
      assert.ok(chunks.every(update => update.content.type === "text"));
      assert.equal(chunks.map(update => update.content.type === "text" ? update.content.text : "").join(""), script);
      assert.deepEqual(rewindTargetsFromSessionMessages(messages).map(target => target.input_text), [script]);
      assert.deepEqual(messages, before);
    }
  }
});

test("recognizing command-shaped text never drops image attachments or tool outcomes", () => {
  const messages = [
    { ...user("tool", ""), type: "assistant" as const, message: { role: "assistant", content: [{ type: "tool_use", id: "read", name: "Read", input: { file_path: "fixture.xml" } }] } },
    user("result", [{ type: "text", text: rename }, { type: "tool_result", tool_use_id: "read", content: "<root>file content</root>" }]),
    user("attachment", [{ type: "text", text: color }, { type: "image", source: { type: "base64", media_type: "image/png", data: "fixture" } }]),
  ];
  const updates = mapSessionMessagesToUpdates(messages);
  assert.deepEqual(updates.filter(update => update.type === "user_message_chunk").map(update => update.content), [
    { type: "text", text: rename }, { type: "text", text: color }, { type: "text", text: "[image]" },
  ]);
  const results = updates.filter(update => update.type === "tool_call_update");
  assert.equal(results.length, 1);
  assert.equal(results[0].tool_call_update.tool_call_id, "read");
  assert.equal(results[0].tool_call_update.fields.status, "completed");
  assert.equal(results[0].tool_call_update.fields.raw_output, "<root>file content</root>");
});
