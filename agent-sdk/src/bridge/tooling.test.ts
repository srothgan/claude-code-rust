import test from "node:test";
import assert from "node:assert/strict";
import {
  CACHE_SPLIT_POLICY,
  buildToolResultFields,
  createToolCall,
  normalizeToolResultText,
  previewKilobyteLabel,
  unwrapToolUseResult,
} from "../bridge.js";

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

test("createToolCall preserves parent tool linkage metadata", () => {
  const toolCall = createToolCall(
    "tc-child",
    "Bash",
    { command: "echo hi" },
    "tc-parent",
  );

  assert.deepEqual(toolCall.meta, {
    claudeCode: { toolName: "Bash", parentToolUseId: "tc-parent" },
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

test("createToolCall includes search and webfetch context in title", () => {
  const glob = createToolCall("tc-g", "Glob", {
    pattern: "**/*.md",
    path: "notes",
  });
  assert.equal(glob.title, "Glob **/*.md in notes");

  const grep = createToolCall("tc-grep", "Grep", {
    pattern: "TODO",
    path: "src",
    glob: "**/*.rs",
    output_mode: "content",
    "-i": true,
    "-C": 2,
    type: "rust",
    head_limit: 10,
    offset: 5,
    multiline: true,
  });
  assert.equal(
    grep.title,
    "Grep TODO in src (glob **/*.rs, type rust, content, case-insensitive, context 2, limit 10, offset 5, multiline)",
  );

  const fetch = createToolCall("tc-f", "WebFetch", {
    url: "https://example.com",
  });
  assert.equal(fetch.title, "WebFetch https://example.com");
});

test("createToolCall builds Agent title from name and type without description fallback", () => {
  const named = createToolCall("tc-agent-name", "Agent", {
    description: "review changes",
    prompt: "Review the branch",
    name: " review-worker ",
    subagent_type: " general-purpose ",
    model: " opus ",
  });
  const typed = createToolCall("tc-agent-type", "Agent", {
    description: "inspect state",
    prompt: "Inspect the runtime",
    subagent_type: " general-purpose ",
    model: " sonnet ",
  });
  const describedOnly = createToolCall("tc-agent-description", "Agent", {
    description: "should not become title",
    prompt: "Review",
  });

  assert.equal(named.title, "Agent: review-worker");
  assert.equal(typed.title, "Agent: general-purpose");
  assert.equal(describedOnly.title, "Agent");
});

test("createToolCall builds worktree titles from input rules", () => {
  const namedEnter = createToolCall("tc-enter-name", "EnterWorktree", {
    name: "feature-auth",
  });
  assert.equal(namedEnter.kind, "other");
  assert.equal(namedEnter.title, "feature-auth");

  const pathEnter = createToolCall("tc-enter-path", "EnterWorktree", {
    path: "C:\\repo\\.worktrees\\feature-auth",
  });
  assert.equal(pathEnter.kind, "other");
  assert.equal(pathEnter.title, "EnterWorktree");

  const exit = createToolCall("tc-exit", "ExitWorktree", {
    action: "remove",
    discard_changes: true,
  });
  assert.equal(exit.kind, "other");
  assert.equal(exit.title, "ExitWorktree");
});

test("createToolCall maps cron tools to other kind with stable titles", () => {
  for (const toolName of ["CronCreate", "CronDelete", "CronList"]) {
    const toolCall = createToolCall(`tc-${toolName}`, toolName, {
      cron: "30 9 * * 1",
      prompt: "Send weekly status",
      id: "schedule-1",
    });
    assert.equal(toolCall.kind, "other");
    assert.equal(toolCall.title, toolName);
  }
});

test("createToolCall maps ScheduleWakeup to other kind with stable title", () => {
  const toolCall = createToolCall("tc-wakeup", "ScheduleWakeup", {
    delaySeconds: 90,
    reason: "Poll again after warmup",
    prompt: "/loop check status",
  });

  assert.equal(toolCall.kind, "other");
  assert.equal(toolCall.title, "ScheduleWakeup");
});

test("createToolCall maps PushNotification to other kind with stable title", () => {
  const toolCall = createToolCall("tc-push-notification", "PushNotification", {
    message: "Build finished",
    status: "proactive",
  });

  assert.equal(toolCall.kind, "other");
  assert.equal(toolCall.title, "PushNotification");
});

test("createToolCall maps RemoteTrigger to other kind and action title", () => {
  const toolCall = createToolCall("tc-remote-trigger", "RemoteTrigger", {
    action: " run ",
    trigger_id: "deploy-prod",
  });

  assert.equal(toolCall.kind, "other");
  assert.equal(toolCall.title, "RemoteTrigger: run");
});

test("createToolCall uses RemoteTrigger fallback title without action", () => {
  const toolCall = createToolCall(
    "tc-remote-trigger-fallback",
    "RemoteTrigger",
    {
      trigger_id: "deploy-prod",
    },
  );

  assert.equal(toolCall.kind, "other");
  assert.equal(toolCall.title, "RemoteTrigger");
});

test("createToolCall maps REPL to other kind and code title", () => {
  const toolCall = createToolCall("tc-repl", "REPL", {
    code: "  await inspectState()  ",
    description: "Inspect runtime state",
    timeout: 45_000,
  });

  assert.equal(toolCall.kind, "other");
  assert.equal(toolCall.title, "REPL: await inspectState()");
});

test("createToolCall uses REPL fallback title instead of description", () => {
  const toolCall = createToolCall("tc-repl-fallback", "REPL", {
    description: "Inspect runtime state",
  });

  assert.equal(toolCall.kind, "other");
  assert.equal(toolCall.title, "REPL");
});

test("createToolCall maps Monitor to other kind and description title", () => {
  const toolCall = createToolCall("tc-monitor", "Monitor", {
    description: "watch deploy logs",
    timeout_ms: 30000,
    persistent: false,
    command: "tail -f deploy.log",
  });

  assert.equal(toolCall.kind, "other");
  assert.equal(toolCall.title, "Monitor: watch deploy logs");
});

test("createToolCall maps Workflow to other kind and name title", () => {
  const namedWorkflow = createToolCall("tc-workflow", "Workflow", {
    name: "spec",
    args: { topic: "rendering" },
  });
  const fallbackWorkflow = createToolCall("tc-workflow-fallback", "Workflow", {
    script:
      "export const meta = { name: 'inline', description: 'Run', phases: [] };",
  });

  assert.equal(namedWorkflow.kind, "other");
  assert.equal(namedWorkflow.title, "Workflow: spec");
  assert.equal(fallbackWorkflow.kind, "other");
  assert.equal(fallbackWorkflow.title, "Workflow");
});

test("createToolCall maps Skill to other kind and formats its name for display", () => {
  const cases = [
    ["claude-api", "Skill: Claude API"],
    ["dataviz", "Skill: Dataviz"],
    [" frontend-design:frontend-design ", "Skill: Frontend Design"],
    ["keybindings_help", "Skill: Keybindings Help"],
    ["strava-coach", "Skill: Strava Coach"],
    ["github:gh-fix-ci", "Skill: GitHub / GH Fix CI"],
    ["mobile:iOS-debug", "Skill: Mobile / iOS Debug"],
    ["future-widget", "Skill: Future Widget"],
    ["future::widget", "Skill: future::widget"],
    ["future.widget", "Skill: future.widget"],
  ];

  for (const [skill, expectedTitle] of cases) {
    const toolCall = createToolCall(`tc-skill-${skill}`, "Skill", { skill });
    assert.equal(toolCall.kind, "other");
    assert.equal(toolCall.title, expectedTitle);
  }
  const incompleteSkill = createToolCall("tc-skill-incomplete", "Skill", {});

  assert.equal(incompleteSkill.title, "Skill");
});

test("createToolCall maps project and artifact tools to compact titles", () => {
  const projectInfo = createToolCall("tc-project-info", "Projects", {
    method: "project_info",
  });
  const projectRead = createToolCall("tc-project-read", "Projects", {
    method: "project_read",
    path: "claude/notes.md",
  });
  const projectSearch = createToolCall("tc-project-search", "Projects", {
    method: "project_search",
    query: "migration",
  });
  const artifactWithLabel = createToolCall("tc-artifact-label", "Artifact", {
    file_path: "C:/work/report.html",
    favicon: "R",
    label: "report-v2",
    description: "Quarterly report",
  });
  const artifactFallback = createToolCall("tc-artifact-path", "Artifact", {
    file_path: "C:/work/report.html",
    favicon: "R",
  });
  const rolePicker = createToolCall(
    "tc-role-picker",
    "ShowOnboardingRolePicker",
    {},
  );

  assert.equal(projectInfo.kind, "other");
  assert.equal(projectInfo.title, "Projects: info");
  assert.equal(projectRead.title, "Projects: read claude/notes.md");
  assert.equal(projectSearch.title, "Projects: search migration");
  assert.equal(artifactWithLabel.kind, "other");
  assert.equal(artifactWithLabel.title, "Artifact: report-v2");
  assert.equal(
    (artifactWithLabel.raw_input as Record<string, unknown>).description,
    "Quarterly report",
  );
  assert.equal(artifactFallback.title, "Artifact: C:/work/report.html");
  assert.equal(rolePicker.kind, "other");
  assert.equal(rolePicker.title, "ShowOnboardingRolePicker");
});

test("createToolCall maps EnterPlanMode to switch_mode kind with stable title", () => {
  const toolCall = createToolCall("tc-enter-plan-mode", "EnterPlanMode", {});

  assert.equal(toolCall.kind, "switch_mode");
  assert.equal(toolCall.title, "EnterPlanMode");
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

test("new SDK tools retain generic inputs and structured outputs losslessly", () => {
  const feedbackInput = {
    type: "bug",
    title: "Feedback draft",
    details: "Reproduction details",
    area: "/feedback",
  };
  const feedback = createToolCall("tc-feedback", "SendFeedback", feedbackInput);
  const feedbackOutput = {
    success: true,
    message: "Feedback queued for review.",
  };
  const feedbackFields = buildToolResultFields(
    false,
    feedbackOutput,
    feedback,
    feedbackOutput,
  );

  assert.deepEqual(feedback.raw_input, feedbackInput);
  assert.equal(feedbackFields.raw_output, JSON.stringify(feedbackOutput));
  assert.deepEqual(feedbackFields.content, [
    {
      type: "content",
      content: { type: "text", text: JSON.stringify(feedbackOutput) },
    },
  ]);

  const refreshInput = { server: "docs" };
  const refresh = createToolCall("tc-refresh", "RefreshMcpTools", refreshInput);
  const refreshOutput = [
    {
      server: "docs",
      status: "refreshed",
      toolCount: 3,
      added: ["lookup"],
      removed: ["legacy_lookup"],
    },
    {
      server: "offline",
      status: "not_connected",
      error: "No live connection",
    },
  ];
  const refreshFields = buildToolResultFields(
    false,
    refreshOutput,
    refresh,
    refreshOutput,
  );

  assert.deepEqual(refresh.raw_input, refreshInput);
  assert.equal(refreshFields.raw_output, JSON.stringify(refreshOutput));
  assert.deepEqual(refreshFields.content, [
    {
      type: "content",
      content: { type: "text", text: JSON.stringify(refreshOutput) },
    },
  ]);

  const proposalInput = {
    proposals: [
      {
        name: "diagnose-bridge",
        kind: "new",
        description: "Diagnose the bridge",
        evidence: ["memory/bridge.md"],
        skillMd: "---\nname: diagnose-bridge\n---\n",
      },
    ],
  };
  const proposal = createToolCall("tc-propose", "ProposeSkills", proposalInput);
  const proposalOutput = { proposalCount: 1 };
  const proposalFields = buildToolResultFields(
    false,
    proposalOutput,
    proposal,
    proposalOutput,
  );

  assert.deepEqual(proposal.raw_input, proposalInput);
  assert.equal(proposalFields.raw_output, JSON.stringify(proposalOutput));
  assert.deepEqual(proposalFields.content, [
    {
      type: "content",
      content: { type: "text", text: JSON.stringify(proposalOutput) },
    },
  ]);
});

test("buildToolResultFields renders structured Grep output", () => {
  const base = createToolCall("tc-grep", "Grep", {
    pattern: "TODO",
    path: "src",
    output_mode: "content",
  });
  const fields = buildToolResultFields(false, "raw SDK text", base, {
    mode: "content",
    numFiles: 2,
    filenames: ["src/a.rs", "src/b.rs"],
    content: "src/a.rs:1:TODO\nsrc/b.rs:2:TODO",
    numLines: 2,
    numMatches: 2,
    appliedLimit: 250,
  });

  const expected =
    "src/a.rs:1:TODO\nsrc/b.rs:2:TODO\nSummary: 2 files, 2 matches, 2 lines, mode content, limit 250";
  assert.equal(fields.status, "completed");
  assert.equal(fields.raw_output, expected);
  assert.deepEqual(fields.content, [
    { type: "content", content: { type: "text", text: expected } },
  ]);
});

test("buildToolResultFields prefers target Grep totals over legacy counts", () => {
  const base = createToolCall("tc-grep-totals", "Grep", {
    pattern: "TODO",
    path: "src",
    output_mode: "content",
  });
  const fields = buildToolResultFields(false, "raw SDK text", base, {
    mode: "content",
    numFiles: 2,
    totalFiles: 5,
    filenames: ["src/a.rs", "src/b.rs"],
    content: "src/a.rs:1:TODO\nsrc/b.rs:2:TODO",
    numLines: 2,
    totalLines: 9,
    numMatches: 2,
  });

  assert.equal(
    fields.raw_output,
    "src/a.rs:1:TODO\nsrc/b.rs:2:TODO\nSummary: 5 files, 2 matches, 9 lines, mode content",
  );
});

test("buildToolResultFields omits a contradictory legacy zero Grep file count", () => {
  const base = createToolCall("tc-grep-unreported-files", "Grep", {
    pattern: "ToolCallInfo",
    path: "src",
    output_mode: "content",
  });
  const fields = buildToolResultFields(false, "raw SDK text", base, {
    mode: "content",
    numFiles: 0,
    filenames: [],
    content: "src/app/mod.rs:1:ToolCallInfo\nsrc/ui/mod.rs:2:ToolCallInfo",
    numLines: 239,
    totalLines: 239,
  });

  assert.equal(
    fields.raw_output,
    "src/app/mod.rs:1:ToolCallInfo\nsrc/ui/mod.rs:2:ToolCallInfo\nSummary: 239 lines, mode content",
  );
});

test("buildToolResultFields ignores malformed Grep totals without losing output", () => {
  const base = createToolCall("tc-grep-invalid-totals", "Grep", {
    pattern: "TODO",
    output_mode: "content",
  });
  const fields = buildToolResultFields(false, "raw SDK text", base, {
    mode: "content",
    numFiles: 2,
    totalFiles: -5,
    filenames: ["src/a.rs", "src/b.rs"],
    content: "src/a.rs:1:TODO",
    numLines: 1,
    totalLines: "many",
  });

  assert.equal(
    fields.raw_output,
    "src/a.rs:1:TODO\nSummary: 2 files, 1 line, mode content",
  );
});

test("buildToolResultFields renders structured empty Grep output", () => {
  const base = createToolCall("tc-grep-empty", "Grep", {
    pattern: "<rare string>",
    output_mode: "content",
  });
  const fields = buildToolResultFields(false, "No matches found", base, {
    mode: "content",
    numFiles: 0,
    filenames: [],
    content: "",
    numLines: 0,
  });

  const expected = "No matches found\nSummary: 0 files, 0 lines, mode content";
  assert.equal(fields.raw_output, expected);
  assert.deepEqual(fields.content, [
    { type: "content", content: { type: "text", text: expected } },
  ]);
});

test("buildToolResultFields renders structured Glob output", () => {
  const base = createToolCall("tc-glob", "Glob", {
    pattern: "**/*.rs",
    path: "src",
  });
  const fields = buildToolResultFields(false, "", base, {
    durationMs: 12,
    numFiles: 2,
    filenames: ["src/main.rs", "src/lib.rs"],
    truncated: false,
  });

  const expected = "2 files found\nsrc/main.rs\nsrc/lib.rs\nDuration: 12ms";
  assert.equal(fields.status, "completed");
  assert.equal(fields.raw_output, expected);
  assert.deepEqual(fields.content, [
    { type: "content", content: { type: "text", text: expected } },
  ]);
});

test("normalizeToolResultText collapses persisted-output payload to first meaningful line", () => {
  const normalized = normalizeToolResultText(`
<persisted-output>
  │ Output too large (132.5KB). Full output saved to: C:\\tmp\\tool-results\\bbf63b9.txt
  │
  │ Preview (first 2KB):
  │
  │ {"huge":"payload"}
  │ ...
  │ </persisted-output>
`);
  assert.equal(
    normalized,
    "Output too large (132.5KB). Full output saved to: C:\\tmp\\tool-results\\bbf63b9.txt",
  );
});

test("normalizeToolResultText does not sanitize non-error output", () => {
  const text =
    "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";
  assert.equal(normalizeToolResultText(text), text);
});

test("normalizeToolResultText sanitizes exact SDK rejection payloads for errors", () => {
  const cancelledText =
    "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";
  assert.equal(
    normalizeToolResultText(cancelledText, true),
    "Cancelled by user.",
  );

  const deniedText =
    "Permission for this tool use was denied. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). Try a different approach or report the limitation to complete your task.";
  assert.equal(normalizeToolResultText(deniedText, true), "Permission denied.");
});

test("normalizeToolResultText sanitizes SDK rejection prefixes with user follow-up", () => {
  const cancelledWithUserMessage =
    "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). To tell you how to proceed, the user said:\nPlease skip this";
  assert.equal(
    normalizeToolResultText(cancelledWithUserMessage, true),
    "Cancelled by user.",
  );

  const deniedWithUserMessage =
    "Permission for this tool use was denied. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). The user said:\nNot now";
  assert.equal(
    normalizeToolResultText(deniedWithUserMessage, true),
    "Permission denied.",
  );
});

test("normalizeToolResultText does not sanitize substring matches in error output", () => {
  const bashOutput = "grep output: doesn't want to proceed with this tool use";
  assert.equal(normalizeToolResultText(bashOutput, true), bashOutput);
});

test("cache split policy defaults stay aligned with UI thresholds", () => {
  assert.equal(CACHE_SPLIT_POLICY.softLimitBytes, 1536);
  assert.equal(CACHE_SPLIT_POLICY.hardLimitBytes, 4096);
  assert.equal(CACHE_SPLIT_POLICY.previewLimitBytes, 2048);
  assert.equal(previewKilobyteLabel(CACHE_SPLIT_POLICY), "2KB");
});

test("buildToolResultFields uses normalized persisted-output text", () => {
  const fields = buildToolResultFields(
    false,
    `<persisted-output>
      │ Output too large (14KB). Full output saved to: C:\\tmp\\tool-results\\x.txt
      │
      │ Preview (first 2KB):
      │ {"k":"v"}
      │ </persisted-output>`,
  );
  assert.equal(
    fields.raw_output,
    "Output too large (14KB). Full output saved to: C:\\tmp\\tool-results\\x.txt",
  );
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: "Output too large (14KB). Full output saved to: C:\\tmp\\tool-results\\x.txt",
      },
    },
  ]);
});

test("buildToolResultFields sanitizes SDK rejection text only for failed results", () => {
  const sdkRejectionText =
    "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";

  const successFields = buildToolResultFields(false, sdkRejectionText);
  assert.equal(successFields.raw_output, sdkRejectionText);

  const errorFields = buildToolResultFields(true, sdkRejectionText);
  assert.equal(errorFields.raw_output, "Cancelled by user.");
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
});

test("buildToolResultFields preserves Edit diff content from input and structured repository", () => {
  const base = createToolCall("tc-e", "Edit", {
    file_path: "src/main.ts",
    old_string: "old",
    new_string: "new",
  });
  const fields = buildToolResultFields(
    false,
    [{ text: "Updated successfully" }],
    base,
    {
      result: {
        filePath: "src/main.ts",
        gitDiff: {
          repository: "acme/project",
        },
      },
    },
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
});

test("buildToolResultFields ignores model-facing Bash stale read hints", () => {
  const base = createToolCall("tc-bash", "Bash", { command: "npm test" });
  const fields = buildToolResultFields(
    false,
    {
      stdout: "real stdout",
      stderr: "",
      interrupted: false,
      staleReadFileStateHint: "src/main.rs changed while command ran",
    },
    base,
    {
      result: {
        stdout: "real stdout",
        stderr: "",
        interrupted: false,
        staleReadFileStateHint: "src/main.rs changed while command ran",
      },
    },
  );

  assert.equal(fields.raw_output, "real stdout");
  assert.equal(fields.output_metadata, undefined);
});

test("buildToolResultFields maps PowerShell structured output like shell output", () => {
  const base = createToolCall("tc-powershell", "PowerShell", {
    command: "Get-ChildItem",
  });
  const fields = buildToolResultFields(
    false,
    {
      stdout: "stdout line",
      stderr: "stderr line",
      interrupted: true,
    },
    base,
    {
      result: {
        stdout: "stdout line",
        stderr: "stderr line",
        interrupted: true,
      },
    },
  );

  assert.equal(
    fields.raw_output,
    "stdout line\nstderr line\nCommand was aborted before completion.",
  );
  assert.equal(fields.output_metadata, undefined);
});

test("buildToolResultFields adds Bash auto-backgrounded metadata and message", () => {
  const base = createToolCall("tc-bash-bg", "Bash", {
    command: "npm run watch",
  });
  const fields = buildToolResultFields(
    false,
    {
      stdout: "",
      stderr: "",
      interrupted: false,
      backgroundTaskId: "task-42",
      assistantAutoBackgrounded: true,
    },
    base,
    {
      result: {
        stdout: "",
        stderr: "",
        interrupted: false,
        backgroundTaskId: "task-42",
        assistantAutoBackgrounded: true,
      },
    },
  );

  assert.equal(
    fields.raw_output,
    "Command was auto-backgrounded by assistant mode with ID: task-42.",
  );
  assert.deepEqual(fields.output_metadata, {
    bash: {
      assistant_auto_backgrounded: true,
    },
  });
});

test("buildToolResultFields preserves Bash timeout and cwd metadata", () => {
  const base = createToolCall("tc-bash-timeout", "Bash", {
    command: "npm run watch",
  });
  const fields = buildToolResultFields(
    false,
    {
      stdout: "watching",
      stderr: "",
      interrupted: false,
      backgroundTaskId: "task-43",
      timedOutAfterMs: 10_000.9,
      backgroundCwdHint: " session cwd unchanged ",
      backgroundEndsWithFinalResponse: true,
    },
    base,
  );

  assert.equal(
    fields.raw_output,
    "watching\nCommand was auto-backgrounded after 10,000 ms with ID: task-43.",
  );
  assert.deepEqual(fields.output_metadata, {
    bash: {
      timed_out_after_ms: 10_000,
      background_cwd_hint: "session cwd unchanged",
      background_ends_with_final_response: true,
    },
  });
});

test("buildToolResultFields rejects malformed Bash timeout metadata", () => {
  const base = createToolCall("tc-bash-invalid-timeout", "Bash", {
    command: "npm run watch",
  });
  const fields = buildToolResultFields(
    false,
    {
      stdout: "",
      stderr: "",
      interrupted: false,
      backgroundTaskId: "task-44",
      timedOutAfterMs: -1,
      backgroundCwdHint: " ",
    },
    base,
  );

  assert.equal(
    fields.raw_output,
    "Command is running in background with ID: task-44.",
  );
  assert.equal(fields.output_metadata, undefined);
});

test("buildToolResultFields maps structured ReadMcpResource output to typed resource content", () => {
  const base = createToolCall("tc-mcp", "ReadMcpResource", {
    server: "docs",
    uri: "file://manual.pdf",
  });
  const fields = buildToolResultFields(
    false,
    {
      contents: [
        {
          uri: "file://manual.pdf",
          mimeType: "application/pdf",
          text: "[Resource from docs at file://manual.pdf] Saved to C:\\tmp\\manual.pdf",
          blobSavedTo: "C:\\tmp\\manual.pdf",
        },
      ],
    },
    base,
    {
      result: {
        contents: [
          {
            uri: "file://manual.pdf",
            mimeType: "application/pdf",
            text: "[Resource from docs at file://manual.pdf] Saved to C:\\tmp\\manual.pdf",
            blobSavedTo: "C:\\tmp\\manual.pdf",
          },
        ],
      },
    },
  );

  assert.equal(fields.status, "completed");
  assert.deepEqual(fields.content, [
    {
      type: "mcp_resource",
      uri: "file://manual.pdf",
      mime_type: "application/pdf",
      text: "[Resource from docs at file://manual.pdf] Saved to C:\\tmp\\manual.pdf",
      blob_saved_to: "C:\\tmp\\manual.pdf",
    },
  ]);
});

test("buildToolResultFields restores ReadMcpResource blob paths from transcript JSON text", () => {
  const base = createToolCall("tc-mcp-history", "ReadMcpResource", {
    server: "docs",
    uri: "file://manual.pdf",
  });
  const transcriptJson = JSON.stringify({
    contents: [
      {
        uri: "file://manual.pdf",
        mimeType: "application/pdf",
        text: "[Resource from docs at file://manual.pdf] Saved to C:\\tmp\\manual.pdf",
        blobSavedTo: "C:\\tmp\\manual.pdf",
      },
    ],
  });
  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-mcp-history",
    content: transcriptJson,
  });

  assert.deepEqual(fields.content, [
    {
      type: "mcp_resource",
      uri: "file://manual.pdf",
      mime_type: "application/pdf",
      text: "[Resource from docs at file://manual.pdf] Saved to C:\\tmp\\manual.pdf",
      blob_saved_to: "C:\\tmp\\manual.pdf",
    },
  ]);
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
});

test("buildToolResultFields renders structured ReadMcpResourceDir listings", () => {
  const base = createToolCall("tc-mcp-dir", "ReadMcpResourceDir", {
    server: "docs",
    uri: "file://manuals/",
  });
  const fields = buildToolResultFields(
    false,
    {
      resources: [
        {
          name: "guide.md",
          uri: "file://manuals/guide.md",
          mimeType: "text/markdown",
        },
        {
          name: "images",
          uri: "file://manuals/images",
          mimeType: "inode/directory",
        },
        {
          name: "readme",
          uri: "file://manuals/readme",
        },
      ],
    },
    base,
  );

  const expected =
    "guide.md - file://manuals/guide.md (text/markdown)\n" +
    "images - file://manuals/images (directory)\n" +
    "readme - file://manuals/readme";
  assert.equal(fields.status, "completed");
  assert.equal(fields.raw_output, expected);
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: { type: "text", text: expected },
    },
  ]);
});

