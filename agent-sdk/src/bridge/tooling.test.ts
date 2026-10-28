import test from "node:test";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import assert from "node:assert/strict";
import {
  buildToolResultFields,
  createToolCall,
} from "../bridge.js";

function assertTextProjection(
  fields: ReturnType<typeof buildToolResultFields>,
  expected?: string,
  status = "completed",
): string {
  assert.equal(fields.status, status);
  const text = String(fields.raw_output ?? "");
  assert.ok(text.length > 0);
  if (expected !== undefined) assert.equal(text, expected);
  assert.deepEqual(fields.content, [{ type: "content", content: { type: "text", text } }]);
  return text;
}

function projectInUtc(
  name: string,
  input: Record<string, unknown>,
  result: unknown,
): ReturnType<typeof buildToolResultFields> {
  const child = spawnSync(
    process.execPath,
    [
      "--input-type=module",
      "--eval",
      `import {readFileSync} from "node:fs";import {buildToolResultFields,createToolCall} from ${JSON.stringify(new URL("../bridge.js", import.meta.url).href)};const [name,input,result]=JSON.parse(readFileSync(0,"utf8"));process.stdout.write(JSON.stringify(buildToolResultFields(false,result,createToolCall("utc",name,input))));`,
    ],
    {
      input: JSON.stringify([name, input, result]),
      encoding: "utf8",
      env: { ...process.env, TZ: "UTC" },
      timeout: 10_000,
      windowsHide: true,
    },
  );
  assert.equal(child.error, undefined);
  assert.equal(child.status, 0, child.stderr);
  return JSON.parse(child.stdout);
}

function sectionValues(
  fields: ReturnType<typeof buildToolResultFields>,
  id: string,
): Record<string, string> {
  const sections = fields.output_metadata?.presentation?.sections ?? [];
  const found = sections.filter((section) =>
    section.fields.some((field) => field.label === "Schedule ID" && field.value.value === id),
  );
  assert.equal(found.length, 1, `one section for ${id}`);
  return Object.fromEntries(
    (found[0]?.fields ?? []).map((field) => [field.label, field.value.value]),
  );
}

test("createToolCall builds edit diff content", () => {
  const toolCall = createToolCall("tc-1", "Edit", {
    file_path: "src/main.rs",
    old_string: "old",
    new_string: "new",
  });
  assert.equal(toolCall.kind, "edit");
  assert.equal(toolCall.content.length, 1);
  assert.deepEqual(toolCall.content[0], {
    type: "diff",
    old_path: "src/main.rs",
    new_path: "src/main.rs",
    old: "old",
    new: "new",
  });
  assert.deepEqual(toolCall.meta, {
    claudeCode: { toolName: "Edit", parentToolUseId: null },
  });
});

test("createToolCall builds write preview diff content", () => {
  const toolCall = createToolCall("tc-w", "Write", {
    file_path: "src/new-file.ts",
    content: "export const x = 1;\n",
  });
  assert.equal(toolCall.kind, "edit");
  assert.deepEqual(toolCall.content, [
    {
      type: "diff",
      old_path: "src/new-file.ts",
      new_path: "src/new-file.ts",
      old: "",
      new: "export const x = 1;\n",
    },
  ]);
});

test("search and fetch titles associate each supplied context value with its own operation", () => {
  const input = {
      pattern: "UNIQUE_PATTERN",
      path: "source-target",
      glob: "**/*.rs",
      type: "rust",
      output_mode: "content",
      "-i": true,
      "-C": 3,
      head_limit: 17,
      offset: 9,
      multiline: true,
    },
    before = structuredClone(input),
    grep = createToolCall("grep", "Grep", input);
  for (const value of [
    "UNIQUE_PATTERN",
    "source-target",
    "**/*.rs",
    "type rust",
    "mode content",
    "case-insensitive",
    "context 3",
    "limit 17",
    "offset 9",
    "multiline",
  ])
    assert.ok(grep.title.includes(value === "mode content" ? "content" : value), value);
  assert.deepEqual(grep.raw_input, before);
  assert.deepEqual(input, before);
  const glob = createToolCall("glob", "Glob", { pattern: "**/*.md", path: "notes-target" });
  assert.ok(glob.title.includes("**/*.md"));
  assert.ok(glob.title.includes("notes-target"));
  const bare = createToolCall("bare", "Grep", { pattern: "bare-pattern" });
  assert.ok(bare.title.includes("bare-pattern"));
  for (const invented of ["glob ", "type ", "limit ", "offset ", "context "])
    assert.ok(!bare.title.includes(invented));
  const scoped = createToolCall("scoped", "Glob", { path: "scope-only" });
  assert.ok(scoped.title.includes("scope-only"));
  const url = "https://example.test/exact/path?q=one%20two#fragment",
    fetch = createToolCall("fetch", "WebFetch", { url, prompt: "not-the-url" });
  assert.ok(fetch.title.includes(url));
  assert.ok(!fetch.title.includes("not-the-url"));
  assert.deepEqual(fetch.raw_input, { url, prompt: "not-the-url" });
});

test("compact tool titles choose named inputs without promoting distractor fields to identity", () => {
  const rows: Array<{
    tool: string;
    input: Record<string, unknown>;
    expected?: string;
    absent: string[];
  }> = [
    {
      tool: "EnterWorktree",
      input: { name: "chosen-tree", path: "wrong-path" },
      expected: "chosen-tree",
      absent: ["wrong-path"],
    },
    { tool: "EnterWorktree", input: { path: "not-a-tree-name" }, absent: ["not-a-tree-name"] },
    {
      tool: "RemoteTrigger",
      input: { action: " run ", trigger_id: "wrong-trigger" },
      expected: "run",
      absent: ["wrong-trigger"],
    },
    { tool: "RemoteTrigger", input: { trigger_id: "not-an-action" }, absent: ["not-an-action"] },
    {
      tool: "REPL",
      input: { code: "  await selectedCode()  ", description: "wrong-description" },
      expected: "await selectedCode()",
      absent: ["wrong-description"],
    },
    {
      tool: "REPL",
      input: { description: "not-executable-code" },
      absent: ["not-executable-code"],
    },
    {
      tool: "Monitor",
      input: { description: "watch selected logs", command: "wrong-command", timeout_ms: 30000 },
      expected: "watch selected logs",
      absent: ["wrong-command"],
    },
    {
      tool: "Workflow",
      input: { name: "chosen-workflow", script: "wrong-script-name" },
      expected: "chosen-workflow",
      absent: ["wrong-script-name"],
    },
    { tool: "Workflow", input: { script: "not-a-declared-name" }, absent: ["not-a-declared-name"] },
  ];
  for (const [index, row] of rows.entries()) {
    const before = structuredClone(row.input),
      tool = createToolCall(`title-${index}`, row.tool, row.input);
    if (row.expected) assert.ok(tool.title.includes(row.expected), row.tool);
    else assert.ok(tool.title.includes(row.tool));
    for (const value of row.absent) assert.ok(!tool.title.includes(value), row.tool);
    assert.deepEqual(tool.raw_input, before);
    assert.deepEqual(row.input, before);
  }
});

