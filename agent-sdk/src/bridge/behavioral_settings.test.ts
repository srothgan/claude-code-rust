import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import http from "node:http";
import { exec, spawnSync } from "node:child_process";
import { promisify } from "node:util";
import path from "node:path";
import { afterEach, beforeEach, test } from "node:test";
import type { Json, SettingsScope, SettingsSnapshot } from "../types.js";
import { inspectSettings, mutateSetting } from "./settings_service.js";

let root: string;
let cwd: string;
const oldProfile = process.env.CLAUDE_CONFIG_DIR;
beforeEach(async () => {
  root = await fs.mkdtemp(path.join(os.tmpdir(), "claude-rs-behavior-"));
  cwd = path.join(root, "project");
  process.env.CLAUDE_CONFIG_DIR = path.join(root, "profile");
  await fs.mkdir(path.join(cwd, ".claude"), { recursive: true });
  await fs.mkdir(process.env.CLAUDE_CONFIG_DIR);
});
afterEach(async () => {
  if (oldProfile === undefined) delete process.env.CLAUDE_CONFIG_DIR;
  else process.env.CLAUDE_CONFIG_DIR = oldProfile;
  await fs.rm(root, { recursive: true, force: true });
});
function change(
  snapshot: SettingsSnapshot,
  id: string,
  scope: SettingsScope,
  value?: Json,
) {
  const saved = snapshot.sources
    .find((source) => source.scope === scope)
    ?.values.find((value) => value.id === id);
  assert.ok(saved);
  return {
    context: snapshot.context,
    id,
    scope,
    expected_revision: saved.revision,
    operation: value === undefined ? ("remove" as const) : ("set" as const),
    ...(value === undefined ? {} : { value }),
  };
}

const runCommand = promisify(exec);
function fileFor(scope: SettingsScope): string {
  return scope === "user"
    ? path.join(process.env.CLAUDE_CONFIG_DIR as string, "settings.json")
    : path.join(
        cwd,
        ".claude",
        scope === "local" ? "settings.local.json" : "settings.json",
      );
}
async function documentAt(scope: SettingsScope): Promise<Record<string, Json>> {
  return JSON.parse(await fs.readFile(fileFor(scope), "utf8"));
}
function expectedDocument(
  seed: Record<string, Json>,
  keys: string[],
  value: Json,
): Record<string, Json> {
  const expected = structuredClone(seed);
  let current = expected;
  for (const key of keys.slice(0, -1)) {
    current[key] ??= {};
    current = current[key] as Record<string, Json>;
  }
  current[keys[keys.length - 1]] = value;
  return expected;
}
function scopedValue(
  snapshot: SettingsSnapshot | undefined,
  scope: SettingsScope,
  id: string,
) {
  return snapshot?.sources
    .find((source) => source.scope === scope)
    ?.values.find((entry) => entry.id === id)?.value;
}

