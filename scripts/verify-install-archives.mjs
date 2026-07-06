#!/usr/bin/env node
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  BUNDLED_BUN_RUNTIME_VERSION,
  PLATFORM_PACKAGES,
  expectedAgentSdkNativePackage,
  installArchiveBaseName,
  installArchiveFormat,
  installArchiveName,
  readBunRuntimeManifest,
  readCargoPackageMetadata
} from "./npm-package-config.mjs";
import {
  assertNoSymlinks,
  extractArchive,
  fileSha256,
  findSingleTopLevelDirectory,
  listRelativeFiles,
  normalizePath,
  readArgValue,
  readJson
} from "./install-archive-common.mjs";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..");

const AGENT_SDK_NATIVE_PACKAGES = [
  "@anthropic-ai/claude-agent-sdk-darwin-arm64",
  "@anthropic-ai/claude-agent-sdk-darwin-x64",
  "@anthropic-ai/claude-agent-sdk-linux-arm64",
  "@anthropic-ai/claude-agent-sdk-linux-arm64-musl",
  "@anthropic-ai/claude-agent-sdk-linux-x64",
  "@anthropic-ai/claude-agent-sdk-linux-x64-musl",
  "@anthropic-ai/claude-agent-sdk-win32-arm64",
  "@anthropic-ai/claude-agent-sdk-win32-x64"
];

const options = parseArgs(process.argv.slice(2));
if (options.help) {
  printHelp();
  process.exit(0);
}

const cargoPackage = readCargoPackageMetadata(path.join(repoRoot, "Cargo.toml"));
const expectedVersion = options.version ?? cargoPackage.version;
const archiveRoot = path.resolve(repoRoot, options.archiveRoot ?? "dist-install");
const manifestsDir = path.resolve(repoRoot, options.manifestsDir ?? path.join(archiveRoot, "manifests"));
const bunRuntimeManifest = readBunRuntimeManifest(repoRoot);
const failures = [];

expectEqual(bunRuntimeManifest.version, BUNDLED_BUN_RUNTIME_VERSION, "Bun runtime manifest version");

for (const platformPackage of PLATFORM_PACKAGES) {
  verifyPlatformArchive(platformPackage);
}

