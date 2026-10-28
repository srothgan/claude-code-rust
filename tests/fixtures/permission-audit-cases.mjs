import assert from "node:assert/strict";

export async function runPermissionAudit({
  session,
  targetOptions,
  peer,
  peerOptions,
  events,
  respond,
  mode,
}) {
  const sessionRule = {
    type: "addRules",
    rules: [{ toolName: "Write", ruleContent: "/owned/project/**" }],
    behavior: "allow",
    destination: "session",
  };
  const persistentRule = {
    type: "addRules",
    rules: [
      { toolName: "Bash", ruleContent: "git status:*" },
      { toolName: "Read", ruleContent: "/owned/shared/**" },
    ],
    behavior: "allow",
    destination: "projectSettings",
  };
  const cliRule = {
    type: "addRules",
    rules: [{ toolName: "Write", ruleContent: "/owned/cli/**" }],
    behavior: "allow",
    destination: "cliArg",
  };
  const localRule = {
    type: "addRules",
    rules: [{ toolName: "Bash", ruleContent: "npm test:*" }],
    behavior: "allow",
    destination: "localSettings",
  };
  const userRule = {
    type: "addRules",
    rules: [{ toolName: "Bash", ruleContent: "cargo check:*" }],
    behavior: "allow",
    destination: "userSettings",
  };
  const rows =
    mode === "outcomes"
      ? [
          { name: "once", outcome: { outcome: "selected", option_id: "allow_once" } },
          { name: "deny", outcome: { outcome: "selected", option_id: "reject_once" } },
          { name: "cancel", outcome: { outcome: "cancelled" } },
        ]
      : mode === "choices"
        ? [
            { name: "session", suggestions: [sessionRule] },
            { name: "persistent", suggestions: [persistentRule] },
            {
              name: "suppressed-persistent",
              suggestions: [userRule],
              suppressAlwaysAllowRule: true,
            },
            {
              name: "suppressed-mixed",
              suggestions: [cliRule, localRule],
              suppressAlwaysAllowRule: true,
            },
            { name: "absent" },
          ]
        : mode === "grants"
          ? [
              { name: "persistent", suggestions: [persistentRule], select: "allow_always" },
              { name: "session", suggestions: [sessionRule], select: "allow_session" },
              {
                name: "mixed-session",
                suggestions: [sessionRule, persistentRule],
                select: "allow_session",
              },
              {
                name: "mixed-persistent",
                suggestions: [sessionRule, persistentRule],
                select: "allow_always",
              },
              { name: "reachable-fallback", select: "allow_always" },
            ]
          : [
              {
                name: "suppressed-persistent",
                suggestions: [persistentRule],
                suppressAlwaysAllowRule: true,
                select: "allow_always",
              },
              {
                name: "session-only-persistent",
                suggestions: [sessionRule],
                select: "allow_always",
              },
              { name: "absent-session", select: "allow_session" },
              { name: "unknown-choice", select: "unknown-independent-option" },
            ];
  const results = [];
  let sequence = 0;
  async function start(owner, options, name, row = {}) {
    const toolId = `permission-${mode}-${name}-${++sequence}`;
    const input = { command: `printf '${name} independent bytes'`, cwd: "/owned/project" };
    const controller = new AbortController();
    const baseline = JSON.stringify({ input, suggestions: row.suggestions });
    const begin = events.length;
    const pending = options.canUseTool("Bash", input, {
      toolUseID: toolId,
      signal: controller.signal,
      suggestions: row.suggestions,
      suppressAlwaysAllowRule: row.suppressAlwaysAllowRule,
    });
    const request = events
      .slice(begin)
      .find(
        (event) =>
          event.event === "permission_request" &&
          event.session_id === owner.sessionId &&
          event.request?.tool_call?.tool_call_id === toolId,
      );
    assert.ok(request, "installed callback must publish the target request");
    return { owner, toolId, input, baseline, request, pending, controller, row };
  }
  async function finish(work, outcome) {
    const begin = events.length;
    await respond(work.owner.sessionId, work.toolId, outcome);
    const result = await work.pending;
    assert.equal(
      JSON.stringify({ input: work.input, suggestions: work.row.suggestions }),
      work.baseline,
      "native suggestion and input fixtures remain unchanged",
    );
    return {
      name: work.row.name,
      sessionId: work.owner.sessionId,
      toolId: work.toolId,
      input: work.input,
      request: work.request,
      result,
      events: events.slice(begin),
    };
  }
  for (const row of rows) {
    const work = await start(session, targetOptions, row.name, row);
    let sibling;
    try {
      if (mode === "outcomes" || mode === "invalid") {
        sibling = await start(peer, peerOptions, `${row.name}-peer`);
        let peerSettled = false;
        void sibling.pending.then(() => {
          peerSettled = true;
        });
        const result = await finish(
          work,
          row.outcome ?? { outcome: "selected", option_id: row.select ?? "allow_once" },
        );
        assert.equal(
          peerSettled,
          false,
          "target response cannot settle another session's native callback",
        );
        const afterFirst = events.length;
        await respond(session.sessionId, work.toolId, {
          outcome: "selected",
          option_id: "allow_always",
        });
        assert.deepEqual(
          events.slice(afterFirst),
          [],
          "duplicate response cannot publish another approval",
        );
        const peerResult = await finish(sibling, { outcome: "selected", option_id: "allow_once" });
        results.push({ ...result, peer: peerResult });
      } else
        results.push(
          await finish(
            work,
            row.outcome ?? { outcome: "selected", option_id: row.select ?? "allow_once" },
          ),
        );
      if (mode === "invalid") {
        const fresh = await start(session, targetOptions, `${row.name}-recovery`, {
          name: `${row.name}-recovery`,
        });
        results.at(-1).recovery = await finish(fresh, {
          outcome: "selected",
          option_id: "allow_once",
        });
      }
    } finally {
      work.controller.abort();
      sibling?.controller.abort();
      await Promise.allSettled([work.pending, sibling?.pending]);
    }
  }
  return results;
}
