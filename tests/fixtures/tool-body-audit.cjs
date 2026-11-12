// Deterministic NDJSON tool-body peer. Each source belongs to a distinct tool;
// completion is released only after the terminal test has observed all cards.
const fs = require('node:fs');

module.exports = ({ send, record, session, messageUuid, releaseFile, finish }) => {
  const update = update => send({ event: 'session_update', session_id: session, update });
  update({ type: 'agent_activity_update', phase: 'working' });
  for (const tool of [
    {
      tool_call_id: 'wire-bash', title: 'echo BASH_TITLE_ORCHID', kind: 'execute', status: 'failed',
      raw_input: { command: 'echo BASH_TITLE_ORCHID' },
      raw_output: '<tool_use_error>WIRE_DENIED_ORCHID</tool_use_error>', content: [], locations: [],
      meta: { claudeCode: { toolName: 'Bash' } },
    },
    {
      tool_call_id: 'wire-read', title: 'READ_TITLE_ORCHID', kind: 'read', status: 'completed',
      raw_input: { file_path: 'read_source.txt' }, raw_output: null,
      content: [{ type: 'content', content: { type: 'text', text: 'READ_CONTENT_ORCHID\nREAD_CONTENT_JASMINE' } }],
      locations: [{ path: 'read_source.txt' }],
      output_metadata: { read: { file_path: 'read_source.txt', start_line: 41 } },
      meta: { claudeCode: { toolName: 'Read' } },
    },
    {
      tool_call_id: 'wire-edit', title: 'EDIT_TITLE_IRIS', kind: 'edit', status: 'completed',
      raw_input: { file_path: 'wire_payload.rs' }, raw_output: null, locations: [],
      content: [{ type: 'diff', old_path: 'wire_payload.rs', new_path: 'wire_payload.rs', old: 'DIFF_OLD_JASMINE\n', new: 'DIFF_ADDED_ORCHID\n', repository: null }],
      meta: { claudeCode: { toolName: 'Edit' } },
    },
  ]) update({ type: 'tool_call', tool_call: tool });
  record({ type: 'barrier', name: 'body-wire-delivered', message_uuid: messageUuid });
  return setInterval(() => {
    if (!fs.existsSync(releaseFile) || fs.readFileSync(releaseFile, 'utf8') !== 'continue') return;
    finish({ event: 'turn_complete', session_id: session });
    record({ type: 'barrier', name: 'body-wire-completed', message_uuid: messageUuid });
  }, 15);
};
