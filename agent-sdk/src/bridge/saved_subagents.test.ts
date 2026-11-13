import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import type { TaskItem } from "../types.js";
import type { SessionState } from "./session_lifecycle.js";
import { replaceProtocolEventWriter } from "./events.js";
import { applyBackgroundTasksChanged, restoreSavedSubagentTask } from "./tasks.js";

function session(): SessionState {
  return {
    sessionId: "fixture",
    cwd: "fixture",
    connected: true,
    closing: false,
    tasksById: new Map(),
    taskOrder: [],
    toolCalls: new Map(),
    taskToolUseIds: new Map(),
    taskIdsByToolUseId: new Map(),
  } as unknown as SessionState;
}

type TaskEvent = {
  event: string;
  session_id: string;
  update: {
    type: string;
    source: string;
    tasks: TaskItem[];
    removed_task_ids: string[];
    is_complete_snapshot: boolean;
  };
};
function taskView(task: TaskItem) {
  const metadata = task.metadata as Record<string, unknown> | undefined;
  return {
    task_id: task.task_id,
    subject: task.subject,
    status: task.status,
    ...(task.parent_task_id ? { parent: task.parent_task_id } : {}),
    ...(task.source_tool_call_id ? { source: task.source_tool_call_id } : {}),
    ...(task.run_id ? { run: task.run_id } : {}),
    ...(metadata?.task_type ? { type: metadata.task_type } : {}),
    ...(metadata?.subagent_type ? { agent_type: metadata.subagent_type } : {}),
    ...(metadata?.terminal_status ? { terminal: metadata.terminal_status } : {}),
  };
}
function taskDelta(sessionId: string, tasks: TaskItem[]) {
  return {
    event: "session_update",
    session_id: sessionId,
    update: {
      type: "task_state_update",
      source: "task_lifecycle",
      tasks,
      removed_task_ids: [],
      is_complete_snapshot: false,
    },
  };
}

test("saved evidence publishes ancestry once without replacing newer executions or reviving removed runs", () => {
  const writes: TaskEvent[] = [];
  const restore = replaceProtocolEventWriter((line) => writes.push(JSON.parse(line)));
  try {
    for (const status of ["in_progress", "completed"] as const) {
      const state = session();
      state.sessionId = `fixture-${status}`;
      const current: TaskItem = {
        task_id: "child",
        run_id: "B",
        subject: "Current child B",
        status,
        blocks: [],
        blocked_by: [],
        source_tool_call_id: "launch-B",
        source_tool_call_run_id: "B",
        metadata: {
          task_type: "local_agent",
          summary: "B working",
          output: "B output",
          ...(status === "completed" ? { terminal_status: "completed" } : {}),
        },
      };
      state.tasksById.set("child", structuredClone(current));
      state.taskOrder.push("child");
      state.taskToolUseIds.set("child", "launch-B");
      const old = [
        {
          subtype: "task_notification",
          message: {
            run_id: "A",
            tool_use_id: "launch-A",
            status: "failed",
            summary: "PRIVATE OLD A",
            output: "A output",
          },
        },
      ];
      writes.length = 0;
      restoreSavedSubagentTask(state, "child", "parent", old);
      const expected = { ...current, parent_task_id: "parent" };
      assert.deepEqual(
        writes,
        [taskDelta(state.sessionId, [expected])],
        `${status}: exact ancestry-only B publication`,
      );
      restoreSavedSubagentTask(state, "child", "parent", old);
      assert.deepEqual(
        writes,
        [taskDelta(state.sessionId, [expected])],
        "repeated stale evidence is silent",
      );
      assert.deepEqual(state.tasksById.get("child"), expected);
      assert.equal(state.taskToolUseIds.get("child"), "launch-B");
    }
    const removed = session();
    removed.sessionId = "fixture-removed";
    applyBackgroundTasksChanged(removed, {
      tasks: [
        {
          task_id: "removed-child",
          run_id: "B",
          task_type: "local_agent",
          description: "Newer removed B",
        },
      ],
    });
    applyBackgroundTasksChanged(removed, { tasks: [] });
    assert.ok(
      writes.some(
        (event) =>
          event.session_id === removed.sessionId &&
          event.update.removed_task_ids.includes("removed-child"),
      ),
      "supported lifecycle removal publishes the row's removal",
    );
    assert.equal(removed.tasksById.has("removed-child"), false);
    const before = structuredClone(writes);
    restoreSavedSubagentTask(removed, "removed-child", "parent", [
      {
        subtype: "task_started",
        message: { run_id: "A", task_type: "local_agent", tool_use_id: "launch-A" },
      },
      {
        subtype: "task_notification",
        message: { run_id: "A", status: "completed", summary: "PRIVATE OLD A" },
      },
    ]);
    assert.equal(removed.tasksById.has("removed-child"), false, "stale removed run stays absent");
    assert.deepEqual(writes, before, "stale discovery emits no resurrection after removal");
  } finally {
    restore();
  }
});

