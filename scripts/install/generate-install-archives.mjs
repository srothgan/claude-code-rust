#!/usr/bin/env node
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { resolveRepoRoot } from "../shared/repo-root.mjs";
import {
  BUNDLED_BUN_RUNTIME_VERSION,
  PLATFORM_PACKAGES,
  ROOT_PACKAGE_NAME,
  bunRuntimeAssetUrl,
  expectedAgentSdkNativePackage,
  installArchiveBaseName,
  installArchiveFormat,
  installArchiveName,
  npmInstallPlatformArgs,
  readBunRuntimeManifest,
  readCargoPackageMetadata,
  readJson
} from "../shared/npm-package-config.mjs";
import { buildThirdPartyNotices } from "../shared/third-party-notices.mjs";
import {
  assertNoSymlinks,
  commandExists,
  createTarGzArchive,
  createZipArchive,
  fileSha256,
  listRelativeFiles,
  readArgValue,
  requireCommand,
  writeJson
} from "../shared/install-archive-common.mjs";

const repoRoot = resolveRepoRoot(import.meta.url);
const npmCliPath = resolveNpmCliPath();

const options = parseArgs(process.argv.slice(2));
if (options.help) {
  printHelp();
  process.exit(0);
}

const cargoPackage = readCargoPackageMetadata(path.join(repoRoot, "Cargo.toml"));
const rootPackageJson = readJson(path.join(repoRoot, "package.json"));
const bunRuntimeManifest = readBunRuntimeManifest(repoRoot);
const version = options.version ?? cargoPackage.version;
const binaryRoot = path.resolve(repoRoot, options.binaryRoot ?? "dist-platform");
const outDir = path.resolve(repoRoot, options.outDir ?? "dist-install");
const manifestsDir = path.join(outDir, "manifests");
const keptTempRoots = [];

if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
  throw new Error(`Invalid package version: ${version}`);
}

if (bunRuntimeManifest.version !== BUNDLED_BUN_RUNTIME_VERSION) {
  throw new Error(
    `Bun runtime manifest version ${bunRuntimeManifest.version} does not match ${BUNDLED_BUN_RUNTIME_VERSION}`
  );
}

assertArchiveToolsAvailable();
assertAgentSdkBridgeBuilt();

fs.rmSync(outDir, { recursive: true, force: true });
fs.mkdirSync(manifestsDir, { recursive: true });

for (const platformPackage of PLATFORM_PACKAGES) {
  generateInstallArchive(platformPackage);
}

if (keptTempRoots.length > 0) {
  console.log(`Kept install archive staging roots:\n${keptTempRoots.map((entry) => `- ${entry}`).join("\n")}`);
}

console.log(`Generated install archives in ${path.relative(repoRoot, outDir)}`);

function assertArchiveToolsAvailable() {
  requireCommand("tar", "creating Unix install archives");
  if (commandExists("zip")) {
    return;
  }
  if (process.platform === "win32" && commandExists("powershell.exe")) {
    return;
  }
  throw new Error(
    "Missing required command 'zip' for creating Windows install archives. " +
      "Install zip before running this script on non-Windows hosts."
  );
}

function assertAgentSdkBridgeBuilt() {
  const bridgePath = path.join(repoRoot, "agent-sdk", "dist", "bridge.js");
  const typesPath = path.join(repoRoot, "agent-sdk", "dist", "types.js");
  if (!fs.existsSync(bridgePath) || !fs.existsSync(typesPath)) {
    throw new Error(
      "Missing agent-sdk/dist bridge runtime. Run `npm --prefix agent-sdk run build` before generating install archives."
    );
  }
}

function generateInstallArchive(platformPackage) {
  const archiveName = installArchiveName(platformPackage, version);
  const archiveFormat = installArchiveFormat(platformPackage);
  const appRootName = installArchiveBaseName(platformPackage, version);
  const archivePath = path.join(outDir, archiveName);
  const tempParent = fs.mkdtempSync(path.join(os.tmpdir(), `claude-rs-install-${platformPackage.dir}-`));
  const appRoot = path.join(tempParent, appRootName);

  try {
    fs.mkdirSync(appRoot, { recursive: true });
    installProductionDependencies(appRoot, platformPackage);
    removeInstallOnlyFiles(appRoot);
    writeJson(path.join(appRoot, "package.json"), buildInstallPackageJson());
    copyBinaries(platformPackage, appRoot);
    copyAgentSdkRuntime(path.join(appRoot, "agent-sdk"));
    copyFileFromRepo("LICENSE", path.join(appRoot, "LICENSE"));
    fs.writeFileSync(
      path.join(appRoot, "THIRD-PARTY-NOTICES.md"),
      buildThirdPartyNotices({ manifest: bunRuntimeManifest }),
      "utf8"
    );

    assertNoSymlinks(appRoot, `${platformPackage.dir} install archive staging root`);
    if (archiveFormat === "tar.gz") {
      createTarGzArchive({ sourceParent: tempParent, rootName: appRootName, destination: archivePath });
    } else {
      createZipArchive({ sourceParent: tempParent, rootName: appRootName, destination: archivePath });
    }

    writeManifest(platformPackage, appRoot, archiveName, archiveFormat, archivePath);
  } finally {
    if (options.keepTemp) {
      keptTempRoots.push(tempParent);
    } else {
      fs.rmSync(tempParent, { recursive: true, force: true });
    }
  }
}

