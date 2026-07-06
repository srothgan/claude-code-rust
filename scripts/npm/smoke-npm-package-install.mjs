#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { execFileSync, spawn } from "node:child_process";
import { pathToFileURL } from "node:url";
import { resolveRepoRoot } from "../shared/repo-root.mjs";
import {
  DIST_NPM_DIR,
  PLATFORM_PACKAGES,
  ROOT_PACKAGE_NAME,
  readCargoPackageMetadata,
  readJson
} from "../shared/npm-package-config.mjs";

const repoRoot = resolveRepoRoot(import.meta.url);
let npmCliPath;

async function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.help) {
    printHelp();
    process.exit(0);
  }

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
    if (options.registrySmoke) {
      await smokeRegistryInstall({
        tempDir,
        packedPackages,
        platformPackage,
        cargoPackage,
        realBinary: options.realBinary,
        noSystemRuntime: options.noSystemRuntime,
      });
    } else {
      smokeInstall({
        projectDir: tempDir,
        rootTarball,
        platformTarball,
        platformPackage,
        cargoPackage,
        realBinary: options.realBinary,
        noSystemRuntime: options.noSystemRuntime
      });
    }
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
}

function assertHostCanSmokePlatform(platform) {
  const hostKey = `${process.platform}:${process.arch}`;
  const expectedOs = platform.os[0];
  const expectedCpu = platform.cpu[0];

  if (process.platform !== expectedOs || process.arch !== expectedCpu) {
    throw new Error(
      `Cannot smoke-test ${platform.dir} on ${hostKey}. ` +
        "npm platform package constraints must match the current host. " +
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

function smokeInstall({
  projectDir,
  rootTarball,
  platformTarball,
  platformPackage,
  cargoPackage,
  realBinary,
  noSystemRuntime
}) {
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

  const nodeModulesDir = path.join(projectDir, "node_modules");
  assertInstalledPlatformPackageLayout(nodeModulesDir, platformPackage);
  assertInstalledRootPackageLayout(nodeModulesDir);
  const installState = installedPackageState(nodeModulesDir, platformPackage);

  const commandEnv = noSystemRuntime ? envWithoutSystemRuntimePath() : process.env;
  const installedCommands = installedLauncherCommands(projectDir);
  const versionOutputs = runInstalledCommands(installedCommands, ["--version"], projectDir, commandEnv);
  const helpOutputs = runInstalledCommands(installedCommands, ["--help"], projectDir, commandEnv);

  if (realBinary) {
    for (const output of versionOutputs) {
      printCommandOutput(`${output.label} --version`, output);
    }
    for (const output of helpOutputs) {
      printCommandOutput(`${output.label} --help`, output);
    }
    const doctorOutputs = runInstalledCommands(installedCommands, ["doctor", "--json", "--strict"], projectDir, commandEnv);
    for (const output of doctorOutputs) {
      assertDoctorReportsBridgeReadiness(output.stdout, platformPackage, output.label);
      printCommandOutput(`${output.label} doctor --json --strict`, output);
    }
    runBridgeRuntimeContract(installState.runtimePath, installState.bridgeScriptPath, projectDir);
    return;
  }

  for (const output of versionOutputs) {
    assertMockOutput(output.stdout, `${output.label} --version`);
  }
  for (const output of helpOutputs) {
    assertMockOutput(output.stdout, `${output.label} --help`);
  }
}

async function smokeRegistryInstall({
  tempDir,
  packedPackages,
  platformPackage,
  cargoPackage,
  realBinary,
  noSystemRuntime,
}) {
  const prefix = path.join(tempDir, "global-prefix");
  const cacheDir = path.join(tempDir, "npm-cache");
  const dependencyPackDir = path.join(tempDir, "registry-dependencies");
  const npmConfig = createIsolatedNpmConfig(tempDir);
  const npmEnv = sanitizedNpmEnvironment(process.env);
  const seededDependencies = packRegistryDependencyTarballs({
    packDir: dependencyPackDir,
    npmConfigArgs: npmConfig.args,
    env: npmEnv,
  });
  const registryPackages = new Map([...packedPackages, ...seededDependencies]);
  const registry = await startLocalRegistry(registryPackages);
  const installConfigArgs = npmConfig.argsWithRegistry(registry.url);
  const installArgs = [
    "install",
    "-g",
    ROOT_PACKAGE_NAME,
    "--prefix",
    prefix,
    "--cache",
    cacheDir,
    ...installConfigArgs,
    "--ignore-scripts",
    "--no-audit",
    "--include=optional",
    "--fund=false",
  ];
  const uninstallArgs = [
    "uninstall",
    "-g",
    ROOT_PACKAGE_NAME,
    "--prefix",
    prefix,
    "--cache",
    cacheDir,
    ...installConfigArgs,
    "--no-audit",
    "--fund=false",
  ];

  try {
    await runNpmAsync(installArgs, tempDir, { env: npmEnv });
    assertGlobalInstall(prefix, platformPackage, cargoPackage);
    await runNpmAsync(uninstallArgs, tempDir, { env: npmEnv });
    assertGlobalBinsRemoved(prefix);
    await runNpmAsync(installArgs, tempDir, { env: npmEnv });
    const installState = assertGlobalInstall(prefix, platformPackage, cargoPackage);
    const commandEnv = noSystemRuntime ? envWithoutSystemRuntimePath() : process.env;
    const installedCommands = globalLauncherCommands(prefix);
    const versionOutputs = runInstalledCommands(installedCommands, ["--version"], tempDir, commandEnv);
    const helpOutputs = runInstalledCommands(installedCommands, ["--help"], tempDir, commandEnv);
    const manifest = {
      root_package_version: installState.installedRoot.version,
      selected_platform_package: platformPackage.packageName,
      global_bin_path: installState.globalBinPath,
      bundled_bridge_script_path: installState.bridgeScriptPath,
      bundled_bun_runtime_path: installState.runtimePath,
      doctor_bridge_runtime_kind: null,
    };

    if (realBinary) {
      for (const output of versionOutputs) {
        printCommandOutput(`${output.label} --version`, output);
      }
      for (const output of helpOutputs) {
        printCommandOutput(`${output.label} --help`, output);
      }
      const doctorOutputs = runInstalledCommands(
        installedCommands,
        ["doctor", "--json", "--strict"],
        tempDir,
        commandEnv,
      );
      for (const output of doctorOutputs) {
        const doctorDetails = assertDoctorReportsBridgeReadiness(output.stdout, platformPackage, output.label);
        manifest.doctor_bridge_runtime_kind = doctorDetails.runtimeKind;
        manifest.bundled_bridge_script_path = doctorDetails.bridgeScriptPath;
        manifest.bundled_bun_runtime_path = doctorDetails.runtimePath;
        printCommandOutput(`${output.label} doctor --json --strict`, output);
      }
      runBridgeRuntimeContract(installState.runtimePath, installState.bridgeScriptPath, tempDir);
    } else {
      for (const output of versionOutputs) {
        assertMockOutput(output.stdout, `${output.label} --version`);
      }
      for (const output of helpOutputs) {
        assertMockOutput(output.stdout, `${output.label} --help`);
      }
    }

    console.log(`Registry smoke manifest:\n${JSON.stringify(manifest, null, 2)}`);
  } finally {
    await registry.close();
  }
}

function packRegistryDependencyTarballs({ packDir, npmConfigArgs, env }) {
  const packages = new Map();
  const packageDirs = runtimeDependencyPackageDirs(readJson(path.join(repoRoot, "package-lock.json")));
  fs.mkdirSync(packDir, { recursive: true });

  for (const packageDir of packageDirs) {
    const packageJson = readJson(path.join(packageDir, "package.json"));
    const key = `${packageJson.name}@${packageJson.version}`;
    if (packages.has(key)) {
      continue;
    }

    const output = runNpm(
      ["pack", packageDir, "--pack-destination", packDir, "--json", "--ignore-scripts", ...npmConfigArgs],
      repoRoot,
      { env },
    );
    const [packResult] = JSON.parse(output);
    if (!packResult?.filename) {
      throw new Error(`npm pack did not return a filename for registry seed package ${key}`);
    }
    packages.set(key, path.join(packDir, packResult.filename));
  }

  return packages;
}

export function runtimeDependencyPackageDirs(lockfile, {
  nodeModulesRoot = path.join(repoRoot, "node_modules"),
  platform = process.platform,
  arch = process.arch,
  linuxLibc = currentLinuxLibc(),
} = {}) {
  const packages = [];
  const missing = [];

  for (const [packagePath, packageEntry] of Object.entries(lockfile.packages ?? {})) {
    if (!packagePath.startsWith("node_modules/") || packageEntry.dev || !packageEntry.version) {
      continue;
    }

    const relativePackagePath = packagePath.slice("node_modules/".length);
    const packageDir = path.join(nodeModulesRoot, ...relativePackagePath.split("/"));
    if (!fs.existsSync(path.join(packageDir, "package.json"))) {
      if (packageEntry.optional && !packageEntryMatchesHost(packageEntry, { platform, arch, linuxLibc })) {
        continue;
      }
      missing.push(packagePath);
      continue;
    }

    packages.push(packageDir);
  }

  if (missing.length > 0) {
    throw new Error(
      "Cannot seed hermetic npm registry because package-lock runtime dependencies are missing from node_modules. " +
        "Run `npm ci` before `--registry-smoke`.\n" +
        missing.map((entry) => `- ${entry}`).join("\n"),
    );
  }

  return packages;
}

function packageEntryMatchesHost(packageEntry, { platform, arch, linuxLibc }) {
  return listAllows(packageEntry.os, platform) &&
    listAllows(packageEntry.cpu, arch) &&
    (!packageEntry.libc || (platform === "linux" && listAllows(packageEntry.libc, linuxLibc)));
}

function listAllows(values, actual) {
  if (!Array.isArray(values) || values.length === 0) {
    return true;
  }
  if (values.includes(`!${actual}`)) {
    return false;
  }
  const positiveValues = values.filter((value) => !String(value).startsWith("!"));
  return positiveValues.length === 0 || positiveValues.includes(actual);
}

function currentLinuxLibc() {
  if (process.platform !== "linux") {
    return undefined;
  }
  try {
    const glibcVersion = process.report?.getReport?.()?.header?.glibcVersionRuntime;
    return typeof glibcVersion === "string" && glibcVersion.length > 0 ? "glibc" : "musl";
  } catch {
    return "musl";
  }
}

function createIsolatedNpmConfig(tempDir) {
  const userconfig = path.join(tempDir, "registry-smoke-user.npmrc");
  const globalconfig = path.join(tempDir, "registry-smoke-global.npmrc");
  fs.writeFileSync(userconfig, "audit=false\nfund=false\n", "utf8");
  fs.writeFileSync(globalconfig, "", "utf8");
  const args = ["--userconfig", userconfig, "--globalconfig", globalconfig];
  return {
    args,
    argsWithRegistry: (registryUrl) => ["--registry", registryUrl, ...args],
  };
}

export function sanitizedNpmEnvironment(baseEnv = process.env) {
  const env = {};
  for (const [key, value] of Object.entries(baseEnv)) {
    if (/^npm_config_/i.test(key) || key === "NPM_TOKEN" || key === "NODE_AUTH_TOKEN") {
      continue;
    }
    env[key] = value;
  }
  return env;
}

function assertMockOutput(output, commandName) {
  if (!output.includes("claude-rs 0.0.0-mock")) {
    throw new Error(`${commandName} did not run the mock binary. Output:\n${output}`);
  }
}

function assertInstalledPlatformPackageLayout(nodeModulesDir, platformPackage) {
  const packageDir = path.join(nodeModulesDir, ...platformPackage.packageName.split("/"));
  assertInstalledPlatformPackageDirLayout(packageDir, platformPackage);
}

function assertInstalledPlatformPackageDirLayout(packageDir, platformPackage) {
  const packageJson = readJson(path.join(packageDir, "package.json"));
  const binaryPath = path.join(packageDir, "bin", platformPackage.binaryName);
  const runtimePath = path.join(packageDir, "bin", platformPackage.bundledRuntimeName);
  if (packageJson.bin !== undefined) {
    throw new Error(`Installed platform package must not expose npm bins: ${JSON.stringify(packageJson.bin)}`);
  }
  if (!fs.existsSync(binaryPath)) {
    throw new Error(`Installed platform binary is missing: ${binaryPath}`);
  }
  if (!fs.existsSync(runtimePath)) {
    throw new Error(`Installed bundled Bun runtime is missing: ${runtimePath}`);
  }
  for (const genericName of ["bun", "bun.exe"]) {
    const genericPath = path.join(packageDir, "bin", genericName);
    if (fs.existsSync(genericPath)) {
      throw new Error(`Installed platform package exposes a generic Bun command: ${genericPath}`);
    }
  }
}

function assertInstalledRootPackageLayout(nodeModulesDir) {
  const packageDir = path.join(nodeModulesDir, ROOT_PACKAGE_NAME);
  const packageJson = readJson(path.join(packageDir, "package.json"));
  const expectedBin = { "claude-rs": "bin/claude-rs.js" };
  if (JSON.stringify(packageJson.bin) !== JSON.stringify(expectedBin)) {
    throw new Error(`Installed root package did not expose the resolver bin: ${JSON.stringify(packageJson.bin)}`);
  }
  const launcherPath = path.join(packageDir, "bin", "claude-rs.js");
  if (!fs.existsSync(launcherPath)) {
    throw new Error(`Installed root resolver is missing: ${launcherPath}`);
  }
}

function assertGlobalInstall(prefix, platformPackage, cargoPackage) {
  const nodeModulesDir = globalNodeModulesDir(prefix);
  const rootPackageDir = path.join(nodeModulesDir, ROOT_PACKAGE_NAME);
  const platformPackageDir = findInstalledPackageDir(
    [nodeModulesDir, path.join(rootPackageDir, "node_modules")],
    platformPackage.packageName,
  );
  const installedRoot = readJson(path.join(rootPackageDir, "package.json"));
  const installedPlatform = readJson(path.join(platformPackageDir, "package.json"));

  if (installedRoot.version !== cargoPackage.version) {
    throw new Error(`Installed global root version ${installedRoot.version} does not match ${cargoPackage.version}`);
  }
  if (installedPlatform.version !== cargoPackage.version) {
    throw new Error(
      `Installed global platform version ${installedPlatform.version} does not match ${cargoPackage.version}`
    );
  }

  assertInstalledRootPackageLayout(nodeModulesDir);
  assertInstalledPlatformPackageDirLayout(platformPackageDir, platformPackage);
  const globalBinPath = assertSingleGlobalBin(prefix);
  const bridgeScriptPath = path.join(rootPackageDir, "agent-sdk", "dist", "bridge.js");
  const runtimePath = path.join(platformPackageDir, "bin", platformPackage.bundledRuntimeName);
  if (!fs.existsSync(bridgeScriptPath)) {
    throw new Error(`Installed global bridge script is missing: ${bridgeScriptPath}`);
  }
  if (!fs.existsSync(runtimePath)) {
    throw new Error(`Installed global bundled Bun runtime is missing: ${runtimePath}`);
  }

  return {
    installedRoot,
    installedPlatform,
    globalBinPath,
    bridgeScriptPath,
    runtimePath,
  };
}

function findInstalledPackageDir(nodeModulesDirs, packageName) {
  for (const nodeModulesDir of nodeModulesDirs) {
    const packageDir = path.join(nodeModulesDir, ...packageName.split("/"));
    if (fs.existsSync(path.join(packageDir, "package.json"))) {
      return packageDir;
    }
  }

  throw new Error(
    `Installed package ${packageName} was not found in: ` +
      nodeModulesDirs.map((dir) => path.join(dir, ...packageName.split("/"))).join(", ")
  );
}

function installedPackageState(nodeModulesDir, platformPackage) {
  const rootPackageDir = path.join(nodeModulesDir, ROOT_PACKAGE_NAME);
  const platformPackageDir = path.join(nodeModulesDir, ...platformPackage.packageName.split("/"));
  return {
    bridgeScriptPath: path.join(rootPackageDir, "agent-sdk", "dist", "bridge.js"),
    runtimePath: path.join(platformPackageDir, "bin", platformPackage.bundledRuntimeName),
  };
}

function globalNodeModulesDir(prefix) {
  return process.platform === "win32"
    ? path.join(prefix, "node_modules")
    : path.join(prefix, "lib", "node_modules");
}

function globalBinDir(prefix) {
  return process.platform === "win32" ? prefix : path.join(prefix, "bin");
}

function assertSingleGlobalBin(prefix) {
  const binDir = globalBinDir(prefix);
  const entries = fs.existsSync(binDir)
    ? fs.readdirSync(binDir).filter((entry) => entry === "claude-rs" || entry.startsWith("claude-rs."))
    : [];

  const allowedEntries =
    process.platform === "win32"
      ? new Set(["claude-rs", "claude-rs.cmd", "claude-rs.ps1"])
      : new Set(["claude-rs"]);
  for (const entry of entries) {
    if (!allowedEntries.has(entry)) {
      throw new Error(`Unexpected global claude-rs bin entry after install: ${entry}`);
    }
  }
  const requiredEntry = process.platform === "win32" ? "claude-rs.cmd" : "claude-rs";
  if (!entries.includes(requiredEntry)) {
    throw new Error(`Missing global claude-rs bin entry: ${path.join(binDir, requiredEntry)}`);
  }
  return path.join(binDir, requiredEntry);
}

function assertGlobalBinsRemoved(prefix) {
  const binDir = globalBinDir(prefix);
  if (!fs.existsSync(binDir)) {
    return;
  }
  const entries = fs.readdirSync(binDir).filter((entry) => entry === "claude-rs" || entry.startsWith("claude-rs."));
  if (entries.length > 0) {
    throw new Error(`Global claude-rs bin entries remained after uninstall: ${entries.join(", ")}`);
  }
}

function installedLauncherCommands(projectDir) {
  const binDir = path.join(projectDir, "node_modules", ".bin");
  if (process.platform === "win32") {
    return [
      {
        label: "cmd shim",
        command: process.env.ComSpec || "cmd.exe",
        windowsVerbatimArguments: true,
        args: (commandArgs) => [
          "/d",
          "/c",
          ["call", path.join(binDir, "claude-rs.cmd"), ...commandArgs]
            .map((arg, index) => (index === 0 ? arg : quoteCmdArg(arg)))
            .join(" ")
        ]
      },
      {
        label: "PowerShell shim",
        command: "powershell.exe",
        args: (commandArgs) => [
          "-NoProfile",
          "-ExecutionPolicy",
          "Bypass",
          "-File",
          path.join(binDir, "claude-rs.ps1"),
          ...commandArgs
        ]
      }
    ];
  }

  return [
    {
      label: "npm symlink",
      command: path.join(binDir, "claude-rs"),
      args: (commandArgs) => commandArgs
    }
  ];
}

function globalLauncherCommands(prefix) {
  const binDir = globalBinDir(prefix);
  if (process.platform === "win32") {
    return [
      {
        label: "global cmd shim",
        command: process.env.ComSpec || "cmd.exe",
        windowsVerbatimArguments: true,
        args: (commandArgs) => [
          "/d",
          "/c",
          ["call", path.join(binDir, "claude-rs.cmd"), ...commandArgs]
            .map((arg, index) => (index === 0 ? arg : quoteCmdArg(arg)))
            .join(" ")
        ]
      },
      {
        label: "global PowerShell shim",
        command: "powershell.exe",
        args: (commandArgs) => [
          "-NoProfile",
          "-ExecutionPolicy",
          "Bypass",
          "-File",
          path.join(binDir, "claude-rs.ps1"),
          ...commandArgs
        ]
      }
    ];
  }

  return [
    {
      label: "global npm bin",
      command: path.join(binDir, "claude-rs"),
      args: (commandArgs) => commandArgs
    }
  ];
}

function quoteCmdArg(value) {
  return `"${String(value).replaceAll('"', '""')}"`;
}

function runInstalledCommands(installedCommands, args, cwd, env) {
  return installedCommands.map((installedCommand) => ({
    label: installedCommand.label,
    ...runInstalledCommand(installedCommand, args, cwd, env)
  }));
}

function runInstalledCommand(installedCommand, args, cwd, env) {
  try {
    const stdout = execFileSync(installedCommand.command, installedCommand.args(args), {
      cwd,
      env,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
      windowsVerbatimArguments: installedCommand.windowsVerbatimArguments === true
    });
    return { stdout, stderr: "" };
  } catch (error) {
    const stdout = bufferToString(error.stdout);
    const stderr = bufferToString(error.stderr);
    throw new Error(
      `Installed claude-rs command failed via ${installedCommand.label}: ${args.join(" ")}\n` +
        `status: ${error.status ?? "unknown"}\n` +
        `stdout:\n${stdout}\n` +
        `stderr:\n${stderr}`
    );
  }
}

function assertDoctorReportsBridgeReadiness(stdout, platformPackage, label) {
  const report = JSON.parse(stdout);
  const runtimeDetails = assertDoctorReportsBundledRuntime(report, platformPackage, label);
  const bridgeDetails = assertDoctorReportsBridgeScript(report, label);
  return {
    runtimeKind: runtimeDetails.runtimeKind,
    runtimePath: runtimeDetails.runtimePath,
    bridgeScriptPath: bridgeDetails.bridgeScriptPath,
  };
}

function assertDoctorReportsBundledRuntime(report, platformPackage, label) {
  const runtimeCheck = report.checks?.find((check) => check.id === "bridge_runtime");
  if (!runtimeCheck) {
    throw new Error(`${label} doctor output did not include bridge_runtime check`);
  }
  if (runtimeCheck.status !== "pass") {
    throw new Error(`${label} bridge_runtime check did not pass: ${runtimeCheck.message}`);
  }
  if (runtimeCheck.details?.runtime_kind !== "bundled_bun") {
    throw new Error(`${label} bridge_runtime kind was not bundled_bun: ${runtimeCheck.details?.runtime_kind}`);
  }
  const expectedPackagePath = normalizePath(path.join(...platformPackage.packageName.split("/")));
  const runtimePath = normalizePath(runtimeCheck.message);
  if (!runtimePath.includes(expectedPackagePath) || runtimePath.includes("node.exe")) {
    throw new Error(`${label} bridge_runtime path did not point at the installed platform package: ${runtimePath}`);
  }
  return {
    runtimeKind: runtimeCheck.details.runtime_kind,
    runtimePath: runtimeCheck.message,
  };
}

function assertDoctorReportsBridgeScript(report, label) {
  const scriptCheck = report.checks?.find((check) => check.id === "bridge_script");
  if (!scriptCheck) {
    throw new Error(`${label} doctor output did not include bridge_script check`);
  }
  if (scriptCheck.status !== "pass") {
    throw new Error(`${label} bridge_script check did not pass: ${scriptCheck.message}`);
  }

  const expectedBridgePath = normalizePath(
    path.join("node_modules", ROOT_PACKAGE_NAME, "agent-sdk", "dist", "bridge.js")
  );
  const resolvedPath = normalizePath(scriptCheck.message);
  const envPath = normalizePath(scriptCheck.details?.CLAUDE_RS_AGENT_BRIDGE ?? "");
  if (!resolvedPath.includes(expectedBridgePath) || !envPath.includes(expectedBridgePath)) {
    throw new Error(
      `${label} bridge_script path did not point at the installed root package: ` +
        `message=${resolvedPath}; env=${envPath}`
    );
  }
  return {
    bridgeScriptPath: scriptCheck.message,
  };
}

export async function startLocalRegistry(packedPackages) {
  const packages = new Map();
  const tarballs = new Map();

  for (const tarballPath of packedPackages.values()) {
    const packageJson = readPackageJsonFromTarball(tarballPath);
    const tarballBuffer = fs.readFileSync(tarballPath);
    const filename = path.basename(tarballPath);
    const record = {
      packageJson,
      tarballPath,
      filename,
      shasum: crypto.createHash("sha1").update(tarballBuffer).digest("hex"),
      integrity: `sha512-${crypto.createHash("sha512").update(tarballBuffer).digest("base64")}`,
    };
    const versions = packages.get(packageJson.name) ?? new Map();
    versions.set(packageJson.version, record);
    packages.set(packageJson.name, versions);
    tarballs.set(filename, record);
  }

  const server = http.createServer((request, response) => {
    void handleRegistryRequest({ request, response, packages, tarballs }).catch((error) => {
      response.writeHead(500, { "content-type": "text/plain; charset=utf-8" });
      response.end(error instanceof Error ? error.message : String(error));
    });
  });

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const url = `http://127.0.0.1:${address.port}`;
  return {
    url,
    close: () => new Promise((resolve, reject) => server.close((error) => (error ? reject(error) : resolve()))),
  };
}

async function handleRegistryRequest({ request, response, packages, tarballs }) {
  if (!request.url) {
    response.writeHead(400);
    response.end("missing url");
    return;
  }

  const url = new URL(request.url, "http://127.0.0.1");
  if ((request.method === "GET" || request.method === "HEAD") && url.pathname.startsWith("/tarballs/")) {
    const filename = decodeURIComponent(url.pathname.split("/").at(-1) ?? "");
    const record = tarballs.get(filename);
    if (!record) {
      response.writeHead(404);
      response.end("missing tarball");
      return;
    }
    response.writeHead(200, {
      "content-type": "application/octet-stream",
      "content-length": fs.statSync(record.tarballPath).size,
    });
    if (request.method === "HEAD") {
      response.end();
      return;
    }
    fs.createReadStream(record.tarballPath).pipe(response);
    return;
  }

  const packageName = decodeURIComponent(url.pathname.slice(1));
  const versions = packages.get(packageName);
  if ((request.method === "GET" || request.method === "HEAD") && versions) {
    const baseUrl = `http://${request.headers.host}`;
    const metadata = packageMetadata(packageName, versions, baseUrl);
    const body = JSON.stringify(metadata);
    response.writeHead(200, {
      "content-type": "application/json",
      "content-length": Buffer.byteLength(body),
    });
    response.end(request.method === "HEAD" ? undefined : body);
    return;
  }

  response.writeHead(404, { "content-type": "application/json" });
  response.end(JSON.stringify({ error: `local registry has no package metadata for ${packageName}` }));
}

function packageMetadata(packageName, versions, baseUrl) {
  const sortedRecords = [...versions.values()].sort((left, right) =>
    left.packageJson.version.localeCompare(right.packageJson.version, "en", { numeric: true })
  );
  const latest = sortedRecords.at(-1)?.packageJson.version;
  return {
    name: packageName,
    "dist-tags": {
      latest,
    },
    versions: Object.fromEntries(
      sortedRecords.map(({ packageJson, filename, shasum, integrity }) => [
        packageJson.version,
        {
          ...packageJson,
          dist: {
            shasum,
            integrity,
            tarball: `${baseUrl}/tarballs/${encodeURIComponent(filename)}`,
          },
        },
      ])
    ),
  };
}

function readPackageJsonFromTarball(tarballPath) {
  const output = execFileSync("tar", ["-xOf", tarballPath, "package/package.json"], {
    encoding: "utf8",
    windowsHide: true,
  });
  return JSON.parse(output);
}

export function envWithoutSystemRuntimePath(baseEnv = process.env, platform = process.platform) {
  const env = { ...baseEnv };
  const pathKey = Object.keys(env).find((key) => key.toLowerCase() === "path") ?? "PATH";
  const runtimeDirs = runtimeExecutableDirs(env, platform);
  const entries = (env[pathKey] ?? "")
    .split(path.delimiter)
    .filter((entry) => entry.trim().length > 0)
    .filter((entry) => !runtimeDirs.has(normalizePathForCompare(entry, platform)));
  env[pathKey] = entries.join(path.delimiter);
  return env;
}

function runtimeExecutableDirs(env, platform) {
  const dirs = new Set();
  for (const executable of resolveExecutablesOnPath("bun", env, platform)) {
    dirs.add(normalizePathForCompare(path.dirname(executable.path), platform));
    if (executable.realPath) {
      dirs.add(normalizePathForCompare(path.dirname(executable.realPath), platform));
    }
  }
  return dirs;
}

export function resolveExecutablesOnPath(command, env = process.env, platform = process.platform, options = {}) {
  const includeWhere = options.includeWhere ?? true;
  const pathKey = Object.keys(env).find((key) => key.toLowerCase() === "path") ?? "PATH";
  const entries = (env[pathKey] ?? "").split(path.delimiter).filter((entry) => entry.trim().length > 0);
  const executables = new Map();
  const candidates =
    platform === "win32"
      ? windowsExecutableNames(command, env)
      : [command];

  for (const entry of entries) {
    for (const candidate of candidates) {
      const executablePath = path.join(entry, candidate);
      const stat = statExecutable(executablePath, platform);
      if (!stat) {
        continue;
      }
      const realPath = realpathOrUndefined(executablePath);
      executables.set(normalizePathForCompare(executablePath, platform), {
        path: executablePath,
        realPath
      });
    }
  }

  if (platform === "win32" && includeWhere) {
    for (const wherePath of whereExecutablePaths(command)) {
      executables.set(normalizePathForCompare(wherePath, platform), {
        path: wherePath,
        realPath: realpathOrUndefined(wherePath)
      });
    }
  }

  return [...executables.values()];
}

function windowsExecutableNames(command, env) {
  const extension = path.extname(command);
  if (extension) {
    return [command];
  }
  const pathExt = env.PATHEXT ?? env.PathExt ?? ".COM;.EXE;.BAT;.CMD";
  return [command, ...pathExt.split(";").filter(Boolean).map((ext) => `${command}${ext.toLowerCase()}`)];
}

function whereExecutablePaths(command) {
  try {
    return execFileSync("where.exe", [command], { encoding: "utf8", windowsHide: true })
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean);
  } catch {
    return [];
  }
}

function runBridgeRuntimeContract(runtimePath, bridgeScriptPath, cwd) {
  const contractScript = path.join(repoRoot, "agent-sdk", "scripts", "bridge-runtime-contract.mjs");
  execFileSync(process.execPath, [contractScript, "--runtime", runtimePath, "--bridge", bridgeScriptPath], {
    cwd,
    stdio: "inherit",
    windowsHide: true,
  });
}

function statExecutable(executablePath, platform) {
  try {
    const stat = fs.statSync(executablePath);
    if (!stat.isFile()) {
      return undefined;
    }
    if (platform !== "win32" && process.platform !== "win32" && (stat.mode & 0o111) === 0) {
      return undefined;
    }
    return stat;
  } catch {
    return undefined;
  }
}

function realpathOrUndefined(executablePath) {
  try {
    return fs.realpathSync(executablePath);
  } catch {
    return undefined;
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

function normalizePath(filePath) {
  return filePath.replaceAll(path.sep, "/");
}

function normalizePathForCompare(filePath, platform = process.platform) {
  const resolved = path.resolve(filePath);
  return platform === "win32" ? resolved.toLowerCase() : resolved;
}

function printManifestSummary(distDir) {
  const summaryPath = path.join(distDir, "manifests", "summary.json");
  if (!fs.existsSync(summaryPath)) {
    return;
  }

  console.log("Package manifest summary:");
  console.log(fs.readFileSync(summaryPath, "utf8").trim());
}

function runNpm(args, cwd, { env = process.env } = {}) {
  return execFileSync(process.execPath, [resolvedNpmCliPath(), ...args], {
    cwd,
    env,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
    windowsHide: true
  });
}

function runNpmAsync(args, cwd, { env = process.env } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [resolvedNpmCliPath(), ...args], {
      cwd,
      env,
      stdio: ["ignore", "pipe", "inherit"],
      windowsHide: true,
    });
    let stdout = "";

    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) {
        resolve(stdout);
        return;
      }
      reject(new Error(`npm ${args.join(" ")} failed with exit code ${code ?? "unknown"}`));
    });
  });
}