test("buildToolResultFields renders empty ReadMcpResourceDir listings", () => {
  const base = createToolCall("tc-mcp-dir-empty", "ReadMcpResourceDir", {
    server: "docs",
    uri: "file://empty/",
  });
  const fields = buildToolResultFields(false, { resources: [] }, base);

  assert.equal(fields.status, "completed");
  assert.equal(fields.raw_output, "No resources found.");
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: { type: "text", text: "No resources found." },
    },
  ]);
});

test("buildToolResultFields marks ReadMcpResourceDir error output as failed", () => {
  const base = createToolCall("tc-mcp-dir-error", "ReadMcpResourceDir", {
    server: "docs",
    uri: "file://missing/",
  });
  const fields = buildToolResultFields(
    false,
    {
      resources: [],
      error: "directory not found",
    },
    base,
  );

  assert.equal(fields.status, "failed");
  assert.equal(fields.raw_output, "Error: directory not found");
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: { type: "text", text: "Error: directory not found" },
    },
  ]);
});

test("buildToolResultFields parses ReadMcpResourceDir transcript JSON", () => {
  const base = createToolCall("tc-mcp-dir-history", "ReadMcpResourceDir", {
    server: "docs",
    uri: "file://manuals/",
  });
  const transcriptJson = JSON.stringify({
    resources: [
      {
        name: "api.json",
        uri: "file://manuals/api.json",
        mimeType: "application/json",
      },
    ],
  });
  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-mcp-dir-history",
    content: transcriptJson,
  });

  assert.equal(
    fields.raw_output,
    "api.json - file://manuals/api.json (application/json)",
  );
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: "api.json - file://manuals/api.json (application/json)",
      },
    },
  ]);
});