function installProductionDependencies(appRoot, platformPackage) {
  copyFileFromRepo("package.json", path.join(appRoot, "package.json"));
  copyFileFromRepo("package-lock.json", path.join(appRoot, "package-lock.json"));
  runNpm(["ci", "--omit=dev", "--ignore-scripts", ...npmInstallPlatformArgs(platformPackage)], appRoot);
  pruneProductionNodeModules(appRoot);
}

function removeInstallOnlyFiles(appRoot) {
  fs.rmSync(path.join(appRoot, "package-lock.json"), { force: true });
  fs.rmSync(path.join(appRoot, "node_modules", ".package-lock.json"), { force: true });
  fs.rmSync(path.join(appRoot, "node_modules", ".bin"), { recursive: true, force: true });
}

function pruneProductionNodeModules(appRoot) {
  const nodeModulesDir = path.join(appRoot, "node_modules");
  if (!fs.existsSync(nodeModulesDir)) {
    return;
  }
  pruneTree(nodeModulesDir);
}

function pruneTree(currentDir) {
  for (const entry of fs.readdirSync(currentDir, { withFileTypes: true }).sort(compareDirentNames)) {
    const fullPath = path.join(currentDir, entry.name);

    if (entry.isSymbolicLink()) {
      throw new Error(`Refusing to archive symlink from npm install: ${fullPath}`);
    }

    if (entry.isDirectory()) {
      if (shouldPruneDirectory(entry.name)) {
        fs.rmSync(fullPath, { recursive: true, force: true });
        continue;
      }
      pruneTree(fullPath);
      continue;
    }

    if (entry.isFile() && shouldPruneFile(entry.name)) {
      fs.rmSync(fullPath, { force: true });
    }
  }
}

function shouldPruneDirectory(name) {
  return [".bin", ".cache", ".github", "__tests__", "test", "tests", "src"].includes(name);
}

function shouldPruneFile(name) {
  return (
    name === ".package-lock.json" ||
    name.endsWith(".log") ||
    name.endsWith(".map") ||
    name.endsWith(".test.js") ||
    name.endsWith(".spec.js") ||
    name.endsWith(".ts") ||
    name.endsWith(".mts") ||
    name.endsWith(".cts")
  );
}

function buildInstallPackageJson() {
  return {
    name: ROOT_PACKAGE_NAME,
    version,
    private: true,
    description: rootPackageJson.description,
    license: rootPackageJson.license
  };
}

function copyBinaries(platformPackage, appRoot) {
  const binaryDestination = path.join(appRoot, platformPackage.binaryName);
  const runtimeDestination = path.join(appRoot, platformPackage.bundledRuntimeName);

  if (options.mockBinaries) {
    writeMockBinary(platformPackage, binaryDestination, "claude-rs 0.0.0-mock");
    writeMockBinary(platformPackage, runtimeDestination, bunRuntimeManifest.version);
    return;
  }

  copyStagedBinary(
    platformPackage,
    path.join(binaryRoot, platformPackage.dir, "bin", platformPackage.binaryName),
    binaryDestination,
    "native binary"
  );
  copyStagedBinary(
    platformPackage,
    path.join(binaryRoot, platformPackage.dir, "bin", platformPackage.bundledRuntimeName),
    runtimeDestination,
    "bundled Bun runtime"
  );
}

function copyStagedBinary(platformPackage, source, destination, label) {
  if (!fs.existsSync(source)) {
    throw new Error(
      `Missing ${label} for ${platformPackage.dir}: ${path.relative(repoRoot, source)}. ` +
        "Build/stage binaries first or pass --mock-binaries."
    );
  }

  fs.copyFileSync(source, destination);
  ensureExecutableMode(platformPackage, destination);
}

function writeMockBinary(platformPackage, destination, output) {
  const content = platformPackage.os.includes("win32")
    ? `@echo off\r\necho ${output}\r\n`
    : `#!/usr/bin/env sh\necho "${output}"\n`;

  fs.writeFileSync(destination, content, "utf8");
  ensureExecutableMode(platformPackage, destination);
}

