#!/usr/bin/env node
"use strict";

const { spawn } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const TARGETS = {
  "darwin:arm64": {
    packageName: "@srothgan/claude-code-rust-darwin-arm64",
    exe: "claude-rs",
    display: "darwin:arm64"
  },
  "darwin:x64": {
    packageName: "@srothgan/claude-code-rust-darwin-x64",
    exe: "claude-rs",
    display: "darwin:x64"
  },
  "linux:x64": {
    packageName: "@srothgan/claude-code-rust-linux-x64-gnu",
    exe: "claude-rs",
    libc: "glibc",
    display: "linux:x64 glibc"
  },
  "linux:arm64": {
    packageName: "@srothgan/claude-code-rust-linux-arm64-gnu",
    exe: "claude-rs",
    libc: "glibc",
    display: "linux:arm64 glibc"
  },
  "win32:x64": {
    packageName: "@srothgan/claude-code-rust-win32-x64-msvc",
    exe: "claude-rs.exe",
    display: "win32:x64"
  },
  "win32:arm64": {
    packageName: "@srothgan/claude-code-rust-win32-arm64-msvc",
    exe: "claude-rs.exe",
    display: "win32:arm64"
  }
};

function detectLinuxLibc(processLike = process) {
  if (processLike.platform !== "linux") {
    return undefined;
  }

  try {
    const report = processLike.report?.getReport?.();
    const glibcVersion = report?.header?.glibcVersionRuntime;
    if (typeof glibcVersion === "string" && glibcVersion.length > 0) {
      return "glibc";
    }
  } catch {
    // A missing or disabled process report should not be mistaken for glibc.
  }

  return "musl";
}

function supportedPlatformsText() {
  return Object.values(TARGETS)
    .map((target) => target.display)
    .join(", ");
}

function selectTarget(processLike = process) {
  const key = `${processLike.platform}:${processLike.arch}`;
  const info = TARGETS[key];
  if (!info) {
    return {
      error:
        `Unsupported platform/arch for claude-rs: ${key}\n` +
        `Supported platforms: ${supportedPlatformsText()}\n` +
        "Use one of the supported npm platforms, or build claude-code-rust from source for this host."
    };
  }

  if (processLike.platform === "linux") {
    const libc = detectLinuxLibc(processLike);
    if (libc !== info.libc) {
      return {
        error:
          `Unsupported Linux libc for claude-rs: linux/${processLike.arch} ${libc}\n` +
          `linux/${processLike.arch} musl is not supported by the current npm packages.\n` +
          "Linux npm packages currently require glibc. Build claude-code-rust from source for this host."
      };
    }
  }

  return { key, info };
}

function resolveInstall(options = {}) {
  const processLike = options.processLike || process;
  const requireResolve = options.requireResolve || require.resolve;
  const existsSync = options.existsSync || fs.existsSync;
  const dirname = options.dirname || __dirname;
  const selected = selectTarget(processLike);
  if (selected.error) {
    return { error: selected.error };
  }

  const { key, info } = selected;
  let packageJsonPath;
  try {
    packageJsonPath = requireResolve(`${info.packageName}/package.json`);
  } catch (error) {
    if (error && error.code === "MODULE_NOT_FOUND") {
      return {
        error:
          `Missing platform package ${info.packageName} for ${key}.\n` +
          "This usually means npm optional dependencies were omitted, for example by `npm install --omit=optional`.\n" +
          "Check `npm config get omit`, then reinstall with:\n" +
          "  npm install -g claude-code-rust"
      };
    }
    throw error;
  }

  const binaryPath = path.join(path.dirname(packageJsonPath), "bin", info.exe);
  if (!existsSync(binaryPath)) {
    return {
      error:
        `Missing binary at ${binaryPath}\n` +
        `The installed ${info.packageName} package is incomplete. Reinstall with:\n` +
        "  npm install -g claude-code-rust"
    };
  }

  const bundledBridgeScript = path.join(dirname, "..", "agent-sdk", "dist", "bridge.js");
  if (!existsSync(bundledBridgeScript)) {
    return {
      error:
        `Missing bundled bridge at ${bundledBridgeScript}\n` +
        "The installed claude-code-rust package is incomplete. Reinstall with:\n" +
        "  npm install -g claude-code-rust"
    };
  }

  return { binaryPath, bundledBridgeScript };
}

function main() {
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
}

if (require.main === module) {
  main();
}

module.exports = {
  TARGETS,
  detectLinuxLibc,
  resolveInstall,
  selectTarget,
  supportedPlatformsText
};