if (failures.length > 0) {
  console.error(`Install archive verification failed with ${failures.length} issue(s):`);
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log(`Verified ${PLATFORM_PACKAGES.length} install archives in ${path.relative(repoRoot, archiveRoot)}`);

function verifyPlatformArchive(platformPackage) {
  const archiveName = installArchiveName(platformPackage, expectedVersion);
  const archivePath = path.join(archiveRoot, archiveName);
  const context = `${platformPackage.dir} install archive`;

  if (!fs.existsSync(archivePath)) {
    fail(`${context} is missing: ${path.relative(repoRoot, archivePath)}`);
    return;
  }

  const extractDir = fs.mkdtempSync(path.join(os.tmpdir(), `claude-rs-verify-${platformPackage.dir}-`));
  try {
    extractArchive({ archivePath, destinationDir: extractDir });
    const appRoot = findSingleTopLevelDirectory(extractDir);
    expectEqual(path.basename(appRoot), installArchiveBaseName(platformPackage, expectedVersion), `${context} app root name`);
    verifyExtractedApp(platformPackage, appRoot, archivePath, archiveName, context);
  } catch (error) {
    fail(`${context}: ${error instanceof Error ? error.message : String(error)}`);
  } finally {
    fs.rmSync(extractDir, { recursive: true, force: true });
  }
}

function verifyExtractedApp(platformPackage, appRoot, archivePath, archiveName, context) {
  const files = listRelativeFiles(appRoot);
  const requiredFiles = [
    platformPackage.binaryName,
    platformPackage.bundledRuntimeName,
    "package.json",
    "LICENSE",
    "THIRD-PARTY-NOTICES.md",
    "agent-sdk/package.json",
    "agent-sdk/dist/bridge.js",
    "agent-sdk/dist/types.js",
    "node_modules/@anthropic-ai/claude-agent-sdk/package.json",
    `${packagePath(expectedAgentSdkNativePackage(platformPackage))}/package.json`
  ];

  assertNoSymlinksWithFailures(appRoot, context);
  expectFilesExist(files, requiredFiles, context);
  expectNoWrongNativePackages(files, platformPackage, context);
  expectNoForbiddenFiles(files, appRoot, context);
  expectArchiveSpecifics(platformPackage, appRoot, context);
  expectManifestMatches(platformPackage, appRoot, archivePath, archiveName, files, context);
}

function assertNoSymlinksWithFailures(appRoot, context) {
  try {
    assertNoSymlinks(appRoot, context);
  } catch (error) {
    fail(error instanceof Error ? error.message : String(error));
  }
}

function expectNoWrongNativePackages(files, platformPackage, context) {
  const expectedPackagePath = packagePath(expectedAgentSdkNativePackage(platformPackage));
  for (const packageName of AGENT_SDK_NATIVE_PACKAGES) {
    const nativePackagePath = packagePath(packageName);
    const present = files.some((file) => file === `${nativePackagePath}/package.json`);
    if (nativePackagePath === expectedPackagePath) {
      if (!present) {
        fail(`${context} is missing expected Agent SDK native package: ${packageName}`);
      }
      continue;
    }
    if (present) {
      fail(`${context} contains wrong-platform Agent SDK native package: ${packageName}`);
    }
  }
}

function expectNoForbiddenFiles(files, appRoot, context) {
  if (fs.existsSync(path.join(appRoot, "node_modules", ".bin"))) {
    fail(`${context} contains node_modules/.bin`);
  }

  for (const file of files) {
    if (isForbiddenFile(file)) {
      fail(`${context} contains forbidden file: ${file}`);
    }
  }
}

function isForbiddenFile(file) {
  const segments = file.split("/");
  const basename = segments.at(-1) ?? "";

  return (
    basename === ".env" ||
    basename === ".package-lock.json" ||
    basename === "package-lock.json" ||
    basename === "npm-shrinkwrap.json" ||
    basename.endsWith(".log") ||
    basename.endsWith(".map") ||
    basename.endsWith(".test.js") ||
    basename.endsWith(".spec.js") ||
    basename.endsWith(".ts") ||
    basename.endsWith(".mts") ||
    basename.endsWith(".cts") ||
    segments.includes(".git") ||
    segments.includes(".cache") ||
    segments.includes(".github") ||
    segments.includes("__tests__") ||
    segments.includes("test") ||
    segments.includes("tests") ||
    file.startsWith("agent-sdk/src/")
  );
}

function expectArchiveSpecifics(platformPackage, appRoot, context) {
  const isWindows = platformPackage.os.includes("win32");
  if (isWindows) {
    if (!platformPackage.binaryName.endsWith(".exe") || !platformPackage.bundledRuntimeName.endsWith(".exe")) {
      fail(`${context} Windows platform metadata must use .exe names`);
    }
    if (fs.existsSync(path.join(appRoot, "claude-rs")) || fs.existsSync(path.join(appRoot, "claude-rs-bridge-bun"))) {
      fail(`${context} contains Unix launcher names in Windows archive`);
    }
    return;
  }

  if (platformPackage.binaryName.endsWith(".exe") || platformPackage.bundledRuntimeName.endsWith(".exe")) {
    fail(`${context} Unix platform metadata must not use .exe names`);
  }
  if (process.platform !== "win32") {
    expectExecutable(path.join(appRoot, platformPackage.binaryName), `${context} native binary`);
    expectExecutable(path.join(appRoot, platformPackage.bundledRuntimeName), `${context} bundled Bun runtime`);
  }
}

function expectManifestMatches(platformPackage, appRoot, archivePath, archiveName, files, context) {
  const manifestPath = path.join(manifestsDir, `${platformPackage.dir}.json`);
  if (!fs.existsSync(manifestPath)) {
    fail(`${context} manifest is missing: ${path.relative(repoRoot, manifestPath)}`);
    return;
  }

  const manifest = readJson(manifestPath);
  expectEqual(manifest.manifestVersion, 1, `${context} manifest version`);
  expectEqual(manifest.platform, platformPackage.dir, `${context} manifest platform`);
  expectEqual(manifest.version, expectedVersion, `${context} manifest package version`);
  expectEqual(manifest.archiveName, archiveName, `${context} manifest archive name`);
  expectEqual(manifest.archiveFormat, installArchiveFormat(platformPackage), `${context} manifest archive format`);
  expectEqual(manifest.archiveSha256, fileSha256(archivePath), `${context} manifest archive SHA256`);
  expectEqual(manifest.fileCount, files.length, `${context} manifest file count`);
  expectDeepEqual(manifest.files, files, `${context} manifest files`);
  expectEqual(
    manifest.expectedAgentSdkNativePackage,
    expectedAgentSdkNativePackage(platformPackage),
    `${context} manifest expected native package`
  );
  expectEqual(manifest.bundledRuntime?.name, platformPackage.bundledRuntimeName, `${context} manifest runtime name`);
  expectEqual(manifest.bundledRuntime?.version, bunRuntimeManifest.version, `${context} manifest runtime version`);
  expectEqual(
    manifest.bundledRuntime?.runtimeSha256,
    fileSha256(path.join(appRoot, platformPackage.bundledRuntimeName)),
    `${context} manifest runtime SHA256`
  );
}

function expectFilesExist(files, expectedFiles, context) {
  for (const expectedFile of expectedFiles) {
    if (!files.includes(expectedFile)) {
      fail(`${context} is missing required file: ${expectedFile}`);
    }
  }
}

function expectExecutable(filePath, label) {
  if (!fs.existsSync(filePath)) {
    fail(`${label} is missing`);
    return;
  }

  const mode = fs.statSync(filePath).mode;
  if ((mode & 0o111) === 0) {
    fail(`${label} is not executable: ${normalizePath(path.relative(repoRoot, filePath))}`);
  }
}

function packagePath(packageName) {
  return normalizePath(path.join("node_modules", ...packageName.split("/")));
}

function expectEqual(actual, expected, label) {
  if (actual !== expected) {
    fail(`${label}: expected ${formatValue(expected)}, got ${formatValue(actual)}`);
  }
}

function expectDeepEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    fail(`${label}: expected ${formatValue(expected)}, got ${formatValue(actual)}`);
  }
}

function fail(message) {
  failures.push(message);
}

function formatValue(value) {
  return JSON.stringify(value);
}

function parseArgs(args) {
  const parsed = {
    help: false,
    archiveRoot: undefined,
    manifestsDir: undefined,
    version: undefined
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--help":
      case "-h":
        parsed.help = true;
        break;
      case "--archive-root":
        parsed.archiveRoot = readArgValue(args, ++index, arg);
        break;
      case "--manifests-dir":
        parsed.manifestsDir = readArgValue(args, ++index, arg);
        break;
      case "--version":
        parsed.version = readArgValue(args, ++index, arg);
        break;
      default:
        throw new Error(`Unknown argument: ${arg}`);
    }
  }

  return parsed;
}

function printHelp() {
  console.log(`Usage: node scripts/verify-install-archives.mjs [options]

Options:
  --archive-root <dir>    Directory containing install archives. Defaults to dist-install.
  --manifests-dir <dir>   Directory containing install manifests. Defaults to <archive-root>/manifests.
  --version <version>     Expected package version. Defaults to Cargo.toml.
  -h, --help              Show this help.
`);
}
