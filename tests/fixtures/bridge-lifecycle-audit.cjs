// SPDX-License-Identifier: Apache-2.0
// A direct child with explicit protocol/file barriers; never launches the SDK.
const fs = require('node:fs');
const path = require('node:path');
const readline = require('node:readline');
const root = __dirname;
const mode = JSON.parse(fs.readFileSync(path.join(root, 'options.json'), 'utf8')).mode;
const mark = (name, value) => {
  fs.writeFileSync(path.join(root, `${name}.tmp`), JSON.stringify(value));
  fs.renameSync(path.join(root, `${name}.tmp`), path.join(root, `${name}.json`));
};
const emit = value => fs.writeSync(1, `${JSON.stringify(value)}\n`);
const keepAlive = setInterval(() => {}, 1000);
const initialized = {event: 'initialized', result: {agent_name: 'fixture', agent_version: '0', auth_methods: [], capabilities: {prompt_image: false, prompt_embedded_context: false, supports_session_listing: false, supports_resume_session: false}}};
mark('ready', {pid: process.pid});
if (mode === 'blocked-writer') {
  const bytes = Buffer.alloc(64);
  const count = fs.readSync(0, bytes, 0, bytes.length, null);
  mark('prefix', {text: bytes.subarray(0, count).toString('utf8')});
  // No further stdin reads: the large command cannot flush. The process stays alive.
} else {
  let questions = 0;
  readline.createInterface({input: process.stdin}).on('line', line => {
    const command = JSON.parse(line);
    fs.appendFileSync(path.join(root, 'commands.ndjson'), `${line}\n`);
    if (command.command === 'shutdown') {
      if (mode !== 'unresponsive') process.exit(0);
    } else if (command.command === 'cancel_turn') {
      mark(command.session_id, command);
      if (mode === 'deadline' && command.session_id === 'emit-auth') {
        emit({event: 'auth_required', method_name: 'late-auth', method_description: 'late guidance'});
      }
    } else if (command.command === 'initialize') {
      mark('initialize', command);
      if (mode === 'failure') {
        emit({event: 'connection_failed', request_id: 'init-request-7', message: 'Safe SDK startup cause', startup_failure: {reason: 'fixture-policy', errors: ['Use the selected organization']}});
      } else if (mode === 'eof') {
        process.exit(7);
      } else if (mode === 'handshake') {
        const gate = setInterval(() => {
          if (!fs.existsSync(path.join(root, 'release.json'))) return;
          clearInterval(gate);
          emit(initialized);
        }, 5);
      }
    } else if (command.command === 'create_session' || command.command === 'resume_session') {
      mark('session', command);
      emit({event: 'auth_required', method_name: 'session-live', method_description: 'selected session command received'});
    } else if (command.command === 'side_question') {
      const event = {session_id: command.session_id, btw_id: command.btw_id, question: command.question};
      emit(questions++ === 0
        ? {...event, event: 'btw_failed', error: 'fixture SDK refusal'}
        : {...event, event: 'btw_result', answer: 'answer from child', metadata: {synthetic: false}});
    }
  });
  if (mode === 'pre-ready') {
    emit({event: 'auth_required', method_name: 'pre-ready-auth', method_description: 'complete this login'});
  }
}
