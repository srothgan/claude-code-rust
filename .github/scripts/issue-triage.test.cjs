const assert = require("node:assert/strict");
const test = require("node:test");
const runIssueTriage = require("./issue-triage.cjs");

function fixture({ permission = "write", state = "open", triage = true, targetState = "open", targetPr = false } = {}) {
  const source = {
    number: 40, id: 90040, state, assignees: [{ login: "owner" }],
    labels: [{ name: "type: bug" }, { name: "area: tui" }, ...(triage ? [{ name: "status: triage" }] : [])],
  };
  const target = { number: 7, id: 812345, state: targetState, ...(targetPr ? { pull_request: {} } : {}) };
  const calls = [];
  const github = { rest: { repos: {
    async getCollaboratorPermissionLevel(args) {
      calls.push(["permission", args]);
      return { data: { permission } };
    },
  }, issues: {
    async get(args) {
      calls.push(["get", args]);
      const issue = args.issue_number === 40 ? source : args.issue_number === 7 ? target : undefined;
      if (!issue) throw Object.assign(new Error("not found"), { status: 404 });
      return { data: structuredClone(issue) };
    },
    async addLabels(args) {
      calls.push(["add", args]);
      for (const name of args.labels) {
        if (!source.labels.some(label => label.name === name)) source.labels.push({ name });
      }
    },
    async removeLabel(args) {
      calls.push(["remove", args]);
      source.labels = source.labels.filter(label => label.name !== args.name);
    },
    async update(args) {
      calls.push(["update", args]);
      Object.assign(source, { state: args.state, state_reason: args.state_reason, duplicate_issue_id: args.duplicate_issue_id });
    },
  } } };
  const context = {
    repo: { owner: "maintainer", repo: "project" },
    eventName: "issues",
    payload: { action: "opened", issue: { number: 40 }, comment: { body: "/triaged", user: { login: "reviewer", type: "User" } } },
  };
  return {
    github, context, source, target, calls,
    mutations: () => calls.filter(([method]) => ["add", "remove", "update"].includes(method)),
    run: () => runIssueTriage({ github, context }),
    command(body) { context.eventName = "issue_comment"; context.payload.action = "created"; context.payload.comment.body = body; },
  };
}

for (const action of ["opened", "reopened"]) {
  test(`${action} queues assessment without changing existing classification or assignment`, async () => {
    const f = fixture({ triage: false });
    f.context.payload.action = action;
    await f.run();
    assert.deepEqual(f.source.labels.map(label => label.name), ["type: bug", "area: tui", "status: triage"]);
    assert.deepEqual(f.source.assignees, [{ login: "owner" }]);
    await f.run();
    assert.equal(f.mutations().length, 1, "replayed event must not write another label");
  });
}

test("closure clears only triage and a delayed closure cannot clear a reopened issue", async () => {
  for (const state of ["closed", "open"]) {
    const f = fixture({ state });
    f.context.payload.action = "closed";
    await f.run();
    assert.deepEqual(f.source.labels.map(label => label.name), state === "closed" ? ["type: bug", "area: tui"] : ["type: bug", "area: tui", "status: triage"]);
  }
});

test("a delayed opened event cannot retriage an already closed issue", async () => {
  const f = fixture({ state: "closed", triage: false });
  await f.run();
  assert.deepEqual(f.mutations(), []);
});

test("triaged requires actual write access and never changes ownership or closes the issue", async () => {
  for (const permission of ["write", "admin", "read", "none"]) {
    const f = fixture({ permission });
    f.command("/triaged");
    f.context.payload.comment.author_association = "CONTRIBUTOR";
    await f.run();
    const allowed = ["write", "admin"].includes(permission);
    assert.equal(f.source.labels.some(label => label.name === "status: triage"), !allowed);
    assert.equal(f.source.state, "open");
    assert.deepEqual(f.source.assignees, [{ login: "owner" }]);
    assert.equal(f.calls.some(([method]) => method === "update"), false);
  }
});

