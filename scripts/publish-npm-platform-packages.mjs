#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  PLATFORM_PACKAGES,
  readCargoPackageMetadata,
} from "./npm-package-config.mjs";
import { execNpmSync, npmTarballName } from "./dry-run-npm-publish.mjs";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..");

export function tarballIntegrity(tarballPath) {
  return `sha512-${crypto.createHash("sha512").update(fs.readFileSync(tarballPath)).digest("base64")}`;
}

export function platformPublishPlan(packDir, version) {
  return PLATFORM_PACKAGES.map((platformPackage) => ({
    packageName: platformPackage.packageName,
    version,
    tarball: path.join(packDir, npmTarballName(platformPackage.packageName, version)),
  }));
}

function publishPlatformPackages({ packDir, version, ledgerDir }) {
  fs.mkdirSync(ledgerDir, { recursive: true });
  const ledgerEntries = [];

  for (const plan of platformPublishPlan(packDir, version)) {
    if (!fs.existsSync(plan.tarball)) {
      throw new Error(`Missing npm package tarball for ${plan.packageName}: ${relativePath(plan.tarball)}`);
    }

    const localIntegrity = tarballIntegrity(plan.tarball);
    const existingIntegrity = npmViewIntegrity(plan.packageName, version);
    if (existingIntegrity) {
      if (existingIntegrity !== localIntegrity) {
        throw new Error(
          `${plan.packageName}@${version} already exists with different integrity: ` +
            `registry ${existingIntegrity}, local ${localIntegrity}`
        );
      }
      ledgerEntries.push(ledgerEntry(plan, localIntegrity, existingIntegrity, "already_published"));
      console.log(`OK: ${plan.packageName}@${version} already published with matching integrity`);
      continue;
    }

    execNpmSync(["publish", plan.tarball, "--access", "public"], {
      cwd: repoRoot,
      stdio: "inherit",
      windowsHide: true,
    });
    const registryIntegrity = npmViewIntegrity(plan.packageName, version);
    if (registryIntegrity !== localIntegrity) {
      throw new Error(
        `${plan.packageName}@${version} registry integrity mismatch after publish: ` +
          `registry ${registryIntegrity ?? "<missing>"}, local ${localIntegrity}`
      );
    }
    ledgerEntries.push(ledgerEntry(plan, localIntegrity, registryIntegrity, "published"));
  }

  const ledgerPath = path.join(ledgerDir, `platform-packages-${version}.json`);
  fs.writeFileSync(
    ledgerPath,
    `${JSON.stringify({ manifestVersion: 1, version, entries: ledgerEntries }, null, 2)}\n`,
    "utf8",
  );
  console.log(`Wrote npm publish ledger to ${relativePath(ledgerPath)}`);
}

function ledgerEntry(plan, localIntegrity, registryIntegrity, status) {
  return {
    package_name: plan.packageName,
    version: plan.version,
    tarball_filename: path.basename(plan.tarball),
    local_npm_pack_integrity: localIntegrity,
    registry_dist_integrity: registryIntegrity,
    status,
    timestamp: new Date().toISOString(),
  };
}

function npmViewIntegrity(packageName, version) {
  try {
    const output = execNpmSync(["view", `${packageName}@${version}`, "dist.integrity", "--json"], {
      cwd: repoRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    }).trim();
    if (!output) {
      return undefined;
    }
    const parsed = JSON.parse(output);
    return typeof parsed === "string" && parsed.length > 0 ? parsed : undefined;
  } catch (error) {
    const stderr = bufferToString(error.stderr);
    const stdout = bufferToString(error.stdout);
    if (`${stdout}\n${stderr}`.includes("E404")) {
      return undefined;
    }
    throw new Error(
      `Could not inspect npm registry integrity for ${packageName}@${version}:\n${stdout}\n${stderr}`.trim()
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
  console.log(`Usage: node scripts/publish-npm-platform-packages.mjs [options]

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