test("buildToolResultFields skips invalid ReadMcpResourceDir entries", () => {
  const base = createToolCall("tc-mcp-dir-invalid", "ReadMcpResourceDir", {
    server: "docs",
    uri: "file://manuals/",
  });
  const fields = buildToolResultFields(
    false,
    {
      resources: [
        { name: "missing-uri" },
        { uri: "file://manuals/missing-name" },
        null,
        {
          name: "valid.txt",
          uri: "file://manuals/valid.txt",
          mimeType: "text/plain",
        },
      ],
    },
    base,
  );

  assert.equal(fields.status, "completed");
  assert.equal(
    fields.raw_output,
    "valid.txt - file://manuals/valid.txt (text/plain)",
  );
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: "valid.txt - file://manuals/valid.txt (text/plain)",
      },
    },
  ]);
});

test("buildToolResultFields preserves WebFetch artifactRead only as metadata", () => {
  const base = createToolCall("tc-web-fetch-artifact", "WebFetch", {
    url: "https://artifact.local/dashboard",
  });
  const fields = buildToolResultFields(
    false,
    {
      bytes: 128,
      code: 200,
      codeText: "OK",
      durationMs: 42,
      result: "Artifact content summary",
      url: "https://artifact.local/dashboard",
      artifactRead: {
        slug: "dashboard",
        ver: "v3",
        seeded: false,
      },
    },
    base,
  );

  assert.equal(fields.raw_output, "Artifact content summary");
  assert.equal(fields.raw_output?.includes("artifactRead"), false);
  assert.deepEqual(fields.output_metadata, {
    web_fetch: {
      artifact_read: {
        slug: "dashboard",
        ver: "v3",
        seeded: false,
      },
    },
  });
});