test("skill display normalization preserves distinct lexical rules and the original execution identifier", () => {
  const rows = [
    ["claude-api", "Claude API"],
    ["keybindings_help", "Keybindings Help"],
    [" frontend-design:frontend-design ", "Frontend Design"],
    ["github:gh-fix-ci", "GitHub / GH Fix CI"],
    ["mobile:iOS-debug", "Mobile / iOS Debug"],
    ["future::widget", "future::widget"],
    ["future.widget", "future.widget"],
  ];
  for (const [skill, display] of rows) {
    const tool = createToolCall(`skill-${skill}`, "Skill", { skill });
    assert.ok(tool.title.endsWith(display));
    assert.deepEqual(tool.raw_input, { skill });
  }
  const missing = createToolCall("missing", "Skill", {});
  assert.ok(missing.title.includes("Skill"));
  assert.deepEqual(missing.raw_input, {});
});

test("project and artifact titles identify the selected target while retaining complete original input", () => {
  const rows = [
    {
      tool: "Projects",
      input: { method: "project_read", path: "claude/actual-notes.md", query: "wrong-query" },
      expected: ["read", "claude/actual-notes.md"],
      absent: ["wrong-query"],
    },
    {
      tool: "Projects",
      input: { method: "project_search", query: "chosen-query", description: "wrong-description" },
      expected: ["search", "chosen-query"],
      absent: ["wrong-description"],
    },
    {
      tool: "Artifact",
      input: {
        label: "selected-report",
        file_path: "C:/work/source.html",
        description: "Detailed source description",
        favicon: "R",
      },
      expected: ["selected-report"],
      absent: ["C:/work/source.html", "Detailed source description"],
    },
    {
      tool: "Artifact",
      input: { file_path: "C:/work/fallback.html", description: "not-a-label" },
      expected: ["C:/work/fallback.html"],
      absent: ["not-a-label"],
    },
  ];
  for (const [index, row] of rows.entries()) {
    const before = structuredClone(row.input),
      tool = createToolCall(`target-${index}`, row.tool, row.input);
    for (const value of row.expected) assert.ok(tool.title.includes(value));
    for (const value of row.absent) assert.ok(!tool.title.includes(value));
    assert.deepEqual(tool.raw_input, before);
    assert.deepEqual(row.input, before);
  }
});

test("buildToolResultFields extracts plain-text output", () => {
  const fields = buildToolResultFields(false, [
    { text: "line 1" },
    { text: "line 2" },
  ]);
  assert.equal(fields.status, "completed");
  assert.equal(fields.raw_output, "line 1\nline 2");
  assert.deepEqual(fields.content, [
    { type: "content", content: { type: "text", text: "line 1\nline 2" } },
  ]);
});

test("generic object and array results preserve nested values without mutating authored inputs", () => {
  const rows = [
    {
      name: "SendFeedback",
      input: { type: "bug", title: "Feedback draft", details: "Reproduction details" },
      output: {
        success: false,
        count: 0,
        message: "",
        nested: { list: [null, false, { token: "kept" }] },
      },
    },
    {
      name: "RefreshMcpTools",
      input: { server: "docs", options: { force: false } },
      output: [
        { server: "docs", toolCount: 0, added: ["lookup"], removed: [] },
        { server: "offline", status: "not_connected", error: "No live connection" },
      ],
    },
  ];
  for (const [index, row] of rows.entries()) {
    const inputBefore = structuredClone(row.input),
      expected = structuredClone(row.output),
      tool = createToolCall(`generic-${index}`, row.name, row.input),
      fields = buildToolResultFields(false, row.output, tool, row.output),
      text = assertTextProjection(fields);
    assert.deepEqual(JSON.parse(text), expected);
    assert.deepEqual(tool.raw_input, inputBefore);
    assert.deepEqual(row.input, inputBefore);
    assert.deepEqual(row.output, expected);
  }
});

test("Grep projects independent content and correctly labeled authoritative totals without invented counts", () => {
  const base = createToolCall("grep", "Grep", { pattern: "needle", output_mode: "content" });
  const rows = [
    {
      result: {
        mode: "content",
        content: "a.rs:7:needle\nb.rs:9:needle",
        filenames: ["a.rs", "b.rs"],
        numFiles: 2,
        totalFiles: 7,
        numMatches: 11,
        numLines: 2,
        totalLines: 19,
        appliedLimit: 37,
        appliedOffset: 5,
      },
      body: "a.rs:7:needle\nb.rs:9:needle",
      labels: [/7 files/, /11 matches/, /19 lines/, /mode content/, /limit 37/, /offset 5/],
      absent: [/2 files/, /2 lines/],
    },
    {
      result: {
        mode: "content",
        content: "c.rs:4:needle",
        numFiles: 3,
        totalFiles: -1,
        numLines: 1,
        totalLines: "invalid",
      },
      body: "c.rs:4:needle",
      labels: [/3 files/, /1 line/],
      absent: [/invalid/, /-1 files/],
    },
    {
      result: {
        mode: "content",
        content: "visible.rs:1:needle",
        numFiles: 0,
        filenames: [],
        numLines: 17,
      },
      body: "visible.rs:1:needle",
      labels: [/17 lines/],
      absent: [/0 files/],
    },
    {
      result: { mode: "content", content: "", numFiles: 0, filenames: [], numLines: 0 },
      body: "No matches found",
      labels: [/0 files/, /0 lines/],
      absent: [],
    },
    {
      result: { mode: "count", content: "counted.rs:13", numMatches: 13, numFiles: 4 },
      body: "counted.rs:13",
      labels: [/13 matches/, /4 files/, /mode count/],
      absent: [],
    },
    {
      result: { mode: "files_with_matches", filenames: ["x/one.rs", "y/two.rs"], numFiles: 9 },
      body: "x/one.rs\ny/two.rs",
      labels: [/9 files/, /mode files\b/],
      absent: [],
    },
  ];
  for (const row of rows) {
    const fields = buildToolResultFields(false, "DISTINCT-RAW-FALLBACK", base, row.result),
      text = assertTextProjection(fields);
    assert.ok(text.includes(row.body));
    assert.ok(!text.includes("DISTINCT-RAW-FALLBACK"));
    for (const label of row.labels) assert.match(text, label);
    for (const absent of row.absent) assert.doesNotMatch(text, absent);
  }
});

test("Glob associates the supplied filenames with reported totals, duration and upstream truncation", () => {
  const base = createToolCall("glob", "Glob", { pattern: "**/*.rs", path: "src" });
  const rows = [
    {
      result: {
        durationMs: 23,
        numFiles: 9,
        filenames: ["src/first.rs", "src/second.rs"],
        truncated: false,
      },
      names: ["src/first.rs", "src/second.rs"],
      total: /9 files/,
      duration: /23ms/,
      truncated: false,
    },
    {
      result: { durationMs: 0, numFiles: 0, filenames: [], truncated: false },
      names: [],
      total: /No files found/,
      duration: /0ms/,
      truncated: false,
    },
    {
      result: { durationMs: 31, numFiles: 50, filenames: ["partial/only.rs"], truncated: true },
      names: ["partial/only.rs"],
      total: /50 files/,
      duration: /31ms/,
      truncated: true,
    },
  ];
  for (const row of rows) {
    const text = assertTextProjection(
      buildToolResultFields(false, "wrong fallback", base, row.result),
    );
    assert.match(text, row.total);
    assert.match(text, row.duration);
    for (const name of row.names) assert.ok(text.includes(name));
    assert.equal(/truncated/i.test(text), row.truncated);
    assert.ok(!text.includes("wrong fallback"));
  }
  const malformed = assertTextProjection(
    buildToolResultFields(false, "", base, {
      durationMs: "bad",
      numFiles: -1,
      filenames: ["kept.rs"],
      truncated: false,
    }),
  );
  assert.ok(malformed.includes("kept.rs"));
  assert.ok(!malformed.includes("bad"));
  assert.doesNotMatch(malformed, /-1 files/);
});

