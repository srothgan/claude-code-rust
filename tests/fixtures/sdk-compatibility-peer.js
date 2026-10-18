// A local CLI peer for the real SDK transport; no network, model or user configuration.
const readline = require('node:readline');
const fs = require('node:fs');
const path = require('node:path');
const send = value => process.stdout.write(`${JSON.stringify(value)}\n`);
readline.createInterface({input: process.stdin}).on('line', line => {
  const message = JSON.parse(line);
  if (message.type === 'control_request') {
    send({type: 'control_response', response: {subtype: 'success', request_id: message.request_id,
      response: {commands: [{name: 'received-argv', description: JSON.stringify(process.argv.slice(2))}], models: [], account: {}}}});
  } else if (message.type === 'user') {
    const frames = JSON.parse(fs.readFileSync(path.join(__dirname, 'sdk-citation-frames.json'), 'utf8'));
    for (const frame of frames) send(frame);
  }
});
