import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { resolveRepoRoot } from "../shared/repo-root.mjs";

const repoRoot = resolveRepoRoot(import.meta.url);
const installerPath = path.join(repoRoot, "scripts", "install", "install.sh");
const selectedVersion = "9.8.7-preview.1+build.5";
const selectedTag = `v${selectedVersion}`;
const platformDir = unixPlatformDir();
const skipReason = platformDir ? false : `Unix installer test is not supported on ${process.platform}:${process.arch}`;

test("Unix installer skips a pinned same-version install before release downloads", { skip: skipReason }, () => {
  const result = runInstallerScenario();

  assert.equal(result.status, 0, result.output);
  assert.match(result.output, /already installed; no changes made/);
  assert.deepEqual(result.downloads, []);
  assert.equal(result.installedVersion, selectedVersion);
});

test("Unix installer treats a same-version update as a no-op even though update mode accepts prompts", { skip: skipReason }, () => {
  const result = runInstallerScenario({ extraArgs: ["--update"] });

  assert.equal(result.status, 0, result.output);
  assert.match(result.output, /already installed; no changes made/);
  assert.doesNotMatch(result.output, /Reinstalling claude-rs/);
  assert.deepEqual(result.downloads, []);
});

test("Unix installer --yes explicitly proceeds with a same-version reinstall", { skip: skipReason }, () => {
  const result = runInstallerScenario({ extraArgs: ["--yes"], allowReleasePayload: true });

  assert.equal(result.status, 0, result.output);
  assert.match(result.output, new RegExp(`Reinstalling claude-rs ${escapeRegex(selectedVersion)}`));
  assert.match(result.output, new RegExp(`Verified claude-rs ${escapeRegex(selectedVersion)}`));
  assert.deepEqual(result.downloads, [
    `https://github.com/srothgan/claude-code-rust/releases/download/${selectedTag}/SHA256SUMS`,
    `https://github.com/srothgan/claude-code-rust/releases/download/${selectedTag}/claude-code-rust-${selectedVersion}-${platformDir}.tar.gz`,
  ]);
  assert.equal(result.replacementMarker, true, "approved reinstall did not replace the existing app");
});

test("Unix installer resolves latest metadata but skips same-version release payloads", { skip: skipReason }, () => {
  const result = runInstallerScenario({ requestedRelease: "latest" });

  assert.equal(result.status, 0, result.output);
  assert.match(result.output, new RegExp(`Release ${escapeRegex(selectedTag)} selected`));
  assert.match(result.output, /already installed; no changes made/);
  assert.deepEqual(result.downloads, [
    "https://api.github.com/repos/srothgan/claude-code-rust/releases/latest",
  ]);
});

test("Unix installer compares release identity exactly", { skip: skipReason }, () => {
  const differentlyCasedTag = "v9.8.7-Preview.1+build.5";
  const result = runInstallerScenario({ requestedRelease: differentlyCasedTag });

  assert.equal(result.status, 1, "mock checksum failure should prove the different release proceeded");
  assert.doesNotMatch(result.output, /already installed; no changes made/);
  assert.deepEqual(result.downloads, [
    `https://github.com/srothgan/claude-code-rust/releases/download/${differentlyCasedTag}/SHA256SUMS`,
  ]);
});

function runInstallerScenario({ requestedRelease = selectedTag, extraArgs = [], allowReleasePayload = false } = {}) {
  const sandbox = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-version-guard-"));
  const homeDir = path.join(sandbox, "home");
  const tempDir = path.join(sandbox, "tmp");
  const installDir = path.join(sandbox, "app", "claude-rs");
  const binDir = path.join(sandbox, "bin");
  const shimDir = path.join(sandbox, "shims");
  const downloadLog = path.join(sandbox, "downloads.log");
  for (const directory of [homeDir, tempDir, installDir, binDir, shimDir]) {
    fs.mkdirSync(directory, { recursive: true });
  }

  writeOwnedInstall(installDir, selectedVersion);
  const releaseFixture = createReleaseFixture(sandbox, selectedVersion);
  writeCommandShims(shimDir);

  const env = { ...process.env };
  for (const name of Object.keys(env)) {
    if (name.startsWith("CLAUDE_RS_")) {
      delete env[name];
    }
  }
  delete env.CI;
  Object.assign(env, {
    HOME: homeDir,
    TMPDIR: tempDir,
    PATH: `${shimDir}${path.delimiter}${process.env.PATH ?? ""}`,
    MOCK_DOWNLOAD_LOG: downloadLog,
    MOCK_LATEST_TAG: selectedTag,
    MOCK_ALLOW_RELEASE_PAYLOAD: allowReleasePayload ? "1" : "0",
    MOCK_ARCHIVE_FILE: releaseFixture.archivePath,
    MOCK_CHECKSUM_FILE: releaseFixture.checksumsPath,
  });

  try {
    const commandResult = spawnSync(
      "sh",
      [
        installerPath,
        "--release",
        requestedRelease,
        "--install-dir",
        installDir,
        "--bin-dir",
        binDir,
        "--non-interactive",
        "--no-modify-path",
        "--keep-npm",
        ...extraArgs,
      ],
      {
        encoding: "utf8",
        env,
        maxBuffer: 10 * 1024 * 1024,
        timeout: 30_000,
      },
    );
    if (commandResult.error) {
      throw commandResult.error;
    }

    assert.deepEqual(fs.readdirSync(tempDir), [], "installer left temporary files behind");
    assert.equal(
      fs.existsSync(path.join(path.dirname(installDir), ".claude-rs-install.lock")),
      false,
      "installer left its install lock behind",
    );

    const downloads = fs.existsSync(downloadLog)
      ? fs.readFileSync(downloadLog, "utf8").split(/\r?\n/u).filter(Boolean)
      : [];
    const installedPackage = JSON.parse(fs.readFileSync(path.join(installDir, "package.json"), "utf8"));
    return {
      status: commandResult.status,
      signal: commandResult.signal,
      output: `${commandResult.stdout}${commandResult.stderr}`,
      downloads,
      installedVersion: installedPackage.version,
      replacementMarker: installedPackage.versionGuardReplacement === true,
    };
  } finally {
    fs.rmSync(sandbox, { recursive: true, force: true });
  }
}

