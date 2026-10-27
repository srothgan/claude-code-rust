import assert from "node:assert/strict";
import test from "node:test";
import { buildRewindConversationPlan, rewindTargetsFromSessionMessages } from "../bridge.js";
import { mapSessionMessagesToEntries, mapSessionMessagesToUpdates } from "./history.js";

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

test("only a reminder the SDK marks as metadata is a system reminder, and the chat leaves it out", () => {
  const assistant = (uuid: string, text: string) => ({ ...user(uuid, [{ type: "text", text }]), type: "assistant" as const, message: { role: "assistant", content: [{ type: "text", text }] } });
  const messages = [
    user("reminder", "<system-reminder>\nReport through SubagentHandback.\n</system-reminder>\n", { is_meta: true }),
    user("typed", "<system-reminder>typed by the user</system-reminder>"),
    user("quoted", "Quote <system-reminder>tags</system-reminder> as written."),
    user("meta-with-more", "<system-reminder>first</system-reminder> and more", { is_meta: true }),
    assistant("reply", "<system-reminder>an agent's own words</system-reminder>"),
  ];
  const text = (entry: ReturnType<typeof mapSessionMessagesToEntries>[number]) =>
    entry.type === "system_reminder" ? ["reminder", entry.text]
      : (entry.type === "user_message_chunk" || entry.type === "agent_message_chunk") && entry.content.type === "text" ? [entry.type, entry.content.text]
      : undefined;
  const literal = [
    ["user_message_chunk", "<system-reminder>typed by the user</system-reminder>"],
    ["user_message_chunk", "Quote <system-reminder>tags</system-reminder> as written."],
    ["user_message_chunk", "<system-reminder>first</system-reminder> and more"],
    ["agent_message_chunk", "<system-reminder>an agent's own words</system-reminder>"],
  ];
  assert.deepEqual(mapSessionMessagesToEntries(messages).map(text).filter(Boolean), [["reminder", "Report through SubagentHandback."], ...literal]);
  assert.deepEqual(mapSessionMessagesToUpdates(messages).map(text).filter(Boolean), literal);
  assert.ok(!rewindTargetsFromSessionMessages(messages).some(target => target.uuid === "reminder"), "a reminder is no fork point");
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

test("resume restores internal tasks while visible text, metadata and fork targets retain their own source identities", () => {
  const notification =
    "<task-notification><task-id>task-1</task-id><status>completed</status><summary>Finished</summary></task-notification>";
  const completed =
    "<task-notification>\n<task-id>task-1</task-id>\n<status>completed</status>\n<summary>Background command completed</summary>\n</task-notification>";
  const stopped =
    "<task-notification>\n<task-id>task-2</task-id>\n<status>stopped</status>\n<summary>Background shell command stopped</summary>\n</task-notification>";
  const timestamp = "2026-10-06T11:10:22.531Z";
  const assistant = (uuid: string, content: string) => ({
    ...user(uuid, content),
    type: "assistant" as const,
    message: { role: "assistant", content },
  });
  const messages = [
    user("before", "Before notification"),
    user("internal", notification, {
      origin: { kind: "task-notification", producer: "session-task" },
    }),
    user("stopped-internal", [{ type: "text", text: stopped }], {
      origin: { kind: "task-notification" },
    }),
    user("malformed-internal", "<task-notification>unfinished", {
      origin: { kind: "task-notification" },
    }),
    user("summary", "Internal summary", { isCompactSummary: true }),
    user("block-summary", [{ type: "text", text: "Internal block summary" }], {
      isCompactSummary: true,
    }),
    user("after", "hello?", { origin: { kind: "human" } }),
    user("human", notification, { origin: { kind: "human" } }),
    user("user-xml", completed, { origin: { kind: "human" } }),
    assistant("assistant-xml", "<example>XML output</example>"),
    user("peer", "Peer message", {
      origin: { kind: "task-notification", subkind: "peer-send-message" },
    }),
    user("scheduled", "Scheduled message", {
      origin: { kind: "task-notification", subkind: "scheduled-trigger" },
    }),
    user("user", "User prompt"),
    assistant("assistant", "Assistant response"),
    user("xml-example", "<example>Keep this XML</example>"),
  ].map((message) => ({ ...message, timestamp }));
  const original = structuredClone(messages);
  const updates = mapSessionMessagesToUpdates(messages);
  assert.deepEqual(
    updates
      .filter(
        (update) =>
          update.type === "user_message_chunk" ||
          update.type === "agent_message_chunk",
      )
      .map((update) => [
        update.type,
        update.source_message_uuid,
        update.content,
      ]),
    [
      [
        "user_message_chunk",
        "before",
        { type: "text", text: "Before notification" },
      ],
      ["user_message_chunk", "after", { type: "text", text: "hello?" }],
      ["user_message_chunk", "human", { type: "text", text: notification }],
      ["user_message_chunk", "user-xml", { type: "text", text: completed }],
      [
        "agent_message_chunk",
        "assistant-xml",
        { type: "text", text: "<example>XML output</example>" },
      ],
      ["user_message_chunk", "peer", { type: "text", text: "Peer message" }],
      [
        "user_message_chunk",
        "scheduled",
        { type: "text", text: "Scheduled message" },
      ],
      ["user_message_chunk", "user", { type: "text", text: "User prompt" }],
      [
        "agent_message_chunk",
        "assistant",
        { type: "text", text: "Assistant response" },
      ],
      [
        "user_message_chunk",
        "xml-example",
        { type: "text", text: "<example>Keep this XML</example>" },
      ],
    ],
  );
  // Hidden records have valid timestamps too: absence must follow provenance,
  // rather than missing fixture metadata. Positive rows must still publish it.
  assert.deepEqual(
    updates
      .filter((update) => update.type === "message_metadata")
      .map((update) => [
        update.source_message_uuid,
        update.role,
        update.timestamp,
      ]),
    [
      ["before", "user", timestamp],
      ["after", "user", timestamp],
      ["human", "user", timestamp],
      ["user-xml", "user", timestamp],
      ["assistant-xml", "assistant", timestamp],
      ["peer", "user", timestamp],
      ["scheduled", "user", timestamp],
      ["user", "user", timestamp],
      ["assistant", "assistant", timestamp],
      ["xml-example", "user", timestamp],
    ],
  );
  const tasks = new Map(
    updates
      .filter((update) => update.type === "task_state_update")
      .flatMap((update) => update.tasks)
      .map((task) => [task.task_id, task]),
  );
  assert.deepEqual(
    [...tasks.keys()].sort(),
    ["task-1", "task-2"],
    "only genuine internal outcomes restore tasks",
  );
  const task = tasks.get("task-1");
  assert.deepEqual(
    [task?.task_id, task?.status, task?.description],
    ["task-1", "completed", "Finished"],
  );
  assert.deepEqual(
    rewindTargetsFromSessionMessages(messages).map((target) => [
      target.uuid,
      target.input_text,
    ]),
    [
      ["xml-example", "<example>Keep this XML</example>"],
      ["user", "User prompt"],
      ["scheduled", "Scheduled message"],
      ["peer", "Peer message"],
      ["user-xml", completed],
      ["human", notification],
      ["after", "hello?"],
      ["before", "Before notification"],
    ],
  );
  assert.deepEqual(messages, original);
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
