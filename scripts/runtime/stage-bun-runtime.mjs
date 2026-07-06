#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import https from "node:https";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { resolveRepoRoot } from "../shared/repo-root.mjs";
import {
  BUNDLED_BUN_RUNTIME_VERSION,
  PLATFORM_PACKAGES,
  bunRuntimeAssetUrl,
  readBunRuntimeManifest
} from "../shared/npm-package-config.mjs";

const repoRoot = resolveRepoRoot(import.meta.url);
const options = parseArgs(process.argv.slice(2));

if (options.help) {
  printHelp();
  process.exit(0);
}

const manifest = readBunRuntimeManifest(repoRoot);
validateManifest(manifest);

if (options.verifyUpstream) {
  await verifyUpstreamChecksums(manifest);
}

if ((options.check || options.verifyUpstream) && !options.download) {
  console.log(`Validated Bun runtime manifest for Bun ${manifest.version}`);
  process.exit(0);
}

if (!options.download) {
  printHelp();
  process.exit(1);
}

const selectedPackages = selectPackages(options.platform);
const distPlatformDir = path.resolve(repoRoot, options.distPlatform ?? "dist-platform");
await stageRuntimes(selectedPackages, distPlatformDir);

async function stageRuntimes(platformPackages, distPlatformDir) {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-bun-runtime-"));

  try {
    for (const platformPackage of platformPackages) {
      const runtimeAsset = manifest.assets[platformPackage.dir];
      const url = bunRuntimeAssetUrl(manifest.version, runtimeAsset.assetName);
      const zipPath = path.join(tempDir, runtimeAsset.assetName);
      const extractDir = path.join(tempDir, platformPackage.dir);
      const destination = path.join(distPlatformDir, platformPackage.dir, "bin", platformPackage.bundledRuntimeName);

      await downloadFile(url, zipPath);
      verifySha256(zipPath, runtimeAsset.sha256, runtimeAsset.assetName);
      extractSingleFile(zipPath, runtimeAsset.archiveBinaryPath, extractDir);

      const extractedBinary = path.join(extractDir, ...runtimeAsset.archiveBinaryPath.split("/"));
      if (!fs.existsSync(extractedBinary)) {
        throw new Error(`Bun archive did not contain ${runtimeAsset.archiveBinaryPath}`);
      }
      verifySha256(extractedBinary, runtimeAsset.binarySha256, `${runtimeAsset.assetName} ${runtimeAsset.archiveBinaryPath}`);

      fs.mkdirSync(path.dirname(destination), { recursive: true });
      fs.copyFileSync(extractedBinary, destination);
      fs.chmodSync(destination, 0o755);
      console.log(
        `Staged ${runtimeAsset.assetName} as ${path.relative(repoRoot, destination).replaceAll(path.sep, "/")}`
      );
    }
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
}

function validateManifest(manifest) {
  const failures = [];

  if (manifest.manifestVersion !== 1) {
    failures.push(`manifestVersion must be 1, got ${manifest.manifestVersion}`);
  }

  if (manifest.version !== BUNDLED_BUN_RUNTIME_VERSION) {
    failures.push(
      `manifest version ${manifest.version} must match BUNDLED_BUN_RUNTIME_VERSION ${BUNDLED_BUN_RUNTIME_VERSION}`
    );
  }

  if (!/^\d+\.\d+\.\d+$/.test(manifest.version ?? "")) {
    failures.push(`Bun runtime version must be pinned as x.y.z, got ${manifest.version}`);
  }

  const expectedChecksumSourceUrl = `https://github.com/oven-sh/bun/releases/download/bun-v${manifest.version}/SHASUMS256.txt`;
  if (manifest.checksumSourceUrl !== expectedChecksumSourceUrl) {
    failures.push(`checksumSourceUrl must be ${expectedChecksumSourceUrl}`);
  }

  const expectedDirs = PLATFORM_PACKAGES.map((platformPackage) => platformPackage.dir).sort();
  const actualDirs = Object.keys(manifest.assets ?? {}).sort();
  if (JSON.stringify(actualDirs) !== JSON.stringify(expectedDirs)) {
    failures.push(`manifest assets must cover exactly: ${expectedDirs.join(", ")}`);
  }

  for (const platformPackage of PLATFORM_PACKAGES) {
    const asset = manifest.assets?.[platformPackage.dir];
    if (!asset) {
      continue;
    }

    if (asset.assetName !== platformPackage.bunAssetName) {
      failures.push(`${platformPackage.dir} assetName must be ${platformPackage.bunAssetName}`);
    }

    if (asset.archiveBinaryPath !== platformPackage.bunArchiveBinaryPath) {
      failures.push(`${platformPackage.dir} archiveBinaryPath must be ${platformPackage.bunArchiveBinaryPath}`);
    }

    if (!/^[a-f0-9]{64}$/.test(asset.sha256 ?? "")) {
      failures.push(`${platformPackage.dir} sha256 must be a 64-character lowercase hex digest`);
    }
    if (!/^[a-f0-9]{64}$/.test(asset.binarySha256 ?? "")) {
      failures.push(`${platformPackage.dir} binarySha256 must be a 64-character lowercase hex digest`);
    }
  }

  if (failures.length > 0) {
    throw new Error(`Invalid Bun runtime manifest:\n- ${failures.join("\n- ")}`);
  }
}

function selectPackages(platform) {
  if (!platform) {
    return PLATFORM_PACKAGES;
  }

  const platformPackage = PLATFORM_PACKAGES.find((candidate) => candidate.dir === platform);
  if (!platformPackage) {
    throw new Error(`Unknown platform package directory: ${platform}`);
  }
  return [platformPackage];
}

function verifySha256(filePath, expectedSha256, label) {
  const actualSha256 = crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex");
  if (actualSha256 !== expectedSha256) {
    throw new Error(`${label} SHA256 mismatch: expected ${expectedSha256}, got ${actualSha256}`);
  }
}

async function verifyUpstreamChecksums(manifest) {
  const shasums = parseShasums256(await fetchText(manifest.checksumSourceUrl));
  const failures = [];

  for (const platformPackage of PLATFORM_PACKAGES) {
    const runtimeAsset = manifest.assets[platformPackage.dir];
    const upstreamSha256 = shasums.get(runtimeAsset.assetName);
    if (!upstreamSha256) {
      failures.push(`missing ${runtimeAsset.assetName} in ${manifest.checksumSourceUrl}`);
      continue;
    }
    if (upstreamSha256 !== runtimeAsset.sha256) {
      failures.push(
        `${runtimeAsset.assetName} SHA256 mismatch: manifest ${runtimeAsset.sha256}, upstream ${upstreamSha256}`
      );
    }
  }

  await verifyGithubReleaseDigests(manifest, failures);

  if (failures.length > 0) {
    throw new Error(`Bun upstream checksum verification failed:\n- ${failures.join("\n- ")}`);
  }

  console.log(`Verified Bun ${manifest.version} runtime checksums against upstream release metadata`);
}

function parseShasums256(text) {
  const entries = new Map();
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) {
      continue;
    }
    const match = trimmed.match(/^([a-f0-9]{64})\s+\*?(.+)$/);
    if (match) {
      entries.set(path.basename(match[2].trim()), match[1]);
    }
  }
  return entries;
}

