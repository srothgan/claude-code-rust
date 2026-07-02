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
const npmCliPath = resolveNpmCliPath();

const options = parseArgs(process.argv.slice(2));
if (options.help) {
  printHelp();
  process.exit(0);
}

const distDir = path.resolve(repoRoot, options.distDir ?? DIST_NPM_DIR);
const manifestsDir = path.join(distDir, "manifests");
const cargoPackage = readCargoPackageMetadata(path.join(repoRoot, "Cargo.toml"));
const expectedVersion = options.version ?? cargoPackage.version;
const failures = [];
const packageManifests = [];

verifyDistDir();
exitIfFailures();
fs.rmSync(manifestsDir, { recursive: true, force: true });
fs.mkdirSync(manifestsDir, { recursive: true });

const rootPackageManifest = verifyRootPackage();
packageManifests.push(rootPackageManifest);

for (const platformPackage of PLATFORM_PACKAGES) {
  const manifest = verifyPlatformPackage(platformPackage);
  packageManifests.push(manifest);
}

writeJson(path.join(manifestsDir, "summary.json"), {
  manifestVersion: 1,
  packageCount: packageManifests.length,
  packages: packageManifests.map(({ name, version, directory, files }) => ({
    name,
    version,
    directory,
    fileCount: files.length
  }))
});

if (failures.length > 0) {
  exitIfFailures();
}

console.log(`Verified ${packageManifests.length} npm packages in ${path.relative(repoRoot, distDir)}`);
console.log(`Wrote manifests to ${path.relative(repoRoot, manifestsDir)}`);

function verifyDistDir() {
  if (!fs.existsSync(distDir)) {
    fail(`Missing generated package directory: ${path.relative(repoRoot, distDir)}`);
  }
}

function verifyRootPackage() {
  const packageDir = path.join(distDir, "root");
  const packageJson = readPackageJson(packageDir);
  const context = "root package";

  expectEqual(packageJson.name, ROOT_PACKAGE_NAME, `${context} name`);
  expectEqual(packageJson.version, expectedVersion, `${context} version`);
  expectDeepEqual(packageJson.bin, { "claude-rs": "bin/claude-rs.js" }, `${context} bin`);
  expectNoLifecycleScripts(packageJson, context);
  expectNoForbiddenManifestFields(packageJson, context);

  const optionalDependencies = packageJson.optionalDependencies ?? {};
  for (const platformPackage of PLATFORM_PACKAGES) {
    expectEqual(
      optionalDependencies[platformPackage.packageName],
      expectedVersion,
      `${context} optional dependency ${platformPackage.packageName}`
    );
  }
  expectEqual(
    Object.keys(optionalDependencies).length,
    PLATFORM_PACKAGES.length,
    `${context} optional dependency count`
  );

  const files = listPackageFiles(packageDir);
  expectFilesExist(files, ["package.json", "bin/claude-rs.js", "agent-sdk/package.json", "README.md", "LICENSE"], context);
  expectFilesExist(files, ["agent-sdk/dist/bridge.js", "agent-sdk/dist/types.js"], context);
  expectOnlyAllowedFiles(files, rootAllowedFile, context);
  expectNoForbiddenFiles(files, context);
  expectLauncherUsesPlatformPackages(packageDir, context);

  const packManifest = packAndVerify(packageDir, context, rootAllowedFile);
  writePackageManifest("root", packageJson, packManifest.files);

  return {
    directory: "root",
    name: packageJson.name,
    version: packageJson.version,
    files: packManifest.files
  };
}