test("buildToolResultFields preserves versionless WebFetch artifacts and rejects malformed metadata", () => {
  const base = createToolCall("tc-web-fetch-versionless", "WebFetch", {
    url: "https://artifact.local/dashboard",
  });
  assert.deepEqual(
    buildToolResultFields(
      false,
      { result: "Seeded", artifactRead: { slug: "dashboard", seeded: false } },
      base,
    ).output_metadata,
    { web_fetch: { artifact_read: { slug: "dashboard", seeded: false } } },
  );
  assert.deepEqual(
    buildToolResultFields(
      false,
      {
        result: "Unexpected seed value",
        artifactRead: { slug: "dashboard", ver: "v4", seeded: true },
      },
      base,
    ).output_metadata,
    { web_fetch: { artifact_read: { slug: "dashboard", ver: "v4" } } },
  );
  assert.equal(
    buildToolResultFields(
      false,
      { result: "Malformed", artifactRead: { slug: "", ver: "v4" } },
      base,
    ).output_metadata,
    undefined,
  );
});

test("buildToolResultFields marks only structured Skill background launches", () => {
  const skill = createToolCall("tc-skill-background", "Skill", {
    skill: "review",
  });
  assert.deepEqual(
    buildToolResultFields(false, "launched", skill, { background: true })
      .output_metadata,
    {
      skill: { background: true },
    },
  );
  assert.equal(
    buildToolResultFields(false, "complete", skill, { background: false })
      .output_metadata,
    undefined,
  );
  assert.equal(
    buildToolResultFields(false, "complete", skill, { background: "true" })
      .output_metadata,
    undefined,
  );

  const bash = createToolCall("tc-bash-coincidental-background", "Bash", {
    command: "echo ok",
  });
  assert.equal(
    buildToolResultFields(false, "ok", bash, { background: true })
      .output_metadata,
    undefined,
  );
});