test("standalone SDK persisted-output wrappers reduce to their supplied location while ordinary markup remains literal", () => {
  const location =
    "Output too large (14KB). Full output saved to: C:\\tmp\\tool-results\\literal.txt";
  const wrappers = [
    `<persisted-output>\n\u2502 ${location}\n\u2502 Preview (first 2KB):\n\u2502 PREVIEW-SENTINEL\n\u2502 </persisted-output>`,
    `  <persisted-output>\r\n | ${location}\r\n | Preview (first 2KB):\r\n | PREVIEW-SENTINEL\r\n </persisted-output>  `,
  ];
  for (const wrapper of wrappers) {
    const text = assertTextProjection(buildToolResultFields(false, wrapper), location);
    assert.ok(!text.includes("PREVIEW-SENTINEL"));
  }
  for (const ordinary of [
    `Leading explanation\n<persisted-output>\n${location}\nExample body\n</persisted-output>\nTrailing literal line`,
    `<persisted-output>\n${location}\nmissing closing tag`,
    "<persisted-output>ordinary example without a saved-output receipt</persisted-output>",
  ]) {
    const tool = createToolCall("literal-markup", "mcp__docs__example", {});
    assertTextProjection(buildToolResultFields(false, ordinary, tool), ordinary);
  }
});

test("native cancellation and denial are compact only on failure while ordinary diagnostics remain exact", () => {
  const cancelled =
      "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.",
    denied =
      "Permission for this tool use was denied. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). Try a different approach or report the limitation to complete your task.",
    cancelledFollowUp =
      "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). To tell you how to proceed, the user said:\nSkip this",
    deniedFollowUp =
      "Permission for this tool use was denied. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). The user said:\nNot now";
  for (const [input, expected] of [
    [cancelled, "Cancelled by user."],
    [cancelledFollowUp, "Cancelled by user."],
    [denied, "Permission denied."],
    [deniedFollowUp, "Permission denied."],
  ]) {
    assertTextProjection(buildToolResultFields(true, input), expected, "failed");
    assertTextProjection(buildToolResultFields(false, input), input);
  }
  const unrelated =
    "Compiler diagnostic: doesn't want to proceed with this tool use; preserve the surrounding cause.";
  assertTextProjection(buildToolResultFields(true, unrelated), unrelated, "failed");
});

test("buildToolResultFields maps structured Write output to diff content", () => {
  const base = createToolCall("tc-w", "Write", {
    file_path: "src/main.ts",
    content: "new",
  });
  const fields = buildToolResultFields(
    false,
    {
      type: "update",
      filePath: "src/main.ts",
      content: "new",
      originalFile: "old",
      structuredPatch: [],
      gitDiff: {
        repository: "acme/project",
      },
    },
    base,
  );
  assert.equal(fields.status, "completed");
  assert.deepEqual(fields.content, [
    {
      type: "diff",
      old_path: "src/main.ts",
      new_path: "src/main.ts",
      old: "old",
      new: "new",
      repository: "acme/project",
    },
  ]);
  for (const staged of [undefined, true]) {
    const result = {
      type: "update",
      filePath: "confirmed.ts",
      content: "CONFIRMED\r\n",
      originalFile: "ORIGINAL\n",
      structuredPatch: [],
      gitDiff: { repository: "other/repo" },
      ...(staged === undefined ? {} : { staged }),
    };
    for (const representation of [result, JSON.stringify(result)]) {
      const actual = buildToolResultFields(false, "receipt", base, representation);
      assert.equal(actual.status, "completed");
      assert.deepEqual(actual.content, [
        {
          type: "diff",
          old_path: "confirmed.ts",
          new_path: "confirmed.ts",
          old: "ORIGINAL\n",
          new: "CONFIRMED\r\n",
          repository: "other/repo",
        },
      ]);
      assert.equal(actual.output_metadata?.staged, staged);
    }
  }
});

test("Write outcomes distinguish confirmed creation, unchanged bytes and unavailable originals", () => {
  const base = createToolCall("write-outcomes", "Write", {
    file_path: "requested.ts",
    content: "REQUESTED",
  });
  const created = buildToolResultFields(false, "confirmation", base, {
    type: "create",
    filePath: "actual/new.ts",
    content: "ACTUAL\r\n",
    originalFile: null,
    structuredPatch: [],
  });
  assert.equal(created.status, "completed");
  assert.deepEqual(created.content, [
    {
      type: "diff",
      old_path: "actual/new.ts",
      new_path: "actual/new.ts",
      old: "",
      new: "ACTUAL\r\n",
    },
  ]);
  const unchanged = buildToolResultFields(false, "confirmation", base, {
    type: "update",
    filePath: "actual/same.ts",
    content: "UNCHANGED\n",
    originalFile: "UNCHANGED\n",
    structuredPatch: [],
  });
  const unchangedText = assertTextProjection(unchanged);
  assert.match(unchangedText, /No changes/);
  assert.ok(unchangedText.includes("actual/same.ts"));
  for (const variant of [{ type: "update" }, { type: "update", gitDiff: { patch: "" } }, {}]) {
    const fields = buildToolResultFields(false, "confirmation", base, {
      ...variant,
      filePath: "actual/unavailable.ts",
      content: "CONFIRMED",
      originalFile: null,
      structuredPatch: [],
    });
    const text = assertTextProjection(fields);
    assert.ok(text.includes("actual/unavailable.ts"));
    assert.match(text, /previous content.*unavailable/);
    assert.doesNotMatch(text, /Created|REQUESTED/);
  }
  const failed = buildToolResultFields(true, "WRITE-DENIED", base, {
    type: "create",
    filePath: "actual/new.ts",
    content: "STALE",
    originalFile: null,
  });
  assertTextProjection(failed, "WRITE-DENIED", "failed");
});

