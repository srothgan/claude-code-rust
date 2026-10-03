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
// Only the first turn is gated. Tests release it after exercising the composer
// or a fullscreen surface, rather than racing a fixed reply duration.
const SCENARIO = process.env.FAKE_BRIDGE_SCENARIO ?? 'stream';
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
function settingsSnapshot() {
  return {
    cwd, context: 'fixture-settings', diagnostics: [], resolution_sources: [], provenance: {},
    catalog: settingDefinitions.map(([id, label, description, kind, key_path]) => ({
      id, label, description, kind, key_path, options: kind === 'boolean' ? [true, false] : [],
      allows_custom: kind !== 'boolean', writable_scopes: ['user'],
      reset: 'Reset removes the saved value here', application: 'next_session',
    })),
    sources: [{ scope: 'user', path: path.join(process.env.CLAUDE_CONFIG_DIR, 'settings.json'), status: 'valid',
      values: Object.entries(preferences).map(([id, value]) => ({ id, value, revision: String(settingsRevision) })) }],
    values: Object.entries(preferences).map(([id, value]) => ({ id, value, contributors: ['user'], policy_restricted: false })),
  };
}

function finish(event) {
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
  send({
    event: 'session_update',
    session_id: SESSION,
    update: {
      type: 'agent_message_chunk',
      content: { type: 'text', text: `reply ${++replyNumber} started\n` },
      source_message_uuid: null,
    },
  });
  // Follow-up replies stay short so their start marker remains on screen.
  const lines = replyNumber === 1 ? LINES : FOLLOW_UP_LINES;
  const gated = replyNumber === 1 && SCENARIO.startsWith('hold-');
  let line = 0;
  const timer = setInterval(() => {
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
        content: { type: 'text', text: `${line}. streamed line ${line}\n` },
        source_message_uuid: null,
      },
    });
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
          history_updates: null,
          restored_input: null,
        });
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
        send({ event: 'settings_result', session_id: SESSION, request_id: message.request_id, result: {
          persistence: 'not_requested', application: 'blocked', snapshot: settingsSnapshot(),
        } });
        break;
      case 'mutate_setting': {
        const mutation = message.mutation;
        const definition = settingDefinitions.find(([id]) => id === mutation.id);
        if (!definition || mutation.scope !== 'user' || mutation.expected_revision !== String(settingsRevision)) throw new Error('Unexpected settings mutation');
        const file = path.join(process.env.CLAUDE_CONFIG_DIR, 'settings.json');
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
          persistence: 'saved', application: 'next_session', snapshot: settingsSnapshot(),
        } });
        break;
      }
      case 'cancel_turn':
        send({ event: 'turn_interrupt_receipt', session_id: SESSION, still_queued: pending.slice(), request_id: message.request_id });
        if (active) finish({ event: 'turn_complete', session_id: SESSION, terminal_reason: 'aborted_streaming', queued_turn_count: pending.length });
        break;
      case 'permission_response':
      case 'question_response':
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
