#!/usr/bin/env node
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import {
  DIST_NPM_DIR,
  PLATFORM_PACKAGES,
  ROOT_PACKAGE_NAME,
  readCargoPackageMetadata,
  readJson
} from "./npm-package-config.mjs";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..");

const options = parseArgs(process.argv.slice(2));
if (options.help) {
  printHelp();
  process.exit(0);
}

try {
  const distDir = path.resolve(repoRoot, options.distDir ?? DIST_NPM_DIR);
  const packDir = path.resolve(repoRoot, options.packDir ?? "dist-pack");
  const platformDir = options.platform ?? "linux-x64-gnu";
  const platformPackage = PLATFORM_PACKAGES.find((entry) => entry.dir === platformDir);

  if (!platformPackage) {
    throw new Error(
      `Unknown platform package directory: ${platformDir}. Expected one of: ${PLATFORM_PACKAGES.map((entry) => entry.dir).join(", ")}`
    );
  }

  assertHostCanSmokePlatform(platformPackage);

  const cargoPackage = readCargoPackageMetadata(path.join(repoRoot, "Cargo.toml"));
  const packedPackages = options.useExistingTarballs
    ? findExistingPackageTarballs({ packDir, cargoPackage })
    : packGeneratedPackages({ distDir, packDir, cargoPackage });
  const rootTarball = packedPackages.get("root");
  const platformTarball = packedPackages.get(platformPackage.dir);
  if (!rootTarball || !platformTarball) {
    throw new Error(`Missing packed root or ${platformPackage.dir} tarball`);
  }

  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-npm-smoke-"));
  try {
    smokeInstall({
      projectDir: tempDir,
      rootTarball,
      platformTarball,
      platformPackage,
      cargoPackage,
      realBinary: options.realBinary
    });
  } finally {
    if (!options.keepTemp) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    } else {
      console.log(`Kept smoke project at ${tempDir}`);
    }
  }

  printManifestSummary(distDir);
  console.log(`Packed npm tarballs in ${path.relative(repoRoot, packDir)}`);
  console.log(`Smoke-tested ${ROOT_PACKAGE_NAME} with ${platformPackage.packageName}`);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}

function assertHostCanSmokePlatform(platform) {
  const hostKey = `${process.platform}:${process.arch}`;
  const expectedOs = platform.os[0];
  const expectedCpu = platform.cpu[0];

  if (process.platform !== expectedOs || process.arch !== expectedCpu) {
    throw new Error(
      `Cannot smoke-test ${platform.dir} on ${hostKey}. ` +
        "The launcher resolves the platform package from the current Node platform. " +
        "Run this smoke on a matching host or pass a matching --platform."
    );
  }
}

function packGeneratedPackages({ distDir, packDir, cargoPackage }) {
  const packages = new Map();
  const packageDirs = ["root", ...PLATFORM_PACKAGES.map((entry) => entry.dir)];

  fs.rmSync(packDir, { recursive: true, force: true });
  fs.mkdirSync(packDir, { recursive: true });

  for (const packageDir of packageDirs) {
    const fullPackageDir = path.join(distDir, packageDir);
    const packageJson = readJson(path.join(fullPackageDir, "package.json"));
    const output = runNpm(["pack", fullPackageDir, "--pack-destination", packDir, "--json"], repoRoot);
    const [packResult] = JSON.parse(output);
    if (!packResult?.filename) {
      throw new Error(`npm pack did not return a filename for ${packageDir}`);
    }

    if (packageJson.version !== cargoPackage.version) {
      throw new Error(
        `${packageDir} package version ${packageJson.version} does not match Cargo.toml version ${cargoPackage.version}`
      );
    }

    packages.set(packageDir, path.join(packDir, packResult.filename));
  }

  return packages;
}

function findExistingPackageTarballs({ packDir, cargoPackage }) {
  const packages = new Map();
  packages.set("root", existingTarballPath(packDir, ROOT_PACKAGE_NAME, cargoPackage.version));

  for (const platformPackage of PLATFORM_PACKAGES) {
    packages.set(
      platformPackage.dir,
      existingTarballPath(packDir, platformPackage.packageName, cargoPackage.version)
    );
  }

  return packages;
}

function existingTarballPath(packDir, packageName, version) {
  const tarballPath = path.join(packDir, npmTarballName(packageName, version));
  if (!fs.existsSync(tarballPath)) {
    throw new Error(`Missing existing npm tarball: ${path.relative(repoRoot, tarballPath)}`);
  }
  return tarballPath;
}

function npmTarballName(packageName, version) {
  const filenamePrefix = packageName.replace(/^@/, "").replace("/", "-");
  return `${filenamePrefix}-${version}.tgz`;
}

