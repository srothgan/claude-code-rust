// Minimal NDJSON bridge stand-in for terminal tests: completes the handshake,
// then streams a long first reply and short follow-up replies. No model or
// network is involved.
const readline = require('node:readline');
const fs = require('node:fs');
const path = require('node:path');

let SESSION = 'fake-session';
let cwd = process.cwd();
const LINES = Number(process.env.FAKE_BRIDGE_LINES ?? 1500);
const FOLLOW_UP_LINES = Number(process.env.FAKE_BRIDGE_FOLLOW_UP_LINES ?? 3);
const INTERVAL_MS = Number(process.env.FAKE_BRIDGE_INTERVAL_MS ?? 15);
// Held turns are released after tests exercise input or rendering, rather than
// racing a fixed reply duration. The resize-code scenario holds the second turn.
const SCENARIO = process.env.FAKE_BRIDGE_SCENARIO ?? 'stream';
const resizeCodeScenario = SCENARIO === 'resize-code';
const activityScenario = SCENARIO.startsWith('hold-activity');
const RELEASE_FILE = process.env.FAKE_BRIDGE_RELEASE_FILE;
const JOURNAL = process.env.FAKE_BRIDGE_JOURNAL;
let replyNumber = 0;
let active = null;
const pending = [];

const record = entry => {
  if (JOURNAL) fs.appendFileSync(JOURNAL, `${JSON.stringify(entry)}\n`);
};
const send = event => {
  record({ type: 'event', ...event });
  process.stdout.write(`${JSON.stringify(event)}\n`);
};


const preferences = { language: 'German', 'permissions.deny': ['Read(./.env)'], alwaysThinkingEnabled: false };
let settingsRevision = 0;
const settingDefinitions = [
  ['language', 'Language', 'Saved response language', 'string', ['language']],
  ['permissions.deny', 'Permissions: deny rules', 'One denied tool rule per line', 'string_list', ['permissions', 'deny']],
  ['alwaysThinkingEnabled', 'Thinking', 'Saved thinking preference', 'boolean', ['alwaysThinkingEnabled']],
];
const settingsUiFixture = JSON.parse(fs.readFileSync(path.join(__dirname, 'settings-ui-catalog.json'), 'utf8'));
if (activityScenario) {
  Object.assign(preferences, { prefersReducedMotion: true, spinnerTipsEnabled: SCENARIO !== 'hold-activity-off' });
  fs.writeFileSync(path.join(process.env.CLAUDE_CONFIG_DIR, 'settings.json'), JSON.stringify(preferences));
  settingDefinitions.push(['prefersReducedMotion', 'Reduce motion', 'Activity preference', 'boolean', ['prefersReducedMotion']]);
  const setting = settingsUiFixture.catalog.find(setting => setting.id === 'spinnerTipsEnabled');
  settingDefinitions.push([setting.id, setting.label, setting.description, setting.kind, setting.key_path]);
}
const hooksScenario = SCENARIO === 'hold-hooks';
if (hooksScenario) {
  preferences.hooks = { Stop: [{ hooks: [{ type: 'command', command: 'keep-original', future: 'keep' }] }] };
  const setting = settingsUiFixture.catalog.find(setting => setting.id === 'hooks');
  settingDefinitions.push([setting.id, setting.label, setting.description, setting.kind, setting.key_path]);
}
const notificationScenario = SCENARIO === 'notifications';
let notificationAppPath;
if (notificationScenario) {
  Object.assign(preferences, { preferredNotifChannel: 'terminal_bell', 'notifications.modelDirected': true, 'notifications.actionsRequired': true, 'notifications.turnComplete': false });
  fs.writeFileSync(path.join(process.env.CLAUDE_CONFIG_DIR, 'settings.json'), JSON.stringify({ preferredNotifChannel: 'terminal_bell' }));
  settingDefinitions.splice(0, settingDefinitions.length,
    ['preferredNotifChannel', 'Notification method', 'Choose local transport', 'string', ['preferredNotifChannel']],
    ['notifications.turnComplete', 'Notify when a turn finishes', 'Completed turns', 'boolean', ['notifications', 'turnComplete']],
    ['notifications.modelDirected', 'Notify when Claude requests it', 'Local proactive alerts', 'boolean', ['notifications', 'modelDirected']],
    ['notifications.actionsRequired', 'Notify when input is needed', 'Waiting interactions', 'boolean', ['notifications', 'actionsRequired']]);
}
const presentationScenario = SCENARIO.includes('presentation');
if (presentationScenario) {
  Object.assign(preferences, { autoScrollEnabled: true, showMessageTimestamps: true, showTurnDuration: true, timeFormat: '24-hour-utc' });
  fs.writeFileSync(path.join(process.env.CLAUDE_CONFIG_DIR, 'settings.json'), JSON.stringify(preferences));
  settingDefinitions.unshift(['autoScrollEnabled', 'Auto-scroll', 'Follow new output', 'boolean', ['autoScrollEnabled']]);
  settingDefinitions.push(['showMessageTimestamps', 'Show message timestamps', 'Show message times', 'boolean', ['showMessageTimestamps']], ['showTurnDuration', 'Show turn duration', 'Show completed turn clocks', 'boolean', ['showTurnDuration']], ['timeFormat', 'Time format', 'Clock format', 'string', ['timeFormat']]);
}
function settingsSnapshot() {
  return {
    categories: hooksScenario ? settingsUiFixture.categories : [{ id: 'general', label: 'General', short_label: 'General' }, { id: 'permissions', label: 'Permissions', short_label: 'Permissions' }], cwd, context: 'fixture-settings', diagnostics: [], resolution_sources: [], provenance: {},
    catalog: settingDefinitions.map(([id, label, description, kind, key_path]) => ({
      category: id === 'hooks' ? 'hooks' : id.startsWith('permissions.') ? 'permissions' : 'general', id, label, description, kind, key_path, options: kind === 'boolean' ? [true, false] : id === 'preferredNotifChannel' ? ['auto', 'iterm2', 'terminal_bell', 'iterm2_with_bell', 'kitty', 'ghostty', 'notifications_disabled'] : [],
      ...(settingsUiFixture.catalog.find(setting => setting.id === id)?.editor ? { editor: settingsUiFixture.catalog.find(setting => setting.id === id).editor } : {}),
      allows_custom: kind !== 'boolean' && id !== 'preferredNotifChannel', writable_scopes: ['user'],
      reset: 'Reset removes the saved value here', application: presentationScenario || notificationScenario || activityScenario ? 'host' : 'next_session',
    })).sort((a, b) => a.label.localeCompare(b.label)),
    sources: [{ scope: 'user', path: path.join(process.env.CLAUDE_CONFIG_DIR, 'settings.json'), status: 'valid',
      values: Object.entries(preferences).map(([id, value]) => ({ id, value, revision: String(settingsRevision) })) }],
    values: Object.entries(preferences).map(([id, value]) => ({ id, value, contributors: ['user'], policy_restricted: false })),
  };
}

