// Minimal NDJSON bridge stand-in for terminal tests: completes the handshake,
// then streams a long first reply and short follow-up replies. No model or
// network is involved.
const readline = require('node:readline');

const SESSION = 'fake-session';
const LINES = Number(process.env.FAKE_BRIDGE_LINES ?? 1500);
const FOLLOW_UP_LINES = Number(process.env.FAKE_BRIDGE_FOLLOW_UP_LINES ?? 3);
const INTERVAL_MS = Number(process.env.FAKE_BRIDGE_INTERVAL_MS ?? 15);
let replyNumber = 0;

const send = event => process.stdout.write(`${JSON.stringify(event)}\n`);

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
  let line = 0;
  const timer = setInterval(() => {
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
      clearInterval(timer);
      send({ event: 'turn_complete', session_id: SESSION });
    }
  }, INTERVAL_MS);
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
        streamReply(message.message_uuid);
        break;
      case 'shutdown':
        process.exit(0);
    }
  })
  .on('close', () => process.exit(0));