test("structured behavioral saves persist each selected leaf and reset it without running command or HTTP hooks", async () => {
  const untouched = {
    future: { value: [1, "keep"] },
    sandbox: { futureOption: "keep" },
  };
  const personal = { future: { value: [2, "keep"] } };
  await fs.writeFile(fileFor("local"), JSON.stringify(untouched));
  await fs.writeFile(fileFor("user"), JSON.stringify(personal));
  const marker = path.join(root, "hook-executed");
  const script = path.join(root, "fixture-hook.cjs");
  await fs.writeFile(
    script,
    "require('node:fs').writeFileSync(process.argv[2], 'fixture hook ran');",
  );
  const command = `"${process.execPath}" "${script}" "${marker}"`;
  const requests: string[] = [];
  const server = http.createServer((request, response) => {
    requests.push(`${request.method} ${request.url}`);
    response.end("fixture observer");
  });
  try {
    await new Promise<void>((resolve, reject) => {
      server.once("error", reject);
      server.listen(0, "127.0.0.1", resolve);
    });
    const address = server.address();
    assert.ok(address && typeof address !== "string");
    const url = `http://127.0.0.1:${address.port}/preview-hook`;
    await runCommand(command, { cwd, timeout: 10_000, windowsHide: true });
    assert.equal(
      await fs.readFile(marker, "utf8"),
      "fixture hook ran",
      "the exact stored command can execute its observer",
    );
    await fs.unlink(marker);
    const control = await fetch(url, {
      method: "POST",
      body: "positive control",
    });
    await control.text();
    assert.deepEqual(
      requests,
      ["POST /preview-hook"],
      "the stored hook endpoint is reachable",
    );
    requests.length = 0;
    const cases: Array<[string, string[], Json, SettingsScope?]> = [
      [
        "permissions.allow",
        ["permissions", "allow"],
        ["Bash(npm run test *)", "Read(./docs/**)"],
      ],
      ["permissions.ask", ["permissions", "ask"], ["Bash(git push *)"]],
      ["permissions.deny", ["permissions", "deny"], ["Read(./.env)"]],
      [
        "permissions.additionalDirectories",
        ["permissions", "additionalDirectories"],
        ["../shared"],
      ],
      [
        "worktree.symlinkDirectories",
        ["worktree", "symlinkDirectories"],
        ["node_modules", ".cache"],
      ],
      ["worktree.sparsePaths", ["worktree", "sparsePaths"], ["src", "docs"]],
      ["autoMemoryDirectory", ["autoMemoryDirectory"], "~/custom-memory"],
      ["claudeMdExcludes", ["claudeMdExcludes"], ["**/generated/CLAUDE.md"]],
      ["plansDirectory", ["plansDirectory"], ".plans"],
      [
        "hooks",
        ["hooks"],
        {
          SessionStart: [
            {
              hooks: [
                { type: "command", command },
                { type: "http", url },
              ],
            },
          ],
          PreToolUse: [
            {
              matcher: "Bash",
              hooks: [
                {
                  type: "command",
                  command,
                  args: ["preserve spaces"],
                  timeout: 10,
                },
              ],
            },
          ],
          Stop: [
            {
              hooks: [
                {
                  type: "http",
                  url,
                  headers: { Authorization: "Bearer $HOOK_TOKEN" },
                  allowedEnvVars: ["HOOK_TOKEN"],
                },
              ],
            },
          ],
        },
      ],
      ["allowedHttpHookUrls", ["allowedHttpHookUrls"], [`${url}*`]],
      ["httpHookAllowedEnvVars", ["httpHookAllowedEnvVars"], ["HOOK_TOKEN"]],
      [
        "sandbox.filesystem.allowRead",
        ["sandbox", "filesystem", "allowRead"],
        ["./docs"],
      ],
      [
        "sandbox.filesystem.allowWrite",
        ["sandbox", "filesystem", "allowWrite"],
        ["./build"],
      ],
      [
        "sandbox.filesystem.denyRead",
        ["sandbox", "filesystem", "denyRead"],
        ["./.env"],
      ],
      [
        "sandbox.filesystem.denyWrite",
        ["sandbox", "filesystem", "denyWrite"],
        ["./.git"],
      ],
      [
        "sandbox.network.allowedDomains",
        ["sandbox", "network", "allowedDomains"],
        ["*.example.com"],
      ],
      [
        "sandbox.network.deniedDomains",
        ["sandbox", "network", "deniedDomains"],
        ["blocked.example.com"],
      ],
      [
        "sandbox.network.allowUnixSockets",
        ["sandbox", "network", "allowUnixSockets"],
        ["/tmp/example.sock"],
      ],
      [
        "sandbox.network.allowMachLookup",
        ["sandbox", "network", "allowMachLookup"],
        ["com.apple.coresimulator.*"],
      ],
      [
        "sandbox.network.httpProxyPort",
        ["sandbox", "network", "httpProxyPort"],
        3128,
      ],
      [
        "sandbox.network.socksProxyPort",
        ["sandbox", "network", "socksProxyPort"],
        1080,
      ],
      [
        "sandbox.network.tlsTerminate",
        ["sandbox", "network", "tlsTerminate"],
        { caCertPath: "~/ca.pem", caKeyPath: "~/ca.key" },
        "user",
      ],
      [
        "sandbox.credentials",
        ["sandbox", "credentials"],
        {
          files: [{ path: "~/.secret", mode: "deny" }],
          envVars: [{ name: "PRIVATE_TOKEN", mode: "deny" }],
        },
        "user",
      ],
      [
        "sandbox.ignoreViolations",
        ["sandbox", "ignoreViolations"],
        { "*": ["/tmp"] },
      ],
      [
        "sandbox.excludedCommands",
        ["sandbox", "excludedCommands"],
        ["git status"],
      ],
      [
        "sandbox.ripgrep",
        ["sandbox", "ripgrep"],
        { command: "rg", args: ["--hidden"] },
        "user",
      ],
    ];
    for (const [id, keys, value, scope = "local"] of cases) {
      const expected = structuredClone(value);
      const saved = await mutateSetting(
        cwd,
        change(await inspectSettings(cwd), id, scope, value),
      );
      assert.equal(saved.persistence, "saved", `${id}: ${saved.error}`);
      assert.equal(saved.application, "next_session", id);
      assert.deepEqual(scopedValue(saved.snapshot, scope, id), expected, id);
      assert.deepEqual(
        saved.snapshot?.values.find((entry) => entry.id === id)?.value,
        expected,
        id,
      );
      assert.deepEqual(
        await documentAt("local"),
        scope === "local"
          ? expectedDocument(untouched, keys, expected)
          : untouched,
        id,
      );
      assert.deepEqual(
        await documentAt("user"),
        scope === "user" ? expectedDocument(personal, keys, expected) : personal,
        id,
      );
      const reset = await mutateSetting(
        cwd,
        change(await inspectSettings(cwd), id, scope),
      );
      assert.equal(reset.persistence, "saved", `${id}: ${reset.error}`);
      assert.equal(
        scopedValue(reset.snapshot, scope, id),
        undefined,
        `${id}: reset removes only this leaf`,
      );
      assert.deepEqual(await documentAt("local"), untouched, id);
      assert.deepEqual(await documentAt("user"), personal, id);
      await assert.rejects(fs.access(marker), { code: "ENOENT" });
      assert.equal(server.listening, true, "HTTP observer remains available");
      assert.deepEqual(
        requests,
        [],
        `${id}: preview/save/reset must not invoke HTTP hooks`,
      );
    }
  } finally {
    if (server.listening) {
      server.closeAllConnections();
      await new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      );
    }
  }
});

