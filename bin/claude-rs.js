#!/usr/bin/env node
"use strict";

const { spawn } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const TARGETS = {
  "darwin:arm64": { target: "aarch64-apple-darwin", exe: "claude-rs" },
  "darwin:x64": { target: "x86_64-apple-darwin", exe: "claude-rs" },
  "linux:x64": { target: "x86_64-unknown-linux-gnu", exe: "claude-rs" },
  "win32:x64": { target: "x86_64-pc-windows-msvc", exe: "claude-rs.exe" }
};

const BRIDGE_RUNTIME_EXE =
  process.platform === "win32" ? "claude-rs-bridge-node.exe" : "claude-rs-bridge-node";
const BRIDGE_RUNTIME_VERSION_MARKER = ".bridge-node-version";

// Postinstall copies the installing Node as the bridge runtime, but postinstall
// only runs on package install - a later Node upgrade leaves the copy stale.
// Since this launcher runs under the user's current Node on every start, it can
// cheaply detect the mismatch via the version marker and refresh the copy.
// Best-effort: if the copy is locked by a running session or anything else
// fails, keep the existing runtime and retry on a future launch.
function refreshBridgeRuntime(installDir) {
  const runtimePath = path.join(installDir, BRIDGE_RUNTIME_EXE);
  const markerPath = path.join(installDir, BRIDGE_RUNTIME_VERSION_MARKER);
  const tempPath = `${runtimePath}.tmp-${process.pid}`;

  try {
    if (!fs.existsSync(runtimePath)) {
      return;
    }

    let markerVersion = "";
    try {
      markerVersion = fs.readFileSync(markerPath, "utf8").trim();
    } catch {
      // Missing or unreadable marker: treat the copy as stale and refresh.
    }
    if (markerVersion === process.version) {
      return;
    }

    fs.copyFileSync(process.execPath, tempPath);
    if (process.platform !== "win32") {
      fs.chmodSync(tempPath, 0o755);
    }
    fs.renameSync(tempPath, runtimePath);
    fs.writeFileSync(markerPath, `${process.version}\n`);
  } catch {
    try {
      fs.rmSync(tempPath, { force: true });
    } catch {
      // Leave the temp file for the OS/next run; the old runtime still works.
    }
  }
}

function resolveInstall() {
  const key = `${process.platform}:${process.arch}`;
  const info = TARGETS[key];
  if (!info) {
    return { error: `Unsupported platform/arch for claude-rs: ${key}` };
  }

  const binaryPath = path.join(__dirname, "..", "vendor", info.target, info.exe);
  if (!fs.existsSync(binaryPath)) {
    return {
      error:
        `Missing binary at ${binaryPath}\n` +
        "Reinstall with `npm install -g claude-code-rust` to fetch release artifacts."
    };
  }

  return { binaryPath };
}

const resolved = resolveInstall();
if (resolved.error) {
  console.error(resolved.error);
  process.exit(1);
}

refreshBridgeRuntime(path.dirname(resolved.binaryPath));

const child = spawn(resolved.binaryPath, process.argv.slice(2), {
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