function writeOwnedInstall(installDir, version) {
  fs.writeFileSync(
    path.join(installDir, "package.json"),
    `${JSON.stringify({ name: "claude-code-rust", version }, null, 2)}\n`,
    "utf8",
  );
  fs.writeFileSync(path.join(installDir, "claude-rs"), "existing binary\n", "utf8");
  fs.writeFileSync(path.join(installDir, "claude-rs-bridge-bun"), "existing runtime\n", "utf8");
}

function createReleaseFixture(sandbox, version) {
  const fixtureParent = path.join(sandbox, "release-fixture");
  const appRootName = `claude-code-rust-${version}-${platformDir}`;
  const appRoot = path.join(fixtureParent, appRootName);
  const archiveName = `${appRootName}.tar.gz`;
  const archivePath = path.join(sandbox, archiveName);
  const checksumsPath = path.join(sandbox, "SHA256SUMS");
  for (const directory of [
    appRoot,
    path.join(appRoot, "agent-sdk", "dist"),
    path.join(appRoot, "node_modules", "@anthropic-ai", "claude-agent-sdk"),
  ]) {
    fs.mkdirSync(directory, { recursive: true });
  }

  fs.writeFileSync(
    path.join(appRoot, "package.json"),
    `${JSON.stringify(
      { name: "claude-code-rust", version, private: true, versionGuardReplacement: true },
      null,
      2,
    )}\n`,
    "utf8",
  );
  fs.writeFileSync(path.join(appRoot, "THIRD-PARTY-NOTICES.md"), "test notices\n", "utf8");
  fs.writeFileSync(path.join(appRoot, "agent-sdk", "package.json"), '{"private":true}\n', "utf8");
  fs.writeFileSync(path.join(appRoot, "agent-sdk", "dist", "bridge.js"), "export {};\n", "utf8");
  fs.writeFileSync(path.join(appRoot, "agent-sdk", "dist", "types.js"), "export {};\n", "utf8");
  fs.writeFileSync(
    path.join(appRoot, "node_modules", "@anthropic-ai", "claude-agent-sdk", "package.json"),
    '{"name":"@anthropic-ai/claude-agent-sdk"}\n',
    "utf8",
  );

  const binaryPath = path.join(appRoot, "claude-rs");
  fs.writeFileSync(
    binaryPath,
    `#!/bin/sh
case "\${1:-}" in
  --version) printf '%s\\n' 'claude-rs ${version}' ;;
  --help) printf '%s\\n' 'mock help' ;;
  *) exit 0 ;;
esac
`,
    "utf8",
  );
  const runtimePath = path.join(appRoot, "claude-rs-bridge-bun");
  fs.writeFileSync(runtimePath, "#!/bin/sh\nexit 0\n", "utf8");
  fs.chmodSync(binaryPath, 0o755);
  fs.chmodSync(runtimePath, 0o755);

  const tarResult = spawnSync("tar", ["-czf", archivePath, "-C", fixtureParent, appRootName], {
    encoding: "utf8",
  });
  if (tarResult.error) {
    throw tarResult.error;
  }
  assert.equal(tarResult.status, 0, `${tarResult.stdout}${tarResult.stderr}`);

  const archiveSha256 = crypto.createHash("sha256").update(fs.readFileSync(archivePath)).digest("hex");
  fs.writeFileSync(checksumsPath, `${archiveSha256}  dist-install/${archiveName}\n`, "utf8");
  return { archivePath, checksumsPath };
}

function writeCommandShims(shimDir) {
  const curlPath = path.join(shimDir, "curl");
  fs.writeFileSync(
    curlPath,
    `#!/bin/sh
url=""
destination=""
head_only=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o | --output)
      shift
      destination="$1"
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
[ "$head_only" -eq 0 ] || exit 0
printf '%s\\n' "$url" >> "$MOCK_DOWNLOAD_LOG"
case "$url" in
  */releases/latest)
    printf '{"tag_name":"%s"}\\n' "$MOCK_LATEST_TAG" > "$destination"
    exit 0
    ;;
esac
if [ "$MOCK_ALLOW_RELEASE_PAYLOAD" != "1" ]; then
    printf '%s\\n' 'mock release payload download blocked' >&2
    exit 22
fi
case "$url" in
  */SHA256SUMS)
    cp "$MOCK_CHECKSUM_FILE" "$destination"
    ;;
  *)
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
}

function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}

function unixPlatformDir() {
  if (process.platform === "darwin" && ["arm64", "x64"].includes(process.arch)) {
    return `darwin-${process.arch}`;
  }
  if (process.platform === "linux" && ["arm64", "x64"].includes(process.arch)) {
    return `linux-${process.arch}-gnu`;
  }
  return undefined;
}
