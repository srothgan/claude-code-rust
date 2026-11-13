import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import type {
  SDKMessage,
  SessionMessage,
} from "@anthropic-ai/claude-agent-sdk";
import type { CronCreateOutput } from "@anthropic-ai/claude-agent-sdk/sdk-tools";
import {
  buildToolResultFields,
  createToolCall,
  handleSdkMessage,
  mapSessionMessagesToUpdates,
} from "../bridge.js";
import { replaceProtocolEventWriter } from "./events.js";
import type { SessionState } from "./session_lifecycle.js";
import type {
  BridgeEvent,
  Json,
  SessionUpdate,
  ToolResultPresentation,
} from "../types.js";

type Fixture = {
  name: string;
  tool: string;
  input: Record<string, Json>;
  result: unknown;
  presentation: ToolResultPresentation;
};
const fixtures: Fixture[] = JSON.parse(
  readFileSync(
    new URL("../../../tests/fixtures/tool-result-times.json", import.meta.url),
    "utf8",
  ),
);
const legacyQueue: Fixture = {
  name: "legacy-queue",
  tool: "ReadNotifications",
  input: {},
  result: {
    notifications: [
      {
        notification_id: "legacy-delivery",
        content: "Older delivery without observed times",
      },
    ],
    remaining: 0,
  },
  presentation: {
    sections: [
      { fields: [{ label: "Remaining", value: { type: "text", value: "0" } }] },
      {
        fields: [
          {
            label: "Notification ID",
            value: { type: "text", value: "legacy-delivery" },
          },
        ],
        text: "Older delivery without observed times",
      },
    ],
  },
};
const timingFixtures = [...fixtures, legacyQueue];
type Section = ToolResultPresentation["sections"][number];

function field(
  section: Section,
  label: string,
): Section["fields"][number]["value"] {
  const matching = section.fields.filter(
    (candidate) => candidate.label === label,
  );
  assert.equal(matching.length, 1, `one ${label} field at the addressed item`);
  return matching[0].value;
}

function sectionFor(
  presentation: ToolResultPresentation | undefined,
  label: string,
  identity: string,
): Section {
  assert.ok(presentation, `presentation for ${identity}`);
  const matching = presentation.sections.filter((section) =>
    section.fields.some(
      (candidate) =>
        candidate.label === label &&
        candidate.value.type === "text" &&
        candidate.value.value === identity,
    ),
  );
  assert.equal(matching.length, 1, `one section for ${label} ${identity}`);
  return matching[0];
}

function assertItemTimes(
  actual: ToolResultPresentation | undefined,
  fixture: Fixture,
): void {
  assert.ok(actual, fixture.name);
  for (const expected of fixture.presentation.sections) {
    const identity = expected.fields.find((candidate) =>
      ["Schedule ID", "Notification ID", "Remaining"].includes(candidate.label),
    );
    assert.ok(
      identity?.value.type === "text",
      "independent fixture item identity",
    );
    const section = sectionFor(actual, identity.label, identity.value.value);
    for (const label of ["Prompt", "Remaining"]) {
      const expectedField = expected.fields.find(
        (candidate) => candidate.label === label,
      );
      if (expectedField)
        assert.deepEqual(
          field(section, label),
          expectedField.value,
          `${fixture.name}: ${label}`,
        );
    }
    const timestamps = (value: Section) =>
      value.fields
        .filter((candidate) => candidate.value.type === "timestamp")
        .map((candidate) => [candidate.label, candidate.value.value])
        .sort(([left], [right]) => left.localeCompare(right));
    assert.deepEqual(
      timestamps(section),
      timestamps(expected),
      `${fixture.name}: timestamps belong to ${identity.value.value}`,
    );
    if (expected.text !== undefined)
      assert.equal(
        section.text,
        expected.text,
        "original delivery content stays with its identity and times",
      );
  }
}

function resultPresentation(
  updates: SessionUpdate[],
  id: string,
): ToolResultPresentation | undefined {
  const results = updates.filter(
    (update) =>
      update.type === "tool_call_update" &&
      update.tool_call_update.tool_call_id === id &&
      update.tool_call_update.fields.status === "completed",
  );
  assert.equal(results.length, 1, `one completed result for tool ${id}`);
  const update = results[0];
  assert.ok(update.type === "tool_call_update");
  return update.tool_call_update.fields.output_metadata?.presentation;
}

