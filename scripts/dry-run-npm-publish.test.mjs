import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { PLATFORM_PACKAGES, ROOT_PACKAGE_NAME } from "./npm-package-config.mjs";
import {
  expectedPublishTarballs,
  npmTarballName,
  resolveNpmInvocation,
} from "./dry-run-npm-publish.mjs";

test("npmTarballName matches npm scoped package tarball filenames", () => {
  assert.equal(
    npmTarballName("@srothgan/claude-code-rust-linux-x64-gnu", "1.2.3"),
    "srothgan-claude-code-rust-linux-x64-gnu-1.2.3.tgz",
  );
  assert.equal(npmTarballName(ROOT_PACKAGE_NAME, "1.2.3"), "claude-code-rust-1.2.3.tgz");
});

test("expectedPublishTarballs includes all platform packages before root", () => {
  const tarballs = expectedPublishTarballs("dist-pack", "1.2.3");

  assert.equal(tarballs.length, PLATFORM_PACKAGES.length + 1);
  assert.deepEqual(
    tarballs.slice(0, PLATFORM_PACKAGES.length).map((entry) => entry.packageName),
    PLATFORM_PACKAGES.map((entry) => entry.packageName),
  );
  assert.equal(tarballs.at(-1).packageName, ROOT_PACKAGE_NAME);
  assert.equal(tarballs.at(-1).tarball, path.join("dist-pack", "claude-code-rust-1.2.3.tgz"));
});

test("resolveNpmInvocation uses direct npm outside Windows", () => {
  assert.deepEqual(resolveNpmInvocation(["publish"], { platform: "linux" }), {
    command: "npm",
    args: ["publish"],
  });
});

test("resolveNpmInvocation uses npm CLI script on Windows", () => {
  const execPath = path.join("node-home", "node.exe");
  const npmCli = path.join("node-home", "node_modules", "npm", "bin", "npm-cli.js");

  assert.deepEqual(
    resolveNpmInvocation(["publish", "pkg.tgz"], {
      env: {},
      execPath,
      existsSync: (candidate) => candidate === npmCli,
      platform: "win32",
    }),
    {
      command: execPath,
      args: [npmCli, "publish", "pkg.tgz"],
    },
  );
});

test("resolveNpmInvocation prefers npm_execpath on Windows", () => {
  const execPath = path.join("node-home", "node.exe");
  const npmCli = path.join("custom-npm", "npm-cli.js");

  assert.deepEqual(
    resolveNpmInvocation(["view", "pkg"], {
      env: { npm_execpath: npmCli },
      execPath,
      existsSync: (candidate) => candidate === npmCli,
      platform: "win32",
    }),
    {
      command: execPath,
      args: [npmCli, "view", "pkg"],
    },
  );
});
