// SPDX-License-Identifier: Apache-2.0
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const profile = process.env.CLAUDE_CONFIG_DIR;
const file = path.join(profile, 'persistent-plugins.json');
const argv = process.argv.slice(2);
fs.appendFileSync(path.join(profile, 'plugin-cli.jsonl'), JSON.stringify({ argv, cwd: process.cwd() }) + '\n');
const state = JSON.parse(fs.readFileSync(file, 'utf8'));
if (JSON.stringify(argv) === JSON.stringify(['plugin', 'list', '--json'])) {
  console.log(JSON.stringify(state.installed));
} else if (JSON.stringify(argv) === JSON.stringify(['plugin', 'list', '--available', '--json'])) {
  console.log(JSON.stringify({ available: state.available }));
} else if (JSON.stringify(argv) === JSON.stringify(['plugin', 'marketplace', 'list', '--json'])) {
  console.log(JSON.stringify(state.marketplaces));
} else {
  assert.equal(argv[0], 'plugin');
  const marketplace = argv[1] === 'marketplace';
  const action = argv[marketplace ? 2 : 1];
  const target = argv[marketplace ? 3 : 2];
  const failure = path.join(profile, 'plugin-failure');
  if (fs.existsSync(failure) && fs.readFileSync(failure, 'utf8') === action) {
    console.error('FIXTURE_PLUGIN_WRITE_DENIED');
    process.exit(7);
  }
  if (marketplace) {
    if (action === 'add') {
      assert.deepEqual(argv.slice(4), ['--scope', 'user']);
      state.marketplaces.push({ name: 'new-market', source: 'github', repo: target });
    } else {
      assert.equal(argv.length, 4);
      const index = state.marketplaces.findIndex(entry => entry.name === target);
      assert.ok(index >= 0, 'selected marketplace exists');
      if (action === 'remove') state.marketplaces.splice(index, 1);
      else { assert.equal(action, 'update'); state.marketplaces[index].repo = 'updated/repository'; }
    }
  } else {
    assert.equal(argv.length, 5);
    assert.equal(argv[3], '--scope');
    const scope = argv[4];
    assert.ok(['user', 'project', 'local'].includes(scope));
    if (action === 'install') {
      assert.ok(state.available.some(entry => entry.pluginId === target));
      state.installed.push({ id: target, scope, enabled: true, version: '1.0', ...(scope === 'user' ? {} : { projectPath: process.cwd() }) });
    } else {
      const index = state.installed.findIndex(entry => entry.id === target && entry.scope === scope && (scope === 'user' || path.resolve(entry.projectPath) === process.cwd()));
      assert.ok(index >= 0, 'selected registration exists in the requested cwd and scope');
      if (action === 'uninstall') state.installed.splice(index, 1);
      else if (action === 'enable' || action === 'disable') state.installed[index].enabled = action === 'enable';
      else { assert.equal(action, 'update'); state.installed[index].version = '2.0'; }
    }
  }
  fs.writeFileSync(file + '.next', JSON.stringify(state));
  fs.renameSync(file + '.next', file);
}
