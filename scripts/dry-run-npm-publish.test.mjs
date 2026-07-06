import fs from "node:fs";
import os from "node:os";
import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { PLATFORM_PACKAGES, ROOT_PACKAGE_NAME } from "./npm-package-config.mjs";
import {
  dryRunNpmPublish,
  expectedPublishTarballs,
  npmPackageVersionExists,
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

test("npmPackageVersionExists returns true when npm reports the requested version", () => {
  const calls = [];

  const exists = npmPackageVersionExists("@scope/pkg", "1.2.3", {
    execNpm(args) {
      calls.push(args);
      return '"1.2.3"\n';
    },
  });

  assert.equal(exists, true);
  assert.deepEqual(calls, [["view", "@scope/pkg@1.2.3", "version", "--json"]]);
});

test("npmPackageVersionExists returns false for missing package versions", () => {
  const exists = npmPackageVersionExists("@scope/pkg", "1.2.3", {
    execNpm() {
      throw npmError({ stderr: "npm error code E404\nNo match found for version 1.2.3" });
    },
  });

  assert.equal(exists, false);
});

test("npmPackageVersionExists rejects unexpected npm lookup failures", () => {
  assert.throws(
    () =>
      npmPackageVersionExists("@scope/pkg", "1.2.3", {
        execNpm() {
          throw npmError({ stderr: "npm error code E500\nregistry unavailable" });
        },
      }),
    /Could not inspect npm package state for @scope\/pkg@1\.2\.3/,
  );
});

test("dryRunNpmPublish skips existing versions and dry-runs missing versions", () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-dry-run-"));
  try {
    const packDir = path.join(tempDir, "dist-pack");
    const version = "1.2.3";
    fs.mkdirSync(packDir, { recursive: true });

    const tarballs = expectedPublishTarballs(packDir, version);
    for (const entry of tarballs) {
      fs.writeFileSync(entry.tarball, "package bytes", "utf8");
    }

    const alreadyPublished = tarballs[0].packageName;
    const dryRuns = [];

    dryRunNpmPublish({
      packDir,
      version,
      execNpm(args) {
        if (args[0] !== "view") {
          throw new Error(`unexpected npm command: ${args.join(" ")}`);
        }
        if (args[1] === `${alreadyPublished}@${version}`) {
          return `"${version}"\n`;
        }
        throw npmError({ stderr: "npm error code E404\nNo match found" });
      },
      spawn(command, args) {
        dryRuns.push({ command, args });
        return { status: 0, stdout: "", stderr: "" };
      },
    });

    assert.equal(dryRuns.length, tarballs.length - 1);
    assert.equal(
      dryRuns.some((entry) => entry.args.includes(tarballs[0].tarball)),
      false,
    );
    assert.equal(
      dryRuns.every((entry) => entry.args.includes("--dry-run")),
      true,
    );
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

function npmError({ stdout = "", stderr = "" }) {
  const error = new Error("npm failed");
  error.stdout = Buffer.from(stdout, "utf8");
  error.stderr = Buffer.from(stderr, "utf8");
  return error;
}