test("buildToolResultFields suppresses successful inline Skill launch text", () => {
  const skill = createToolCall("tc-skill-inline", "Skill", {
    skill: "frontend-design:frontend-design",
  });
  const fields = buildToolResultFields(
    false,
    "Launching skill: frontend-design:frontend-design",
    skill,
    {
      success: true,
      commandName: "frontend-design:frontend-design",
      allowedTools: ["Read"],
    },
  );

  assert.equal(fields.status, "completed");
  assert.equal(fields.title, "Skill: Frontend Design");
  assert.equal(fields.raw_output, undefined);
  assert.equal(fields.content, undefined);
});

test("buildToolResultFields preserves forked Skill results and background metadata", () => {
  const skill = createToolCall("tc-skill-forked", "Skill", { skill: "review" });
  const fields = buildToolResultFields(
    false,
    "model-facing launch text",
    skill,
    {
      success: true,
      commandName: "code-review",
      status: "forked",
      agentId: "agent-1",
      result: "Review worker launched",
      background: true,
    },
  );

  assert.equal(fields.title, "Skill: Code Review");
  assert.equal(fields.raw_output, "Review worker launched");
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: { type: "text", text: "Review worker launched" },
    },
  ]);
  assert.deepEqual(fields.output_metadata, { skill: { background: true } });
});

test("buildToolResultFields falls back losslessly for malformed Skill results", () => {
  const skill = createToolCall("tc-skill-malformed", "Skill", {
    skill: "review",
  });
  const fields = buildToolResultFields(
    false,
    "Launching skill: review",
    skill,
    {
      success: true,
      commandName: "review",
      status: "forked",
      result: "missing agent ID",
    },
  );

  assert.equal(fields.title, undefined);
  assert.equal(fields.raw_output, "Launching skill: review");
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: { type: "text", text: "Launching skill: review" },
    },
  ]);
});

test("buildToolResultFields preserves Agent resolvedModel metadata", () => {
  const base = createToolCall("tc-agent-model", "Agent", {
    description: "review changes",
    prompt: "Review the branch",
  });
  const fields = buildToolResultFields(
    false,
    {
      agentId: "agent-1",
      agentType: "reviewer",
      resolvedModel: "claude-sonnet-4-7",
      content: [{ type: "text", text: "Done" }],
      totalToolUseCount: 1,
      totalDurationMs: 100,
      totalTokens: 25,
      usage: {
        input_tokens: 10,
        output_tokens: 15,
        cache_creation_input_tokens: null,
        cache_read_input_tokens: null,
        server_tool_use: null,
        service_tier: null,
        cache_creation: null,
      },
      status: "completed",
      prompt: "Review the branch",
    },
    base,
  );

  assert.equal(fields.title, "Agent: reviewer");
  assert.deepEqual(fields.output_metadata, {
    agent: {
      resolved_model: "claude-sonnet-4-7",
    },
  });
});

test("buildToolResultFields preserves ordered Agent modelsUsed metadata", () => {
  const base = createToolCall("tc-agent-model-route", "Agent", {
    description: "review changes",
    prompt: "Review the branch",
    model: "opus",
  });
  const fields = buildToolResultFields(
    false,
    {
      agentId: "agent-1",
      resolvedModel: " claude-sonnet-4-7 ",
      modelsUsed: [
        " claude-opus-4-8 ",
        "",
        "claude-opus-4-8",
        42,
        "claude-sonnet-4-7",
      ],
      content: [{ type: "text", text: "Done" }],
      status: "completed",
      prompt: "Review the branch",
      futureField: true,
    },
    base,
  );

  assert.deepEqual(fields.output_metadata, {
    agent: {
      resolved_model: "claude-sonnet-4-7",
      models_used: ["claude-opus-4-8", "claude-sonnet-4-7"],
    },
  });
});

test("buildToolResultFields preserves Agent modelsUsed independently of resolvedModel", () => {
  const base = createToolCall("tc-agent-background-route", "Agent", {
    description: "review changes",
    prompt: "Review the branch",
  });
  const fields = buildToolResultFields(
    false,
    {
      status: "async_launched",
      agentId: "agent-1",
      description: "Review changes",
      modelsUsed: ["claude-opus-4-8", "claude-sonnet-4-7"],
      prompt: "Review the branch",
      outputFile: "C:/tmp/agent.output",
    },
    base,
  );

  assert.deepEqual(fields.output_metadata, {
    agent: {
      models_used: ["claude-opus-4-8", "claude-sonnet-4-7"],
    },
  });
});

test("buildToolResultFields keeps Agent input name while preserving resolvedModel metadata", () => {
  const base = createToolCall("tc-agent-named-model", "Agent", {
    description: "review changes",
    prompt: "Review the branch",
    name: "review-worker",
    subagent_type: "general-purpose",
    model: "opus",
  });
  const fields = buildToolResultFields(
    false,
    {
      agentId: "agent-1",
      agentType: "general-purpose",
      resolvedModel: "claude-opus-4-8",
      content: [{ type: "text", text: "Done" }],
      status: "completed",
      prompt: "Review the branch",
    },
    base,
  );

  assert.equal(fields.title, undefined);
  assert.deepEqual(fields.output_metadata, {
    agent: {
      resolved_model: "claude-opus-4-8",
    },
  });
});

test("unwrapToolUseResult extracts error/content payload", () => {
  const parsed = unwrapToolUseResult({
    is_error: true,
    content: [{ text: "failure output" }],
  });
  assert.equal(parsed.isError, true);
  assert.deepEqual(parsed.content, [{ text: "failure output" }]);
});

test("unwrapToolUseResult exposes MCP wrapper content without rendering opaque metadata", () => {
  const parsed = unwrapToolUseResult({
    content: [{ type: "text", text: "visible MCP output" }],
    _meta: { privateTrace: "must-not-render" },
  });
  assert.equal(parsed.isError, false);
  assert.deepEqual(parsed.content, [
    { type: "text", text: "visible MCP output" },
  ]);
  assert.equal(JSON.stringify(parsed).includes("privateTrace"), false);
});

test("buildToolResultFields replaces a structured Read image result with its filename", () => {
  const base = createToolCall("tc-image-read", "Read", {
    file_path: "C:\\work\\captures\\screen.png",
  });
  const fields = buildToolResultFields(
    false,
    [
      {
        type: "image",
        source: {
          type: "base64",
          media_type: "image/png",
          data: "raw-content-image-data",
        },
      },
    ],
    base,
    {
      type: "image",
      file: {
        base64: "structured-image-data",
        type: "image/png",
      },
    },
  );

  assert.equal(base.title, "Read C:\\work\\captures\\screen.png");
  assert.deepEqual(fields, {
    status: "completed",
    raw_output: "Viewed Image screen.png",
    content: [
      {
        type: "content",
        content: { type: "text", text: "Viewed Image screen.png" },
      },
    ],
  });
  assert.equal(
    JSON.stringify(fields).includes("raw-content-image-data"),
    false,
  );
  assert.equal(JSON.stringify(fields).includes("structured-image-data"), false);
});