test("buildToolResultFields maps structured Edit output to whole-file diff content", () => {
  const original = "one\ntwo\nthree\nfour\nfive\n";
  const input = { file_path: "src/main.ts", old_string: "four", new_string: "4a\n4b" };
  const editResult = (overrides: Record<string, unknown>) =>
    buildToolResultFields(
      false,
      [{ text: "Updated successfully" }],
      createToolCall("tc-e-full", "Edit", input),
      {
        result: {
          filePath: "src/main.ts",
          oldString: "four",
          newString: "4a\n4b",
          originalFile: original,
          structuredPatch: [
            {
              oldStart: 1,
              oldLines: 5,
              newStart: 1,
              newLines: 6,
              lines: [" one", " two", " three", "-four", "+4a", "+4b", " five"],
            },
          ],
          userModified: false,
          replaceAll: false,
          ...overrides,
        },
      },
    );
  const diff = (old: string, next: string) => [
    { type: "diff", old_path: "src/main.ts", new_path: "src/main.ts", old, new: next },
  ];

  assert.deepEqual(
    editResult({}).content,
    diff(original, "one\ntwo\nthree\n4a\n4b\nfive\n"),
  );

  // Deleting a whole line takes its line break with it.
  assert.deepEqual(
    editResult({
      newString: "",
      structuredPatch: [
        { oldStart: 1, oldLines: 5, newStart: 1, newLines: 4, lines: [] },
      ],
    }).content,
    diff(original, "one\ntwo\nthree\nfive\n"),
  );

  assert.deepEqual(
    editResult({
      oldString: "e",
      newString: "E",
      replaceAll: true,
      structuredPatch: [
        { oldStart: 1, oldLines: 5, newStart: 1, newLines: 5, lines: [] },
      ],
    }).content,
    diff(original, "onE\ntwo\nthrEE\nfour\nfivE\n"),
  );

  // Replacement patterns in the new text are inserted literally.
  for (const replaceAll of [false, true]) {
    assert.deepEqual(
      editResult({
        newString: "$& $1 $$",
        replaceAll,
        structuredPatch: [
          { oldStart: 1, oldLines: 5, newStart: 1, newLines: 5, lines: [] },
        ],
      }).content,
      diff(original, "one\ntwo\nthree\n$& $1 $$\nfive\n"),
    );
  }

  // Carriage returns are carried through untouched.
  assert.deepEqual(
    editResult({
      originalFile: "one\r\nfour\r\nfive\r\n",
      structuredPatch: [
        { oldStart: 1, oldLines: 3, newStart: 1, newLines: 4, lines: [] },
      ],
    }).content,
    diff("one\r\nfour\r\nfive\r\n", "one\r\n4a\n4b\r\nfive\r\n"),
  );

  // An empty old string creates the file.
  assert.deepEqual(
    editResult({
      oldString: "",
      originalFile: "",
      structuredPatch: [
        { oldStart: 0, oldLines: 0, newStart: 1, newLines: 2, lines: ["+4a", "+4b"] },
      ],
    }).content,
    diff("", "4a\n4b"),
  );

  // Results that cannot be reproduced keep the fragment diff from the input.
  for (const overrides of [
    { originalFile: null },
    { originalFile: undefined },
    { oldString: "missing" },
    { oldString: "", originalFile: original },
    { oldString: "three\nfour", originalFile: "three\r\nfour\r\n" },
    { filePath: "src/other.ts" },
    { structuredPatch: undefined },
    { structuredPatch: "@@ -1,5 +1,6 @@" },
    { structuredPatch: [null] },
    { structuredPatch: [{ oldStart: 1, oldLines: "5", newStart: 1, newLines: 6 }] },
    {
      structuredPatch: [
        { oldStart: 1, oldLines: 5, newStart: 1, newLines: 9, lines: [] },
      ],
    },
  ]) {
    assert.deepEqual(
      editResult(overrides).content,
      diff("four", "4a\n4b"),
      JSON.stringify(overrides),
    );
  }

  // A resumed transcript carries only the tool_result block, not the structured output.
  const base = createToolCall("tc-e-resumed", "Edit", input);
  const block = {
    type: "tool_result",
    tool_use_id: "tc-e-resumed",
    content: "The file src/main.ts has been updated successfully.",
  };
  assert.deepEqual(
    buildToolResultFields(false, block.content, base, block).content,
    diff("four", "4a\n4b"),
  );
  const fragment = createToolCall("fragment", "Edit", {
    file_path: "fragment.ts",
    old_string: "OLD-FRAGMENT",
    new_string: "NEW-FRAGMENT",
  });
  const projected = buildToolResultFields(false, "Edit receipt", fragment, {
    result: { filePath: "fragment.ts", gitDiff: { repository: "fragment/repo" }, staged: true },
  });
  assert.equal(projected.status, "completed");
  assert.equal(projected.output_metadata?.staged, true);
  assert.deepEqual(projected.content, [
    {
      type: "diff",
      old_path: "fragment.ts",
      new_path: "fragment.ts",
      old: "OLD-FRAGMENT",
      new: "NEW-FRAGMENT",
      repository: "fragment/repo",
    },
  ]);
});

test("Bash and PowerShell preserve actual diagnostic streams while excluding model-only hints", () => {
  for (const name of ["Bash", "PowerShell"]) {
    for (const failed of [false, true]) {
      const fields = buildToolResultFields(
        failed,
        "MODEL-FACING-FALLBACK",
        createToolCall(name, name, { command: "probe" }),
        {
          stdout: `${name}-OUT\nsecond`,
          stderr: `${name}-ERR`,
          interrupted: true,
          staleReadFileStateHint: "PRIVATE-STALE-HINT",
        },
      );
      const text = assertTextProjection(fields, undefined, failed ? "failed" : "completed");
      assert.ok(text.includes(`${name}-OUT\nsecond`));
      assert.ok(text.includes(`${name}-ERR`));
      assert.match(text, /abort|interrupt/i);
      assert.doesNotMatch(
        JSON.stringify(fields),
        /MODEL-FACING-FALLBACK|PRIVATE-STALE-HINT|staleReadFileStateHint/,
      );
    }
  }
});

test("Bash background receipts retain output, task identity and independently observed reason and timing", () => {
  const rows = [
    {
      input: {
        timedOutAfterMs: 10_000.9,
        backgroundCwdHint: " actual cwd unchanged ",
        backgroundEndsWithFinalResponse: true,
        assistantAutoBackgrounded: true,
      },
      metadata: {
        timed_out_after_ms: 10_000,
        background_cwd_hint: "actual cwd unchanged",
        background_ends_with_final_response: true,
        assistant_auto_backgrounded: true,
      },
      reason: /10,000/,
    },
    { input: { timedOutAfterMs: 0 }, metadata: { timed_out_after_ms: 0 }, reason: /0 ms/ },
    {
      input: { assistantAutoBackgrounded: true },
      metadata: { assistant_auto_backgrounded: true },
      reason: /assistant/i,
    },
    { input: { backgroundedByUser: true }, metadata: undefined, reason: /user/i },
    { input: {}, metadata: undefined, reason: /background/i },
    {
      input: { timedOutAfterMs: -1, backgroundCwdHint: " " },
      metadata: undefined,
      reason: /background/i,
    },
    {
      input: { timedOutAfterMs: Number.POSITIVE_INFINITY },
      metadata: undefined,
      reason: /background/i,
    },
  ];
  for (const [index, row] of rows.entries()) {
    const id = `actual-task-${index}`,
      out = `launch-output-${index}`;
    const fields = buildToolResultFields(
      false,
      "MODEL-FALLBACK",
      createToolCall(id, "Bash", { command: "watch" }),
      { stdout: out, stderr: "", interrupted: false, backgroundTaskId: id, ...row.input },
    );
    const text = assertTextProjection(fields, undefined, "detached");
    assert.ok(text.includes(id));
    assert.ok(text.includes(out));
    assert.match(text, row.reason);
    assert.doesNotMatch(text, /MODEL-FALLBACK/);
    assert.deepEqual(fields.output_metadata?.bash, row.metadata);
    if (index >= 5) assert.doesNotMatch(text, /-1|Infinity|after .*ms/);
  }
});

test("MCP resources preserve each text or saved blob with its own URI, MIME type and payload", () => {
  const result = {
    contents: [
      null,
      { uri: "file://guide.md", mimeType: "text/markdown", text: "GUIDE-BODY" },
      { uri: "file://figure.png", mimeType: "image/png", blobSavedTo: "C:/owned/figure.png" },
      { text: "INVALID-NO-URI" },
    ],
  };
  for (const structured of [result, JSON.stringify(result)]) {
    const fields = buildToolResultFields(
      false,
      "MODEL-FALLBACK",
      createToolCall("resources", "ReadMcpResource", { server: "docs", uri: "file://root" }),
      structured,
    );
    assert.equal(fields.status, "completed");
    assert.deepEqual(fields.content, [
      {
        type: "mcp_resource",
        uri: "file://guide.md",
        mime_type: "text/markdown",
        text: "GUIDE-BODY",
      },
      {
        type: "mcp_resource",
        uri: "file://figure.png",
        mime_type: "image/png",
        blob_saved_to: "C:/owned/figure.png",
      },
    ]);
  }
});