function verifyPlatformPackage(platformPackage) {
  const packageDir = path.join(distDir, platformPackage.dir);
  const packageJson = readPackageJson(packageDir);
  const context = `${platformPackage.dir} package`;
  const expectedBinaryPath = `bin/${platformPackage.binaryName}`;

  expectEqual(packageJson.name, platformPackage.packageName, `${context} name`);
  expectEqual(packageJson.version, expectedVersion, `${context} version`);
  expectDeepEqual(packageJson.os, platformPackage.os, `${context} os`);
  expectDeepEqual(packageJson.cpu, platformPackage.cpu, `${context} cpu`);
  expectDeepEqual(packageJson.libc, platformPackage.libc, `${context} libc`);
  expectEqual(packageJson.bin, undefined, `${context} bin`);
  expectNoLifecycleScripts(packageJson, context);
  expectNoForbiddenManifestFields(packageJson, context);

  const files = listPackageFiles(packageDir);
  expectDeepEqual(
    files,
    ["LICENSE", "README.md", expectedBinaryPath, "package.json"].sort(),
    `${context} local file list`
  );
  expectNoForbiddenFiles(files, context);
  expectUnixBinaryExecutable(packageDir, platformPackage, expectedBinaryPath, context);

  const packManifest = packAndVerify(packageDir, context, (filePath) =>
    ["LICENSE", "README.md", expectedBinaryPath, "package.json"].includes(filePath)
  );
  writePackageManifest(platformPackage.dir, packageJson, packManifest.files);

  return {
    directory: platformPackage.dir,
    name: packageJson.name,
    version: packageJson.version,
    files: packManifest.files
  };
}

function readPackageJson(packageDir) {
  const packageJsonPath = path.join(packageDir, "package.json");
  if (!fs.existsSync(packageJsonPath)) {
    fail(`Missing package.json in ${path.relative(repoRoot, packageDir)}`);
    return {};
  }
  return readJson(packageJsonPath);
}

function listPackageFiles(packageDir) {
  if (!fs.existsSync(packageDir)) {
    fail(`Missing package directory: ${path.relative(repoRoot, packageDir)}`);
    return [];
  }

  const files = [];
  collectFiles(packageDir, packageDir, files);
  return files.sort();
}

function collectFiles(root, currentDir, files) {
  for (const entry of fs.readdirSync(currentDir, { withFileTypes: true })) {
    const fullPath = path.join(currentDir, entry.name);
    const relativePath = normalizePath(path.relative(root, fullPath));

    if (entry.isDirectory()) {
      collectFiles(root, fullPath, files);
      continue;
    }

    if (entry.isFile()) {
      files.push(relativePath);
    }
  }
}

function packAndVerify(packageDir, context, allowedFile) {
  if (!fs.existsSync(packageDir)) {
    return { files: [] };
  }

  const cacheDir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-npm-pack-cache-"));
  try {
    const output = runNpm(["pack", ".", "--dry-run", "--json", "--cache", cacheDir], packageDir);

    const parsed = JSON.parse(output);
    const [packResult] = parsed;
    if (!packResult) {
      fail(`${context} npm pack returned no package metadata`);
      return { files: [] };
    }

    const files = packResult.files
      .map((file) => ({
        path: normalizePath(file.path),
        size: file.size,
        mode: file.mode
      }))
      .sort((left, right) => left.path.localeCompare(right.path));

    expectNoForbiddenFiles(
      files.map((file) => file.path),
      `${context} packed tarball`
    );

    for (const file of files) {
      if (!allowedFile(file.path)) {
        fail(`${context} packed tarball contains unexpected file: ${file.path}`);
      }
    }

    return {
      filename: packResult.filename,
      files
    };
  } finally {
    fs.rmSync(cacheDir, { recursive: true, force: true });
  }
}

