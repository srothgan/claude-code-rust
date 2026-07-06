#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import {
  BUNDLED_BUN_RUNTIME_VERSION,
  DIST_NPM_DIR,
  PLATFORM_PACKAGES,
  ROOT_PACKAGE_NAME,
  bunRuntimeAssetUrl,
  readCargoPackageMetadata,
  readBunRuntimeManifest,
  readJson
} from "./npm-package-config.mjs";
import { verifyThirdPartyNoticeCoverage } from "./verify-third-party-notices.mjs";

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
const bunRuntimeManifest = readBunRuntimeManifest(repoRoot);

expectEqual(bunRuntimeManifest.version, BUNDLED_BUN_RUNTIME_VERSION, "Bun runtime manifest version");
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
  packages: packageManifests.map(({ name, version, directory, files, bundledRuntime }) =>
    removeUndefined({
      name,
      version,
      directory,
      fileCount: files.length,
      bundledRuntime
    })
  )
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
  expectDeepEqual(
    packageJson.optionalDependencies,
    expectedPlatformOptionalDependencies(),
    `${context} optionalDependencies`
  );
  expectDeepEqual(packageJson.engines, { node: ">=18" }, `${context} engines`);
  expectNoLifecycleScripts(packageJson, context);
  expectNoForbiddenManifestFields(packageJson, context);

  const files = listPackageFiles(packageDir);
  expectFilesExist(files, ["package.json", "bin/claude-rs.js", "agent-sdk/package.json", "README.md", "LICENSE"], context);
  expectFilesExist(files, ["agent-sdk/dist/bridge.js", "agent-sdk/dist/types.js"], context);
  expectOnlyAllowedFiles(files, rootAllowedFile, context);
  expectNoForbiddenFiles(files, context);
  expectNoNodeShebangs(packageDir, files, context, ["bin/claude-rs.js"]);

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
  const expectedRuntimePath = `bin/${platformPackage.bundledRuntimeName}`;

  expectEqual(packageJson.name, platformPackage.packageName, `${context} name`);
  expectEqual(packageJson.version, expectedVersion, `${context} version`);
  expectDeepEqual(packageJson.os, platformPackage.os, `${context} os`);
  expectDeepEqual(packageJson.cpu, platformPackage.cpu, `${context} cpu`);
  expectDeepEqual(packageJson.libc, platformPackage.libc, `${context} libc`);
  expectEqual(packageJson.bin, undefined, `${context} bin`);
  expectEqual(packageJson.dependencies, undefined, `${context} dependencies`);
  expectNoLifecycleScripts(packageJson, context);
  expectNoForbiddenManifestFields(packageJson, context);

  const files = listPackageFiles(packageDir);
  expectDeepEqual(
    files,
    ["LICENSE", "README.md", "THIRD-PARTY-NOTICES.md", expectedBinaryPath, expectedRuntimePath, "package.json"].sort(),
    `${context} local file list`
  );
  expectNoForbiddenFiles(files, context);
  expectNoNodeShebangs(packageDir, files, context);
  expectThirdPartyNoticeCoverage(packageDir, context);
  expectPlatformReadme(packageDir, platformPackage, context);
  expectUnixBinaryExecutable(packageDir, platformPackage, expectedBinaryPath, context);
  expectUnixBinaryExecutable(packageDir, platformPackage, expectedRuntimePath, context);
  const bundledRuntime = buildBunRuntimeProvenance(platformPackage, packageDir, expectedRuntimePath, context);

  const packManifest = packAndVerify(packageDir, context, (filePath) =>
    ["LICENSE", "README.md", "THIRD-PARTY-NOTICES.md", expectedBinaryPath, expectedRuntimePath, "package.json"].includes(filePath)
  );
  writePackageManifest(platformPackage.dir, packageJson, packManifest.files, { bundledRuntime });

  return {
    directory: platformPackage.dir,
    name: packageJson.name,
    version: packageJson.version,
    files: packManifest.files,
    bundledRuntime
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

function writePackageManifest(directory, packageJson, files, extra = {}) {
  writeJson(path.join(manifestsDir, `${directory}.json`), {
    name: packageJson.name,
    version: packageJson.version,
    ...extra,
    files
  });
}

function buildBunRuntimeProvenance(platformPackage, packageDir, runtimeFilePath, context) {
  const runtimeAsset = bunRuntimeManifest.assets?.[platformPackage.dir];
  if (!runtimeAsset) {
    fail(`${context} missing Bun runtime manifest entry`);
    return undefined;
  }

  expectEqual(runtimeAsset.assetName, platformPackage.bunAssetName, `${context} Bun asset name`);
  expectEqual(
    runtimeAsset.archiveBinaryPath,
    platformPackage.bunArchiveBinaryPath,
    `${context} Bun archive binary path`
  );
  const actualRuntimeSha256 = fileSha256(path.join(packageDir, runtimeFilePath));
  expectEqual(actualRuntimeSha256, runtimeAsset.binarySha256, `${context} bundled Bun runtime SHA256`);

  return {
    name: platformPackage.bundledRuntimeName,
    version: bunRuntimeManifest.version,
    sourceAsset: runtimeAsset.assetName,
    sourceUrl: bunRuntimeAssetUrl(bunRuntimeManifest.version, runtimeAsset.assetName),
    checksumSourceUrl: bunRuntimeManifest.checksumSourceUrl,
    archiveBinaryPath: runtimeAsset.archiveBinaryPath,
    sourceArchiveSha256: runtimeAsset.sha256,
    runtimeSha256: actualRuntimeSha256
  };
}

function fileSha256(filePath) {
  return crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex");
}

function removeUndefined(value) {
  if (Array.isArray(value)) {
    return value.map(removeUndefined);
  }

  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .filter(([, entryValue]) => entryValue !== undefined)
        .map(([entryKey, entryValue]) => [entryKey, removeUndefined(entryValue)])
    );
  }

  return value;
}

