#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  PLATFORM_PACKAGES,
  readBunRuntimeManifest,
} from "./npm-package-config.mjs";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..");

export function verifyStagedBunRuntimes({
  distPlatformDir,
  platformPackages = PLATFORM_PACKAGES,
  manifest = readBunRuntimeManifest(repoRoot),
  rootForMessages = repoRoot,
}) {
  const failures = [];

  for (const platformPackage of platformPackages) {
    const runtimeAsset = manifest.assets?.[platformPackage.dir];
    const packageBinDir = path.join(distPlatformDir, platformPackage.dir, "bin");
    const runtimePath = path.join(packageBinDir, platformPackage.bundledRuntimeName);

    if (!runtimeAsset) {
      failures.push(`${platformPackage.dir}: missing Bun runtime manifest entry`);
      continue;
    }
    if (!fs.existsSync(runtimePath)) {
      failures.push(`${platformPackage.dir}: missing staged runtime ${relativePath(rootForMessages, runtimePath)}`);
      continue;
    }
    if (!fs.statSync(runtimePath).isFile()) {
      failures.push(`${platformPackage.dir}: staged runtime is not a file ${relativePath(rootForMessages, runtimePath)}`);
    }
    if (["bun", "bun.exe"].includes(path.basename(runtimePath))) {
      failures.push(`${platformPackage.dir}: staged runtime must be privately renamed, got ${path.basename(runtimePath)}`);
    }
    if (!platformPackage.os.includes("win32") && process.platform !== "win32") {
      const mode = fs.statSync(runtimePath).mode;
      if ((mode & 0o111) === 0) {
        failures.push(`${platformPackage.dir}: staged runtime is not executable ${relativePath(rootForMessages, runtimePath)}`);
      }
    }
    const actualSha256 = fileSha256(runtimePath);
    if (actualSha256 !== runtimeAsset.binarySha256) {
      failures.push(
        `${platformPackage.dir}: staged runtime SHA256 mismatch for ${relativePath(
          rootForMessages,
          runtimePath,
        )}: expected ${runtimeAsset.binarySha256}, got ${actualSha256}`
      );
    }

    for (const genericName of ["bun", "bun.exe"]) {
      const genericRuntimePath = path.join(packageBinDir, genericName);
      if (fs.existsSync(genericRuntimePath)) {
        failures.push(
          `${platformPackage.dir}: staged runtime directory contains forbidden generic Bun name ${relativePath(
            rootForMessages,
            genericRuntimePath,
          )}`
        );
      }
    }
  }

  if (failures.length > 0) {
    throw new Error(`Staged Bun runtime verification failed:\n- ${failures.join("\n- ")}`);
  }
}

function fileSha256(filePath) {
  return crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex");
}

function relativePath(root, target) {
  return path.relative(root, target).replaceAll(path.sep, "/");
}

function parseArgs(args) {
  const parsed = {
    distPlatform: "dist-platform",
    platform: undefined,
    help: false,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--dist-platform":
        parsed.distPlatform = readArgValue(args, ++index, arg);
        break;
      case "--platform":
        parsed.platform = readArgValue(args, ++index, arg);
        break;
      case "--help":
      case "-h":
        parsed.help = true;
        break;
      default:
        throw new Error(`Unknown argument: ${arg}`);
    }
  }

  return parsed;
}

function readArgValue(args, index, flag) {
  const value = args[index];
  if (!value || value.startsWith("--")) {
    throw new Error(`Missing value for ${flag}`);
  }
  return value;
}

function printHelp() {
  console.log(`Usage: node scripts/verify-staged-bun-runtimes.mjs [options]

Options:
  --dist-platform <dir>      Staging root. Defaults to dist-platform.
  --platform <package-dir>   Verify one platform package directory only.
  -h, --help                 Show this help.
`);
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.help) {
    printHelp();
    return;
  }

  const platformPackages = options.platform
    ? PLATFORM_PACKAGES.filter((platformPackage) => platformPackage.dir === options.platform)
    : PLATFORM_PACKAGES;
  if (options.platform && platformPackages.length === 0) {
    throw new Error(`Unknown platform package directory: ${options.platform}`);
  }

  const distPlatformDir = path.resolve(repoRoot, options.distPlatform);
  verifyStagedBunRuntimes({ distPlatformDir });
  console.log(`Verified staged Bun runtimes in ${relativePath(repoRoot, distPlatformDir)}`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  });
}