function runNpm(args, cwd) {
  return execFileSync(process.execPath, [npmCliPath, ...args], {
    cwd,
    encoding: "utf8",
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

  throw new Error(
    "Could not locate npm's CLI entrypoint. Re-run this script with npm_execpath set to npm-cli.js."
  );
}

function writePackageManifest(directory, packageJson, files) {
  writeJson(path.join(manifestsDir, `${directory}.json`), {
    name: packageJson.name,
    version: packageJson.version,
    files
  });
}

function rootAllowedFile(filePath) {
  if (["LICENSE", "README.md", "package.json", "bin/claude-rs.js", "agent-sdk/package.json"].includes(filePath)) {
    return true;
  }

  if (filePath === "agent-sdk/dist/bridge.js" || filePath === "agent-sdk/dist/types.js") {
    return true;
  }

  return /^agent-sdk\/dist\/bridge\/[A-Za-z0-9_-]+\.js$/.test(filePath);
}

function expectNoLifecycleScripts(packageJson, context) {
  const lifecycleScripts = [
    "preinstall",
    "install",
    "postinstall",
    "prepublish",
    "prepublishOnly",
    "prepare",
    "prepack",
    "postpack",
    "publish",
    "postpublish"
  ];
  const scripts = packageJson.scripts ?? {};

  for (const scriptName of lifecycleScripts) {
    if (Object.hasOwn(scripts, scriptName)) {
      fail(`${context} must not define lifecycle script ${scriptName}`);
    }
  }
}

function expectNoForbiddenManifestFields(packageJson, context) {
  if (packageJson.scripts && Object.keys(packageJson.scripts).length > 0) {
    fail(`${context} must not define scripts`);
  }
}

function expectFilesExist(files, expectedFiles, context) {
  for (const expectedFile of expectedFiles) {
    if (!files.includes(expectedFile)) {
      fail(`${context} is missing required file: ${expectedFile}`);
    }
  }
}

function expectOnlyAllowedFiles(files, allowedFile, context) {
  for (const file of files) {
    if (!allowedFile(file)) {
      fail(`${context} contains unexpected file: ${file}`);
    }
  }
}

function expectNoForbiddenFiles(files, context) {
  for (const file of files) {
    if (isForbiddenFile(file)) {
      fail(`${context} contains forbidden file: ${file}`);
    }
  }
}

function expectLauncherUsesPlatformPackages(packageDir, context) {
  const launcher = fs.readFileSync(path.join(packageDir, "bin", "claude-rs.js"), "utf8");
  if (launcher.includes('"vendor"') || launcher.includes("'vendor'")) {
    fail(`${context} launcher must not resolve vendor binaries`);
  }
  if (launcher.includes("refreshBridgeRuntime") || launcher.includes("claude-rs-bridge-node")) {
    fail(`${context} launcher must not manage a copied Node bridge runtime`);
  }
  if (!launcher.includes("CLAUDE_RS_AGENT_BRIDGE") || !launcher.includes("agent-sdk")) {
    fail(`${context} launcher must pass the bundled agent-sdk bridge path to the native binary`);
  }

  for (const platformPackage of PLATFORM_PACKAGES) {
    if (!launcher.includes(platformPackage.packageName)) {
      fail(`${context} launcher does not reference ${platformPackage.packageName}`);
    }
  }
}

function isForbiddenFile(filePath) {
  const normalized = normalizePath(filePath);
  const segments = normalized.split("/");
  const basename = segments.at(-1) ?? "";

  return (
    basename === ".env" ||
    basename === ".npmrc" ||
    basename.endsWith(".log") ||
    basename.endsWith(".map") ||
    basename.endsWith(".test.js") ||
    segments.includes("node_modules") ||
    segments.includes("target") ||
    segments.includes("dist-npm") ||
    segments.includes("dist-assets") ||
    normalized === "scripts/postinstall.js"
  );
}

function expectUnixBinaryExecutable(packageDir, platformPackage, binaryPath, context) {
  if (process.platform === "win32" || platformPackage.os.includes("win32")) {
    return;
  }

  const mode = fs.statSync(path.join(packageDir, binaryPath)).mode;
  if ((mode & 0o111) === 0) {
    fail(`${context} Unix binary is not executable: ${binaryPath}`);
  }
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

function exitIfFailures() {
  if (failures.length === 0) {
    return;
  }

  console.error(`npm package verification failed with ${failures.length} issue(s):`);
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

function writeJson(destination, value) {
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function normalizePath(filePath) {
  return filePath.replaceAll(path.sep, "/");
}

function formatValue(value) {
  return JSON.stringify(value);
}

function parseArgs(args) {
  const parsed = {
    help: false,
    distDir: undefined,
    version: undefined
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
      case "--version":
        parsed.version = readArgValue(args, ++index, arg);
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
  console.log(`Usage: node scripts/verify-npm-packages.mjs [options]

Options:
  --dist-dir <dir>      Generated package directory. Defaults to dist-npm.
  --version <version>   Expected package version. Defaults to Cargo.toml.
  -h, --help            Show this help.
`);
}
