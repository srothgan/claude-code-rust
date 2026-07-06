#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { execFileSync, spawnSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  PLATFORM_PACKAGES,
  ROOT_PACKAGE_NAME,
  readCargoPackageMetadata,
} from "./npm-package-config.mjs";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..");

export function npmTarballName(packageName, version) {
  const filenamePrefix = packageName.replace(/^@/, "").replace("/", "-");
  return `${filenamePrefix}-${version}.tgz`;
}

export function expectedPublishTarballs(packDir, version) {
  return [
    ...PLATFORM_PACKAGES.map((platformPackage) => ({
      packageName: platformPackage.packageName,
      tarball: path.join(packDir, npmTarballName(platformPackage.packageName, version)),
    })),
    {
      packageName: ROOT_PACKAGE_NAME,
      tarball: path.join(packDir, npmTarballName(ROOT_PACKAGE_NAME, version)),
    },
  ];
}

export function resolveNpmInvocation(
  args,
  {
    env = process.env,
    execPath = process.execPath,
    existsSync = fs.existsSync,
    platform = process.platform,
  } = {},
) {
  if (platform !== "win32") {
    return { command: "npm", args };
  }

  const cliCandidates = [
    env.npm_execpath,
    env.npm_config_npm_execpath,
    path.join(path.dirname(execPath), "node_modules", "npm", "bin", "npm-cli.js"),
  ].filter((candidate) => typeof candidate === "string" && candidate.endsWith(".js"));

  const npmCli = cliCandidates.find((candidate) => existsSync(candidate));
  if (!npmCli) {
    throw new Error("Could not resolve npm CLI script on Windows; set npm_execpath or install Node.js with npm");
  }

  return { command: execPath, args: [npmCli, ...args] };
}

export function execNpmSync(args, options) {
  const invocation = resolveNpmInvocation(args);
  return execFileSync(invocation.command, invocation.args, options);
}

function dryRunNpmPublish({ packDir, version }) {
  for (const { packageName, tarball } of expectedPublishTarballs(packDir, version)) {
    if (!fs.existsSync(tarball)) {
      throw new Error(`Missing npm package tarball for ${packageName}: ${relativePath(tarball)}`);
    }

    runNpmPublishDryRun({ packageName, tarball });
  }
}

function runNpmPublishDryRun({ packageName, tarball }) {
  const invocation = resolveNpmInvocation(["publish", tarball, "--access", "public", "--dry-run"]);
  const result = spawnSync(invocation.command, invocation.args, {
    cwd: repoRoot,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });

  if (result.stdout) {
    process.stdout.write(result.stdout);
  }
  if (result.stderr) {
    process.stderr.write(result.stderr);
  }
  if (result.error) {
    throw result.error;
  }
  if (result.status === 0) {
    return;
  }

  const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
  throw new Error(
    `npm publish --dry-run failed for ${packageName} with exit code ${result.status}\n${output}`.trim()
  );
}

function parseArgs(args) {
  const parsed = {
    packDir: "dist-pack",
    version: undefined,
    help: false,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--pack-dir":
        parsed.packDir = readArgValue(args, ++index, arg);
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

function relativePath(filePath) {
  return path.relative(repoRoot, filePath).replaceAll(path.sep, "/");
}

function printHelp() {
  console.log(`Usage: node scripts/dry-run-npm-publish.mjs [options]

Options:
  --pack-dir <dir>           Directory containing npm tarballs. Defaults to dist-pack.
  --version <version>        Expected package version. Defaults to Cargo.toml.
  -h, --help                 Show this help.
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
  const packDir = path.resolve(repoRoot, options.packDir);

  dryRunNpmPublish({
    packDir,
    version,
  });
  console.log(`Dry-run checked ${PLATFORM_PACKAGES.length + 1} npm tarballs from ${relativePath(packDir)}`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  });
}
