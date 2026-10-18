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
import type { SessionMessage } from "@anthropic-ai/claude-agent-sdk";
import type {
  CronCreateOutput,
  CronListOutput,
  ReadNotificationsOutput,
} from "@anthropic-ai/claude-agent-sdk/sdk-tools";
import {
  buildToolResultFields,
  createToolCall,
  mapSessionMessagesToUpdates,
} from "../bridge.js";
import type { Json, ToolResultPresentation } from "../types.js";

type Fixture = {
  name: string;
  tool: string;
  input: Record<string, Json>;
  result: CronCreateOutput | CronListOutput | ReadNotificationsOutput;
  presentation: ToolResultPresentation;
};
const fixtures: Fixture[] = JSON.parse(
  readFileSync(
    new URL("../../../tests/fixtures/tool-result-times.json", import.meta.url),
    "utf8",
  ),
);

test("SDK scheduler and queue timestamps remain grouped with their original item through live and saved result normalization", () => {
  for (const fixture of fixtures) {
    const base = createToolCall(fixture.name, fixture.tool, fixture.input);
    for (const raw of [
      fixture.result,
      JSON.stringify(fixture.result),
      { data: fixture.result },
    ]) {
      const fields = buildToolResultFields(false, raw, base, raw);
      assert.equal(fields.status, "completed");
      assert.deepEqual(
        fields.output_metadata?.presentation,
        fixture.presentation,
        fixture.name,
      );
    }
    const messages = [
      {
        type: "assistant",
        uuid: "a",
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
        uuid: "u",
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
    const replay = mapSessionMessagesToUpdates(messages);
    const result = replay.find(
      (update) =>
        update.type === "tool_call_update" &&
        update.tool_call_update.fields.output_metadata?.presentation,
    );
    assert.ok(result?.type === "tool_call_update", fixture.name);
    assert.deepEqual(
      result.tool_call_update.fields.output_metadata?.presentation,
      fixture.presentation,
    );
    assert.equal(
      replay.some((update) => update.type === "notification_update"),
      false,
      "reading saved queue results must not emit another alert",
    );
  }
});

test("SDK next-run times are optional observations and invalid numeric values never create a timestamp", () => {
  const native: CronCreateOutput = {
    id: "job",
    humanSchedule: "Every minute",
    recurring: true,
  };
  const base = createToolCall("job", "CronCreate", { cron: "* * * * *" });
  for (const nextFireAt of [
    undefined,
    null,
    -1,
    Number.NaN,
    Number.POSITIVE_INFINITY,
    9e15,
    "1774747807123",
  ]) {
    const fields = buildToolResultFields(false, "", base, {
      ...native,
      nextFireAt,
    });
    const timestamps = fields.output_metadata?.presentation?.sections.flatMap(
      (section) =>
        section.fields.filter((field) => field.value.type === "timestamp"),
    );
    assert.deepEqual(timestamps, [], String(nextFireAt));
  }
  // SDK 0.3.302 emits fractional milliseconds for scheduler jitter. Date retains millisecond precision.
  const fractional = buildToolResultFields(false, "", base, {
    ...native,
    nextFireAt: 1774747807123.7246,
  });
  assert.ok(
    fractional.output_metadata?.presentation?.sections[0].fields.some(
      (field) =>
        field.value.type === "timestamp" &&
        field.value.value === "2026-03-29T01:30:07.123Z",
    ),
  );
  const list = fixtures[1].result as CronListOutput;
  assert.equal(typeof list.jobs[0].nextFireAt, "number");
  const drain = fixtures[2].result as ReadNotificationsOutput;
  assert.equal(drain.notifications[0].arrived_at, "2026-03-29T01:00:01.250Z");
  assert.equal(drain.read_at, "2026-03-29T02:10:12.345+01:00");
});

test("empty drains retain their read identity and failures retain their original error", () => {
  const base = createToolCall("drain", "ReadNotifications", {});
  const fields = buildToolResultFields(false, "", base, {
    notifications: [],
    remaining: 0,
    read_at: "2026-03-29T01:10:12Z",
  });
  assert.deepEqual(fields.output_metadata?.presentation?.sections[0].fields, [
    { label: "Remaining", value: { type: "text", value: "0" } },
    {
      label: "Read at",
      value: { type: "timestamp", value: "2026-03-29T01:10:12Z" },
    },
    { label: "Notifications", value: { type: "text", value: "none" } },
  ]);
  const error = buildToolResultFields(true, "Queue read failed", base);
  assert.equal(error.status, "failed");
  assert.equal(error.output_metadata?.presentation, undefined);
  assert.equal(error.raw_output, "Queue read failed");
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
    for (const fixture of fixtures) {
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
    const saved =
      `${records.map((record) => JSON.stringify(record)).join("\n")}\n`;
    writeFileSync(transcript, saved);
    const child = spawnSync(process.execPath, ["--input-type=module", "-"], {
      input: `import { getSessionTranscriptMessages, mapSessionMessagesToUpdates } from ${JSON.stringify(new URL("./history.js", import.meta.url).href)};
        console.log(JSON.stringify(mapSessionMessagesToUpdates(await getSessionTranscriptMessages(${JSON.stringify(sessionId)}))));`,
      encoding: "utf8",
      windowsHide: true,
      env: {
        ...process.env,
        CLAUDE_CONFIG_DIR: profile,
        CLAUDE_CODE_PROJECT_DIR_NAME: "tool-times-fixture",
      },
    });
    assert.equal(child.status, 0, child.stderr);
    const updates = JSON.parse(child.stdout);
    for (const fixture of fixtures) {
      const result = updates.find(
        (update: {
          type: string;
          tool_call_update?: { tool_call_id: string };
        }) =>
          update.type === "tool_call_update" &&
          update.tool_call_update?.tool_call_id === fixture.name,
      );
      assert.deepEqual(
        result?.tool_call_update.fields.output_metadata?.presentation,
        fixture.presentation,
        fixture.name,
      );
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
