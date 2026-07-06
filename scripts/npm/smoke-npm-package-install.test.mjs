import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import {
  envWithoutSystemRuntimePath,
  runtimeDependencyPackageDirs,
  resolveExecutablesOnPath,
  sanitizedNpmEnvironment,
  startLocalRegistry,
} from "./smoke-npm-package-install.mjs";

function makeExecutable(filePath) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, "#!/usr/bin/env sh\n", "utf8");
  fs.chmodSync(filePath, 0o755);
}

test("envWithoutSystemRuntimePath removes every Unix Bun directory from PATH", () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-path-test-"));
  try {
    const firstBunDir = path.join(tempDir, "first");
    const secondBunDir = path.join(tempDir, "second");
    const nonBunDir = path.join(tempDir, "plain");
    makeExecutable(path.join(firstBunDir, "bun"));
    makeExecutable(path.join(secondBunDir, "bun"));
    fs.mkdirSync(nonBunDir, { recursive: true });

    const env = {
      PATH: [firstBunDir, nonBunDir, secondBunDir, ""].join(path.delimiter),
    };
    const filtered = envWithoutSystemRuntimePath(env, "linux");

    assert.equal(filtered.PATH, nonBunDir);
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("resolveExecutablesOnPath ignores non-executable Unix files", { skip: process.platform === "win32" }, () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-path-test-"));
  try {
    const binDir = path.join(tempDir, "bin");
    fs.mkdirSync(binDir, { recursive: true });
    fs.writeFileSync(path.join(binDir, "bun"), "", "utf8");

    assert.deepEqual(resolveExecutablesOnPath("bun", { PATH: binDir }, "linux"), []);
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("resolveExecutablesOnPath respects Windows PATHEXT path shapes", () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-path-test-"));
  try {
    const bunDir = path.join(tempDir, "bin");
    fs.mkdirSync(bunDir, { recursive: true });
    fs.writeFileSync(path.join(bunDir, "bun.exe"), "", "utf8");

    const matches = resolveExecutablesOnPath(
      "bun",
      { PATH: bunDir, PATHEXT: ".EXE;.CMD" },
      "win32",
      { includeWhere: false },
    );

    assert.equal(matches.length, 1);
    assert.equal(matches[0].path, path.join(bunDir, "bun.exe"));
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("sanitizedNpmEnvironment removes npm registry and token overrides", () => {
  const sanitized = sanitizedNpmEnvironment({
    PATH: "bin",
    npm_config_registry: "https://registry.example.test",
    NPM_CONFIG_USERCONFIG: "custom.npmrc",
    NPM_TOKEN: "secret",
    NODE_AUTH_TOKEN: "secret",
  });

  assert.deepEqual(sanitized, { PATH: "bin" });
});

test("runtimeDependencyPackageDirs skips missing incompatible optional packages", () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-lock-test-"));
  try {
    const installedPackage = path.join(tempDir, "node_modules", "present");
    fs.mkdirSync(installedPackage, { recursive: true });
    fs.writeFileSync(
      path.join(installedPackage, "package.json"),
      JSON.stringify({ name: "present", version: "1.0.0" }),
      "utf8",
    );

    const dirs = runtimeDependencyPackageDirs(
      {
        packages: {
          "node_modules/present": { version: "1.0.0" },
          "node_modules/missing-dev": { version: "1.0.0", dev: true },
          "node_modules/missing-darwin": {
            version: "1.0.0",
            optional: true,
            os: ["darwin"],
          },
        },
      },
      { nodeModulesRoot: path.join(tempDir, "node_modules"), platform: "win32", arch: "x64" },
    );

    assert.deepEqual(dirs, [installedPackage]);
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("startLocalRegistry returns 404 for unseeded packages instead of proxying", async () => {
  const registry = await startLocalRegistry(new Map());
  try {
    const response = await fetch(`${registry.url}/left-pad`);
    assert.equal(response.status, 404);
    assert.match(await response.text(), /local registry has no package metadata/);
  } finally {
    await registry.close();
  }
});