test("buildToolResultFields marks ReadMcpResource error output as failed", () => {
  const base = createToolCall("tc-mcp-error", "ReadMcpResource", {
    server: "docs",
    uri: "file://missing.md",
  });
  const fields = buildToolResultFields(
    false,
    {
      contents: [],
      error: "resource not found",
    },
    base,
  );

  assert.equal(fields.status, "failed");
  assert.equal(fields.raw_output, "Error: resource not found");
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: { type: "text", text: "Error: resource not found" },
    },
  ]);
  const directory = buildToolResultFields(
    false,
    { resources: [], error: "directory permission denied" },
    createToolCall("directory-error", "ReadMcpResourceDir", {}),
  );
  assertTextProjection(directory, "Error: directory permission denied", "failed");
});

test("MCP directory listings associate each name with its own URI and file or directory meaning", () => {
  const resources = [
    { name: "guide.md", uri: "file://docs/guide.md", mimeType: "text/markdown" },
    { name: "pictures", uri: "file://assets/pictures", mimeType: "inode/directory" },
    { name: "readme", uri: "file://other/readme" },
  ];
  for (const result of [
    { resources },
    JSON.stringify({ resources }),
    { resources: [null, { name: "INVALID" }, ...resources, { uri: "INVALID-NO-NAME" }] },
  ]) {
    const text = assertTextProjection(
      buildToolResultFields(
        false,
        "MODEL-FALLBACK",
        createToolCall("directory", "ReadMcpResourceDir", {}),
        result,
      ),
    );
    const lines = text.split("\n");
    for (const [name, uri, kind] of [
      ["guide.md", "file://docs/guide.md", "text/markdown"],
      ["pictures", "file://assets/pictures", "directory"],
      ["readme", "file://other/readme", ""],
    ]) {
      const own = lines.filter((line) => line.startsWith(name));
      assert.equal(own.length, 1);
      assert.ok(own[0]?.includes(uri));
      if (kind) assert.ok(own[0]?.includes(kind));
    }
    assert.doesNotMatch(text, /INVALID|MODEL-FALLBACK/);
  }
  const empty = assertTextProjection(
    buildToolResultFields(
      false,
      { resources: [] },
      createToolCall("empty", "ReadMcpResourceDir", {}),
    ),
  );
  assert.match(empty, /no resources|empty/i);
  assert.doesNotMatch(empty, /file:\/\//);
});

test("WebFetch retains readable results while projecting valid artifact observations separately", () => {
  const rows = [
    {
      artifact: { slug: "dashboard", ver: "v3", seeded: false },
      expected: { slug: "dashboard", ver: "v3", seeded: false },
    },
    {
      artifact: { slug: "versionless", seeded: false },
      expected: { slug: "versionless", seeded: false },
    },
    {
      artifact: { slug: "supported-slug", ver: "v4", seeded: true },
      expected: { slug: "supported-slug", ver: "v4" },
    },
    { artifact: { slug: "", ver: "v5" }, expected: undefined },
  ];
  for (const [index, row] of rows.entries()) {
    const body = `Readable artifact result ${index}`,
      fields = buildToolResultFields(
        false,
        { result: body, artifactRead: row.artifact },
        createToolCall(`artifact-${index}`, "WebFetch", { url: "https://artifact.local/target" }),
      );
    assertTextProjection(fields, body);
    assert.deepEqual(fields.output_metadata?.web_fetch?.artifact_read, row.expected);
  }
  const other = buildToolResultFields(
    false,
    { result: "Other-tool-body", artifactRead: { slug: "dashboard", seeded: false } },
    createToolCall("other", "Unrelated", {}),
  );
  assert.equal(other.output_metadata?.web_fetch, undefined);
});

test("Skill compacts only recognized inline success and preserves fork results and failure diagnostics", () => {
  const skill = createToolCall("skill", "Skill", { skill: "review" });
  const inline = { success: true, commandName: "code-review", allowedTools: ["Read"] };
  const compact = buildToolResultFields(false, "Launching review", skill, inline);
  assert.equal(compact.status, "completed");
  assert.match(compact.title ?? "", /Code Review/);
  assert.equal(compact.raw_output, undefined);
  assert.equal(compact.content, undefined);
  for (const background of [true, false]) {
    const fork = buildToolResultFields(false, "MODEL-LAUNCH", skill, {
      success: true,
      commandName: "code-review",
      status: "forked",
      agentId: "worker-7",
      result: "ACTUAL-FORK-RESULT",
      background,
    });
    assertTextProjection(fork, "ACTUAL-FORK-RESULT");
    assert.match(fork.title ?? "", /Code Review/);
    assert.deepEqual(
      fork.output_metadata?.skill,
      background === true ? { background: true } : undefined,
    );
  }
  assertTextProjection(
    buildToolResultFields(false, "MALFORMED-BACKGROUND-FALLBACK", skill, {
      success: true,
      commandName: "review",
      status: "forked",
      agentId: "worker",
      result: "UNTRUSTED",
      background: "true",
    }),
    "MALFORMED-BACKGROUND-FALLBACK",
  );
  const incomplete = buildToolResultFields(false, "USEFUL-FALLBACK", skill, {
    success: true,
    commandName: "untrusted-name",
    status: "forked",
    result: "missing worker",
  });
  assertTextProjection(incomplete, "USEFUL-FALLBACK");
  assert.equal(incomplete.title, undefined);
  assertTextProjection(
    buildToolResultFields(true, "OUTER-DENIED", skill, inline),
    "OUTER-DENIED",
    "failed",
  );
  assertTextProjection(
    buildToolResultFields(false, "NATIVE-FAILURE", skill, {
      success: false,
      commandName: "review",
    }),
    "NATIVE-FAILURE",
    "failed",
  );
  const unrelated = buildToolResultFields(
    false,
    "OTHER-OUTPUT",
    createToolCall("other-skill", "Bash", { command: "probe" }),
    { background: true },
  );
  assertTextProjection(unrelated, "OTHER-OUTPUT");
  assert.equal(unrelated.output_metadata?.skill, undefined);
});

test("Agent publishes observed model routes independently of the requested model and keeps readable results", () => {
  const base = createToolCall("models", "Agent", { model: "requested-alias", prompt: "review" });
  const rows = [
    {
      observations: {
        resolvedModel: " actual-final ",
        modelsUsed: [" actual-first ", "", "actual-first", 42, "actual-final"],
      },
      expected: { resolved_model: "actual-final", models_used: ["actual-first", "actual-final"] },
    },
    {
      observations: { resolvedModel: "resolved-only" },
      expected: { resolved_model: "resolved-only" },
    },
    {
      observations: { modelsUsed: ["route-only-first", "route-only-next"] },
      expected: { models_used: ["route-only-first", "route-only-next"] },
    },
    { observations: { resolvedModel: " ", modelsUsed: [null, 42, ""] }, expected: undefined },
  ];
  for (const [index, row] of rows.entries()) {
    const text = `Independent agent answer ${index}`;
    const fields = buildToolResultFields(false, text, base, {
      agentId: "worker",
      status: "completed",
      ...row.observations,
    });
    assertTextProjection(fields, text);
    assert.deepEqual(fields.output_metadata?.agent, row.expected);
  }
  const launch = buildToolResultFields(false, "Worker started", base, {
    status: "async_launched",
    agentId: "route-worker",
    description: "Review changes",
    modelsUsed: ["route-first", "route-last"],
    prompt: "review",
    outputFile: "C:/owned/worker.output",
  });
  assert.equal(launch.status, "detached");
  assertTextProjection(launch, "Review changes", "detached");
  assert.deepEqual(launch.output_metadata?.agent, { models_used: ["route-first", "route-last"] });
});

test("Read image receipts use the requested filename and exclude both encoded payload sources", () => {
  for (const file of ["C:\\captures\\windows-image.png", "/captures/unix-image.webp"]) {
    const base = createToolCall("image", "Read", { file_path: file });
    const block = [
      {
        type: "image",
        source: { type: "base64", media_type: "image/webp", data: "RAW-BASE64-SENTINEL" },
      },
    ];
    for (const structured of [
      undefined,
      { type: "image", file: { base64: "STRUCTURED-BASE64-SENTINEL", type: "image/png" } },
    ]) {
      const fields = buildToolResultFields(false, block, base, structured),
        text = assertTextProjection(fields);
      assert.ok(text.includes(file.startsWith("C:") ? "windows-image.png" : "unix-image.webp"));
      assert.match(text, /viewed image/i);
      assert.doesNotMatch(JSON.stringify(fields), /RAW-BASE64-SENTINEL|STRUCTURED-BASE64-SENTINEL/);
    }
  }
});

test("buildToolResultFields preserves failed Read image output", () => {
  const base = createToolCall("tc-image-read-error", "Read", {
    file_path: "assets/broken.png",
  });
  const fields = buildToolResultFields(
    true,
    [{ type: "text", text: "Image data could not be decoded." }],
    base,
    { type: "image" },
  );

  assert.equal(fields.status, "failed");
  assert.equal(fields.raw_output, "Image data could not be decoded.");
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: { type: "text", text: "Image data could not be decoded." },
    },
  ]);
});

