import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { resolveRepoRoot } from "../shared/repo-root.mjs";
import {
  PLATFORM_PACKAGES,
  installArchiveName,
  readCargoPackageMetadata,
} from "../shared/npm-package-config.mjs";

const repoRoot = resolveRepoRoot(import.meta.url);
const installerPath = path.join(repoRoot, "scripts", "install", "install.sh");
const cargoPackage = readCargoPackageMetadata(path.join(repoRoot, "Cargo.toml"));
const releaseTag = `v${cargoPackage.version}`;
const platformPackage = unixPlatformPackage();
const skipReason = platformPackage ? false : `unsupported test host ${process.platform}:${process.arch}`;

const runningMessages = [
  "Resolving release",
  "Downloading release archive",
  "Verifying release archive",
  "Installing files",
  "Verifying installed command",
  "Running runtime diagnostics",
];

const successfulMessages = [
  `Release ${releaseTag} selected`,
  "Downloaded release archive",
  "Verified release archive integrity",
  "Installed files",
  "Verified claude-rs 0.0.0-mock",
];

test("Unix installer keeps successful redirected output completed-step-only", { skip: skipReason }, () => {
  const result = runInstallerScenario("success");
  assertScenarioResult(result, 0, successfulMessages);
  assert.equal(result.installed, true, "successful install did not create the installed command");
  assert.equal(result.launcherInstalled, true, "successful install did not create the launcher");
});

test("Unix installer warns when the Claude Code CLI is missing", { skip: skipReason }, () => {
  const result = runInstallerScenario("success", { claudeCliAvailable: false });
  assertScenarioResult(result, 0, [
    "Claude Code CLI ('claude') not found on PATH",
    "Install it from https://claude.com/claude-code",
  ]);
});

test("Unix installer stays silent when the Claude Code CLI is available", { skip: skipReason }, () => {
  const result = runInstallerScenario("success", { claudeCliAvailable: true });
  const output = `${result.stdout}${result.stderr}`;
  assert.doesNotMatch(output, /Claude Code CLI \('claude'\) not found on PATH/);
});

test("Unix installer preserves checksum-download failures without progress noise", { skip: skipReason }, () => {
  const result = runInstallerScenario("checksum-download-failure");
  assertScenarioResult(result, 1, [
    `Release ${releaseTag} selected`,
    `could not download SHA256SUMS for ${releaseTag}`,
  ]);
});

test("Unix installer preserves archive-unavailable failures without progress noise", { skip: skipReason }, () => {
  const result = runInstallerScenario("archive-unavailable");
  assertScenarioResult(result, 1, [
    `Release ${releaseTag} selected`,
    "install script is currently not available for this release",
  ]);
});

test("Unix installer preserves checksum-mismatch failures without progress noise", { skip: skipReason }, () => {
  const result = runInstallerScenario("checksum-mismatch");
  assertScenarioResult(result, 1, [
    `Release ${releaseTag} selected`,
    "Downloaded release archive",
    `checksum mismatch for ${result.archiveName}`,
  ]);
});

test("Unix installer keeps CI output completed-step-only", { skip: skipReason }, () => {
  const result = runInstallerScenario("ci");
  assertScenarioResult(result, 0, successfulMessages);
});

test("Unix installer keeps NO_COLOR output plain and completed-step-only", { skip: skipReason }, () => {
  const result = runInstallerScenario("no-color");
  assertScenarioResult(result, 0, successfulMessages);
});

