#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { resolveRepoRoot } from "../shared/repo-root.mjs";
import {
  PLATFORM_PACKAGES,
  readCargoPackageMetadata,
} from "../shared/npm-package-config.mjs";
import { execNpmSync, npmTarballName } from "./dry-run-npm-publish.mjs";

const repoRoot = resolveRepoRoot(import.meta.url);

export function platformPublishPlan(packDir, version) {
  return PLATFORM_PACKAGES.map((platformPackage) => ({
    packageName: platformPackage.packageName,
    version,
    tarball: path.join(packDir, npmTarballName(platformPackage.packageName, version)),
  }));
}

export function publishPlatformPackages({
  packDir,
  version,
  ledgerDir,
  execNpm = execNpmSync,
  existsSync = fs.existsSync,
  mkdirSync = fs.mkdirSync,
  writeFileSync = fs.writeFileSync,
  now = () => new Date().toISOString(),
}) {
  mkdirSync(ledgerDir, { recursive: true });
  const ledgerEntries = [];

  for (const plan of platformPublishPlan(packDir, version)) {
    if (!existsSync(plan.tarball)) {
      throw new Error(`Missing npm package tarball for ${plan.packageName}: ${relativePath(plan.tarball)}`);
    }

    if (npmPackageVersionExists(plan.packageName, version, { execNpm })) {
      ledgerEntries.push(ledgerEntry(plan, "already_published", now()));
      console.log(`SKIP: ${plan.packageName}@${version} already published`);
      continue;
    }

    execNpm(["publish", plan.tarball, "--access", "public"], {
      cwd: repoRoot,
      stdio: "inherit",
      windowsHide: true,
    });
    ledgerEntries.push(ledgerEntry(plan, "published", now()));
  }

  const ledgerPath = path.join(ledgerDir, `platform-packages-${version}.json`);
  mkdirSync(ledgerDir, { recursive: true });
  writeFileSync(
    ledgerPath,
    `${JSON.stringify({ manifestVersion: 1, version, entries: ledgerEntries }, null, 2)}\n`,
    "utf8",
  );
  console.log(`Wrote npm publish ledger to ${relativePath(ledgerPath)}`);
}

function ledgerEntry(plan, status, timestamp) {
  return {
    package_name: plan.packageName,
    version: plan.version,
    tarball_filename: path.basename(plan.tarball),
    status,
    timestamp,
  };
}

export function npmPackageVersionExists(packageName, version, { execNpm = execNpmSync } = {}) {
  try {
    const output = execNpm(["view", `${packageName}@${version}`, "version", "--json"], {
      cwd: repoRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    }).trim();
    if (!output) {
      return false;
    }
    const parsed = JSON.parse(output);
    if (parsed === version) {
      return true;
    }
    throw new Error(`npm returned unexpected version for ${packageName}@${version}: ${output}`);
  } catch (error) {
    if (error instanceof Error && error.message.startsWith("npm returned unexpected version")) {
      throw error;
    }
    const stderr = bufferToString(error.stderr);
    const stdout = bufferToString(error.stdout);
    if (`${stdout}\n${stderr}`.includes("E404")) {
      return false;
    }
    throw new Error(
      `Could not inspect npm package state for ${packageName}@${version}:\n${stdout}\n${stderr}`.trim(),
    );
  }
}

function parseArgs(args) {
  const parsed = {
    packDir: "dist-pack",
    ledgerDir: "dist-publish-ledger",
    version: undefined,
    help: false,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--pack-dir":
        parsed.packDir = readArgValue(args, ++index, arg);
        break;
      case "--ledger-dir":
        parsed.ledgerDir = readArgValue(args, ++index, arg);
        break;
      case "--version":
        parsed.version = readArgValue(args, ++index, arg);
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

function bufferToString(value) {
  if (!value) {
    return "";
  }
  return Buffer.isBuffer(value) ? value.toString("utf8") : String(value);
}

function relativePath(filePath) {
  return path.relative(repoRoot, filePath).replaceAll(path.sep, "/");
}

function printHelp() {
  console.log(`Usage: node scripts/npm/publish-npm-platform-packages.mjs [options]

Options:
  --pack-dir <dir>      Directory containing platform npm tarballs. Defaults to dist-pack.
  --ledger-dir <dir>    Directory for publish ledger JSON. Defaults to dist-publish-ledger.
  --version <version>   Expected package version. Defaults to Cargo.toml.
  -h, --help            Show this help.
`);
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.help) {
    printHelp();
    return;
  }

  const cargoPackage = readCargoPackageMetadata(path.join(repoRoot, "Cargo.toml"));
  const version = options.version ?? cargoPackage.version;
  publishPlatformPackages({
    packDir: path.resolve(repoRoot, options.packDir),
    ledgerDir: path.resolve(repoRoot, options.ledgerDir),
    version,
  });
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  });
}