async function verifyGithubReleaseDigests(manifest, failures) {
  const releaseApiUrl = `https://api.github.com/repos/oven-sh/bun/releases/tags/bun-v${manifest.version}`;
  let release;
  try {
    release = JSON.parse(await fetchText(releaseApiUrl));
  } catch (error) {
    console.warn(
      `Warning: could not cross-check GitHub release API asset digests for Bun ${manifest.version}: ${
        error instanceof Error ? error.message : String(error)
      }`
    );
    return;
  }

  const digestsByName = new Map(
    (release.assets ?? [])
      .filter((asset) => typeof asset.name === "string" && typeof asset.digest === "string")
      .map((asset) => [asset.name, asset.digest])
  );

  for (const platformPackage of PLATFORM_PACKAGES) {
    const runtimeAsset = manifest.assets[platformPackage.dir];
    const digest = digestsByName.get(runtimeAsset.assetName);
    if (!digest) {
      continue;
    }
    const expectedDigest = `sha256:${runtimeAsset.sha256}`;
    if (digest !== expectedDigest) {
      failures.push(
        `${runtimeAsset.assetName} GitHub release digest mismatch: manifest ${expectedDigest}, API ${digest}`
      );
    }
  }
}

async function fetchText(url) {
  const response = await requestWithRedirects(url);
  if (response.statusCode !== 200) {
    response.resume();
    throw new Error(`Fetch failed for ${url}: HTTP ${response.statusCode}`);
  }
  return new Promise((resolve, reject) => {
    let body = "";
    response.setEncoding("utf8");
    response.on("data", (chunk) => {
      body += chunk;
    });
    response.on("end", () => resolve(body));
    response.on("error", reject);
  });
}