test("empty permission lists persist explicitly while reset removes only the local leaf", async () => {
  const personal = {
    future: "keep-user",
    permissions: { deny: ["Read(./.env)"] },
  };
  const local = {
    future: "keep-local",
    permissions: { ask: ["Bash(git push *)"] },
  };
  const userBytes = JSON.stringify(personal);
  await fs.writeFile(fileFor("user"), userBytes);
  await fs.writeFile(fileFor("local"), JSON.stringify(local));
  const empty = await mutateSetting(
    cwd,
    change(await inspectSettings(cwd), "permissions.deny", "local", []),
  );
  assert.equal(empty.persistence, "saved");
  assert.deepEqual(await documentAt("local"), {
    ...local,
    permissions: { ...local.permissions, deny: [] },
  });
  assert.deepEqual(
    scopedValue(empty.snapshot, "local", "permissions.deny"),
    [],
  );
  assert.deepEqual(
    empty.snapshot?.values.find((value) => value.id === "permissions.deny")
      ?.value,
    ["Read(./.env)"],
  );
  const emptyBytes = await fs.readFile(fileFor("local"), "utf8");
  const repeated = await mutateSetting(
    cwd,
    change(await inspectSettings(cwd), "permissions.deny", "local", []),
  );
  assert.equal(repeated.persistence, "unchanged");
  assert.equal(await fs.readFile(fileFor("local"), "utf8"), emptyBytes);
  const additional = await mutateSetting(
    cwd,
    change(await inspectSettings(cwd), "permissions.deny", "local", [
      "Bash(rm *)",
    ]),
  );
  assert.equal(additional.persistence, "saved");
  assert.deepEqual(await documentAt("local"), {
    ...local,
    permissions: { ...local.permissions, deny: ["Bash(rm *)"] },
  });
  assert.deepEqual(
    scopedValue(additional.snapshot, "local", "permissions.deny"),
    ["Bash(rm *)"],
  );
  const merged = additional.snapshot?.values.find(
    (value) => value.id === "permissions.deny",
  )?.value;
  assert.ok(Array.isArray(merged));
  assert.deepEqual([...merged].sort(), ["Read(./.env)", "Bash(rm *)"].sort());
  const reset = await mutateSetting(
    cwd,
    change(await inspectSettings(cwd), "permissions.deny", "local"),
  );
  assert.equal(reset.persistence, "saved");
  assert.deepEqual(await documentAt("local"), local);
  assert.equal(
    scopedValue(reset.snapshot, "local", "permissions.deny"),
    undefined,
  );
  assert.deepEqual(
    reset.snapshot?.values.find((value) => value.id === "permissions.deny")
      ?.value,
    ["Read(./.env)"],
  );
  assert.equal(await fs.readFile(fileFor("user"), "utf8"), userBytes);
});