function resolvedNpmCliPath() {
  npmCliPath ??= resolveNpmCliPath();
  return npmCliPath;
}

function resolveNpmCliPath() {
  const candidates = [
    process.env.npm_execpath,
    path.join(path.dirname(process.execPath), "node_modules", "npm", "bin", "npm-cli.js"),
    path.resolve(path.dirname(process.execPath), "..", "lib", "node_modules", "npm", "bin", "npm-cli.js")
  ].filter(Boolean);

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }

  throw new Error(
    "Could not locate npm's CLI entrypoint. Re-run this script with npm_execpath set to npm-cli.js."
  );
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
    realBinary: false,
    noSystemRuntime: false,
    registrySmoke: false
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
      case "--no-system-runtime":
        parsed.noSystemRuntime = true;
        break;
      case "--registry-smoke":
        parsed.registrySmoke = true;
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
  console.log(`Usage: node scripts/npm/smoke-npm-package-install.mjs [options]

Options:
  --dist-dir <dir>      Generated package directory. Defaults to dist-npm.
  --pack-dir <dir>      Directory for npm tarballs. Defaults to dist-pack.
  --platform <dir>      Platform package directory to install. Defaults to linux-x64-gnu.
  --use-existing-tarballs
                        Install tarballs that already exist in --pack-dir instead of packing dist-npm.
  --registry-smoke      Serve tarballs from a temp registry and npm install -g the root package.
  --real-binary         Require --version and --help to exit successfully without mock output checks.
  --no-system-runtime   Strip directories containing bun from PATH when running claude-rs.
  --keep-temp           Keep the temporary smoke project for inspection.
  -h, --help            Show this help.
`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  });
}