function expectedPlatformOptionalDependencies() {
  return Object.fromEntries(
    PLATFORM_PACKAGES.map((platformPackage) => [platformPackage.packageName, expectedVersion])
  );
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

function expectNoNodeShebangs(packageDir, files, context, allowedFiles = []) {
  const allowed = new Set(allowedFiles);
  for (const file of files) {
    const fullPath = path.join(packageDir, file);
    if (!fs.statSync(fullPath).isFile()) {
      continue;
    }
    const buffer = fs.readFileSync(fullPath);
    const prefix = buffer.subarray(0, 64).toString("utf8");
    if (prefix.startsWith("#!/usr/bin/env node") && !allowed.has(file)) {
      fail(`${context} contains forbidden Node shebang: ${file}`);
    }
  }
}

function expectThirdPartyNoticeCoverage(packageDir, context) {
  try {
    verifyThirdPartyNoticeCoverage({
      noticesPath: path.join(packageDir, "THIRD-PARTY-NOTICES.md"),
      manifest: bunRuntimeManifest
    });
  } catch (error) {
    fail(`${context} third-party notices: ${error instanceof Error ? error.message : String(error)}`);
  }
}

function expectPlatformReadme(packageDir, platformPackage, context) {
  const readme = fs.readFileSync(path.join(packageDir, "README.md"), "utf8");
  expectIncludes(readme, `# \`${platformPackage.packageName}\``, `${context} README title`);
  expectIncludes(readme, platformPackage.rustTarget, `${context} README Rust target`);
  expectIncludes(readme, `version \`${expectedVersion}\``, `${context} README version`);
  expectIncludes(readme, `npm install -g ${ROOT_PACKAGE_NAME}`, `${context} README install command`);
  expectIncludes(readme, "selected automatically by the root", `${context} README root selection`);
  expectIncludes(readme, platformPackage.bundledRuntimeName, `${context} README bundled runtime`);
  expectIncludes(readme, "not a user-facing `bun` command", `${context} README private runtime`);
  expectIncludes(
    readme,
    `https://www.npmjs.com/package/${platformPackage.packageName}`,
    `${context} README npm package link`
  );

  if (platformPackage.libc) {
    expectIncludes(readme, `- libc: \`${platformPackage.libc.join(", ")}\``, `${context} README libc`);
  } else if (readme.includes("- libc:")) {
    fail(`${context} README must not include libc details`);
  }
}

function isForbiddenFile(filePath) {
  const normalized = normalizePath(filePath);
  const segments = normalized.split("/");
  const basename = segments.at(-1) ?? "";

  return (
    basename === ".env" ||
    basename === ".npmrc" ||
    basename === "bun" ||
    basename === "bun.exe" ||
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

function expectIncludes(haystack, needle, label) {
  if (!haystack.includes(needle)) {
    fail(`${label}: expected to include ${formatValue(needle)}`);
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