function sessionFixture(sessionId: string): SessionState {
  return {
    sessionId,
    cwd: process.cwd(),
    connected: true,
    closing: false,
    availableModels: [],
    mode: null,
    supportedModeIds: [],
    runtimeUnavailableModeIds: [],
    supportsBypassPermissionsMode: false,
    fastModeState: "off",
    connectEvent: "connected",
    toolCalls: new Map(),
    hiddenToolUseIds: new Set(),
    tasksById: new Map(),
    taskOrder: [],
    taskToolUseIds: new Map(),
    taskIdsByToolUseId: new Map(),
    pendingPermissions: new Map(),
    pendingQuestions: new Map(),
    pendingUserDialogs: new Map(),
    pendingElicitations: new Map(),
    informationalDedupKeys: new Set(),
    knownConnectedMcpServers: new Set(),
    mcpStatusRevalidatedAt: new Map(),
    mcpAuthMonitors: new Map(),
    authHintSent: false,
  } as unknown as SessionState;
}

function toolMessages(fixture: Fixture): SessionMessage[] {
  return [
    {
      type: "assistant",
      uuid: `${fixture.name}-launch`,
      message: {
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: fixture.name,
            name: fixture.tool,
            input: fixture.input,
          },
        ],
      },
    },
    {
      type: "user",
      uuid: `${fixture.name}-result`,
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: fixture.name,
            content: JSON.stringify(fixture.result),
          },
        ],
      },
    },
  ] as unknown as SessionMessage[];
}

test("SDK scheduler and queue times stay with the addressed tool and item through live ingress and replay", () => {
  for (const fixture of timingFixtures) {
    const base = createToolCall(fixture.name, fixture.tool, fixture.input);
    for (const raw of [
      fixture.result,
      JSON.stringify(fixture.result),
      { data: fixture.result },
    ]) {
      const fields = buildToolResultFields(false, raw, base, raw);
      assert.equal(fields.status, "completed");
      assertItemTimes(fields.output_metadata?.presentation, fixture);
    }
  }

  const sessions = [
    sessionFixture("timing-session-A"),
    sessionFixture("timing-session-B"),
  ];
  const events: BridgeEvent[] = [];
  const restore = replaceProtocolEventWriter((line) => {
    events.push(JSON.parse(line));
  });
  const owner = (index: number) => sessions[index % sessions.length];
  const emit = (session: SessionState, message: SessionMessage) =>
    handleSdkMessage(session, {
      ...message,
      session_id: session.sessionId,
    } as unknown as SDKMessage);
  try {
    timingFixtures.forEach((fixture, index) => {
      emit(owner(index), toolMessages(fixture)[0]);
    });
    // All siblings exist before results arrive, in a different completion order.
    for (let index = timingFixtures.length - 1; index >= 0; index--) {
      const fixture = timingFixtures[index];
      const message = {
        ...toolMessages(fixture)[1],
        tool_use_result: fixture.result,
        message: {
          role: "user",
          content: [
            {
              type: "tool_result",
              tool_use_id: fixture.name,
              content: "Native summary without precise timestamps",
            },
          ],
        },
      };
      emit(owner(index), message);
    }
  } finally {
    restore();
  }
  timingFixtures.forEach((fixture, index) => {
    const session = owner(index);
    const updates = events.flatMap((event) =>
      event.event === "session_update" && event.session_id === session.sessionId
        ? [event.update]
        : [],
    );
    assertItemTimes(resultPresentation(updates, fixture.name), fixture);
    // Later sibling completions must not overwrite the already stored result.
    assertItemTimes(
      session.toolCalls.get(fixture.name)?.output_metadata?.presentation,
      fixture,
    );
  });

  const replay = mapSessionMessagesToUpdates(
    timingFixtures.flatMap(toolMessages),
  );
  for (const fixture of timingFixtures)
    assertItemTimes(resultPresentation(replay, fixture.name), fixture);
  assert.equal(
    replay.some((update) => update.type === "notification_update"),
    false,
    "saved queue reads never replay alerts",
  );
});

test("SDK next-run observations retain useful job data while rejecting invalid times and accepting epoch zero", () => {
  const native: CronCreateOutput = {
    id: "observed-job",
    humanSchedule: "Every minute",
    recurring: true,
  };
  const base = createToolCall("observed-tool", "CronCreate", {
    cron: "* * * * *",
    prompt: "Keep this job prompt",
  });
  for (const [nextFireAt, expected] of [
    ...[
      undefined,
      null,
      -1,
      Number.NaN,
      Number.POSITIVE_INFINITY,
      9e15,
      "1774747807123",
    ].map((value) => [value, undefined] as const),
    [0, "1970-01-01T00:00:00.000Z"],
    [1774747807123.7246, "2026-03-29T01:30:07.123Z"],
  ] as const) {
    const fields = buildToolResultFields(false, "", base, {
      ...native,
      nextFireAt,
    });
    assert.equal(fields.status, "completed");
    const section = sectionFor(
      fields.output_metadata?.presentation,
      "Schedule ID",
      "observed-job",
    );
    assert.deepEqual(field(section, "Prompt"), {
      type: "text",
      value: "Keep this job prompt",
    });
    const times = section.fields.filter(
      (candidate) => candidate.value.type === "timestamp",
    );
    assert.deepEqual(
      times,
      expected === undefined
        ? []
        : [
            {
              label: "Next run",
              value: { type: "timestamp", value: expected },
            },
          ],
      String(nextFireAt),
    );
  }
});