test("incomplete saved evidence publishes interrupted then recovered state without duplicate sibling changes", () => {
  const state = session();
  const sibling: TaskItem = {
    task_id: "sibling",
    subject: "Unrelated sibling",
    status: "completed",
    blocks: [],
    blocked_by: [],
  };
  state.tasksById.set(sibling.task_id, structuredClone(sibling));
  state.taskOrder.push(sibling.task_id);
  const events: TaskEvent[] = [];
  const restore = replaceProtocolEventWriter((line) => events.push(JSON.parse(line)));
  const published = () =>
    events.map((event) => {
      assert.equal(event.event, "session_update");
      assert.equal(event.session_id, state.sessionId);
      assert.equal(event.update.type, "task_state_update");
      assert.equal(event.update.source, "task_lifecycle");
      assert.deepEqual(event.update.removed_task_ids, []);
      assert.equal(event.update.is_complete_snapshot, false);
      return event.update.tasks.map(taskView);
    });
  try {
    restoreSavedSubagentTask(state, "child", undefined, [
      { subtype: "task_started", message: { task_type: "local_agent", synthesized_launch: true } },
    ]);
    const interrupted = {
      task_id: "child",
      subject: "child",
      status: "completed",
      type: "local_agent",
      terminal: "interrupted",
    };
    assert.deepEqual(published(), [[interrupted]]);
    const complete = [
      {
        subtype: "task_started",
        message: {
          task_type: "local_agent",
          subject: "Recovered child",
          tool_use_id: "launch",
          synthesized_launch: true,
        },
      },
      { subtype: "task_notification", message: { tool_use_id: "launch", status: "completed" } },
    ];
    restoreSavedSubagentTask(state, "child", "parent", complete);
    const recovered = {
      task_id: "child",
      subject: "Recovered child",
      status: "completed",
      parent: "parent",
      source: "launch",
      type: "local_agent",
      terminal: "completed",
    };
    assert.deepEqual(published(), [[interrupted], [recovered]]);
    restoreSavedSubagentTask(state, "child", "parent", complete);
    assert.deepEqual(
      published(),
      [[interrupted], [recovered]],
      "repeated complete evidence publishes no duplicate change",
    );
    assert.deepEqual(taskView(state.tasksById.get("child") as TaskItem), recovered);
    assert.deepEqual(state.tasksById.get("sibling"), sibling);
    assert.deepEqual([...state.tasksById.keys()].sort(), ["child", "sibling"]);
  } finally {
    restore();
  }
});