test("Read unchanged receipts identify the returned file and retain fallback for malformed records", () => {
  const base = createToolCall("unchanged", "Read", { file_path: "requested/fallback.rs" });
  for (const result of [
    { type: "file_unchanged", file: { filePath: "actual/source.rs" } },
    { result: [{ type: "file_unchanged", file: { filePath: "actual/source.rs" } }] },
  ]) {
    const text = assertTextProjection(buildToolResultFields(false, "MODEL-FALLBACK", base, result));
    assert.ok(text.includes("actual/source.rs"));
    assert.match(text, /unchanged/i);
    assert.doesNotMatch(text, /requested\/fallback|MODEL-FALLBACK/);
  }
  assertTextProjection(
    buildToolResultFields(false, "UNRECOGNIZED-READ", base, { type: "file_unchanged", file: {} }),
    "UNRECOGNIZED-READ",
  );
});

test("Worktree receipts preserve confirmed branch or path and leave failures readable", () => {
  for (const row of [
    {
      name: "EnterWorktree",
      input: { name: "requested-name" },
      result: {
        message: "Entered actual worktree",
        worktreeBranch: "actual-branch",
        worktreePath: "C:/owned/actual-worktree",
        originalCwd: "C:/owned/restored",
      },
      identity: "actual-branch",
    },
    {
      name: "ExitWorktree",
      input: { action: "keep" },
      result: {
        message: "Exited actual worktree",
        worktreePath: "C:/owned/exited-worktree",
        originalCwd: "C:/owned/restored",
      },
      identity: "C:/owned/exited-worktree",
    },
  ]) {
    const base = createToolCall(row.name, row.name, row.input);
    const fields = buildToolResultFields(false, "MODEL-FALLBACK", base, row.result),
      text = assertTextProjection(fields);
    assert.ok(text.includes(row.identity));
    assert.doesNotMatch(text, /MODEL-FALLBACK|requested-name/);
    assert.equal(fields.title ?? base.title, base.title);
    assertTextProjection(
      buildToolResultFields(true, `${row.name}-FAILED`, base, row.result),
      `${row.name}-FAILED`,
      "failed",
    );
  }
});

test("Cron operation sections retain confirmed identities and the entire independently supplied prompt", () => {
  const prompt = `Check {literal braces}\n${"long prompt ".repeat(80)}PROMPT-TAIL`;
  const create = buildToolResultFields(
    false,
    { id: "confirmed-create", recurring: false, durable: true },
    createToolCall("create", "CronCreate", { cron: "*/5 * * * *", prompt, recurring: true }),
  );
  const createText = assertTextProjection(create);
  assert.deepEqual(sectionValues(create, "confirmed-create"), {
    "Schedule ID": "confirmed-create",
    Schedule: "Every 5 minutes",
    Prompt: prompt,
    Recurring: "no",
    Durable: "yes",
  });
  assert.ok(createText.includes(prompt));
  const deleted = buildToolResultFields(
    false,
    { id: "confirmed-delete" },
    createToolCall("delete", "CronDelete", { id: "requested-delete" }),
  );
  assertTextProjection(deleted);
  assert.deepEqual(sectionValues(deleted, "confirmed-delete"), {
    "Schedule ID": "confirmed-delete",
  });
  assert.doesNotMatch(String(deleted.raw_output), /requested-delete/);
  const jobs = [
    { id: "job-long", cron: "*/5 * * * *", prompt, recurring: false, durable: true },
    { id: "job-short", cron: "30 9 * * 1", prompt: "SHORT-INDEPENDENT", recurring: true },
  ];
  for (const result of [{ jobs }, JSON.stringify({ jobs })]) {
    const fields = buildToolResultFields(false, result, createToolCall("list", "CronList", {})),
      text = assertTextProjection(fields);
    assert.equal(sectionValues(fields, "job-long").Prompt, prompt);
    assert.equal(sectionValues(fields, "job-short").Prompt, "SHORT-INDEPENDENT");
    assert.ok(text.includes(prompt));
    assert.ok(text.includes("SHORT-INDEPENDENT"));
  }
  const single = buildToolResultFields(
    false,
    { jobs: [jobs[0]] },
    createToolCall("single", "CronList", {}),
  );
  assertTextProjection(single);
  assert.equal(sectionValues(single, "job-long").Recurring, "no");
  assert.equal(sectionValues(single, "job-long").Durable, "yes");
  const empty = buildToolResultFields(false, { jobs: [] }, createToolCall("empty", "CronList", {}));
  assert.match(assertTextProjection(empty), /none|no jobs/i);
  assert.deepEqual(empty.output_metadata?.presentation?.sections, [
    { fields: [{ label: "Jobs", value: { type: "text", value: "none" } }] },
  ]);
});

test("Cron schedules remain associated with their own job including literal unsupported expressions", () => {
  const rows = [
    { id: "minute", cron: "* * * * *", schedule: "Every minute" },
    { id: "step", cron: "*/5 * * * *", schedule: "Every 5 minutes" },
    {
      id: "hour",
      cron: "7 * * * *",
      humanSchedule: "Every hour at :07",
      schedule: "Every hour at minute 07",
    },
    { id: "hours", cron: "0 */2 * * *", schedule: "Every 2 hours on the hour" },
    { id: "daily", cron: "30 9 * * *", schedule: "Every day at 09:30" },
    { id: "weekly", cron: "30 9 * * 1", schedule: "Every Monday at 09:30" },
    { id: "monthly", cron: "30 9 15 * *", schedule: "Every month on day 15 at 09:30" },
    { id: "yearly", cron: "30 9 15 6 *", schedule: "Every June 15 at 09:30" },
    { id: "literal", cron: "0 9 1 * 1", schedule: undefined },
    {
      id: "producer-label",
      cron: "0 9 1 * 1",
      humanSchedule: "On the producer's calendar",
      schedule: "On the producer's calendar",
    },
  ];
  const fields = buildToolResultFields(
      false,
      { jobs: rows.map((row) => ({ ...row, prompt: `PROMPT-${row.id}`, recurring: true })) },
      createToolCall("schedules", "CronList", {}),
    ),
    text = assertTextProjection(fields);
  for (const row of rows) {
    const own = sectionValues(fields, row.id);
    assert.equal(own.Prompt, `PROMPT-${row.id}`);
    if (row.schedule) assert.equal(own.Schedule, row.schedule);
    else {
      assert.equal(own.Cron, row.cron);
      assert.equal(own.Schedule, undefined);
    }
    assert.ok(text.includes(`PROMPT-${row.id}`));
  }
});

