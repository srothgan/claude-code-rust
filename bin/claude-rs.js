#!/usr/bin/env node
"use strict";

const { spawn } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const TARGETS = {
  "darwin:arm64": {
    packageName: "@srothgan/claude-code-rust-darwin-arm64",
    exe: "claude-rs"
  },
  "darwin:x64": {
    packageName: "@srothgan/claude-code-rust-darwin-x64",
    exe: "claude-rs"
  },
  "linux:x64": {
    packageName: "@srothgan/claude-code-rust-linux-x64-gnu",
    exe: "claude-rs"
  },
  "win32:x64": {
    packageName: "@srothgan/claude-code-rust-win32-x64-msvc",
    exe: "claude-rs.exe"
  }
};

function resolveInstall() {
  const key = `${process.platform}:${process.arch}`;
  const info = TARGETS[key];
  if (!info) {
    return { error: `Unsupported platform/arch for claude-rs: ${key}` };
  }

  let packageJsonPath;
  try {
    packageJsonPath = require.resolve(`${info.packageName}/package.json`);
  } catch (error) {
    if (error && error.code === "MODULE_NOT_FOUND") {
      return {
        error:
          `Missing platform package ${info.packageName} for ${key}.\n` +
          "This usually means npm optional dependencies were omitted.\n" +
          "Check `npm config get omit`, then reinstall with:\n" +
          "  npm install -g claude-code-rust"
      };
    }
    throw error;
  }

  const binaryPath = path.join(path.dirname(packageJsonPath), "bin", info.exe);
  if (!fs.existsSync(binaryPath)) {
    return {
      error:
        `Missing binary at ${binaryPath}\n` +
        `The installed ${info.packageName} package is incomplete. Reinstall with:\n` +
        "  npm install -g claude-code-rust"
    };
  }

  const bundledBridgeScript = path.join(__dirname, "..", "agent-sdk", "dist", "bridge.js");
  if (!fs.existsSync(bundledBridgeScript)) {
    return {
      error:
        `Missing bundled bridge at ${bundledBridgeScript}\n` +
        "The installed claude-code-rust package is incomplete. Reinstall with:\n" +
        "  npm install -g claude-code-rust"
    };
  }

  return { binaryPath, bundledBridgeScript };
}

const resolved = resolveInstall();
if (resolved.error) {
  console.error(resolved.error);
  process.exit(1);
}

const child = spawn(resolved.binaryPath, process.argv.slice(2), {
  env: {
    ...process.env,
    CLAUDE_RS_AGENT_BRIDGE: process.env.CLAUDE_RS_AGENT_BRIDGE || resolved.bundledBridgeScript
  },
  stdio: "inherit",
  windowsHide: true
});

child.on("error", (error) => {
  console.error(`Failed to launch claude-rs: ${error.message}`);
  process.exit(1);
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});