test("real SDK discovery restores nested saved tasks without leaking private conversations", () => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-saved-agents-"));
  const profile = join(directory, "profile");
  const projectName = "saved-agent-fixture";
  const project = join(profile, "projects", projectName);
  const id = "11111111-2222-4333-8444-555555555555";
  const subagents = join(project, id, "subagents");
  const paths: string[] = [];
  const sidecar = (path: string, value: unknown) => {
    paths.push(path);
    writeFileSync(path, JSON.stringify(value));
  };
  const writeHistory = (path: string, records: Record<string, unknown>[]) => {
    let previous: string | null = null;
    const lines = records.map((record, index) => {
      const uuid = String(record.uuid ?? `record-${index}`);
      const row = {
        parentUuid: previous,
        sessionId: id,
        cwd: directory,
        timestamp: new Date(1_700_000_000_000 + index * 1000).toISOString(),
        ...record,
        uuid,
      };
      previous = uuid;
      return JSON.stringify(row);
    });
    paths.push(path);
    writeFileSync(path, `${lines.join("\n")}\n`);
  };
  const call = (tool: string, name: string, input: Record<string, unknown>) => ({
    type: "assistant",
    message: { role: "assistant", content: [{ type: "tool_use", id: tool, name, input }] },
  });
  const result = (tool: string, content: unknown) => ({
    type: "user",
    message: { role: "user", content: [{ type: "tool_result", tool_use_id: tool, content }] },
  });
  const prompt = (content: string) => ({ type: "user", message: { role: "user", content } });
  const notification = (task: string, runId: string, tool: string, status: string) => ({
    ...prompt(
      `<task-notification><task-id>${task}</task-id><tool-use-id>${tool}</tool-use-id><status>${status}</status><summary>${task} ${status}</summary></task-notification>`,
    ),
    origin: { kind: "task-notification", producer: "session-task", runId },
  });
  try {
    mkdirSync(subagents, { recursive: true });
    writeHistory(join(project, `${id}.jsonl`), [
      prompt("Main prompt"),
      call("parent-launch", "Agent", {
        description: "Migration Parent",
        subagent_type: "general-purpose",
        run_in_background: true,
      }),
      result("parent-launch", "Async agent launched"),
      notification("parent", "A", "parent-launch", "stopped"),
      {
        type: "assistant",
        message: { role: "assistant", content: [{ type: "text", text: "Main reply only" }] },
      },
    ]);
    writeHistory(join(subagents, "agent-parent.jsonl"), [
      prompt("Private parent prompt"),
      call("child-launch", "Agent", {
        description: "Migration Child",
        subagent_type: "reviewer",
        prompt: "Private child instructions",
        run_in_background: false,
      }),
      result("child-launch", [{ type: "text", text: "agentId: child\nChild finished." }]),
      call("unfinished-launch", "Agent", {
        description: "Unfinished child",
        run_in_background: true,
      }),
      result("unfinished-launch", "Async agent launched successfully"),
      call("ack-launch", "Agent", {
        description: "Foreground call returned asynchronously",
        run_in_background: false,
      }),
      result("ack-launch", [
        { type: "text", text: "Async agent launched successfully.\nagentId: acknowledgement" },
      ]),
      call("structured-ack-launch", "Agent", {
        description: "Structured async acknowledgement",
        run_in_background: false,
      }),
      result("structured-ack-launch", [
        {
          type: "text",
          text: JSON.stringify({ status: "async_launched", agentId: "structured-ack" }),
        },
      ]),
      call("resumed-launch", "Agent", { description: "Resumed child", run_in_background: true }),
      result("resumed-launch", "Async agent launched"),
      notification("resumed", "A", "resumed-launch", "failed"),
      call("resume-B", "SendMessage", { to: "resumed", message: "Continue" }),
      result("resume-B", [
        { type: "text", text: JSON.stringify({ success: true, resumedAgentId: "resumed" }) },
      ]),
      notification("resumed", "B", "resume-B", "completed"),
      notification("resumed", "A", "resumed-launch", "stopped"),
    ]);
    for (const [agent, tool] of [
      ["child", "child-launch"],
      ["unfinished", "unfinished-launch"],
      ["acknowledgement", "ack-launch"],
      ["structured-ack", "structured-ack-launch"],
      ["resumed", "resumed-launch"],
      ["grandchild", "grand-launch"],
    ]) {
      writeHistory(join(subagents, `agent-${agent}.jsonl`), [
        prompt("Private saved conversation"),
        ...(agent === "child"
          ? [
              call("grand-launch", "Agent", {
                description: "Deep descendant",
                subagent_type: "researcher",
                run_in_background: false,
              }),
              result("grand-launch", "Grandchild finished."),
            ]
          : []),
        call(`${agent}-report`, "SubagentHandback", {
          message: "Saved report alone is not an outcome",
        }),
      ]);
      sidecar(join(subagents, `agent-${agent}.meta.json`), {
        parentAgentId: agent === "grandchild" ? "child" : "parent",
        toolUseId: tool,
        agentType: agent === "grandchild" ? "researcher" : "general-purpose",
      });
    }
    sidecar(join(subagents, "agent-parent.meta.json"), {
      toolUseId: "parent-launch",
      agentType: "general-purpose",
    });
    const before = paths.map((path) => readFileSync(path));
    const script = `
      import { listSubagents } from '@anthropic-ai/claude-agent-sdk';
      import { restoreSavedSubagents } from ${JSON.stringify(new URL("./saved_subagents.js", import.meta.url).href)};
      import { replaceProtocolEventWriter } from ${JSON.stringify(new URL("./events.js", import.meta.url).href)};
      import { getSessionTranscriptMessages, mapSessionMessagesToUpdates } from ${JSON.stringify(new URL("./history.js", import.meta.url).href)};
      const make = () => ({ sessionId: ${JSON.stringify(id)}, cwd: ${JSON.stringify(directory)}, connected: true, closing: false, tasksById: new Map(), taskOrder: [], toolCalls: new Map(), taskToolUseIds: new Map(), taskIdsByToolUseId: new Map() });
      let events=[]; const restore=replaceProtocolEventWriter(line=>events.push(JSON.parse(line)));
      try {
        const baseline=mapSessionMessagesToUpdates(await getSessionTranscriptMessages(${JSON.stringify(id)}));
        const ids=await listSubagents(${JSON.stringify(id)});
        const states=[];
        for(let reopen=0; reopen<2; reopen++) {
          const state=make(); events=[];
          await restoreSavedSubagents(state,ids);
          const initial=events; events=[];
          await restoreSavedSubagents(state,ids);
          states.push({tasks:[...state.tasksById.values()],events:initial,repeatEvents:events});
        }
        const state=make(); events=[];
        const pending=restoreSavedSubagents(state,ids);
        state.tasksById.set('child',{task_id:'child',run_id:'Z',subject:'Live Z',status:'in_progress',blocks:[],blocked_by:[],metadata:{task_type:'local_agent'},source_tool_call_id:'launch-Z'});
        await pending;
        const racing={tasks:[...state.tasksById.values()],events};
        const closing=make(); events=[];
        const closedSeed={task_id:'keep-closed',subject:'Before close',status:'in_progress',blocks:[],blocked_by:[]};
        closing.tasksById.set(closedSeed.task_id,closedSeed);closing.taskOrder.push(closedSeed.task_id);
        const closingRead=restoreSavedSubagents(closing,ids);
        closing.closing=true;
        await closingRead;
        const closed={tasks:[...closing.tasksById.values()],events};
        const after=mapSessionMessagesToUpdates(await getSessionTranscriptMessages(${JSON.stringify(id)}));
        console.log(JSON.stringify({states,baseline,after,racing,closed}));
      } finally { restore(); }
    `;
    const env = { ...process.env };
    for (const key of Object.keys(env))
      if (/^(ANTHROPIC_|CLAUDE_|CLAUDECODE$)/.test(key)) delete env[key];
    Object.assign(env, { CLAUDE_CONFIG_DIR: profile, CLAUDE_CODE_PROJECT_DIR_NAME: projectName });
    const child = spawnSync(process.execPath, ["--input-type=module", "-"], {
      input: script,
      encoding: "utf8",
      windowsHide: true,
      timeout: 15_000,
      maxBuffer: 4 * 1024 * 1024,
      env,
    });
    assert.deepEqual(
      paths.map((path) => readFileSync(path)),
      before,
      "discovery preserves JSONL and metadata sidecar bytes",
    );
    assert.equal(child.error, undefined, String(child.error));
    assert.equal(child.signal, null, child.stderr);
    assert.equal(child.status, 0, child.stderr);
    const output = JSON.parse(child.stdout);
    const expected = [
      {
        task_id: "parent",
        subject: "Migration Parent",
        status: "completed",
        source: "parent-launch",
        run: "A",
        type: "local_agent",
        agent_type: "general-purpose",
        terminal: "stopped",
      },
      {
        task_id: "child",
        subject: "Migration Child",
        status: "completed",
        parent: "parent",
        source: "child-launch",
        type: "local_agent",
        agent_type: "reviewer",
        terminal: "completed",
      },
      {
        task_id: "grandchild",
        subject: "Deep descendant",
        status: "completed",
        parent: "child",
        source: "grand-launch",
        type: "local_agent",
        agent_type: "researcher",
        terminal: "completed",
      },
      {
        task_id: "unfinished",
        subject: "Unfinished child",
        status: "completed",
        parent: "parent",
        source: "unfinished-launch",
        type: "local_agent",
        terminal: "interrupted",
      },
      {
        task_id: "acknowledgement",
        subject: "Foreground call returned asynchronously",
        status: "completed",
        parent: "parent",
        source: "ack-launch",
        type: "local_agent",
        terminal: "interrupted",
      },
      {
        task_id: "structured-ack",
        subject: "Structured async acknowledgement",
        status: "completed",
        parent: "parent",
        source: "structured-ack-launch",
        type: "local_agent",
        terminal: "interrupted",
      },
      {
        task_id: "resumed",
        subject: "Resumed child",
        status: "completed",
        parent: "parent",
        source: "resume-B",
        run: "B",
        type: "local_agent",
        terminal: "completed",
      },
    ];
    const sorted = <T extends { task_id: string }>(tasks: T[]) =>
      [...tasks].sort((left, right) => left.task_id.localeCompare(right.task_id));
    function published(events: TaskEvent[]) {
      return events.flatMap((event) => {
        assert.equal(event.event, "session_update");
        assert.equal(event.session_id, id);
        assert.equal(event.update.type, "task_state_update");
        assert.equal(event.update.source, "task_lifecycle");
        assert.deepEqual(event.update.removed_task_ids, []);
        assert.equal(event.update.is_complete_snapshot, false);
        return event.update.tasks.map(taskView);
      });
    }
    for (const state of output.states) {
      assert.deepEqual(
        sorted(state.tasks.map(taskView)),
        sorted(expected),
        "authored saved inventory on every reopening",
      );
      assert.deepEqual(
        sorted(published(state.events)),
        sorted(expected),
        "actual discovery publishes every intended task once",
      );
      assert.deepEqual(state.repeatEvents, [], "repeated refresh publishes no duplicate changes");
    }
    for (const updates of [output.baseline, output.after]) {
      const chunks = updates.filter((update: { type: string }) =>
        ["user_message_chunk", "agent_message_chunk"].includes(update.type),
      );
      assert.deepEqual(
        chunks.map((update: { type: string; source_message_uuid: string; content: unknown }) => [
          update.type,
          update.source_message_uuid,
          update.content,
        ]),
        [
          ["user_message_chunk", "record-0", { type: "text", text: "Main prompt" }],
          ["agent_message_chunk", "record-4", { type: "text", text: "Main reply only" }],
        ],
      );
      for (const privateText of [
        "Private parent prompt",
        "Private child instructions",
        "Private saved conversation",
        "Saved report alone",
      ])
        assert.equal(
          JSON.stringify(updates).includes(privateText),
          false,
          "private agent data is absent from chat/tool/notice updates",
        );
    }
    const racingChild = {
      task_id: "child",
      subject: "Live Z",
      status: "in_progress",
      parent: "parent",
      source: "launch-Z",
      run: "Z",
      type: "local_agent",
    };
    const expectedRace = [...expected.filter((task) => task.task_id !== "child"), racingChild];
    assert.deepEqual(sorted(output.racing.tasks.map(taskView)), sorted(expectedRace));
    assert.deepEqual(
      sorted(published(output.racing.events)),
      sorted(expectedRace),
      "saved completion cannot overwrite the live run-Z publication",
    );
    assert.deepEqual(output.closed.events, [], "closing during discovery publishes no tasks");
    assert.deepEqual(
      output.closed.tasks,
      [
        {
          task_id: "keep-closed",
          subject: "Before close",
          status: "in_progress",
          blocks: [],
          blocked_by: [],
        },
      ],
      "closing preserves pre-existing rows without adding saved ones",
    );
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("partial saved-agent reads recover through request-correlated listings without discarding available work", () => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-saved-partial-"));
  try {
    const cwd = join(directory, "selected"),
      otherCwd = join(directory, "main-failure");
    mkdirSync(cwd);
    mkdirSync(otherCwd);
    const selectedId = "11111111-3333-4333-8333-111111111111";
    const otherId = "22222222-3333-4333-8333-222222222222";
    const sdkUrl = import.meta.resolve("@anthropic-ai/claude-agent-sdk");
    const sdk = `
      export * from ${JSON.stringify(sdkUrl)};
      export async function listSubagents(session, options) {
        globalThis.calls.push({operation:'list',session,options,phase:globalThis.phase});
        return ['bad','good','live'];
      }
      export async function getSessionMessages(session, options) {
        globalThis.calls.push({operation:'main',session,options,phase:globalThis.phase});
        if(globalThis.mode === 'main-fails') throw new Error('controlled main metadata failure');
        return globalThis.fixture.main;
      }
      export async function getSubagentMessages(session, agent, options) {
        globalThis.calls.push({operation:'child',session,agent,options,phase:globalThis.phase});
        if(agent === 'bad' && globalThis.mode === 'child-fails') {
          globalThis.badEntered(); await globalThis.badRelease;
          throw new Error('controlled child metadata failure');
        }
        return globalThis.fixture[agent];
      }
    `;
    const script = `
      import assert from 'node:assert/strict';
      import { registerHooks } from 'node:module';
      globalThis.calls=[];globalThis.phase='setup';globalThis.mode='child-fails';
      const call=(id,description,subagent_type)=>({type:'assistant',message:{role:'assistant',content:[{type:'tool_use',id,name:'Agent',input:{description,subagent_type,run_in_background:false}}]}});
      const result=id=>({type:'user',message:{role:'user',content:[{type:'tool_result',tool_use_id:id,content:'Foreground execution completed'}]}});
      globalThis.fixture={
        main:[call('good-launch','Available child','reviewer'),result('good-launch')],
        good:[
          {type:'user',parent_tool_use_id:'good-launch',message:{role:'user',content:'PRIVATE GOOD TRANSCRIPT'}},
          call('recovered-launch','Recovered nested child','researcher'),result('recovered-launch')
        ],
        bad:[{type:'user',parent_agent_id:'good',parent_tool_use_id:'recovered-launch',message:{role:'user',content:'PRIVATE BAD TRANSCRIPT'}}],
        live:[{type:'user',parent_agent_id:'good',parent_tool_use_id:'old-A',message:{role:'user',content:'PRIVATE OLD LIVE'}}]
      };
      const fixtureBefore=structuredClone(globalThis.fixture);
      registerHooks({
        resolve(specifier,context,next) { return specifier==='@anthropic-ai/claude-agent-sdk'?{url:'fixture:saved-sdk',shortCircuit:true}:next(specifier,context); },
        load(url,context,next) { return url==='fixture:saved-sdk'?{format:'module',source:${JSON.stringify(sdk)},shortCircuit:true}:next(url,context); }
      });
      const { handleSubagentCommand }=await import(${JSON.stringify(new URL("./subagents.js", import.meta.url).href)});
      const { parseCommandEnvelope }=await import(${JSON.stringify(new URL("./commands.js", import.meta.url).href)});
      const { sessions }=await import(${JSON.stringify(new URL("./session_lifecycle.js", import.meta.url).href)});
      const { replaceProtocolEventWriter }=await import(${JSON.stringify(new URL("./events.js", import.meta.url).href)});
      const live={task_id:'live',run_id:'Z',parent_task_id:'good',subject:'Live Z',status:'in_progress',blocks:[],blocked_by:[],source_tool_call_id:'live-Z',source_tool_call_run_id:'Z',metadata:{task_type:'local_agent',summary:'Z output'}};
      const make=(sessionId,cwd)=>({sessionId,cwd,connected:true,closing:false,tasksById:new Map([['live',structuredClone(live)]]),taskOrder:['live'],toolCalls:new Map(),taskToolUseIds:new Map([['live','live-Z']]),taskIdsByToolUseId:new Map()});
      const selected=make(${JSON.stringify(selectedId)},${JSON.stringify(cwd)});
      const other=make(${JSON.stringify(otherId)},${JSON.stringify(otherCwd)});
      const unrelated=make('unrelated-session','unused');
      const states=[selected,other,unrelated];
      for(const state of states){assert.equal(sessions.has(state.sessionId),false);sessions.set(state.sessionId,state);}
      let events=[];const phases=[];
      const restore=replaceProtocolEventWriter(line=>events.push(JSON.parse(line)));
      async function dispatch(state,name,mode) {
        globalThis.phase=name;globalThis.mode=mode;events=[];
        const parsed=parseCommandEnvelope(JSON.stringify({command:'list_subagents',session_id:state.sessionId,request_id:name}));
        assert.equal(parsed.command.command,'list_subagents');
        const pending=handleSubagentCommand(parsed.command,parsed.requestId);
        if(mode==='child-fails') {
          await entered;
          assert.equal(events.some(event=>event.event==='subagents_listed'),false,'listing waits for the controlled read to settle');
          releaseBad();
        }
        await pending;
        phases.push({name,session:state.sessionId,cwd:state.cwd,events,tasks:[...state.tasksById.values()]});
        assert.deepEqual(state.tasksById.get('live'),live,'a failed saved read never replaces the newer live execution');
        assert.equal(state.taskToolUseIds.get('live'),'live-Z');
        assert.strictEqual(sessions.get(state.sessionId),state);
      }
      let releaseBad;
      globalThis.badRelease=new Promise(resolve=>releaseBad=resolve);
      let enterBad;const entered=new Promise(resolve=>enterBad=resolve);globalThis.badEntered=enterBad;
      try {
        await dispatch(selected,'partial','child-fails');
        await dispatch(selected,'child-recovered','recover');
        await dispatch(selected,'unchanged-refresh','recover');
        await dispatch(other,'main-unavailable','main-fails');
        await dispatch(other,'main-recovered','recover');
        assert.deepEqual([...unrelated.tasksById.values()],[live],'unselected session stays usable and unchanged');
        assert.deepEqual(globalThis.fixture,fixtureBefore,'metadata discovery preserves its source messages');
        console.log(JSON.stringify({phases,calls:globalThis.calls}));
      } finally {
        releaseBad();restore();for(const state of states)sessions.delete(state.sessionId);
      }
    `;
    const env = { ...process.env };
    for (const key of Object.keys(env))
      if (/^(ANTHROPIC_|CLAUDE_|CLAUDECODE$)/.test(key)) delete env[key];
    env.CLAUDE_CONFIG_DIR = join(directory, "profile");
    const child = spawnSync(process.execPath, ["--input-type=module", "-"], {
      input: script,
      cwd,
      env,
      encoding: "utf8",
      windowsHide: true,
      timeout: 15_000,
      maxBuffer: 4 * 1024 * 1024,
    });
    assert.equal(child.error, undefined, String(child.error));
    assert.equal(child.signal, null, child.stderr);
    assert.equal(child.status, 0, child.stderr);
    const output = JSON.parse(child.stdout) as {
      phases: Array<{
        name: string;
        session: string;
        cwd: string;
        events: Array<TaskEvent & { request_id?: string; agent_ids?: string[]; error?: string }>;
        tasks: TaskItem[];
      }>;
      calls: Array<{
        phase: string;
        operation: string;
        session: string;
        agent?: string;
        options: { dir: string; includeSystemMessages?: boolean };
      }>;
    };
    const live = {
      task_id: "live",
      subject: "Live Z",
      status: "in_progress",
      parent: "good",
      source: "live-Z",
      run: "Z",
      type: "local_agent",
    };
    const good = {
      task_id: "good",
      subject: "Available child",
      status: "completed",
      source: "good-launch",
      type: "local_agent",
      agent_type: "reviewer",
      terminal: "completed",
    };
    const bad = {
      task_id: "bad",
      subject: "Recovered nested child",
      status: "completed",
      parent: "good",
      source: "recovered-launch",
      type: "local_agent",
      agent_type: "researcher",
      terminal: "completed",
    };
    const incompleteBad = {
      task_id: "bad",
      subject: "bad",
      status: "completed",
      type: "local_agent",
      terminal: "interrupted",
    };
    const incompleteGood = {
      task_id: "good",
      subject: "good",
      status: "completed",
      source: "good-launch",
      type: "local_agent",
      terminal: "interrupted",
    };
    const expected = {
      partial: {
        tasks: [incompleteBad, good, live],
        changes: [incompleteBad, good],
        session: selectedId,
        cwd,
      },
      "child-recovered": { tasks: [bad, good, live], changes: [bad], session: selectedId, cwd },
      "unchanged-refresh": { tasks: [bad, good, live], changes: [], session: selectedId, cwd },
      "main-unavailable": {
        tasks: [bad, incompleteGood, live],
        changes: [bad, incompleteGood],
        session: otherId,
        cwd: otherCwd,
      },
      "main-recovered": {
        tasks: [bad, good, live],
        changes: [good],
        session: otherId,
        cwd: otherCwd,
      },
    };
    const sorted = <T extends { task_id: string }>(items: T[]) =>
      [...items].sort((a, b) => a.task_id.localeCompare(b.task_id));
    assert.deepEqual(
      output.phases.map((phase) => phase.name),
      Object.keys(expected),
      "all controlled reads and recoveries completed",
    );
    for (const phase of output.phases) {
      const wanted = expected[phase.name as keyof typeof expected];
      assert.equal(phase.session, wanted.session);
      assert.equal(phase.cwd, wanted.cwd);
      assert.deepEqual(sorted(phase.tasks.map(taskView)), sorted(wanted.tasks), phase.name);
      const listing = phase.events.filter((event) => event.event === "subagents_listed");
      assert.deepEqual(
        listing,
        [
          {
            event: "subagents_listed",
            session_id: wanted.session,
            request_id: phase.name,
            agent_ids: ["bad", "good", "live"],
          },
        ],
        `${phase.name}: successful correlated listing despite partial metadata failure`,
      );
      const changes = phase.events
        .filter((event) => event.event === "session_update")
        .flatMap((event) => {
          assert.equal(event.session_id, wanted.session);
          assert.equal(event.update.type, "task_state_update");
          assert.equal(event.update.source, "task_lifecycle");
          assert.deepEqual(event.update.removed_task_ids, []);
          assert.equal(event.update.is_complete_snapshot, false);
          return event.update.tasks.map(taskView);
        });
      assert.deepEqual(
        sorted(changes),
        sorted(wanted.changes),
        `${phase.name}: no duplicate sibling publication or fabricated failed-read evidence`,
      );
      assert.ok(
        phase.events.every((event) => ["session_update", "subagents_listed"].includes(event.event)),
        "no chat, tool or notice leakage",
      );
      const calls = output.calls.filter((call) => call.phase === phase.name);
      assert.deepEqual(
        calls.map((call) => `${call.operation}:${call.agent ?? ""}`).sort(),
        ["child:bad", "child:good", "child:live", "list:", "main:"].sort(),
        "the selected listing attempts each real boundary read",
      );
      for (const call of calls) {
        assert.equal(call.session, wanted.session);
        assert.equal(call.options.dir, wanted.cwd);
        if (call.operation === "main") assert.equal(call.options.includeSystemMessages, true);
      }
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