test("selected invalid structures fail validation at an allowed scope before any settings write", async () => {
  const cases: Array<[string, SettingsScope, Json, Json, RegExp]> = [
    [
      "permissions.allow",
      "local",
      [true],
      ["Read(./docs/**)"],
      /nonempty|entry/i,
    ],
    [
      "hooks",
      "local",
      { NotAnEvent: [] },
      { SessionStart: [] },
      /hook event.*NotAnEvent/i,
    ],
    [
      "hooks",
      "local",
      { PreToolUse: [{ hooks: [{ type: "command" }] }] },
      {
        PreToolUse: [
          { hooks: [{ type: "command", command: "node --version" }] },
        ],
      },
      /cannot load|format/i,
    ],
    [
      "sandbox.credentials",
      "user",
      { files: [{ path: "~/.secret", mode: "allow" }] },
      { files: [{ path: "~/.secret", mode: "deny" }] },
      /cannot load|format/i,
    ],
    [
      "sandbox.ignoreViolations",
      "local",
      { "*": true },
      { "*": ["/tmp"] },
      /cannot load|format/i,
    ],
    ["sandbox.network.httpProxyPort", "local", 0, 1, /port|65535/i],
    ["sandbox.network.socksProxyPort", "local", 65536, 65535, /port|65535/i],
    ["sandbox.network.httpProxyPort", "local", 2.5, 1080, /whole|port/i],
    [
      "sandbox.network.tlsTerminate",
      "user",
      { caCertPath: "~/ca.pem" },
      { caCertPath: "~/ca.pem", caKeyPath: "~/ca.key" },
      /both|certificate.*key/i,
    ],
  ];
  for (const [id, scope, invalid, valid, cause] of cases) {
    for (const target of ["user", "project", "local"] as const) {
      await fs.writeFile(
        fileFor(target),
        JSON.stringify({
          future: `preserve-${target}`,
          permissions: { deny: [`Read(./${target}-private)`] },
        }),
      );
    }
    const shown = await inspectSettings(cwd);
    assert.ok(
      shown.catalog
        .find((setting) => setting.id === id)
        ?.writable_scopes.includes(scope),
      `${id}: authored ${scope} scope must be allowed`,
    );
    const expectedValid = structuredClone(valid);
    const control = await mutateSetting(cwd, change(shown, id, scope, valid));
    assert.equal(
      control.persistence,
      "saved",
      `${id}: valid counterpart failed: ${control.error}`,
    );
    assert.deepEqual(scopedValue(control.snapshot, scope, id), expectedValid, id);
    const files = [fileFor("user"), fileFor("project"), fileFor("local")];
    const before = await Promise.all(files.map((file) => fs.readFile(file)));
    const result = await mutateSetting(
      cwd,
      change(await inspectSettings(cwd), id, scope, invalid),
    );
    assert.equal(result.persistence, "failure", `${id}: ${result.error}`);
    assert.match(result.error ?? "", cause, `${id}: intended validation cause`);
    for (let index = 0; index < files.length; index++) {
      assert.deepEqual(
        await fs.readFile(files[index]),
        before[index],
        `${id}/${scope}: no writes to ${files[index]}`,
      );
    }
  }
});