function smokeInstall({ projectDir, rootTarball, platformTarball, platformPackage, cargoPackage, realBinary }) {
  writeJson(path.join(projectDir, "package.json"), {
    private: true,
    name: "claude-rs-npm-smoke",
    version: "0.0.0"
  });

  runNpm(
    [
      "install",
      rootTarball,
      platformTarball,
      "--ignore-scripts",
      "--no-audit",
      "--fund=false",
      "--prefer-offline"
    ],
    projectDir
  );

  const installedRoot = readJson(path.join(projectDir, "node_modules", ROOT_PACKAGE_NAME, "package.json"));
  const installedPlatform = readJson(
    path.join(projectDir, "node_modules", ...platformPackage.packageName.split("/"), "package.json")
  );

  if (installedRoot.version !== cargoPackage.version) {
    throw new Error(`Installed root version ${installedRoot.version} does not match ${cargoPackage.version}`);
  }

  if (installedPlatform.version !== cargoPackage.version) {
    throw new Error(`Installed platform version ${installedPlatform.version} does not match ${cargoPackage.version}`);
  }

  const binPath = path.join(projectDir, "node_modules", ".bin", process.platform === "win32" ? "claude-rs.cmd" : "claude-rs");
  const versionOutput = runInstalledCommand(binPath, ["--version"], projectDir);
  const helpOutput = runInstalledCommand(binPath, ["--help"], projectDir);

  if (realBinary) {
    printCommandOutput("--version", versionOutput);
    printCommandOutput("--help", helpOutput);
    return;
  }

  assertMockOutput(versionOutput.stdout, "--version");
  assertMockOutput(helpOutput.stdout, "--help");
}

function assertMockOutput(output, commandName) {
  if (!output.includes("claude-rs 0.0.0-mock")) {
    throw new Error(`${commandName} did not run the mock binary. Output:\n${output}`);
  }
}

function runInstalledCommand(binPath, args, cwd) {
  try {
    const stdout = execFileSync(binPath, args, {
      cwd,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true
    });
    return { stdout, stderr: "" };
  } catch (error) {
    const stdout = bufferToString(error.stdout);
    const stderr = bufferToString(error.stderr);
    throw new Error(
      `Installed claude-rs command failed: ${args.join(" ")}\n` +
        `status: ${error.status ?? "unknown"}\n` +
        `stdout:\n${stdout}\n` +
        `stderr:\n${stderr}`
    );
  }
}

function bufferToString(value) {
  if (!value) {
    return "";
  }
  return Buffer.isBuffer(value) ? value.toString("utf8") : String(value);
}

function printCommandOutput(commandName, output) {
  const stdout = output.stdout.trim();
  const stderr = output.stderr.trim();
  if (stdout) {
    console.log(`claude-rs ${commandName} stdout:\n${stdout}`);
  }
  if (stderr) {
    console.log(`claude-rs ${commandName} stderr:\n${stderr}`);
  }
}

function printManifestSummary(distDir) {
  const summaryPath = path.join(distDir, "manifests", "summary.json");
  if (!fs.existsSync(summaryPath)) {
    return;
  }

  console.log("Package manifest summary:");
  console.log(fs.readFileSync(summaryPath, "utf8").trim());
}

function runNpm(args, cwd) {
  const command = process.platform === "win32" ? "cmd.exe" : "npm";
  const commandArgs = process.platform === "win32" ? ["/d", "/s", "/c", "npm.cmd", ...args] : args;
  return execFileSync(command, commandArgs, {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
    windowsHide: true
  });
}

function writeJson(destination, value) {
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function parseArgs(args) {
  const parsed = {
    help: false,
    distDir: undefined,
    packDir: undefined,
    platform: undefined,
    keepTemp: false,
    useExistingTarballs: false,
    realBinary: false
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--help":
      case "-h":
        parsed.help = true;
        break;
      case "--dist-dir":
        parsed.distDir = readArgValue(args, ++index, arg);
        break;
      case "--pack-dir":
        parsed.packDir = readArgValue(args, ++index, arg);
        break;
      case "--platform":
        parsed.platform = readArgValue(args, ++index, arg);
        break;
      case "--keep-temp":
        parsed.keepTemp = true;
        break;
      case "--use-existing-tarballs":
        parsed.useExistingTarballs = true;
        break;
      case "--real-binary":
        parsed.realBinary = true;
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
  console.log(`Usage: node scripts/smoke-npm-package-install.mjs [options]

Options:
  --dist-dir <dir>      Generated package directory. Defaults to dist-npm.
  --pack-dir <dir>      Directory for npm tarballs. Defaults to dist-pack.
  --platform <dir>      Platform package directory to install. Defaults to linux-x64-gnu.
  --use-existing-tarballs
                        Install tarballs that already exist in --pack-dir instead of packing dist-npm.
  --real-binary         Require --version and --help to exit successfully without mock output checks.
  --keep-temp           Keep the temporary smoke project for inspection.
  -h, --help            Show this help.
`);
}
