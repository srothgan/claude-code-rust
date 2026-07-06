#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { resolveRepoRoot } from "../shared/repo-root.mjs";
import {
  DIST_NPM_DIR,
  PLATFORM_PACKAGES,
  buildPlatformPackageJson,
  buildRootPackageJson,
  readCargoPackageMetadata,
  readBunRuntimeManifest,
  readJson
} from "../shared/npm-package-config.mjs";
import { buildThirdPartyNotices } from "../shared/third-party-notices.mjs";

const repoRoot = resolveRepoRoot(import.meta.url);

const options = parseArgs(process.argv.slice(2));
if (options.help) {
  printHelp();
  process.exit(0);
}

const outDir = path.resolve(repoRoot, options.outDir ?? DIST_NPM_DIR);
const binaryRoot = path.resolve(repoRoot, options.binaryRoot ?? "dist-platform");
const cargoPackage = readCargoPackageMetadata(path.join(repoRoot, "Cargo.toml"));
const version = options.version ?? cargoPackage.version;
const rootPackageJson = readJson(path.join(repoRoot, "package.json"));
const agentSdkPackageJson = readJson(path.join(repoRoot, "agent-sdk", "package.json"));
const bunRuntimeManifest = readBunRuntimeManifest(repoRoot);

if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
  throw new Error(`Invalid package version: ${version}`);
}

fs.rmSync(outDir, { recursive: true, force: true });
fs.mkdirSync(outDir, { recursive: true });
if (options.mockBinaries) {
  fs.writeFileSync(path.join(outDir, ".mock-binaries"), "mock-binaries\n", "utf8");
}

generateRootPackage();
for (const platformPackage of PLATFORM_PACKAGES) {
  generatePlatformPackage(platformPackage);
}

console.log(`Generated npm packages in ${path.relative(repoRoot, outDir)}`);

function generateRootPackage() {
  const packageDir = path.join(outDir, "root");
  fs.mkdirSync(packageDir, { recursive: true });

  writeJson(
    path.join(packageDir, "package.json"),
    buildRootPackageJson({ version, rootPackageJson, agentSdkPackageJson })
  );
  copyFileFromRepo("bin/claude-rs.js", path.join(packageDir, "bin", "claude-rs.js"));
  copyAgentSdkRuntime(path.join(packageDir, "agent-sdk"));
  copyFileFromRepo("README.md", path.join(packageDir, "README.md"));
  copyFileFromRepo("LICENSE", path.join(packageDir, "LICENSE"));
}

function generatePlatformPackage(platformPackage) {
  const packageDir = path.join(outDir, platformPackage.dir);
  fs.mkdirSync(packageDir, { recursive: true });

  writeJson(
    path.join(packageDir, "package.json"),
    buildPlatformPackageJson({ platformPackage, version, rootPackageJson })
  );
  writePlatformReadme(platformPackage, path.join(packageDir, "README.md"));
  copyFileFromRepo("LICENSE", path.join(packageDir, "LICENSE"));
  writeThirdPartyNotices(path.join(packageDir, "THIRD-PARTY-NOTICES.md"));

  const binaryDestination = path.join(packageDir, "bin", platformPackage.binaryName);
  const runtimeDestination = path.join(packageDir, "bin", platformPackage.bundledRuntimeName);
  fs.mkdirSync(path.dirname(binaryDestination), { recursive: true });

  if (options.mockBinaries) {
    writeMockBinary(platformPackage, binaryDestination, "claude-rs 0.0.0-mock");
    writeMockBinary(platformPackage, runtimeDestination, bunRuntimeManifest.version);
    return;
  }

  if (options.mockNativeBinary) {
    writeMockBinary(platformPackage, binaryDestination, "claude-rs 0.0.0-mock");
  } else {
    copyStagedBinary(
      platformPackage,
      path.join(binaryRoot, platformPackage.dir, "bin", platformPackage.binaryName),
      binaryDestination,
      "native binary"
    );
  }
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
      `Missing ${label} for ${platformPackage.packageName}: ${path.relative(repoRoot, source)}. ` +
        "Build/stage binaries first or pass --mock-binaries."
    );
  }

  fs.copyFileSync(source, destination);
  ensureExecutableMode(platformPackage, destination);
}