test("credential references remain editable data without reading files or exposing environment secrets", async () => {
  const secretFile = path.join(root, "real-credential.txt");
  const fileSecret = "A154_FIXTURE_FILE_SECRET_7139";
  const envSecret = "A154_FIXTURE_ENV_SECRET_8264";
  const envName = "CLAUDE_RS_AUDIT_CREDENTIAL";
  const original = `token=${fileSecret}\n`;
  await fs.writeFile(secretFile, original);
  const initial = {
    files: [{ path: secretFile, mode: "deny" }],
    envVars: [{ name: envName, mode: "deny" }],
  };
  const candidate = {
    files: [
      {
        path: secretFile,
        mode: "mask",
        extract: "token=(.+)",
        onExtractNoMatch: "deny",
      },
    ],
    envVars: [{ name: envName, mode: "deny" }],
  };
  await fs.writeFile(
    fileFor("user"),
    JSON.stringify({ future: "preserve", sandbox: { credentials: initial } }),
  );
  const child = spawnSync(process.execPath, ["--input-type=module", "-"], {
    input: `import assert from "node:assert/strict";
      import fs from "node:fs";
      import { syncBuiltinESMExports } from "node:module";
      import path from "node:path";
      import { fileURLToPath } from "node:url";
      const cwd = ${JSON.stringify(cwd)};
      const secretFile = ${JSON.stringify(secretFile)};
      const fileSecret = ${JSON.stringify(fileSecret)};
      const envSecret = ${JSON.stringify(envSecret)};
      const candidate = ${JSON.stringify(candidate)};
      const reads = [], writes = [], restore = [];
      function observe(file) {
        const name = file instanceof URL ? fileURLToPath(file) : Buffer.isBuffer(file) ? file.toString() : file;
        if (typeof name === "string" && path.resolve(name) === path.resolve(secretFile)) reads.push(name);
      }
      function replace(owner, key, wrap) {
        const original = owner[key];
        owner[key] = wrap(original);
        restore.push(() => { owner[key] = original; });
      }
      for (const key of ["readFileSync", "readFile", "openSync", "open", "createReadStream"]) {
        replace(fs, key, original => function(file, ...args) { observe(file); return original.call(this, file, ...args); });
      }
      replace(fs.promises, "readFile", original => function(file, ...args) { observe(file); return original.call(this, file, ...args); });
      for (const owner of [fs, fs.promises]) {
        for (const key of owner === fs ? ["writeFileSync", "writeFile"] : ["writeFile"]) {
          replace(owner, key, original => function(file, data, ...args) {
            writes.push(String(data)); return original.call(this, file, data, ...args);
          });
        }
      }
      replace(fs.promises, "open", original => async function(file, ...args) {
        observe(file);
        const handle = await original.call(this, file, ...args);
        replace(handle, "writeFile", write => function(data, ...options) {
          writes.push(String(data)); return write.call(this, data, ...options);
        });
        return handle;
      });
      syncBuiltinESMExports();
      try {
        assert.ok(fs.readFileSync(secretFile, "utf8").includes(fileSecret));
        assert.ok((await fs.promises.readFile(secretFile, "utf8")).includes(fileSecret));
        assert.ok(reads.length >= 2, "the real credential read observer must work");
        assert.equal(process.env[candidate.envVars[0].name], envSecret, "the referenced fixture environment value exists");
        reads.length = 0;
        const { inspectSettings, mutateSetting } = await import(${JSON.stringify(new URL("./settings_service.js", import.meta.url).href)});
        const { validateSettingValue } = await import(${JSON.stringify(new URL("./settings_values.js", import.meta.url).href)});
        const before = await inspectSettings(cwd);
        const descriptor = before.catalog.find(entry => entry.id === "sandbox.credentials");
        assert.ok(descriptor?.writable_scopes.includes("user"));
        await validateSettingValue(descriptor, candidate);
        const shown = before.sources.find(source => source.scope === "user").values.find(entry => entry.id === descriptor.id);
        const result = await mutateSetting(cwd, { context: before.context, id: descriptor.id, scope: "user", expected_revision: shown.revision, operation: "set", value: candidate });
        assert.equal(result.persistence, "saved", result.error);
        assert.equal(result.application, "next_session", result.error);
        assert.deepEqual(result.snapshot.sources.find(source => source.scope === "user").values.find(entry => entry.id === descriptor.id).value, candidate);
        const after = await inspectSettings(cwd);
        const references = after.sources.find(source => source.scope === "user").values.find(entry => entry.id === descriptor.id).value;
        assert.deepEqual(references, candidate);
        assert.deepEqual(after.values.find(entry => entry.id === descriptor.id).value, candidate);
        assert.deepEqual(JSON.parse(await fs.promises.readFile(${JSON.stringify(fileFor("user"))}, "utf8")), { future: "preserve", sandbox: { credentials: candidate } });
        assert.equal(reads.length, 0, "inspect, validation and save must not access referenced credential contents");
        assert.ok(writes.length > 0, "actual candidate and settings writes were observed");
        const serialized = JSON.stringify({ before, result, after, writes });
        for (const secret of [fileSecret, envSecret]) assert.equal(serialized.includes(secret), false, "secret values stay out of editable data, diagnostics and written artifacts");
        console.log(JSON.stringify({ references, reads: reads.length, writes: writes.length }));
      } finally {
        for (const undo of restore.reverse()) undo();
        syncBuiltinESMExports();
      }`,
    cwd,
    encoding: "utf8",
    timeout: 30_000,
    windowsHide: true,
    env: { ...process.env, [envName]: envSecret },
  });
  assert.equal(
    child.error,
    undefined,
    `${child.error?.message ?? ""} ${child.stderr}`,
  );
  assert.equal(child.status, 0, `signal=${child.signal}: ${child.stderr}`);
  const observed = JSON.parse(child.stdout);
  assert.deepEqual(observed.references, candidate);
  assert.equal(observed.reads, 0);
  for (const secret of [fileSecret, envSecret])
    assert.equal(`${child.stdout}${child.stderr}`.includes(secret), false);
  assert.equal(await fs.readFile(secretFile, "utf8"), original);
});
test("structured conflicts preserve externally added rules and unsupported objects", async () => {
  const file = path.join(cwd, ".claude", "settings.local.json");
  await fs.writeFile(file, JSON.stringify({ permissions: { allow: ["Read(./a)"] }, future: "preserve" }));
  const shown = await inspectSettings(cwd);
  await fs.writeFile(file, JSON.stringify({ permissions: { allow: ["Read(./b)"] }, future: "preserve" }));
  const result = await mutateSetting(cwd, change(shown, "permissions.allow", "local", ["Read(./c)"]));
  assert.equal(result.persistence, "conflict");
  assert.deepEqual(JSON.parse(await fs.readFile(file, "utf8")), { permissions: { allow: ["Read(./b)"] }, future: "preserve" });
});