test("buildToolResultFields recognizes Read image content blocks without structured output", () => {
  const base = createToolCall("tc-image-block-read", "Read", {
    file_path: "assets/capture.unknown",
  });
  const fields = buildToolResultFields(
    false,
    [
      {
        type: "image",
        source: {
          type: "base64",
          media_type: "image/webp",
          data: "compatibility-image-data",
        },
      },
    ],
    base,
  );

  assert.equal(base.title, "Read assets/capture.unknown");
  assert.equal(fields.raw_output, "Viewed Image capture.unknown");
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: { type: "text", text: "Viewed Image capture.unknown" },
    },
  ]);
  assert.equal(
    JSON.stringify(fields).includes("compatibility-image-data"),
    false,
  );
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

test("buildToolResultFields renders file_unchanged Read results compactly", () => {
  const base = createToolCall("tc-read", "Read", { file_path: "src/main.rs" });
  const fields = buildToolResultFields(
    false,
    {
      type: "file_unchanged",
      file: { filePath: "src/main.rs" },
    },
    base,
    {
      result: {
        type: "file_unchanged",
        file: { filePath: "src/main.rs" },
      },
    },
  );

  assert.equal(fields.raw_output, "File unchanged: src/main.rs");
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: { type: "text", text: "File unchanged: src/main.rs" },
    },
  ]);
});

test("buildToolResultFields renders array-wrapped file_unchanged Read results compactly", () => {
  const base = createToolCall("tc-read", "Read", { file_path: "src/lib.rs" });
  const fields = buildToolResultFields(false, [], base, {
    result: [
      {
        type: "file_unchanged",
        file: { filePath: "src/lib.rs" },
      },
    ],
  });

  assert.equal(fields.raw_output, "File unchanged: src/lib.rs");
});

test("buildToolResultFields uses Agent output agentType as task title", () => {
  const base = createToolCall("tc-agent", "Agent", { prompt: "Review tests" });
  const fields = buildToolResultFields(
    false,
    {
      agentId: "agent-1",
      agentType: "reviewer",
      content: [{ type: "text", text: "Done" }],
      totalToolUseCount: 0,
      totalDurationMs: 10,
      totalTokens: 20,
      usage: {},
      status: "completed",
      prompt: "Review tests",
    },
    base,
  );

  assert.equal(fields.title, "Agent: reviewer");
});

test("buildToolResultFields reads array-wrapped Agent output agentType", () => {
  const base = createToolCall("tc-agent", "Agent", { prompt: "Review tests" });
  const fields = buildToolResultFields(false, [], base, {
    result: [
      {
        agentId: "agent-1",
        agentType: "planner",
        content: [{ type: "text", text: "Done" }],
        status: "completed",
      },
    ],
  });

  assert.equal(fields.title, "Agent: planner");
});

test("buildToolResultFields leaves worktree title unchanged on completed output", () => {
  const enterBase = createToolCall("tc-enter", "EnterWorktree", {
    name: "feature-auth",
  });
  const enterFields = buildToolResultFields(
    false,
    {
      message: "Entered worktree feature-auth",
      worktreeBranch: "feature-auth",
      worktreePath: "C:\\repo\\.worktrees\\feature-auth",
    },
    enterBase,
  );
  assert.equal(enterFields.title, undefined);

  const exitBase = createToolCall("tc-exit", "ExitWorktree", {
    action: "keep",
  });
  const exitFields = buildToolResultFields(
    false,
    {
      message: "Exited worktree feature-auth",
      worktreePath: "C:\\repo\\.worktrees\\feature-auth",
    },
    exitBase,
  );
  assert.equal(exitFields.title, undefined);
});

test("buildToolResultFields renders worktree location without raw JSON", () => {
  const enterBase = createToolCall("tc-enter", "EnterWorktree", {
    name: "feature-auth",
  });
  const enterFields = buildToolResultFields(
    false,
    {
      message: "Entered worktree feature-auth",
      worktreeBranch: "feature-auth",
      worktreePath: "C:\\repo\\.worktrees\\feature-auth",
    },
    enterBase,
  );
  assert.equal(enterFields.raw_output, "Branch: feature-auth");
  assert.deepEqual(enterFields.content, [
    {
      type: "content",
      content: { type: "text", text: "Branch: feature-auth" },
    },
  ]);

  const exitBase = createToolCall("tc-exit", "ExitWorktree", {
    action: "keep",
  });
  const exitFields = buildToolResultFields(
    false,
    {
      message: "Exited worktree feature-auth",
      worktreePath: "C:\\repo\\.worktrees\\feature-auth",
    },
    exitBase,
  );
  assert.equal(
    exitFields.raw_output,
    "Path: C:\\repo\\.worktrees\\feature-auth",
  );
  assert.deepEqual(exitFields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: "Path: C:\\repo\\.worktrees\\feature-auth",
      },
    },
  ]);
});

test("buildToolResultFields renders cron outputs as structured text without raw JSON", () => {
  const createBase = createToolCall("tc-cron-create", "CronCreate", {
    cron: "30 9 * * 1",
    prompt: "Send weekly status",
  });
  const createFields = buildToolResultFields(
    false,
    {
      id: "schedule-1",
      humanSchedule: "every Monday at 09:30",
      recurring: true,
      durable: false,
    },
    createBase,
  );
  assert.equal(
    createFields.raw_output,
    "Schedule ID: schedule-1\nSchedule: Every Monday at 09:30\nRecurring: yes\nDurable: no",
  );
  assert.deepEqual(createFields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: "Schedule ID: schedule-1\nSchedule: Every Monday at 09:30\nRecurring: yes\nDurable: no",
      },
    },
  ]);
  assert.equal(createFields.raw_output?.includes("{"), false);

  const deleteBase = createToolCall("tc-cron-delete", "CronDelete", {
    id: "schedule-1",
  });
  const deleteFields = buildToolResultFields(
    false,
    { id: "schedule-1" },
    deleteBase,
  );
  assert.equal(deleteFields.raw_output, "Schedule ID: schedule-1");

  const listBase = createToolCall("tc-cron-list", "CronList", {});
  const listFields = buildToolResultFields(false, { jobs: [] }, listBase);
  assert.equal(listFields.raw_output, "Jobs: none");

  const singleListFields = buildToolResultFields(
    false,
    {
      jobs: [
        {
          id: "schedule-2",
          cron: "7 * * * *",
          humanSchedule: "Every hour at :07",
          prompt: "Send hourly tick",
          recurring: true,
          durable: false,
        },
      ],
    },
    listBase,
  );
  assert.equal(
    singleListFields.raw_output,
    "Schedule ID: schedule-2\nCron: 7 * * * *\nSchedule: Every hour at minute 07\nPrompt: Send hourly tick\nRecurring: yes\nDurable: no",
  );
});

test("buildToolResultFields preserves full CronList prompt from transcript JSON", () => {
  const base = createToolCall("tc-cron-list-history", "CronList", {});
  const fullPrompt = `Review the branch and write a status update. ${"Keep every detail. ".repeat(80)}END`;
  const transcriptJson = JSON.stringify({
    jobs: [
      {
        id: "schedule-long",
        cron: "*/5 * * * *",
        humanSchedule: "every 5 minutes",
        prompt: fullPrompt,
        recurring: false,
        durable: true,
      },
    ],
  });

  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-cron-list-history",
    content: transcriptJson,
  });

  assert.equal(fields.raw_output?.includes(fullPrompt), true);
  assert.equal(fields.raw_output?.includes("END"), true);
  assert.equal(fields.raw_output?.includes('"jobs"'), false);
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: `Schedule ID: schedule-long\nCron: */5 * * * *\nSchedule: Every 5 minutes\nPrompt: ${fullPrompt}\nRecurring: no\nDurable: yes`,
      },
    },
  ]);
});