test("untrusted users cannot close an issue with a valid duplicate command", async () => {
  const f = fixture({ permission: "read" });
  f.command("/duplicate #7");
  f.context.payload.comment.author_association = "MEMBER";
  await f.run();
  assert.deepEqual(f.mutations(), []);
  assert.equal(f.source.state, "open");
});

test("permission lookup failure cannot fall back to trusting author association", async () => {
  const f = fixture();
  f.command("/duplicate #7");
  f.context.payload.comment.author_association = "OWNER";
  f.github.rest.repos.getCollaboratorPermissionLevel = async () => { throw new Error("permission service unavailable"); };
  await assert.rejects(f.run(), /permission service unavailable/);
  assert.deepEqual(f.mutations(), []);
});

test("quoted commands, code, prose, malformed targets and edited comments do not run", async () => {
  for (const body of ["> /triaged", "`/triaged`", "```\n/duplicate #7\n```", "Example:\n/duplicate #7", "/duplicate #0", "/duplicate owner/other#7", "/duplicate #7; echo unsafe", "/duplicate #7\n/triaged"]) {
    const f = fixture();
    f.command(body);
    await f.run();
    assert.deepEqual(f.calls, [], body);
  }
  const f = fixture();
  f.command("/duplicate #7");
  f.context.payload.action = "edited";
  await f.run();
  assert.deepEqual(f.calls, []);
});

test("bot comments and PR threads make no API calls", async () => {
  for (const kind of ["bot", "pr"]) {
    const f = fixture();
    f.command("/duplicate #7");
    if (kind === "bot") f.context.payload.comment.user.type = "Bot";
    else f.context.payload.issue.pull_request = {};
    await f.run();
    assert.deepEqual(f.calls, []);
  }
});

test("duplicate closure records the canonical issue database ID and preserves other labels", async () => {
  for (const targetState of ["open", "closed"]) {
    const f = fixture({ targetState });
    f.command("/duplicate #7");
    await f.run();
    assert.equal(f.source.state, "closed");
    assert.equal(f.source.state_reason, "duplicate");
    assert.equal(f.source.duplicate_issue_id, 812345, "database ID must not be the issue number 7");
    assert.deepEqual(f.source.labels.map(label => label.name), ["type: bug", "area: tui"]);
    assert.deepEqual(f.calls.find(([method]) => method === "update")[1], {
      owner: "maintainer", repo: "project", issue_number: 40,
      state: "closed", state_reason: "duplicate", duplicate_issue_id: 812345,
      headers: { "X-GitHub-Api-Version": "2026-03-10" },
    });
    await f.run();
    assert.equal(f.calls.filter(([method]) => method === "update").length, 1);
  }
});

test("self, nonexistent, unsafe integer and PR targets leave the source untouched", async () => {
  for (const [body, targetPr] of [["/duplicate #40", false], ["/duplicate #999", false], ["/duplicate #9007199254740992", false], ["/duplicate #7", true]]) {
    const f = fixture({ targetPr });
    f.command(body);
    await assert.rejects(f.run());
    assert.deepEqual(f.mutations(), []);
    assert.equal(f.source.state, "open");
  }
});

test("a failed duplicate write does not remove triage or change classification", async () => {
  const f = fixture();
  f.command("/duplicate #7");
  f.github.rest.issues.update = async () => { throw new Error("write failed"); };
  await assert.rejects(f.run(), /write failed/);
  assert.equal(f.source.state, "open");
  assert.deepEqual(f.mutations(), []);
});

test("retry after successful closure repairs failed label cleanup without reclosing", async () => {
  const f = fixture();
  f.command("/duplicate #7");
  const remove = f.github.rest.issues.removeLabel;
  f.github.rest.issues.removeLabel = async () => { throw new Error("cleanup failed"); };
  await assert.rejects(f.run(), /cleanup failed/);
  assert.equal(f.source.state, "closed");
  f.github.rest.issues.removeLabel = remove;
  await f.run();
  assert.deepEqual(f.source.labels.map(label => label.name), ["type: bug", "area: tui"]);
  assert.equal(f.calls.filter(([method]) => method === "update").length, 1);
});
