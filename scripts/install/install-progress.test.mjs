import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { resolveRepoRoot } from "../shared/repo-root.mjs";
import { runTerminalSession } from "./terminal-session.mjs";
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

test("Unix y/n confirmations ignore trailing input without a timed drain", { skip: process.platform !== "linux" }, async () => {
  for (const [input, expected, suffix] of [
    ["y", "yes"],
    ["n", "no"],
    ["y\n", "yes"],
    ["yes\n", "yes"],
    ["no\n", "no"],
    ["YES\n", "yes"],
    ["NO\n", "no"],
    ["y", "yes", "es\n"],
    ["n", "no", "o\n"],
  ]) {
    const result = await runConfirmationScenario(input, { suffix });
    assertConfirmationResult(result, expected);
    assert.match(result.output, /y Yes \/ N No/);
  }
});

test("Unix y/n confirmation ignores Enter and navigation; typed fallback reads a line", { skip: process.platform !== "linux" }, async () => {
  for (const [input, expected, typed] of [
    ["\ny", "yes"],
    ["\u001b[Cy", "yes"],
    ["\u001b[D\u001b[C\nn", "no"],
    ["\t\nhjkly", "yes"],
    ["\u001by", "yes"],
    ["\n", "no", true],
    ["yes\n", "yes", true],
    ["no\n", "no", true],
  ]) {
    assertConfirmationResult(await runConfirmationScenario(input, { typed }), expected);
  }
});

test("Unix installer keeps successful redirected output completed-step-only", { skip: skipReason }, () => {
  const result = runInstallerScenario("success");
  assertScenarioResult(result, 0, successfulMessages);
  assert.equal(result.installed, true, "successful install did not create the installed command");
  assert.equal(result.launcherInstalled, true, "successful install did not create the launcher");
  assert.equal(result.manualLinked, true, "successful install did not link the manual");
  const archiveInvocation = result.curlInvocations.find(
    (line) => line.includes(result.archiveName) && line.includes("--write-out"),
  );
  assert.ok(archiveInvocation, "archive download did not use the streaming curl path");
  for (const expectedArgument of [
    "--retry 3",
    "--connect-timeout 30",
    "--speed-limit 1024",
    "--speed-time 30",
    "--silent",
    "--show-error",
    "--dump-header",
  ]) {
    assert.match(
      archiveInvocation,
      new RegExp(escapeRegex(expectedArgument)),
      `archive download omitted ${expectedArgument}`,
    );
  }
  assert.doesNotMatch(archiveInvocation, /--progress-bar/, "archive download delegated rendering to curl");
});