test("buildToolResultFields renders readable cron schedule text from common cron expressions", () => {
  const base = createToolCall("tc-cron-readable", "CronList", {});
  const fields = buildToolResultFields(
    false,
    {
      jobs: [
        {
          id: "every-minute",
          cron: "* * * * *",
          prompt: "minute",
          recurring: true,
        },
        {
          id: "every-five-minutes",
          cron: "*/5 * * * *",
          prompt: "minutes",
          recurring: true,
        },
        {
          id: "hourly-minute",
          cron: "7 * * * *",
          humanSchedule: "Every hour at :07",
          prompt: "hourly",
          recurring: true,
        },
        {
          id: "every-two-hours",
          cron: "0 */2 * * *",
          prompt: "hours",
          recurring: true,
        },
        { id: "daily", cron: "30 9 * * *", prompt: "daily", recurring: true },
        { id: "weekly", cron: "30 9 * * 1", prompt: "weekly", recurring: true },
        {
          id: "monthly",
          cron: "30 9 15 * *",
          prompt: "monthly",
          recurring: true,
        },
        {
          id: "yearly",
          cron: "30 9 15 6 *",
          prompt: "yearly",
          recurring: true,
        },
        {
          id: "complex",
          cron: "0 9 1 * 1",
          prompt: "complex",
          recurring: true,
        },
      ],
    },
    base,
  );

  assert.equal(fields.raw_output?.includes("Cron: 7 * * * *"), false);
  assert.equal(fields.raw_output?.includes("Recurring:"), false);
  assert.equal(fields.raw_output?.includes("Durable:"), false);
  assert.equal(fields.raw_output?.includes("Schedule: Every minute"), true);
  assert.equal(fields.raw_output?.includes("Schedule: Every 5 minutes"), true);
  assert.equal(
    fields.raw_output?.includes("Schedule: Every hour at minute 07"),
    true,
  );
  assert.equal(
    fields.raw_output?.includes("Schedule: Every 2 hours on the hour"),
    true,
  );
  assert.equal(
    fields.raw_output?.includes("Schedule: Every day at 09:30"),
    true,
  );
  assert.equal(
    fields.raw_output?.includes("Schedule: Every Monday at 09:30"),
    true,
  );
  assert.equal(
    fields.raw_output?.includes("Schedule: Every month on day 15 at 09:30"),
    true,
  );
  assert.equal(
    fields.raw_output?.includes("Schedule: Every June 15 at 09:30"),
    true,
  );
  assert.equal(fields.raw_output?.includes("Cron: 0 9 1 * 1"), true);
  assert.equal(fields.raw_output?.split("__cron_list_job_divider__").length, 9);
});

test("buildToolResultFields renders ScheduleWakeup output as structured text", () => {
  const base = createToolCall("tc-wakeup", "ScheduleWakeup", {
    delaySeconds: 30,
    reason: "Retry after runtime clamp",
    prompt: "/loop keep checking",
  });
  const fields = buildToolResultFields(
    false,
    {
      scheduledFor: 1_779_990_000_000,
      clampedDelaySeconds: 90,
      wasClamped: true,
    },
    base,
  );

  assert.match(
    fields.raw_output ?? "",
    /^Scheduled for: \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} local\nActual delay: 1m 30s\nClamped: yes$/,
  );
  assert.equal(fields.raw_output?.includes("{"), false);
  assert.equal(fields.raw_output?.includes("1779990000000"), false);
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: fields.raw_output,
      },
    },
  ]);
});

test("buildToolResultFields parses ScheduleWakeup transcript JSON", () => {
  const base = createToolCall("tc-wakeup-history", "ScheduleWakeup", {
    delaySeconds: 3600,
    reason: "Wake at the next loop interval",
    prompt: "/loop continue",
  });
  const transcriptJson = JSON.stringify({
    scheduledFor: 1_779_990_000_000,
    clampedDelaySeconds: 3600,
    wasClamped: false,
  });

  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-wakeup-history",
    content: transcriptJson,
  });

  assert.match(fields.raw_output ?? "", /Actual delay: 1h\nClamped: no$/);
  assert.equal(fields.raw_output?.includes('"scheduledFor"'), false);
});

test("buildToolResultFields renders PushNotification output as structured text", () => {
  const base = createToolCall("tc-push-notification", "PushNotification", {
    message: "Build finished",
    status: "proactive",
  });
  const fields = buildToolResultFields(
    false,
    {
      message: "Build finished",
      pushSent: false,
      localSent: true,
      disabledReason: "config_off",
      idleSec: 90,
      hasFocus: false,
      sentAt: "2026-06-05T12:34:56.000Z",
    },
    base,
  );

  assert.match(
    fields.raw_output ?? "",
    /^Push sent: no\nLocal sent: yes\nDisabled reason: notifications disabled\nIdle time: 1m 30s\nApp focused: no\nSent at: \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} local$/,
  );
  assert.equal(fields.raw_output?.includes("Result:"), false);
  assert.equal(fields.raw_output?.includes("{"), false);
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: fields.raw_output,
      },
    },
  ]);
});

test("buildToolResultFields parses PushNotification transcript JSON", () => {
  const base = createToolCall(
    "tc-push-notification-history",
    "PushNotification",
    {
      message: "Deploy completed",
      status: "proactive",
    },
  );
  const transcriptJson = JSON.stringify({
    message: "Notification queued",
    pushSent: true,
    localSent: false,
    disabledReason: "no_transport",
    idleSec: 3600,
    hasFocus: true,
    sentAt: "not an iso timestamp",
  });

  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-push-notification-history",
    content: transcriptJson,
  });

  assert.equal(
    fields.raw_output,
    "Result: Notification queued\nPush sent: yes\nLocal sent: no\nDisabled reason: no notification transport\nIdle time: 1h\nApp focused: yes\nSent at: not an iso timestamp",
  );
  assert.equal(fields.raw_output?.includes('"pushSent"'), false);
});

test("buildToolResultFields preserves RemoteTrigger response alongside its summary", () => {
  const base = createToolCall("tc-remote-trigger", "RemoteTrigger", {
    action: "run",
    trigger_id: "deploy-prod",
  });
  const fields = buildToolResultFields(
    false,
    {
      status: 200,
      json: '{\n  "ok": true,\n  "run_id": "run-1"\n}',
      summary: "Trigger completed",
    },
    base,
  );

  assert.equal(fields.status, "completed");
  assert.equal(
    fields.raw_output,
    'Status: 200\nSummary: Trigger completed\nResponse: {"ok":true,"run_id":"run-1"}',
  );
  assert.equal(fields.raw_output?.includes("run_id"), true);
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: 'Status: 200\nSummary: Trigger completed\nResponse: {"ok":true,"run_id":"run-1"}',
      },
    },
  ]);
});

test("buildToolResultFields renders RemoteTrigger response when summary is absent", () => {
  const base = createToolCall("tc-remote-trigger-response", "RemoteTrigger", {
    action: "get",
    trigger_id: "deploy-prod",
  });
  const fields = buildToolResultFields(
    false,
    {
      status: 200,
      json: '{\n  "ok": true,\n  "trigger_id": "deploy-prod"\n}',
    },
    base,
  );

  assert.equal(fields.status, "completed");
  assert.equal(
    fields.raw_output,
    'Status: 200\nResponse: {"ok":true,"trigger_id":"deploy-prod"}',
  );
  assert.equal(fields.raw_output?.includes('"json"'), false);
});

test("buildToolResultFields marks RemoteTrigger 4xx output failed", () => {
  const base = createToolCall("tc-remote-trigger-error", "RemoteTrigger", {
    action: "run",
    trigger_id: "missing-trigger",
  });
  const fields = buildToolResultFields(
    false,
    {
      status: 404,
      json: '{"error":"not_found"}',
      summary: "Trigger not found",
    },
    base,
  );

  assert.equal(fields.status, "failed");
  assert.equal(
    fields.raw_output,
    'Status: 404\nSummary: Trigger not found\nResponse: {"error":"not_found"}',
  );
});