function finish(event) {
  if (presentationScenario) send({ event: 'session_update', session_id: SESSION, update: { type: 'turn_timing', duration_ms: 2000, api_duration_ms: 1250 } });
  clearInterval(active.timer);
  active = null;
  send(event.event === 'turn_complete'
    ? { queued_turn_count: pending.length, terminal_reason: 'completed', ...event }
    : event);
  if (pending.length) streamReply(pending.shift());
}

const model = {
  requested_id: null,
  resolved_id: 'fake-model',
  display_name_short: 'Fake',
  display_name_long: 'Fake model',
  catalog_id: null,
  supports_effort: false,
  supported_effort_levels: [],
  supports_fast_mode: null,
  supports_auto_mode: null,
  supports_adaptive_thinking: null,
  is_authoritative: true,
};

function streamReply(messageUuid) {
  send({ event: 'user_message_started', session_id: SESSION, message_uuid: messageUuid, source: 'command_lifecycle' });
  replyNumber++;
  if (SCENARIO === 'background-replies') {
    const update = update => send({ event: 'session_update', session_id: SESSION, update });
    const text = text => update({ type: 'agent_message_chunk', content: { type: 'text', text } });
    if (replyNumber === 1) {
      for (let i = 1; i <= 3; i++) update({ type: 'tool_call', tool_call: {
        tool_call_id: `completion-${i}`, title: `LAUNCH_${i}`, kind: 'execute', status: 'detached',
        content: [], raw_input: { command: `LAUNCH_${i}` }, locations: [], meta: { claudeCode: { toolName: 'Bash' } },
      } });
      text('Foreground reply is complete.');
      send({ event: 'turn_complete', session_id: SESSION });
      let previous = '';
      // Independent completions continue after the foreground turn has ended.
      setInterval(() => {
        const step = fs.existsSync(RELEASE_FILE) ? fs.readFileSync(RELEASE_FILE, 'utf8') : '';
        if (step === previous) return;
        previous = step;
        const match = /^(result|prefix|finish)-([123])$/.exec(step);
        if (!match) return;
        const [, action, number] = match;
        if (action === 'result') {
          for (let i = 0; i < 2; i++) update({ type: 'tool_call_update', tool_call_update: {
            tool_call_id: `completion-${number}`, fields: { status: 'completed', title: `RESULT_${number}`, raw_output: `OUTPUT_${number}` },
          } });
        } else if (action === 'prefix') {
          update({ type: 'agent_response_started' });
          text(number === '1' ? 'The' : 'Short task 2 (`');
        } else {
          text(number === '1' ? ' first background task finished completely.' : 'task-two`) finished with exit code 0.');
          send({ event: 'turn_complete', session_id: SESSION });
        }
        record({ type: 'barrier', name: step });
      }, INTERVAL_MS);
    } else if (replyNumber === 2) {
      update({ type: 'agent_response_started' });
      text('Normal conversation continues completely.');
      send({ event: 'turn_complete', session_id: SESSION });
    } else {
      active = { timer: setInterval(() => {}, 1000) };
      update({ type: 'agent_activity_update', phase: 'thinking' });
      record({ type: 'barrier', name: 'cancellable-turn-started' });
    }
    return;
  }
  if (SCENARIO.startsWith('background-')) {
    const update = update => send({ event: 'session_update', session_id: SESSION, update });
    update({ type: 'message_metadata', role: 'user', timestamp: '2026-10-06T10:41:54.067Z', source_message_uuid: messageUuid });
    const fields = fields => update({ type: 'tool_call_update', tool_call_update: { tool_call_id: 'background-primary', fields } });
    const tool = (id, command, status) => {
      const agent = id === 'background-primary' && SCENARIO.includes('agent');
      update({ type: 'tool_call', tool_call: {
        tool_call_id: id, title: command, kind: agent ? 'other' : 'execute', status, content: [],
        raw_input: agent ? { description: command, prompt: 'Work independently' } : { command },
        raw_output: null, locations: [], meta: { claudeCode: { toolName: agent ? 'Agent' : 'Bash' } },
      } });
    };
    if (replyNumber === 1 && SCENARIO !== 'background-resume') {
      tool('background-primary', 'PRIMARY_TASK', 'in_progress');
      if (SCENARIO.endsWith('-initial')) fields({ status: 'detached' });
    }
    let previous = fs.existsSync(RELEASE_FILE) ? fs.readFileSync(RELEASE_FILE, 'utf8') : '';
    const timer = setInterval(() => {
      const step = fs.existsSync(RELEASE_FILE) ? fs.readFileSync(RELEASE_FILE, 'utf8') : '';
      if (step === previous) return;
      previous = step;
      if (step === 'detach') fields({ status: 'detached' });
      if (step === 'followers') {
        for (let i = 1; i <= LINES; i++) tool(`follower-${i}`, `FOLLOWUP_${String(i).padStart(3, '0')}`, 'completed');
        record({ type: 'barrier', name: 'followers-created' });
      }
      if (step === 'progress') {
        fields({ status: 'detached', title: 'MUTATED_LAUNCH', raw_output: 'BACKGROUND_PROGRESS_ONLY' });
        tool('progress-fence', 'PROGRESS_FENCE', 'completed');
      }
      if (step === 'permission') {
        send({ event: 'permission_request', session_id: SESSION, request: {
          tool_call: { tool_call_id: 'background-primary', title: 'PRIMARY_TASK', kind: 'execute', status: 'detached', content: [], locations: [], meta: { claudeCode: { toolName: 'Bash' } } },
          options: [
            { option_id: 'allow-once', name: 'Allow once', kind: 'allow_once' },
            { option_id: 'deny-once', name: 'Deny once', kind: 'reject_once' },
          ], display: null, mcp_server: null,
        } });
      }
      if (['completed', 'failed', 'killed'].includes(step)) {
        for (let i = 0; i < 2; i++) fields({ status: step, title: 'PRIMARY_RESULT_TASK', raw_output: 'PRIMARY_FINAL_OUTPUT', content: [{ type: 'content', content: { type: 'text', text: 'PRIMARY_FINAL_OUTPUT' } }] });
        tool('result-fence', 'RESULT_FENCE', 'completed');
      }
      if (step === 'turn-end') finish({ event: 'turn_complete', session_id: SESSION });
    }, INTERVAL_MS);
    active = { timer };
    record({ type: 'barrier', name: `background-turn-${replyNumber}-held` });
    return;
  }
  if (!activityScenario || replyNumber !== 1) send({
    event: 'session_update',
    session_id: SESSION,
    update: {
      type: 'agent_message_chunk',
      content: { type: 'text', text: `reply ${replyNumber} started\n` },
      source_message_uuid: null,
    },
  });
  if (presentationScenario) {
    for (const [role, uuid] of [['user', messageUuid], ['assistant', undefined]]) send({ event: 'session_update', session_id: SESSION, update: { type: 'message_metadata', role, timestamp: '2026-10-03T15:02:01Z', source_message_uuid: uuid } });
    if (SCENARIO === 'presentation') send({ event: 'session_update', session_id: SESSION, update: { type: 'agent_message_chunk', content: { type: 'text', text: '\n```rust\nlet answer = 42;\n```\n' }, source_message_uuid: null } });
  }
  if (notificationScenario) {
    const alert = notification => send({ event: 'session_update', session_id: SESSION, update: { type: 'notification_update', notification, replay: false } });
    const native = { origin: 'sdk_notice', session_id: SESSION, uuid: `notice-${replyNumber}`, key: 'replaceable', text: `Native notice ${replyNumber}`, priority: 'immediate', color: 'yellow', timeout_ms: 5000 };
    alert(native); alert(native);
    const proactive = { origin: 'model_tool', session_id: SESSION, tool_use_id: `push-${replyNumber}`, text: 'Review is ready', push_sent: true, local_sent: false, disabled_reason: 'no_transport', sent_at: '2026-10-04T10:00:00Z' };
    alert(proactive); alert(proactive);
    alert({ ...proactive, tool_use_id: `upstream-${replyNumber}`, local_sent: true });
    send({ event: 'session_update', session_id: SESSION, update: { type: 'notification_update', notification: { ...proactive, tool_use_id: `replay-${replyNumber}` }, replay: true } });
  }
  if (SCENARIO === 'resize-replay') {
    const text = Array.from({ length: LINES }, (_, index) => `${index + 1}. streamed line ${index + 1}\n`).join('');
    send({ event: 'session_update', session_id: SESSION, update: { type: 'agent_message_chunk', content: { type: 'text', text }, source_message_uuid: null } });
    send({ event: 'turn_complete', session_id: SESSION, terminal_reason: 'completed', queued_turn_count: 0 });
    return;
  }
  // Follow-up replies stay short so their start marker remains on screen.
  const lines = replyNumber === 1 || resizeCodeScenario ? LINES : FOLLOW_UP_LINES;
  const gated = (replyNumber === 1 && SCENARIO.startsWith('hold-') && SCENARIO !== 'hold-presentation') || (resizeCodeScenario && replyNumber === 2);
  if (resizeCodeScenario && replyNumber === 2) send({ event: 'session_update', session_id: SESSION, update: { type: 'agent_message_chunk', content: { type: 'text', text: '\n```rust\n' }, source_message_uuid: null } });
  // Hold the first activity turn before any assistant content arrives.
  let line = activityScenario && replyNumber === 1 ? lines : 0;
  if (activityScenario && replyNumber === 1) record({ type: 'barrier', name: 'reply-held' });
  let activityStep = '';
  const timer = setInterval(() => {
    if (activityScenario && replyNumber === 1 && line >= lines) {
      const step = fs.existsSync(RELEASE_FILE) ? fs.readFileSync(RELEASE_FILE, 'utf8') : '';
      if (step === activityStep) return;
      activityStep = step;
      if (step === 'done') finish({ event: 'turn_complete', session_id: SESSION });
      else if (step === 'thinking' || step === 'working') send({ event: 'session_update', session_id: SESSION, update: { type: 'agent_activity_update', phase: step } });
      else if (step === 'requires_action' || step === 'running') send({ event: 'session_update', session_id: SESSION, update: { type: 'runtime_session_state_update', state: step } });
      return;
    }
    if (replyNumber === 1 && SCENARIO === 'hold-presentation' && line === 20 && !fs.existsSync(RELEASE_FILE)) return;
    if (gated && line >= lines) {
      if (!fs.existsSync(RELEASE_FILE)) return;
      if (SCENARIO === 'hold-eof') {
        record({ type: 'barrier', name: 'bridge-exiting' });
        process.exit(0);
      }
      if (SCENARIO === 'hold-malformed') {
        clearInterval(active.timer);
        record({ type: 'barrier', name: 'malformed-sent' });
        process.stdout.write('{broken event\n');
        return;
      }
      if (resizeCodeScenario && replyNumber === 2) send({ event: 'session_update', session_id: SESSION, update: { type: 'agent_message_chunk', content: { type: 'text', text: '```\n' }, source_message_uuid: null } });
      finish(SCENARIO === 'hold-error'
        ? { event: 'turn_error', session_id: SESSION, message: 'Fixture service unavailable', error_kind: 'transient_service' }
        : { event: 'turn_complete', session_id: SESSION });
      return;
    }
    line++;
    send({
      event: 'session_update',
      session_id: SESSION,
      update: {
        type: 'agent_message_chunk',
        content: { type: 'text', text: resizeCodeScenario && replyNumber === 2 ? `code line ${line}\n` : `${line}. streamed line ${line}\n` },
        source_message_uuid: null,
      },
    });
    if (SCENARIO === 'hold-presentation' && line === 20) record({ type: 'barrier', name: 'reading-barrier' });
    if (line >= lines) {
      if (gated) record({ type: 'barrier', name: 'reply-held' });
      else finish({ event: 'turn_complete', session_id: SESSION });
    }
  }, INTERVAL_MS);
  active = { timer };
  if (replyNumber === 1 && ['permission', 'question'].includes(SCENARIO)) {
    clearInterval(timer);
    const tool_call = {
      tool_call_id: 'fixture-tool', title: 'Fixture action', kind: 'other',
      status: 'pending', source_message_uuid: null, content: [],
      raw_input: null, raw_output: null, locations: [], meta: null,
    };
    send({ event: 'session_update', session_id: SESSION, update: { type: 'tool_call', tool_call } });
    send(SCENARIO === 'permission' ? {
      event: 'permission_request', session_id: SESSION,
      request: { tool_call, options: [
        { option_id: 'allow-once', name: 'Allow once', kind: 'allow_once', description: null },
        { option_id: 'deny-once', name: 'Deny once', kind: 'reject_once', description: null },
      ], display: null, mcp_server: null },
    } : {
      event: 'question_request', session_id: SESSION,
      request: { tool_call, question_index: 0, total_questions: 1, prompt: {
        question: 'Choose fixture destination', header: 'Destination', multi_select: false,
        options: [
          { option_id: 'alpha', label: 'Alpha', description: null, preview: null },
          { option_id: 'beta', label: 'Beta', description: null, preview: null },
        ],
      } },
    });
  }
}