test("empty queue reads retain summary identity and time while failures preserve the original error", () => {
  const base = createToolCall("empty-drain", "ReadNotifications", {});
  const result = {
    notifications: [],
    remaining: 0,
    read_at: "2026-03-29T01:10:12Z",
  };
  const fields = buildToolResultFields(false, "", base, result);
  assert.equal(fields.status, "completed");
  const presentation = fields.output_metadata?.presentation;
  const summary = sectionFor(presentation, "Remaining", "0");
  assert.deepEqual(field(summary, "Read at"), {
    type: "timestamp",
    value: "2026-03-29T01:10:12Z",
  });
  assert.equal(
    presentation?.sections.some((section) =>
      section.fields.some((candidate) => candidate.label === "Notification ID"),
    ),
    false,
    "an empty read has no invented delivery",
  );
  const error = buildToolResultFields(true, "Queue read failed", base, result);
  assert.equal(error.status, "failed");
  assert.equal(error.raw_output, "Queue read failed");
  assert.equal(
    error.output_metadata?.presentation,
    undefined,
    "failure cannot masquerade as a successful empty read",
  );
});

test("resume reads the SDK saved structured results even when native tool prose omits their precise timestamps", () => {
  const directory = realpathSync(
    mkdtempSync(join(tmpdir(), "claude-rs-tool-times-")),
  );
  try {
    const project = join(directory, "project");
    const profile = join(directory, "profile");
    const transcriptDir = join(profile, "projects", "tool-times-fixture");
    mkdirSync(project);
    mkdirSync(transcriptDir, { recursive: true });
    const sessionId = "11111111-2222-4333-8444-555555555555";
    const records: Record<string, unknown>[] = [];
    let parentUuid: string | null = null;
    for (const fixture of timingFixtures) {
      for (const entry of [
        {
          type: "assistant",
          uuid: `${fixture.name}-launch`,
          message: {
            role: "assistant",
            content: [
              {
                type: "tool_use",
                id: fixture.name,
                name: fixture.tool,
                input: fixture.input,
              },
            ],
          },
        },
        {
          type: "user",
          uuid: `${fixture.name}-result`,
          toolUseResult: fixture.result,
          message: {
            role: "user",
            content: [
              {
                type: "tool_result",
                tool_use_id: fixture.name,
                content: "Native SDK summary without precise timestamps",
              },
            ],
          },
        },
      ]) {
        records.push({
          ...entry,
          parentUuid,
          sessionId,
          cwd: project,
          timestamp: "2026-10-18T12:00:00Z",
        });
        parentUuid = entry.uuid;
      }
    }
    records.push({
      type: "assistant",
      uuid: "final",
      parentUuid,
      sessionId,
      cwd: project,
      message: {
        role: "assistant",
        content: [{ type: "text", text: "Finished" }],
      },
    });
    const transcript = join(transcriptDir, `${sessionId}.jsonl`);
    const saved = `${records.map((record) => JSON.stringify(record)).join("\n")}\n`;
    writeFileSync(transcript, saved);
    const child = spawnSync(process.execPath, ["--input-type=module", "-"], {
      input: `import { getSessionTranscriptMessages, mapSessionMessagesToUpdates } from ${JSON.stringify(new URL("./history.js", import.meta.url).href)};
        console.log(JSON.stringify(mapSessionMessagesToUpdates(await getSessionTranscriptMessages(${JSON.stringify(sessionId)}))));`,
      encoding: "utf8",
      cwd: project,
      timeout: 30_000,
      windowsHide: true,
      env: {
        ...process.env,
        CLAUDE_CONFIG_DIR: profile,
        CLAUDE_CODE_PROJECT_DIR_NAME: "tool-times-fixture",
      },
    });
    assert.equal(
      child.error,
      undefined,
      `${child.error?.message ?? ""} ${child.stderr}`,
    );
    assert.equal(child.status, 0, `signal=${child.signal}: ${child.stderr}`);
    const updates = JSON.parse(child.stdout);
    for (const fixture of timingFixtures) {
      assertItemTimes(resultPresentation(updates, fixture.name), fixture);
    }
    assert.equal(
      updates.some(
        (update: { type: string }) => update.type === "notification_update",
      ),
      false,
    );
    assert.equal(
      readFileSync(transcript, "utf8"),
      saved,
      "display replay leaves SDK history intact",
    );
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