test("Wakeup receipts report the returned instant and actual clamped delay under an isolated timezone", () => {
  for (const [result, delay, clamped] of [
    [
      { scheduledFor: 1_780_662_896_000, clampedDelaySeconds: 90, wasClamped: true },
      "1m 30s",
      "yes",
    ],
    [
      JSON.stringify({
        scheduledFor: 1_780_662_896_000,
        clampedDelaySeconds: 3600,
        wasClamped: false,
      }),
      "1h",
      "no",
    ],
  ] as const) {
    const fields = projectInUtc(
        "ScheduleWakeup",
        { delaySeconds: 7, reason: "Requested delay differs", prompt: "continue" },
        result,
      ),
      text = assertTextProjection(fields);
    assert.ok(text.includes("Scheduled for: 2026-06-05 12:34:56 local"));
    assert.ok(text.includes(`Actual delay: ${delay}`));
    assert.ok(text.includes(`Clamped: ${clamped}`));
    assert.doesNotMatch(text, /Actual delay: 7s/);
  }
});

test("Push receipts retain distinct returned messages and delivery observations at the supplied instant", () => {
  const input = { message: "REQUESTED-MESSAGE", status: "proactive" };
  const fields = projectInUtc("PushNotification", input, {
      message: "ACTUAL-RECEIPT",
      pushSent: false,
      localSent: true,
      disabledReason: "config_off",
      sentAt: "2026-06-05T12:34:56.000Z",
    }),
    text = assertTextProjection(fields);
  for (const expected of [
    "ACTUAL-RECEIPT",
    "Push sent: no",
    "Local sent: yes",
    "notifications disabled",
    "Sent at: 2026-06-05 12:34:56 local",
  ])
    assert.ok(text.includes(expected));
  assert.doesNotMatch(text, /REQUESTED-MESSAGE/);
  const unknown = projectInUtc(
      "PushNotification",
      input,
      JSON.stringify({
        message: "DIFFERENT-RECEIPT",
        pushSent: true,
        localSent: false,
        disabledReason: "no_transport",
        sentAt: "unknown-source-time",
      }),
    ),
    unknownText = assertTextProjection(unknown);
  for (const expected of [
    "DIFFERENT-RECEIPT",
    "Push sent: yes",
    "Local sent: no",
    "no notification transport",
    "unknown-source-time",
  ])
    assert.ok(unknownText.includes(expected));
  const echo = projectInUtc("PushNotification", input, {
    message: "REQUESTED-MESSAGE",
    pushSent: true,
  });
  const echoText = assertTextProjection(echo);
  assert.doesNotMatch(echoText, /REQUESTED-MESSAGE|Local sent:/);
});

test("RemoteTrigger preserves response meaning with or without summaries and honors both failure authorities", () => {
  const base = createToolCall("remote", "RemoteTrigger", { action: "run", trigger_id: "target" });
  for (const row of [
    {
      status: 200,
      summary: "ACTUAL-SUMMARY",
      data: { ok: true, run_id: "actual-run" },
      outer: false,
    },
    { status: 200, summary: undefined, data: { enabled: false, attempts: 0 }, outer: false },
    { status: 404, summary: "MISSING-TRIGGER", data: { error: "not_found" }, outer: false },
    { status: 503, summary: "SERVER-DIAGNOSTIC", data: { error: "unavailable" }, outer: false },
    {
      status: 200,
      summary: "STALE-SUCCESS",
      data: { detail: "outer failure diagnostic" },
      outer: true,
    },
  ]) {
    const result = {
      status: row.status,
      json: JSON.stringify(row.data, null, 2),
      summary: row.summary,
    };
    for (const representation of [result, JSON.stringify(result)]) {
      const text = assertTextProjection(
        buildToolResultFields(row.outer, representation, base),
        undefined,
        row.outer || row.status >= 400 ? "failed" : "completed",
      );
      assert.ok(text.includes(`Status: ${row.status}`));
      if (row.summary) assert.ok(text.includes(row.summary));
      const response = text.split("\n").find((line) => line.startsWith("Response: "));
      assert.ok(response);
      assert.deepEqual(JSON.parse(response.slice("Response: ".length)), row.data);
    }
  }
  assert.ok(
    assertTextProjection(
      buildToolResultFields(false, { status: 200, json: "NON-JSON-RESPONSE" }, base),
    ).includes("NON-JSON-RESPONSE"),
  );
});

test("REPL preserves distinct output channels and media counts without publishing encoded payloads", () => {
  const base = createToolCall("repl", "REPL", { code: "INPUT-CODE-PRIVATE" });
  const result = {
    code: "INPUT-CODE-PRIVATE",
    stdout: "STDOUT-ANSWER",
    stderr: "STDERR-WARNING",
    result: { ok: false, count: 7 },
    registeredTools: ["tool-alpha", "tool-beta"],
    images: [{ base64: "IMAGE-SECRET-ONE" }, { base64: "IMAGE-SECRET-TWO" }],
    documents: [{ base64: "DOCUMENT-SECRET" }],
  };
  for (const representation of [result, JSON.stringify(result)]) {
    const fields = buildToolResultFields(false, representation, base),
      text = assertTextProjection(fields);
    for (const expected of [
      "Stdout: STDOUT-ANSWER",
      "Stderr: STDERR-WARNING",
      "Registered tools: tool-alpha, tool-beta",
      "Images: 2",
      "Documents: 1",
    ])
      assert.ok(text.includes(expected));
    const value = text.split("\n").find((line) => line.startsWith("Result: "));
    assert.ok(value);
    assert.deepEqual(JSON.parse(value.slice(8)), { ok: false, count: 7 });
    assert.doesNotMatch(JSON.stringify(fields), /INPUT-CODE-PRIVATE|IMAGE-SECRET|DOCUMENT-SECRET/);
  }
  for (const outer of [false, true]) {
    const text = assertTextProjection(
      buildToolResultFields(outer, { error: "ACTUAL-REPL-ERROR", stderr: "ACTUAL-STACK" }, base),
      undefined,
      "failed",
    );
    assert.ok(text.includes("ACTUAL-REPL-ERROR"));
    assert.ok(text.includes("ACTUAL-STACK"));
  }
});