test("Unix installer verify mode reports archive transfer diagnostics", { skip: skipReason }, () => {
  const result = runInstallerScenario("success", { verify: true });
  assertScenarioResult(result, 0, [
    "Download:",
    "in 2.000s",
    "HTTP 200",
    "Runtime diagnostics passed",
  ]);
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

test("Unix installer removes owned manual links on upgrade and uninstall", { skip: skipReason }, () => {
  const result = runInstallerScenario("success", { uninstall: true });
  assertScenarioResult(result, 0, successfulMessages);
  assert.equal(result.manualLinked, true);
  assert.equal(result.staleManualExists, false, "upgrade kept an obsolete owned manual link");
  assert.equal(result.manualExistsAfterUninstall, false, "uninstall kept its manual link");
});

test("Unix installer preserves another installation's manual", { skip: skipReason }, () => {
  const result = runInstallerScenario("success", { preserveManual: true, uninstall: true });
  assertScenarioResult(result, 0, successfulMessages);
  assert.equal(result.manualLinked, false);
  assert.equal(result.manualExistsAfterUninstall, true);
  assert.equal(result.manualContentAfterUninstall, "externally managed manual");
});

test("Unix installer links manuals correctly with relative install directories", { skip: skipReason }, () => {
  const result = runInstallerScenario("success", { relativeDirs: true, uninstall: true });
  assertScenarioResult(result, 0, successfulMessages);
  assert.equal(result.manualLinked, true);
  assert.equal(result.manualExistsAfterUninstall, false);
});

async function runConfirmationScenario(input, { suffix = "", typed = false } = {}) {
  // Run the real prompt functions in a controlling terminal, without installing anything.
  const source = fs.readFileSync(installerPath, "utf8").replace(/\r\n/gu, "\n");
  const functionsEnd = source.indexOf("\nneed_cmd() {");
  assert.ok(functionsEnd > 0, "installer prompt functions were not found");
  const harness = `${source.slice(0, functionsEnd)}
non_interactive=0
progress_enabled=${typed ? 0 : 1}
trap restore_tty EXIT
before=$(stty -g < /dev/tty)
if confirm_default_no "FIRST_QUESTION"; then echo FIRST_ANSWER=yes; else echo FIRST_ANSWER=no; fi
if confirm_default_no "SECOND_QUESTION"; then echo SECOND_ANSWER=yes; else echo SECOND_ANSWER=no; fi
after=$(stty -g < /dev/tty)
[ "$before" != "$after" ] || echo TTY_RESTORED
`;
  let firstSent = false;
  let secondSent = false;
  return await runTerminalSession(harness, (output, terminal) => {
    if (!firstSent && output.includes("FIRST_QUESTION")) {
      firstSent = true;
      terminal.type(input);
    }
    if (!secondSent && output.includes("SECOND_QUESTION")) {
      secondSent = true;
      // The suffix reaches the second prompt ahead of its answer, so a prompt that accepts stray keys answers no.
      terminal.type(`${suffix}${typed ? "yes\n" : "y"}`);
    }
  });
}

function assertConfirmationResult(result, expectedFirstAnswer) {
  assert.equal(result.status, 0, result.output);
  assert.match(result.output, new RegExp(`FIRST_ANSWER=${expectedFirstAnswer}`));
  assert.match(result.output, /SECOND_ANSWER=yes/);
  assert.match(result.output, /TTY_RESTORED/);
  assert.match(result.output, /\u001b\[\?25h|\[y\/N\]/u);
}

function runInstallerScenario(scenario, { claudeCliAvailable = true, verify = false, preserveManual = false, uninstall = false, relativeDirs = false } = {}) {
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
  const curlLogPath = path.join(sandbox, "curl.log");
  for (const directory of [homeDir, tempDir, binDir, shimDir]) {
    fs.mkdirSync(directory, { recursive: true });
  }
  const manualDir = path.join(sandbox, "share", "man", "man1");
  const manualPath = path.join(manualDir, "claude-rs.1");
  const staleManualPath = path.join(manualDir, "claude-rs-old.1");
  fs.mkdirSync(manualDir, { recursive: true });
  fs.symlinkSync(path.join(installDir, "share", "man", "man1", "claude-rs-old.1"), staleManualPath);
  if (preserveManual) {
    fs.writeFileSync(manualPath, "externally managed manual", "utf8");
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
    MOCK_CURL_LOG: curlLogPath,
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
    const installerArgs = [
      installerPath,
      "--release",
      releaseTag,
      "--install-dir",
      relativeDirs ? "app/claude-rs" : installDir,
      "--bin-dir",
      relativeDirs ? "bin" : binDir,
      "--yes",
      "--non-interactive",
      "--no-modify-path",
      "--keep-npm",
    ];
    if (verify) {
      installerArgs.push("--verify");
    }
    commandResult = spawnSync(
      "sh",
      installerArgs,
      {
        encoding: "utf8",
        cwd: sandbox,
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

    const result = {
      archiveName,
      curlInvocations: fs.existsSync(curlLogPath)
        ? fs.readFileSync(curlLogPath, "utf8").trim().split(/\r?\n/)
        : [],
      installed: fs.existsSync(path.join(installDir, platformPackage.binaryName)),
      launcherInstalled: fs.existsSync(path.join(binDir, "claude-rs")),
      status: commandResult.status,
      signal: commandResult.signal,
      stderr: commandResult.stderr,
      stdout: commandResult.stdout,
      manualLinked: fs.existsSync(manualPath) && fs.lstatSync(manualPath).isSymbolicLink(),
      staleManualExists: fs.readdirSync(manualDir).includes("claude-rs-old.1"),
    };
    if (uninstall) {
      const removal = spawnSync("sh", [installerPath, "--uninstall", "--install-dir", relativeDirs ? "app/claude-rs" : installDir, "--bin-dir", relativeDirs ? "bin" : binDir, "--yes", "--non-interactive", "--no-modify-path"], { encoding: "utf8", cwd: sandbox, env, timeout: 120_000 });
      assert.equal(removal.status, 0, `uninstall failed\n${removal.stderr}`);
      result.manualExistsAfterUninstall = fs.existsSync(manualPath);
      result.manualContentAfterUninstall = fs.existsSync(manualPath) ? fs.readFileSync(manualPath, "utf8") : undefined;
      assert.equal(fs.existsSync(path.join(binDir, "claude-rs")), false, "uninstall kept its launcher");
    }
    return result;
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
headers=""
stderr_destination=""
write_out=""
head_only=0
printf '%s\\n' "$*" >> "$MOCK_CURL_LOG"
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o | --output)
      shift
      destination="$1"
      ;;
    --dump-header)
      shift
      headers="$1"
      ;;
    --stderr)
      shift
      stderr_destination="$1"
      ;;
    --write-out)
      shift
      write_out="$1"
      ;;
    --head)
      head_only=1
      ;;
    http://* | https://*)
      url="$1"
      ;;
  esac
  shift
done
[ -z "$stderr_destination" ] || : > "$stderr_destination"
emit_error() {
  if [ -n "$stderr_destination" ]; then
    printf '%s\\n' "$1" > "$stderr_destination"
  else
    printf '%s\\n' "$1" >&2
  fi
}
case "$url" in
  */release-advisories.json)
    exit 22
    ;;
  */SHA256SUMS)
    if [ "$MOCK_DOWNLOAD_MODE" = "checksum-download-failure" ]; then
      emit_error 'mock checksum download failed'
      exit 22
    fi
    [ -z "$headers" ] || printf 'HTTP/1.1 200 OK\\r\\nContent-Length: %s\\r\\n\\r\\n' "$(wc -c < "$MOCK_CHECKSUM_FILE")" > "$headers"
    [ "$head_only" -eq 1 ] && exit 0
    cp "$MOCK_CHECKSUM_FILE" "$destination"
    ;;
  *)
    if [ "$MOCK_DOWNLOAD_MODE" = "archive-unavailable" ]; then
      emit_error 'mock archive download failed'
      exit 22
    fi
    archive_size="$(wc -c < "$MOCK_ARCHIVE_FILE")"
    [ -z "$headers" ] || printf 'HTTP/1.1 200 OK\\r\\nContent-Length: %s\\r\\n\\r\\n' "$archive_size" > "$headers"
    [ "$head_only" -eq 1 ] && exit 0
    cp "$MOCK_ARCHIVE_FILE" "$destination"
    if [ -n "$write_out" ]; then
      archive_speed="$((archive_size / 2))"
      printf '__CLAUDE_RS_DOWNLOAD_STATS__200\\t%s\\t%s\\t2.000' "$archive_size" "$archive_speed"
    fi
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