function copyAgentSdkRuntime(destinationDir) {
  const sourceDist = path.join(repoRoot, "agent-sdk", "dist");
  if (!fs.existsSync(path.join(sourceDist, "bridge.js"))) {
    throw new Error(
      "Missing agent-sdk/dist/bridge.js. Run `npm --prefix agent-sdk run build` before generating packages."
    );
  }

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
  for (const entry of fs.readdirSync(sourceDir, { withFileTypes: true })) {
    const source = path.join(sourceDir, entry.name);
    const destination = path.join(destinationDir, entry.name);

    if (!includePath(source)) {
      continue;
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

function copyFileFromRepo(relativeSource, destination) {
  const source = path.join(repoRoot, relativeSource);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.copyFileSync(source, destination);
}

function writePlatformReadme(platformPackage, destination) {
  const projectName = "claude-code-rust";
  const projectUrl = typeof rootPackageJson.homepage === "string" ? rootPackageJson.homepage : undefined;
  const projectReference = projectUrl ? `[${projectName}](${projectUrl})` : `\`${projectName}\``;
  const npmPackageUrl = `https://www.npmjs.com/package/${platformPackage.packageName}`;
  const targetDetails = [
    `- Rust target: \`${platformPackage.rustTarget}\``,
    `- npm package: [\`${platformPackage.packageName}\`](${npmPackageUrl})`,
    `- npm platform directory: \`${platformPackage.dir}\``,
    `- Bundled runtime: \`${platformPackage.bundledRuntimeName}\``,
    `- Bun release asset: \`${platformPackage.bunAssetName}\``,
    `- OS: \`${platformPackage.os.join(", ")}\``,
    `- CPU: \`${platformPackage.cpu.join(", ")}\``
  ];

  if (platformPackage.libc) {
    targetDetails.push(`- libc: \`${platformPackage.libc.join(", ")}\``);
  }

  const content = `# \`${platformPackage.packageName}\`

This is the **${platformPackage.rustTarget}** native binary package for ${projectReference} version \`${version}\`.

This package is selected automatically by the root \`${projectName}\` npm package on matching platforms. It contains the native Rust binary and a private Bun runtime named \`${platformPackage.bundledRuntimeName}\` for the Agent SDK bridge. The bundled runtime is not a user-facing \`bun\` command.

\`\`\`bash
npm install -g ${projectName}
\`\`\`

${targetDetails.join("\n")}
`;

  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, content, "utf8");
}

function writeThirdPartyNotices(destination) {
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(
    destination,
    buildThirdPartyNotices({ manifest: bunRuntimeManifest }),
    "utf8"
  );
}

function writeMockBinary(platformPackage, destination, output) {
  const isWindows = platformPackage.os.includes("win32");
  const content = isWindows
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

function writeJson(destination, value) {
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function parseArgs(args) {
  const parsed = {
    help: false,
    mockBinaries: false,
    mockNativeBinary: false,
    version: undefined,
    binaryRoot: undefined,
    outDir: undefined
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--help":
      case "-h":
        parsed.help = true;
        break;
      case "--mock-binaries":
        parsed.mockBinaries = true;
        break;
      case "--mock-native-binary":
        parsed.mockNativeBinary = true;
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
  console.log(`Usage: node scripts/npm/generate-npm-packages.mjs [options]

Options:
  --version <version>       Override the package version. Defaults to Cargo.toml.
  --binary-root <dir>       Directory containing staged platform binaries.
                            Defaults to dist-platform.
  --out-dir <dir>           Output directory. Defaults to dist-npm.
  --mock-binaries           Generate placeholder native and runtime binaries for layout tests.
  --mock-native-binary      Generate placeholder native binaries while copying staged runtimes.
  -h, --help                Show this help.
`);
}
