import fs from "node:fs";
import path from "node:path";

export const ROOT_PACKAGE_NAME = "claude-code-rust";

export const DIST_NPM_DIR = "dist-npm";

export const BUNDLED_BUN_RUNTIME_VERSION = "1.3.14";

export const BUN_RUNTIME_MANIFEST_RELATIVE_PATH = "scripts/bun-runtime-manifest.json";

export const PLATFORM_PACKAGES = [
  {
    dir: "darwin-arm64",
    packageName: "@srothgan/claude-code-rust-darwin-arm64",
    os: ["darwin"],
    cpu: ["arm64"],
    binaryName: "claude-rs",
    bundledRuntimeName: "claude-rs-bridge-bun",
    bunAssetName: "bun-darwin-aarch64.zip",
    bunArchiveBinaryPath: "bun-darwin-aarch64/bun",
    rustTarget: "aarch64-apple-darwin"
  },
  {
    dir: "darwin-x64",
    packageName: "@srothgan/claude-code-rust-darwin-x64",
    os: ["darwin"],
    cpu: ["x64"],
    binaryName: "claude-rs",
    bundledRuntimeName: "claude-rs-bridge-bun",
    bunAssetName: "bun-darwin-x64-baseline.zip",
    bunArchiveBinaryPath: "bun-darwin-x64-baseline/bun",
    rustTarget: "x86_64-apple-darwin"
  },
  {
    dir: "linux-x64-gnu",
    packageName: "@srothgan/claude-code-rust-linux-x64-gnu",
    os: ["linux"],
    cpu: ["x64"],
    libc: ["glibc"],
    binaryName: "claude-rs",
    bundledRuntimeName: "claude-rs-bridge-bun",
    bunAssetName: "bun-linux-x64-baseline.zip",
    bunArchiveBinaryPath: "bun-linux-x64-baseline/bun",
    rustTarget: "x86_64-unknown-linux-gnu"
  },
  {
    dir: "linux-arm64-gnu",
    packageName: "@srothgan/claude-code-rust-linux-arm64-gnu",
    os: ["linux"],
    cpu: ["arm64"],
    libc: ["glibc"],
    binaryName: "claude-rs",
    bundledRuntimeName: "claude-rs-bridge-bun",
    bunAssetName: "bun-linux-aarch64.zip",
    bunArchiveBinaryPath: "bun-linux-aarch64/bun",
    rustTarget: "aarch64-unknown-linux-gnu"
  },
  {
    dir: "win32-x64-msvc",
    packageName: "@srothgan/claude-code-rust-win32-x64-msvc",
    os: ["win32"],
    cpu: ["x64"],
    binaryName: "claude-rs.exe",
    bundledRuntimeName: "claude-rs-bridge-bun.exe",
    bunAssetName: "bun-windows-x64-baseline.zip",
    bunArchiveBinaryPath: "bun-windows-x64-baseline/bun.exe",
    rustTarget: "x86_64-pc-windows-msvc"
  },
  {
    dir: "win32-arm64-msvc",
    packageName: "@srothgan/claude-code-rust-win32-arm64-msvc",
    os: ["win32"],
    cpu: ["arm64"],
    binaryName: "claude-rs.exe",
    bundledRuntimeName: "claude-rs-bridge-bun.exe",
    bunAssetName: "bun-windows-aarch64.zip",
    bunArchiveBinaryPath: "bun-windows-aarch64/bun.exe",
    rustTarget: "aarch64-pc-windows-msvc"
  }
];

export function readBunRuntimeManifest(repoRoot) {
  return readJson(path.join(repoRoot, BUN_RUNTIME_MANIFEST_RELATIVE_PATH));
}

export function bunRuntimeAssetUrl(version, assetName) {
  return `https://github.com/oven-sh/bun/releases/download/bun-v${version}/${assetName}`;
}

export function readJson(path) {
  return JSON.parse(fs.readFileSync(path, "utf8"));
}

export function readCargoPackageMetadata(cargoTomlPath) {
  const cargoToml = fs.readFileSync(cargoTomlPath, "utf8");
  const packageBody = readTomlSection(cargoToml, "package");
  if (!packageBody) {
    throw new Error(`Could not find [package] section in ${cargoTomlPath}`);
  }

  return {
    name: readTomlString(packageBody, "name"),
    version: readTomlString(packageBody, "version"),
    description: readTomlString(packageBody, "description"),
    license: readTomlString(packageBody, "license")
  };
}

function readTomlSection(toml, sectionName) {
  const lines = toml.split(/\r?\n/);
  const sectionHeader = new RegExp(`^\\s*\\[${sectionName}\\]\\s*$`);
  const anySectionHeader = /^\s*\[/;
  const sectionLines = [];
  let inSection = false;

  for (const line of lines) {
    if (sectionHeader.test(line)) {
      inSection = true;
      continue;
    }

    if (inSection && anySectionHeader.test(line)) {
      break;
    }

    if (inSection) {
      sectionLines.push(line);
    }
  }

  return sectionLines.join("\n");
}

function readTomlString(sectionBody, key) {
  const match = sectionBody.match(new RegExp(`^${key}\\s*=\\s*"([^"]+)"\\s*$`, "m"));
  if (!match) {
    throw new Error(`Could not find string field ${key} in Cargo.toml [package] section`);
  }
  return match[1];
}

export function buildRootPackageJson({ version, rootPackageJson, agentSdkPackageJson }) {
  return removeUndefined({
    name: ROOT_PACKAGE_NAME,
    version,
    description: rootPackageJson.description,
    keywords: rootPackageJson.keywords,
    license: rootPackageJson.license,
    repository: rootPackageJson.repository,
    bugs: rootPackageJson.bugs,
    homepage: rootPackageJson.homepage,
    bin: {
      "claude-rs": "bin/claude-rs.js"
    },
    files: ["bin", "agent-sdk", "README.md", "LICENSE"],
    dependencies: agentSdkPackageJson.dependencies ?? {},
    optionalDependencies: Object.fromEntries(
      PLATFORM_PACKAGES.map((platformPackage) => [platformPackage.packageName, version])
    ),
    engines: rootPackageJson.engines,
    publishConfig: {
      access: "public"
    }
  });
}

export function buildPlatformPackageJson({ platformPackage, version, rootPackageJson }) {
  return removeUndefined({
    name: platformPackage.packageName,
    version,
    description: `Claude Code Rust native binary and private Bun bridge runtime for ${platformPackage.dir}`,
    keywords: rootPackageJson.keywords,
    license: rootPackageJson.license,
    repository: rootPackageJson.repository,
    bugs: rootPackageJson.bugs,
    homepage: rootPackageJson.homepage,
    files: ["bin", "README.md", "LICENSE", "THIRD-PARTY-NOTICES.md"],
    os: platformPackage.os,
    cpu: platformPackage.cpu,
    libc: platformPackage.libc,
    publishConfig: {
      access: "public"
    }
  });
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