test("buildToolResultFields parses RemoteTrigger transcript JSON", () => {
  const base = createToolCall("tc-remote-trigger-history", "RemoteTrigger", {
    action: "get",
    trigger_id: "deploy-prod",
  });
  const transcriptJson = JSON.stringify({
    status: 200,
    json: '{\n  "enabled": true,\n  "name": "Deploy prod"\n}',
  });

  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-remote-trigger-history",
    content: transcriptJson,
  });

  assert.equal(
    fields.raw_output,
    'Status: 200\nResponse: {"enabled":true,"name":"Deploy prod"}',
  );
  assert.equal(fields.raw_output?.includes('"json"'), false);
});

test("buildToolResultFields renders REPL output as structured text without raw JSON", () => {
  const base = createToolCall("tc-repl", "REPL", {
    code: "await main()",
    description: "Run main function",
  });
  const fields = buildToolResultFields(
    false,
    {
      code: "await main()",
      stdout: "done",
      stderr: "warning",
      result: { ok: true },
      registeredTools: ["fetchDocs", "parse"],
      images: [
        { base64: "image-one-base64", mediaType: "image/png" },
        { base64: "image-two-base64", mediaType: "image/png" },
      ],
      documents: [{ base64: "document-base64" }],
    },
    base,
  );

  assert.equal(fields.status, "completed");
  assert.equal(
    fields.raw_output,
    'Stdout: done\nStderr: warning\nResult: {"ok":true}\nRegistered tools: fetchDocs, parse\nImages: 2\nDocuments: 1',
  );
  assert.equal(fields.raw_output?.includes("await main()"), false);
  assert.equal(fields.raw_output?.includes("image-one-base64"), false);
  assert.equal(fields.raw_output?.includes("document-base64"), false);
  assert.equal(fields.raw_output?.includes('{"code"'), false);
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: fields.raw_output,
      },
    },
  ]);
});

test("buildToolResultFields marks REPL error output failed", () => {
  const base = createToolCall("tc-repl-error", "REPL", {
    code: "throw new Error('boom')",
  });
  const fields = buildToolResultFields(
    false,
    {
      code: "throw new Error('boom')",
      error: "boom",
      stdout: "",
      stderr: "stack trace",
      result: {},
    },
    base,
  );

  assert.equal(fields.status, "failed");
  assert.equal(fields.raw_output, "Error: boom\nStderr: stack trace");
});

test("buildToolResultFields parses REPL transcript JSON", () => {
  const base = createToolCall("tc-repl-history", "REPL", {
    code: "await load()",
  });
  const transcriptJson = JSON.stringify({
    code: "await load()",
    stdout: "loaded",
    stderr: "",
    result: { count: 2 },
    registeredTools: ["lookup"],
    images: [{ base64: "hidden-image", mediaType: "image/png" }],
    documents: [{ base64: "hidden-document" }, { base64: "hidden-document-2" }],
  });

  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-repl-history",
    content: transcriptJson,
  });

  assert.equal(
    fields.raw_output,
    'Stdout: loaded\nResult: {"count":2}\nRegistered tools: lookup\nImages: 1\nDocuments: 2',
  );
  assert.equal(fields.raw_output?.includes('"code"'), false);
  assert.equal(fields.raw_output?.includes("hidden-image"), false);
  assert.equal(fields.raw_output?.includes("hidden-document"), false);
});

test("buildToolResultFields renders Monitor launch output as structured text", () => {
  const base = createToolCall("tc-monitor", "Monitor", {
    description: "watch deploy logs",
    timeout_ms: 30000,
    persistent: false,
    command: "tail -f deploy.log",
  });
  const fields = buildToolResultFields(
    false,
    { taskId: "monitor-1", timeoutMs: 30000, persistent: false },
    base,
  );

  assert.equal(fields.status, "in_progress");
  assert.equal(
    fields.raw_output,
    "Task ID: monitor-1\nPersistent: no\nTimeout: 30s",
  );
  assert.equal(fields.raw_output?.includes("{"), false);
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: fields.raw_output,
      },
    },
  ]);
});

test("buildToolResultFields renders Workflow launch output as structured text", () => {
  const base = createToolCall("tc-workflow", "Workflow", {
    name: "spec",
    args: { topic: "rendering" },
  });
  const fields = buildToolResultFields(
    false,
    {
      status: "async_launched",
      taskId: "workflow-1",
      taskType: "local_workflow",
      workflowName: "spec",
      runId: "run-1",
      summary: "Workflow started",
      transcriptDir: "C:/tmp/transcripts",
      scriptPath: "C:/tmp/workflow.js",
      warning: "branch diverged",
    },
    base,
  );

  assert.equal(fields.status, "in_progress");
  assert.equal(
    fields.raw_output,
    "Status: async launched\nTask ID: workflow-1\nTask type: local_workflow\nWorkflow name: spec\nRun ID: run-1\nSummary: Workflow started\nTranscript dir: C:/tmp/transcripts\nScript path: C:/tmp/workflow.js\nWarning: branch diverged",
  );
  assert.equal(fields.raw_output?.includes("{"), false);
  assert.equal(fields.raw_output?.includes('"status"'), false);
});

test("buildToolResultFields marks Workflow output with error as failed", () => {
  const base = createToolCall("tc-workflow-error", "Workflow", {
    script: "bad workflow script",
  });
  const fields = buildToolResultFields(
    false,
    {
      status: "async_launched",
      taskId: "workflow-err",
      error: "Syntax check failed",
    },
    base,
  );

  assert.equal(fields.status, "failed");
  assert.equal(
    fields.raw_output,
    "Status: async launched\nTask ID: workflow-err\nError: Syntax check failed",
  );
  assert.equal(fields.raw_output?.includes("bad workflow script"), false);
});

test("buildToolResultFields parses Workflow transcript JSON", () => {
  const base = createToolCall("tc-workflow-history", "Workflow", {
    name: "remote-spec",
  });
  const transcriptJson = JSON.stringify({
    status: "remote_launched",
    taskId: "workflow-remote",
    sessionUrl: "https://claude.ai/session/remote",
  });

  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-workflow-history",
    content: transcriptJson,
  });

  assert.equal(
    fields.raw_output,
    "Status: remote launched\nTask ID: workflow-remote\nSession URL: https://claude.ai/session/remote",
  );
  assert.equal(fields.raw_output?.includes('"taskId"'), false);
});

test("buildToolResultFields suppresses EnterPlanMode structured output body", () => {
  const base = createToolCall("tc-enter-plan-mode", "EnterPlanMode", {});
  const fields = buildToolResultFields(
    false,
    { message: "Plan mode entered" },
    base,
  );

  assert.equal(fields.status, "completed");
  assert.equal(fields.raw_output, undefined);
  assert.equal(fields.content, undefined);
});

test("buildToolResultFields suppresses EnterPlanMode transcript JSON body", () => {
  const base = createToolCall(
    "tc-enter-plan-mode-history",
    "EnterPlanMode",
    {},
  );
  const transcriptJson = JSON.stringify({ message: "Entered plan mode" });

  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-enter-plan-mode-history",
    content: transcriptJson,
  });

  assert.equal(fields.status, "completed");
  assert.equal(fields.raw_output, undefined);
  assert.equal(fields.content, undefined);
});

test("buildToolResultFields ignores removed TodoWrite verification metadata", () => {
  const base = createToolCall("tc-todo", "TodoWrite", {
    todos: [
      {
        content: "Verify changes",
        status: "pending",
        activeForm: "Verifying changes",
      },
    ],
  });
  const fields = buildToolResultFields(
    false,
    [{ text: "Todos have been modified successfully." }],
    base,
    {
      data: {
        oldTodos: [],
        newTodos: [],
        verificationNudgeNeeded: true,
      },
    },
  );

  assert.equal(fields.output_metadata, undefined);
});