async function downloadFile(url, destination) {
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  const response = await requestWithRedirects(url);
  await new Promise((resolve, reject) => {
    const output = fs.createWriteStream(destination);

    if (response.statusCode !== 200) {
      output.destroy();
      response.resume();
      reject(new Error(`Download failed for ${url}: HTTP ${response.statusCode}`));
      return;
    }

    response.pipe(output);
    output.on("finish", () => {
      output.close(resolve);
    });
    output.on("error", reject);
    response.on("error", (error) => {
      output.destroy();
      reject(error);
    });
  });
}

function requestWithRedirects(url, redirectCount = 0) {
  return new Promise((resolve, reject) => {
    if (redirectCount > 5) {
      reject(new Error(`Too many redirects while downloading ${url}`));
      return;
    }

    const client = url.startsWith("https:") ? https : http;
    const request = client.get(
      url,
      {
        headers: {
          "User-Agent": "claude-code-rust-release-check"
        }
      },
      (response) => {
      if ([301, 302, 303, 307, 308].includes(response.statusCode) && response.headers.location) {
        response.resume();
        const redirectedUrl = new URL(response.headers.location, url).toString();
        requestWithRedirects(redirectedUrl, redirectCount + 1).then(resolve, reject);
        return;
      }

      resolve(response);
    });
    request.on("error", reject);
  });
}

function extractSingleFile(zipPath, archivePath, destinationDir) {
  fs.mkdirSync(destinationDir, { recursive: true });
  if (process.platform === "win32") {
    const command =
      `Expand-Archive -LiteralPath ${quotePowerShellString(zipPath)} ` +
      `-DestinationPath ${quotePowerShellString(destinationDir)} -Force`;
    execFileSync("powershell.exe", ["-NoProfile", "-Command", command], { stdio: "inherit" });
    return;
  }

  execFileSync("unzip", ["-q", zipPath, archivePath, "-d", destinationDir], {
    stdio: "inherit"
  });
}

function quotePowerShellString(value) {
  return `'${value.replaceAll("'", "''")}'`;
}

function parseArgs(args) {
  const parsed = {};

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];

    switch (arg) {
      case "--check":
        parsed.check = true;
        break;
      case "--download":
        parsed.download = true;
        break;
      case "--verify-upstream":
        parsed.verifyUpstream = true;
        break;
      case "--dist-platform":
        parsed.distPlatform = requireValue(args, ++index, arg);
        break;
      case "--platform":
        parsed.platform = requireValue(args, ++index, arg);
        break;
      case "-h":
      case "--help":
        parsed.help = true;
        break;
      default:
        throw new Error(`Unknown argument: ${arg}`);
    }
  }

  return parsed;
}

function requireValue(args, index, flag) {
  const value = args[index];
  if (!value || value.startsWith("-")) {
    throw new Error(`${flag} requires a value`);
  }
  return value;
}

function printHelp() {
  console.log(`Usage: node scripts/runtime/stage-bun-runtime.mjs [options]

Options:
  --check                         Validate scripts/runtime/bun-runtime-manifest.json.
  --verify-upstream               Fetch upstream Bun checksums and verify manifest SHA256 values.
  --download                      Download, verify, extract, and stage Bun runtimes.
  --platform <package-dir>        Stage one platform package directory only.
  --dist-platform <dir>           Staging root. Defaults to dist-platform.
  -h, --help                      Show this help.
`);
}
