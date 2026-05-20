#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
const path = require("node:path");
const https = require("node:https");
const { spawnSync } = require("node:child_process");
const { pipeline } = require("node:stream/promises");

const TARGETS = {
  "darwin:arm64": { target: "aarch64-apple-darwin", exe: "claude-rs" },
  "darwin:x64": { target: "x86_64-apple-darwin", exe: "claude-rs" },
  "linux:x64": { target: "x86_64-unknown-linux-gnu", exe: "claude-rs" },
  "win32:x64": { target: "x86_64-pc-windows-msvc", exe: "claude-rs.exe" }
};

const MAX_REDIRECTS = 5;
const BRIDGE_RUNTIME_EXE =
  process.platform === "win32" ? "claude-rs-bridge-node.exe" : "claude-rs-bridge-node";

function getTargetInfo() {
  return TARGETS[`${process.platform}:${process.arch}`];
}

async function downloadFile(url, outPath, redirects = 0) {
  if (redirects > MAX_REDIRECTS) {
    throw new Error(`Too many redirects while downloading ${url}`);
  }

  await new Promise((resolve, reject) => {
    const req = https.get(
      url,
      { headers: { "User-Agent": "claude-code-rust-installer" } },
      (res) => {
        const status = res.statusCode ?? 0;

        if (status >= 300 && status < 400 && res.headers.location) {
          const nextUrl = new URL(res.headers.location, url).toString();
          res.resume();
          downloadFile(nextUrl, outPath, redirects + 1).then(resolve).catch(reject);
          return;
        }

        if (status !== 200) {
          const chunks = [];
          res.on("data", (chunk) => chunks.push(chunk));
          res.on("end", () => {
            const body = Buffer.concat(chunks).toString("utf8").trim();
            reject(new Error(`Download failed (${status}) for ${url}${body ? `: ${body}` : ""}`));
          });
          return;
        }

        pipeline(res, fs.createWriteStream(outPath)).then(resolve).catch(reject);
      }
    );

    req.on("error", reject);
  });
}

function installRenamedBridgeRuntime(installDir) {
  const sourcePath = process.execPath;
  const runtimePath = path.join(installDir, BRIDGE_RUNTIME_EXE);

  try {
    if (!sourcePath || !fs.existsSync(sourcePath)) {
      throw new Error("current Node.js executable could not be resolved");
    }

    if (path.resolve(sourcePath) !== path.resolve(runtimePath)) {
      fs.copyFileSync(sourcePath, runtimePath);
    }

    if (process.platform !== "win32") {
      fs.chmodSync(runtimePath, 0o755);
    }

    const result = spawnSync(runtimePath, ["--version"], {
      encoding: "utf8",
      windowsHide: true
    });
    const version = String(result.stdout || "").trim();

    if (result.status !== 0 || !/^v\d+\./.test(version)) {
      throw new Error(
        `copied runtime failed validation${result.stderr ? `: ${result.stderr.trim()}` : ""}`
      );
    }

    console.log(`Installed renamed Agent SDK bridge runtime ${BRIDGE_RUNTIME_EXE} (${version})`);
  } catch (error) {
    try {
      fs.rmSync(runtimePath, { force: true });
    } catch {
      // Best-effort cleanup only; the Rust binary can still fall back to `node`.
    }
    console.warn(
      `Skipping renamed Agent SDK bridge runtime: ${error.message}. ` +
        "claude-rs will fall back to the `node` executable on PATH."
    );
  }
}

async function main() {
  const info = getTargetInfo();
  if (!info) {
    const key = `${process.platform}:${process.arch}`;
    throw new Error(`Unsupported platform/arch for claude-code-rust package install: ${key}`);
  }

  const pkgJsonPath = path.join(__dirname, "..", "package.json");
  const pkg = JSON.parse(fs.readFileSync(pkgJsonPath, "utf8"));
  const version = process.env.npm_package_version || pkg.version;
  const tag = `v${version}`;
  const repo = "srothgan/claude-code-rust";
  const assetName = `claude-code-rust-${info.target}${info.exe.endsWith(".exe") ? ".exe" : ""}`;
  const url = `https://github.com/${repo}/releases/download/${tag}/${assetName}`;

  const installDir = path.join(__dirname, "..", "vendor", info.target);
  const binaryPath = path.join(installDir, info.exe);
  const tempPath = `${binaryPath}.tmp`;

  fs.mkdirSync(installDir, { recursive: true });
  await downloadFile(url, tempPath);
  fs.renameSync(tempPath, binaryPath);

  if (process.platform !== "win32") {
    fs.chmodSync(binaryPath, 0o755);
  }

  installRenamedBridgeRuntime(installDir);

  console.log(`Installed claude-code-rust ${version} (${info.target})`);
}

main().catch((error) => {
  console.error(`claude-code-rust postinstall failed: ${error.message}`);
  process.exit(1);
});