function ensureExecutableMode(platformPackage, destination) {
  if (!platformPackage.os.includes("win32")) {
    fs.chmodSync(destination, 0o755);
  }
}

function copyAgentSdkRuntime(destinationDir) {
  const sourceDist = path.join(repoRoot, "agent-sdk", "dist");
  fs.mkdirSync(destinationDir, { recursive: true });
  writeJson(path.join(destinationDir, "package.json"), {
    private: true,
    type: "module"
  });
  copyDirectoryFiltered(sourceDist, path.join(destinationDir, "dist"), (sourcePath) => {
    const relativePath = path.relative(sourceDist, sourcePath).replaceAll(path.sep, "/");
    return !relativePath.endsWith(".test.js") && !relativePath.endsWith(".map");
  });
}

function copyDirectoryFiltered(sourceDir, destinationDir, includePath) {
  fs.mkdirSync(destinationDir, { recursive: true });
  for (const entry of fs.readdirSync(sourceDir, { withFileTypes: true }).sort(compareDirentNames)) {
    const source = path.join(sourceDir, entry.name);
    const destination = path.join(destinationDir, entry.name);

    if (!includePath(source)) {
      continue;
    }

    if (entry.isSymbolicLink()) {
      throw new Error(`Refusing to copy symlink into install archive: ${path.relative(repoRoot, source)}`);
    }

    if (entry.isDirectory()) {
      copyDirectoryFiltered(source, destination, includePath);
      continue;
    }

    if (entry.isFile()) {
      fs.mkdirSync(path.dirname(destination), { recursive: true });
      fs.copyFileSync(source, destination);
    }
  }
}

function writeManifest(platformPackage, appRoot, archiveName, archiveFormat, archivePath) {
  const runtimeAsset = bunRuntimeManifest.assets?.[platformPackage.dir];
  if (!runtimeAsset) {
    throw new Error(`Bun runtime manifest is missing ${platformPackage.dir}`);
  }

  const files = listRelativeFiles(appRoot);
  writeJson(path.join(manifestsDir, `${platformPackage.dir}.json`), {
    manifestVersion: 1,
    platform: platformPackage.dir,
    version,
    archiveName,
    archiveFormat,
    archiveSha256: fileSha256(archivePath),
    fileCount: files.length,
    files,
    binaryName: platformPackage.binaryName,
    expectedAgentSdkNativePackage: expectedAgentSdkNativePackage(platformPackage),
    bundledRuntime: {
      name: platformPackage.bundledRuntimeName,
      version: bunRuntimeManifest.version,
      sourceAsset: runtimeAsset.assetName,
      sourceUrl: bunRuntimeAssetUrl(bunRuntimeManifest.version, runtimeAsset.assetName),
      checksumSourceUrl: bunRuntimeManifest.checksumSourceUrl,
      archiveBinaryPath: runtimeAsset.archiveBinaryPath,
      sourceArchiveSha256: runtimeAsset.sha256,
      runtimeSha256: fileSha256(path.join(appRoot, platformPackage.bundledRuntimeName))
    }
  });
}

function copyFileFromRepo(relativeSource, destination) {
  const source = path.join(repoRoot, relativeSource);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.copyFileSync(source, destination);
}

function runNpm(args, cwd) {
  execFileSync(process.execPath, [npmCliPath, ...args], {
    cwd,
    stdio: "inherit",
    windowsHide: true
  });
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

  throw new Error("Could not locate npm's CLI entrypoint. Re-run with npm_execpath set to npm-cli.js.");
}

function compareDirentNames(left, right) {
  return left.name.localeCompare(right.name);
}

function parseArgs(args) {
  const parsed = {
    help: false,
    version: undefined,
    binaryRoot: undefined,
    outDir: undefined,
    mockBinaries: false,
    keepTemp: false
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
      case "--binary-root":
        parsed.binaryRoot = readArgValue(args, ++index, arg);
        break;
      case "--out-dir":
        parsed.outDir = readArgValue(args, ++index, arg);
        break;
      case "--mock-binaries":
        parsed.mockBinaries = true;
        break;
      case "--keep-temp":
        parsed.keepTemp = true;
        break;
      default:
        throw new Error(`Unknown argument: ${arg}`);
    }
  }

  return parsed;
}

function printHelp() {
  console.log(`Usage: node scripts/install/generate-install-archives.mjs [options]

Options:
  --version <version>       Override the package version. Defaults to Cargo.toml.
  --binary-root <dir>       Directory containing staged platform binaries.
                            Defaults to dist-platform.
  --out-dir <dir>           Output directory. Defaults to dist-install.
  --mock-binaries           Generate placeholder native and runtime binaries for layout tests.
  --keep-temp               Keep temporary staging roots for debugging.
  -h, --help                Show this help.
`);
}
