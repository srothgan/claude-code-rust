// Minimal NDJSON bridge stand-in for terminal tests: completes the handshake,
// then streams a long first reply and short follow-up replies. No model or
// network is involved.
const readline = require('node:readline');
const fs = require('node:fs');

const SESSION = 'fake-session';
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

function finish(event) {
  clearInterval(active.timer);
  active = null;
  send(event);
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
  const gated = replyNumber === 1 && SCENARIO !== 'stream';
  let line = 0;
  const timer = setInterval(() => {
    if (gated && line >= lines) {
      if (!fs.existsSync(RELEASE_FILE)) return;
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
        send({
          event: 'connected',
          session_id: SESSION,
          cwd: message.cwd,
          current_model: model,
          available_models: [],
          mode: null,
          fast_mode_state: 'off',
          fast_mode_disabled_reason: null,
          history_updates: null,
        });
        break;
      case 'prompt':
        if (active) {
          pending.push(message.message_uuid);
          send({ event: 'user_message_queued', session_id: SESSION, message_uuid: message.message_uuid });
        } else streamReply(message.message_uuid);
        break;
      case 'shutdown':
        process.exit(0);
    }
  })
  .on('close', () => process.exit(0));