readline
  .createInterface({ input: process.stdin })
  .on('line', raw => {
    let message;
    try {
      message = JSON.parse(raw);
    } catch {
      return;
    }
    record({ type: 'command', ...message });
    switch (message.command) {
      case 'initialize':
        cwd = message.cwd;
        send({
          event: 'initialized',
          result: {
            agent_name: 'fake',
            agent_version: '0.0.0',
            auth_methods: [],
            capabilities: {
              prompt_image: false,
              prompt_embedded_context: false,
              supports_session_listing: false,
              supports_resume_session: false,
            },
          },
        });
        break;
      case 'create_session':
      case 'new_session':
      case 'resume_session':
        if (message.command === 'new_session') SESSION = 'fake-session-after-login';
        if (message.command === 'resume_session') SESSION = message.session_id;
        if (message.continue_session) SESSION = 'fake-recent-session';
        send({
          event: message.command === 'new_session' ? 'session_replaced' : 'connected',
          session_id: SESSION,
          cwd: message.cwd ?? cwd,
          current_model: model,
          available_models: [],
          mode: null,
          fast_mode_state: 'off',
          fast_mode_disabled_reason: null,
          history_updates: SCENARIO === 'background-resume' ? [
            { type: 'user_message_chunk', content: { type: 'text', text: 'Resumed background launch' }, source_message_uuid: 'saved-user' },
            { type: 'tool_call', tool_call: { tool_call_id: 'background-primary', title: 'PRIMARY_TASK', kind: 'execute', status: 'in_progress', content: [], raw_input: { command: 'PRIMARY_TASK' }, locations: [], meta: { claudeCode: { toolName: 'Bash' } } } },
            { type: 'tool_call_update', tool_call_update: { tool_call_id: 'background-primary', fields: { status: 'detached' } } },
          ] : null,
          restored_input: null,
        });
        if (SCENARIO.startsWith('resize-') || SCENARIO.startsWith('background-')) send({ event: 'status_snapshot', session_id: SESSION, account: { subscription_type: 'Fixture subscription' } });
        if (SCENARIO === 'disconnect-during-auth' && message.command === 'create_session') {
          const timer = setInterval(() => {
            if (fs.existsSync(RELEASE_FILE)) {
              clearInterval(timer);
              record({ type: 'barrier', name: 'bridge-exiting' });
              process.exit(0);
            }
          }, INTERVAL_MS);
        }
        break;
      case 'prompt':
        if (active) {
          pending.push(message.message_uuid);
          send({ event: 'user_message_queued', session_id: SESSION, message_uuid: message.message_uuid });
        } else streamReply(message.message_uuid);
        break;
      case 'inspect_settings':
        if (notificationScenario) {
          notificationAppPath = path.join(process.env.CLAUDE_CONFIG_DIR, 'app-settings.json');
          if (!fs.existsSync(notificationAppPath)) fs.writeFileSync(notificationAppPath, JSON.stringify({ notifications: { actionsRequired: true, modelDirected: true, turnComplete: false } }));
        }
        send({ event: 'settings_result', session_id: SESSION, request_id: message.request_id, result: {
          persistence: 'not_requested', application: 'blocked', snapshot: settingsSnapshot(),
        } });
        break;
      case 'mutate_setting': {
        const mutation = message.mutation;
        const definition = settingDefinitions.find(([id]) => id === mutation.id);
        if (!definition || mutation.scope !== 'user' || mutation.expected_revision !== String(settingsRevision)) throw new Error('Unexpected settings mutation');
        const file = notificationScenario && mutation.id.startsWith('notifications.') ? notificationAppPath : path.join(process.env.CLAUDE_CONFIG_DIR, 'settings.json');
        const document = JSON.parse(fs.readFileSync(file, 'utf8'));
        const keys = definition[4];
        let parent = document;
        for (const key of keys.slice(0, -1)) parent = parent[key] ??= {};
        if (mutation.operation === 'remove') {
          delete parent[keys.at(-1)];
          preferences[mutation.id] = undefined;
        } else {
          parent[keys.at(-1)] = mutation.value;
          preferences[mutation.id] = mutation.value;
        }
        fs.writeFileSync(file, JSON.stringify(document));
        settingsRevision++;
        send({ event: 'settings_result', session_id: SESSION, request_id: message.request_id, result: {
          persistence: 'saved', application: presentationScenario || notificationScenario || activityScenario ? 'host' : 'next_session', snapshot: settingsSnapshot(),
        } });
        break;
      }
      case 'cancel_turn':
        send({ event: 'turn_interrupt_receipt', session_id: SESSION, still_queued: pending.slice(), request_id: message.request_id });
        if (active) finish({ event: 'turn_complete', session_id: SESSION, terminal_reason: 'aborted_streaming', queued_turn_count: pending.length });
        break;
      case 'permission_response':
      case 'question_response':
        if (SCENARIO.startsWith('background-') && message.tool_call_id === 'background-primary') {
          record({ type: 'barrier', name: 'background-permission-answered' });
          break;
        }
        if (!active || message.tool_call_id !== 'fixture-tool') throw new Error('Unexpected interaction response');
        send({ event: 'session_update', session_id: SESSION, update: {
          type: 'tool_call_update', tool_call_update: {
            tool_call_id: 'fixture-tool', source_message_uuid: null,
            fields: { status: message.outcome.option_id === 'deny-once' || message.outcome.outcome === 'cancelled' ? 'failed' : 'completed' },
          },
        } });
        finish({ event: 'turn_complete', session_id: SESSION });
        break;
      case 'shutdown':
        process.exit(0);
    }
  })
  .on('close', () => process.exit(0));
