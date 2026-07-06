#!/usr/bin/env node
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { PLATFORM_PACKAGES, ROOT_PACKAGE_NAME, readCargoPackageMetadata } from "./npm-package-config.mjs";
import {
  extractArchive,
  fileSha256,
  findSingleTopLevelDirectory,
  listRelativeFiles,
  readArgValue,
  readJson
} from "./install-archive-common.mjs";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..");

const options = parseArgs(process.argv.slice(2));
if (options.help) {
  printHelp();
  process.exit(0);
}

const cargoPackage = readCargoPackageMetadata(path.join(repoRoot, "Cargo.toml"));
const version = options.version ?? cargoPackage.version;
const bundleRootName = `${ROOT_PACKAGE_NAME}-${version}-release-bundle`;
const bundlePath = path.resolve(repoRoot, options.bundle ?? `${bundleRootName}.tar.gz`);
const failures = [];

if (!fs.existsSync(bundlePath)) {
  fail(`Missing release bundle: ${path.relative(repoRoot, bundlePath)}`);
} else {
  verifyBundle();
}

if (failures.length > 0) {
  console.error(`Release bundle verification failed with ${failures.length} issue(s):`);
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log(`Verified release bundle ${path.relative(repoRoot, bundlePath)}`);

function verifyBundle() {
  const extractDir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-bundle-verify-"));
  try {
    extractArchive({ archivePath: bundlePath, destinationDir: extractDir });
    const bundleRoot = findSingleTopLevelDirectory(extractDir);
    expectEqual(path.basename(bundleRoot), bundleRootName, "bundle root directory");

    const files = listRelativeFiles(bundleRoot);
    expectRequiredFiles(files);
    expectNoCompleteInstallArchives(files);
    expectReleaseManifest(bundleRoot);
    expectInternalChecksums(bundleRoot);
  } catch (error) {
    fail(error instanceof Error ? error.message : String(error));
  } finally {
    fs.rmSync(extractDir, { recursive: true, force: true });
  }
}

function expectRequiredFiles(files) {
  const requiredFiles = [
    "release-manifest.json",
    "SHA256SUMS.internal",
    "dist-npm/manifests/summary.json",
    "dist-npm/manifests/root.json"
  ];

  for (const platformPackage of PLATFORM_PACKAGES) {
    requiredFiles.push(`dist-npm/manifests/${platformPackage.dir}.json`);
    requiredFiles.push(`dist-install/manifests/${platformPackage.dir}.json`);
  }

  for (const requiredFile of requiredFiles) {
    if (!files.includes(requiredFile)) {
      fail(`bundle is missing required file: ${requiredFile}`);
    }
  }
}

function expectNoCompleteInstallArchives(files) {
  for (const file of files) {
    if (file.startsWith("dist-install/") && (file.endsWith(".tar.gz") || file.endsWith(".zip"))) {
      fail(`bundle must not duplicate complete install archive: ${file}`);
    }
  }
}

function expectReleaseManifest(bundleRoot) {
  const manifestPath = path.join(bundleRoot, "release-manifest.json");
  if (!fs.existsSync(manifestPath)) {
    return;
  }

  const manifest = readJson(manifestPath);
  expectEqual(manifest.manifestVersion, 1, "release manifest version");
  expectEqual(manifest.version, version, "release manifest package version");
  expectEqual(manifest.internalBundle?.rootDirectory, bundleRootName, "release manifest bundle root");
  expectEqual(manifest.targets?.length, PLATFORM_PACKAGES.length, "release manifest target count");
}

function expectInternalChecksums(bundleRoot) {
  const checksumsPath = path.join(bundleRoot, "SHA256SUMS.internal");
  if (!fs.existsSync(checksumsPath)) {
    return;
  }

  const checksums = fs.readFileSync(checksumsPath, "utf8").trim().split(/\r?\n/).filter(Boolean);
  const seen = new Set();
  for (const line of checksums) {
    const match = line.match(/^([a-fA-F0-9]{64}) [ *](.+)$/);
    if (!match) {
      fail(`invalid SHA256SUMS.internal line: ${line}`);
      continue;
    }
    const [, expectedSha, relativePath] = match;
    if (relativePath === "SHA256SUMS.internal") {
      fail("SHA256SUMS.internal must not include itself");
      continue;
    }
    if (relativePath === path.basename(bundlePath)) {
      fail("SHA256SUMS.internal must not include the final release bundle archive");
      continue;
    }
    const filePath = path.join(bundleRoot, ...relativePath.split("/"));
    if (!fs.existsSync(filePath)) {
      fail(`SHA256SUMS.internal references missing file: ${relativePath}`);
      continue;
    }
    const actualSha = fileSha256(filePath);
    if (actualSha !== expectedSha.toLowerCase()) {
      fail(`SHA256SUMS.internal mismatch for ${relativePath}: expected ${expectedSha}, got ${actualSha}`);
    }
    seen.add(relativePath);
  }

  const files = listRelativeFiles(bundleRoot).filter((file) => file !== "SHA256SUMS.internal");
  for (const file of files) {
    if (!seen.has(file)) {
      fail(`SHA256SUMS.internal is missing file: ${file}`);
    }
  }
}

function expectEqual(actual, expected, label) {
  if (actual !== expected) {
    fail(`${label}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}

function fail(message) {
  failures.push(message);
}

function parseArgs(args) {
  const parsed = {
    help: false,
    version: undefined,
    bundle: undefined
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--help":
      case "-h":
        parsed.help = true;
        break;
      case "--version":
        parsed.version = readArgValue(args, ++index, arg);
        break;
      case "--bundle":
        parsed.bundle = readArgValue(args, ++index, arg);
        break;
      default:
        throw new Error(`Unknown argument: ${arg}`);
    }
  }

  return parsed;
}

function printHelp() {
  console.log(`Usage: node scripts/verify-release-bundle.mjs [options]

Options:
  --version <version>   Expected package version. Defaults to Cargo.toml.
  --bundle <path>       Release bundle path. Defaults to claude-code-rust-<version>-release-bundle.tar.gz.
  -h, --help            Show this help.
`);
}