function runInstallerScenario(scenario, { claudeCliAvailable = true } = {}) {
  const archiveName = installArchiveName(platformPackage, cargoPackage.version);
  const archivePath = path.join(repoRoot, "dist-install", archiveName);
  assert.ok(
    fs.existsSync(archivePath),
    `missing mock install archive ${archivePath}; run generate-install-archives.mjs --mock-binaries first`,
  );

  const sandbox = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-installer-progress-"));
  const homeDir = path.join(sandbox, "home");
  const tempDir = path.join(sandbox, "tmp");
  const installDir = path.join(sandbox, "app", "claude-rs");
  const binDir = path.join(sandbox, "bin");
  const shimDir = path.join(sandbox, "shims");
  const checksumsPath = path.join(sandbox, "SHA256SUMS");
  for (const directory of [homeDir, tempDir, binDir, shimDir]) {
    fs.mkdirSync(directory, { recursive: true });
  }

  const archiveSha256 = crypto.createHash("sha256").update(fs.readFileSync(archivePath)).digest("hex");
  const expectedSha256 = scenario === "checksum-mismatch" ? "0".repeat(64) : archiveSha256;
  fs.writeFileSync(checksumsPath, `${expectedSha256}  dist-install/${archiveName}\n`, "utf8");
  writeCommandShims(shimDir, { claudeCliAvailable });

  const env = { ...process.env };
  for (const name of Object.keys(env)) {
    if (name.startsWith("CLAUDE_RS_")) {
      delete env[name];
    }
  }
  delete env.CI;
  delete env.NO_COLOR;
  Object.assign(env, {
    HOME: homeDir,
    TMPDIR: tempDir,
    PATH: `${shimDir}${path.delimiter}${pathWithoutClaude(process.env.PATH ?? "")}`,
    TERM: "xterm",
    MOCK_ARCHIVE_FILE: archivePath,
    MOCK_CHECKSUM_FILE: checksumsPath,
    MOCK_DOWNLOAD_MODE: scenario,
  });
  if (scenario === "ci") {
    env.CI = "1";
  }
  if (scenario === "no-color") {
    env.NO_COLOR = "1";
  }

  let commandResult;
  try {
    commandResult = spawnSync(
      "sh",
      [
        installerPath,
        "--release",
        releaseTag,
        "--install-dir",
        installDir,
        "--bin-dir",
        binDir,
        "--yes",
        "--non-interactive",
        "--no-modify-path",
        "--keep-npm",
      ],
      {
        encoding: "utf8",
        env,
        maxBuffer: 10 * 1024 * 1024,
        timeout: 120_000,
      },
    );
    if (commandResult.error) {
      throw commandResult.error;
    }

    const lockPath = path.join(path.dirname(installDir), ".claude-rs-install.lock");
    assert.equal(fs.existsSync(lockPath), false, `${scenario} left the install lock behind`);
    assert.deepEqual(fs.readdirSync(tempDir), [], `${scenario} left temporary installer files behind`);

    return {
      archiveName,
      installed: fs.existsSync(path.join(installDir, platformPackage.binaryName)),
      launcherInstalled: fs.existsSync(path.join(binDir, "claude-rs")),
      status: commandResult.status,
      signal: commandResult.signal,
      stderr: commandResult.stderr,
      stdout: commandResult.stdout,
    };
  } finally {
    fs.rmSync(sandbox, { recursive: true, force: true });
  }
}

function assertScenarioResult(result, expectedStatus, expectedMessages) {
  assert.equal(result.signal, null, "installer was terminated by a signal");
  assert.equal(result.status, expectedStatus, `unexpected installer exit status\n${result.stderr}`);
  const output = `${result.stdout}${result.stderr}`;
  for (const message of expectedMessages) {
    assert.match(output, new RegExp(escapeRegex(message)), `missing installer message: ${message}`);
  }
  assert.doesNotMatch(output, /\u001b/, "redirected installer output contains ANSI escape bytes");
  assert.doesNotMatch(output, /\r/, "redirected installer output contains carriage returns");
  assert.doesNotMatch(output, /[\u2800-\u28ff]/u, "redirected installer output contains Braille spinner frames");
  for (const message of runningMessages) {
    assert.doesNotMatch(
      output,
      new RegExp(`(?:^|\\n)${escapeRegex(message)}(?:\\n|$)`),
      `redirected installer output contains a step-start line: ${message}`,
    );
  }
}

function writeCommandShims(shimDir, { claudeCliAvailable }) {
  const curlPath = path.join(shimDir, "curl");
  fs.writeFileSync(
    curlPath,
    `#!/bin/sh
url=""
destination=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      shift
      destination="$1"
      ;;
    http://* | https://*)
      url="$1"
      ;;
  esac
  shift
done
case "$url" in
  */SHA256SUMS)
    if [ "$MOCK_DOWNLOAD_MODE" = "checksum-download-failure" ]; then
      printf '%s\\n' 'mock checksum download failed' >&2
      exit 22
    fi
    cp "$MOCK_CHECKSUM_FILE" "$destination"
    ;;
  *)
    if [ "$MOCK_DOWNLOAD_MODE" = "archive-unavailable" ]; then
      printf '%s\\n' 'mock archive download failed' >&2
      exit 22
    fi
    cp "$MOCK_ARCHIVE_FILE" "$destination"
    ;;
esac
`,
    "utf8",
  );
  fs.chmodSync(curlPath, 0o755);

  const npmPath = path.join(shimDir, "npm");
  fs.writeFileSync(npmPath, "#!/bin/sh\nexit 1\n", "utf8");
  fs.chmodSync(npmPath, 0o755);

  if (claudeCliAvailable) {
    const claudePath = path.join(shimDir, "claude");
    fs.writeFileSync(claudePath, "#!/bin/sh\nexit 0\n", "utf8");
    fs.chmodSync(claudePath, 0o755);
  }
}

function pathWithoutClaude(pathValue) {
  return pathValue
    .split(path.delimiter)
    .filter((directory) => directory && !fs.existsSync(path.join(directory, "claude")))
    .join(path.delimiter);
}

function unixPlatformPackage() {
  if (process.platform === "win32") {
    return undefined;
  }
  const expectedDir =
    process.platform === "darwin"
      ? `darwin-${process.arch === "arm64" ? "arm64" : "x64"}`
      : process.platform === "linux"
        ? `linux-${process.arch === "arm64" ? "arm64" : "x64"}-gnu`
        : undefined;
  return PLATFORM_PACKAGES.find((entry) => entry.dir === expectedDir);
}

function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