test("REPL page-failure previews preserve visible associations and account for local and upstream omissions", () => {
  const base = createToolCall("pages", "REPL", { code: "readPages()" });
  for (const count of [3, 137]) {
    const pages = Array.from({ length: count }, (_, index) => ({
      page: index + 1,
      file: `file-${index + 1}.pdf`,
      error: `cause-${index + 1}`,
    }));
    const fields = buildToolResultFields(
        false,
        {
          imagePagesFailed: [null, { page: "invalid" }, ...pages],
          imagePagesFailedOmitted: 7,
          imagesOmitted: 11,
          documentsOmitted: 13,
        },
        base,
      ),
      text = assertTextProjection(fields);
    const lines = text.split("\n").filter((line) => line.startsWith("Failed image page: "));
    assert.ok(lines.length > 0);
    assert.ok(lines.length <= count);
    if (count === 3) assert.equal(lines.length, 3);
    else assert.ok(lines.length < count);
    for (const [index, line] of lines.entries()) {
      assert.ok(line.includes(`Failed image page: ${index + 1} (`));
      assert.ok(line.includes(`file file-${index + 1}.pdf: cause-${index + 1}`));
    }
    assert.ok(text.includes(`Failed image pages omitted: ${7 + count - lines.length}`));
    assert.match(text, /Images omitted: 11/);
    assert.match(text, /Documents omitted: 13/);
    assert.doesNotMatch(text, /invalid/);
  }
});

test("Monitor receipts use returned task and timeout observations including implicit persistence", () => {
  const base = createToolCall("monitor", "Monitor", {
    description: "watch logs",
    timeout_ms: 9000,
    command: "watch",
  });
  for (const result of [
    { taskId: "finite-monitor", timeoutMs: 30000 },
    { taskId: "persistent-monitor", timeoutMs: 0 },
  ]) {
    for (const representation of [result, JSON.stringify(result)]) {
      const text = assertTextProjection(
        buildToolResultFields(false, representation, base),
        undefined,
        "detached",
      );
      assert.ok(text.includes(result.taskId));
      assert.doesNotMatch(text, /Timeout: 9s/);
      if (result.timeoutMs === 0) {
        assert.match(text, /Persistent: yes/);
        assert.doesNotMatch(text, /Timeout:/);
      } else {
        assert.match(text, /Timeout: 30s/);
        assert.match(text, /Persistent: no/);
      }
    }
  }
});

test("Workflow receipts retain local or remote identity and fail truthfully despite launch-shaped errors", () => {
  const base = createToolCall("workflow", "Workflow", {
    name: "requested-name",
    script: "PRIVATE-INPUT-SCRIPT",
  });
  const rows = [
    {
      status: "async_launched",
      taskId: "actual-local-task",
      taskType: "local_workflow",
      workflowName: "actual-name",
      runId: "actual-run",
      summary: "ACTUAL-SUMMARY",
      transcriptDir: "C:/owned/transcript",
      scriptPath: "C:/owned/worker.js",
      warning: "ACTUAL-WARNING",
    },
    {
      status: "remote_launched",
      taskId: "actual-remote-task",
      sessionUrl: "https://claude.ai/session/actual",
    },
  ];
  for (const row of rows) {
    for (const representation of [row, JSON.stringify(row)]) {
      const text = assertTextProjection(
        buildToolResultFields(false, representation, base),
        undefined,
        "detached",
      );
      for (const [key, value] of Object.entries(row))
        if (key !== "status") assert.ok(text.includes(value));
      assert.doesNotMatch(text, /PRIVATE-INPUT-SCRIPT|requested-name/);
    }
  }
  const failed = assertTextProjection(
    buildToolResultFields(
      false,
      { status: "async_launched", taskId: "failed-task", error: "ACTUAL-CHECK-FAILURE" },
      base,
    ),
    undefined,
    "failed",
  );
  assert.ok(failed.includes("failed-task"));
  assert.ok(failed.includes("ACTUAL-CHECK-FAILURE"));
  assert.doesNotMatch(failed, /PRIVATE-INPUT-SCRIPT/);
});

test("EnterPlanMode suppresses recognized success while retaining failures and unrecognized fallback", () => {
  const base = createToolCall("plan", "EnterPlanMode", {});
  for (const result of [
    { message: "Plan mode entered" },
    JSON.stringify({ message: "Entered plan mode" }),
  ]) {
    const fields = buildToolResultFields(false, result, base);
    assert.equal(fields.status, "completed");
    assert.equal(fields.raw_output, undefined);
    assert.equal(fields.content, undefined);
  }
  assertTextProjection(
    buildToolResultFields(true, "PLAN-MODE-DENIED", base, { message: "stale success" }),
    "PLAN-MODE-DENIED",
    "failed",
  );
  assertTextProjection(
    buildToolResultFields(false, "UNRECOGNIZED-PLAN-RECEIPT", base, { future: true }),
    "UNRECOGNIZED-PLAN-RECEIPT",
  );
});

test("bounded Grep and Glob previews account for locally omitted filenames separately from SDK totals", () => {
  const files = Array.from(
    { length: 137 },
    (_, index) => `independent/file-${String(index).padStart(3, "0")}.rs`,
  );
  for (const name of ["Grep", "Glob"]) {
    const result =
      name === "Grep"
        ? { mode: "files_with_matches", filenames: files, numFiles: 901 }
        : { filenames: files, numFiles: 901, durationMs: 17, truncated: true };
    const text = assertTextProjection(
      buildToolResultFields(
        false,
        "unused fallback",
        createToolCall(`bounded-${name}`, name, { pattern: "fixture" }),
        result,
      ),
    );
    const visible = files.filter((file) => text.split("\n").includes(file));
    assert.ok(visible.length > 0 && visible.length < files.length);
    assert.deepEqual(visible, files.slice(0, visible.length));
    assert.equal(new Set(visible).size, visible.length);
    assert.ok(text.includes(files[0]));
    assert.ok(!text.includes(files.at(-1) ?? "missing"));
    const omission = /(?:\.\.\.|\u2026)\s*(\d+)\s+more/i.exec(text);
    assert.ok(omission, text);
    assert.equal(Number(omission[1]), files.length - visible.length);
    assert.match(text, /901 files/);
  }
});

test("confirmed stop, worktree and literal REPL channels retain independent native receipts in direct and saved JSON representations", () => {
	const rows = JSON.parse(
		readFileSync(
			new URL(
				"../../../tests/fixtures/sdk-confirmed-tool-results.json",
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
	for (const row of rows)
		for (const result of [row.result, JSON.stringify(row.result)]) {
			const fields = buildToolResultFields(
				false,
				result,
				createToolCall(row.id, row.tool, row.input),
			);
			assertTextProjection(fields, row.text);
			assert.deepEqual(
				fields.output_metadata?.presentation,
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
				row.id,
			);
			assert.doesNotMatch(
				JSON.stringify(fields),
				/PRIVATE-IMAGE|PRIVATE-DOCUMENT|REQUESTED-CODE/,
			);
			if (row.tool === "ScheduleWakeup")
				assert.doesNotMatch(
					String(fields.raw_output),
					/Scheduled for|Actual delay|requested future/,
				);
			const failed = buildToolResultFields(
				true,
				"INDEPENDENT-OPERATION-FAILURE",
				createToolCall(row.id, row.tool, row.input),
			);
			assertTextProjection(failed, "INDEPENDENT-OPERATION-FAILURE", "failed");
			assert.equal(failed.output_metadata?.presentation, undefined);
		}
	const scheduled = projectInUtc(
		"ScheduleWakeup",
		{ stop: true },
		{
			stopped: false,
			scheduledFor: 1780662896000,
			clampedDelaySeconds: 90,
			wasClamped: true,
		},
	);
	assertTextProjection(
		scheduled,
		"Scheduled for: 2026-06-05 12:34:56 local\nActual delay: 1m 30s\nClamped: yes",
	);
	assert.doesNotMatch(String(scheduled.raw_output), /Stopped|Cancelled/);
});
