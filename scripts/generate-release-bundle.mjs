#!/usr/bin/env node
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  PLATFORM_PACKAGES,
  ROOT_PACKAGE_NAME,
  installArchiveName,
  nativeReleaseAssetName,
  readBunRuntimeManifest,
  readCargoPackageMetadata
} from "./npm-package-config.mjs";
import {
  createTarGzArchive,
  fileSha256,
  listRelativeFiles,
  normalizePath,
  readArgValue,
  writeJson
} from "./install-archive-common.mjs";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..");

const options = parseArgs(process.argv.slice(2));
if (options.help) {
  printHelp();
  process.exit(0);
}

const cargoPackage = readCargoPackageMetadata(path.join(repoRoot, "Cargo.toml"));
const bunRuntimeManifest = readBunRuntimeManifest(repoRoot);
const version = options.version ?? cargoPackage.version;
const assetsDir = path.resolve(repoRoot, options.assetsDir ?? "dist-assets");
const packDir = path.resolve(repoRoot, options.packDir ?? "dist-pack");
const npmManifestsDir = path.resolve(repoRoot, options.npmManifestsDir ?? path.join("dist-npm", "manifests"));
const installManifestsDir = path.resolve(
  repoRoot,
  options.installManifestsDir ?? path.join("dist-install", "manifests")
);
const installArchivesDir = path.dirname(installManifestsDir);
const outDir = path.resolve(repoRoot, options.outDir ?? ".");
const bundleRootName = `${ROOT_PACKAGE_NAME}-${version}-release-bundle`;
const bundleName = `${bundleRootName}.tar.gz`;
const bundlePath = path.join(outDir, bundleName);
const releaseManifestPath = path.join(outDir, "release-manifest.json");
const internalShaPath = path.join(outDir, "SHA256SUMS.internal");
const tempParent = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-release-bundle-"));
const bundleRoot = path.join(tempParent, bundleRootName);

fs.mkdirSync(outDir, { recursive: true });
fs.rmSync(bundlePath, { force: true });
fs.rmSync(releaseManifestPath, { force: true });
fs.rmSync(internalShaPath, { force: true });

try {
  const inputs = verifyInputs();
  fs.mkdirSync(bundleRoot, { recursive: true });
  copyBundleInputs(inputs);

  const releaseManifest = buildReleaseManifest(inputs);
  writeJson(path.join(bundleRoot, "release-manifest.json"), releaseManifest);
  writeJson(releaseManifestPath, releaseManifest);

  const internalChecksums = buildInternalChecksums(bundleRoot);
  fs.writeFileSync(path.join(bundleRoot, "SHA256SUMS.internal"), internalChecksums, "utf8");
  fs.writeFileSync(internalShaPath, internalChecksums, "utf8");

  verifyNoInstallArchivesInBundle();
  createTarGzArchive({ sourceParent: tempParent, rootName: bundleRootName, destination: bundlePath });
} finally {
  fs.rmSync(tempParent, { recursive: true, force: true });
}

console.log(`Generated release bundle ${path.relative(repoRoot, bundlePath)}`);

function verifyInputs() {
  const nativeAssets = PLATFORM_PACKAGES.map((platformPackage) => {
    const assetName = nativeReleaseAssetName(platformPackage);
    const assetPath = path.join(assetsDir, assetName);
    const metadataPath = `${assetPath}.build-metadata.txt`;
    assertFile(assetPath, `${platformPackage.dir} native binary`);
    assertFile(metadataPath, `${platformPackage.dir} build metadata`);
    return {
      platform: platformPackage.dir,
      rustTarget: platformPackage.rustTarget,
      binaryPath: assetPath,
      metadataPath
    };
  });

  const npmTarballs = [
    {
      directory: "root",
      packageName: ROOT_PACKAGE_NAME,
      tarballPath: path.join(packDir, `${ROOT_PACKAGE_NAME}-${version}.tgz`)
    },
    ...PLATFORM_PACKAGES.map((platformPackage) => ({
      directory: platformPackage.dir,
      packageName: platformPackage.packageName,
      tarballPath: path.join(packDir, npmTarballName(platformPackage.packageName, version))
    }))
  ];
  for (const npmTarball of npmTarballs) {
    assertFile(npmTarball.tarballPath, `${npmTarball.directory} npm tarball`);
  }

  const npmManifests = ["summary.json", "root.json", ...PLATFORM_PACKAGES.map((entry) => `${entry.dir}.json`)].map(
    (fileName) => {
      const manifestPath = path.join(npmManifestsDir, fileName);
      assertFile(manifestPath, `npm manifest ${fileName}`);
      return manifestPath;
    }
  );

  const installManifests = PLATFORM_PACKAGES.map((platformPackage) => {
    const manifestPath = path.join(installManifestsDir, `${platformPackage.dir}.json`);
    assertFile(manifestPath, `${platformPackage.dir} install manifest`);
    return manifestPath;
  });

  const installArchives = PLATFORM_PACKAGES.map((platformPackage) => {
    const archivePath = path.join(installArchivesDir, installArchiveName(platformPackage, version));
    assertFile(archivePath, `${platformPackage.dir} public install archive`);
    return {
      platform: platformPackage.dir,
      archivePath
    };
  });

  return {
    nativeAssets,
    npmTarballs,
    npmManifests,
    installManifests,
    installArchives
  };
}

function copyBundleInputs(inputs) {
  for (const nativeAsset of inputs.nativeAssets) {
    copyIntoBundle(nativeAsset.binaryPath, path.join("dist-assets", path.basename(nativeAsset.binaryPath)));
    copyIntoBundle(nativeAsset.metadataPath, path.join("dist-assets", path.basename(nativeAsset.metadataPath)));
  }

  for (const npmTarball of inputs.npmTarballs) {
    copyIntoBundle(npmTarball.tarballPath, path.join("dist-pack", path.basename(npmTarball.tarballPath)));
  }

  for (const npmManifest of inputs.npmManifests) {
    copyIntoBundle(npmManifest, path.join("dist-npm", "manifests", path.basename(npmManifest)));
  }

  for (const installManifest of inputs.installManifests) {
    copyIntoBundle(installManifest, path.join("dist-install", "manifests", path.basename(installManifest)));
  }
}

function buildReleaseManifest(inputs) {
  return {
    manifestVersion: 1,
    version,
    bunRuntimeVersion: bunRuntimeManifest.version,
    generatedAt: new Date(0).toISOString(),
    targets: PLATFORM_PACKAGES.map((platformPackage) => ({
      platform: platformPackage.dir,
      rustTarget: platformPackage.rustTarget,
      installArchive: installArchiveName(platformPackage, version),
      nativeBinary: nativeReleaseAssetName(platformPackage),
      npmPlatformPackage: platformPackage.packageName
    })),
    publicAssets: [
      ...inputs.installArchives.map(({ archivePath }) => ({
        path: normalizePath(path.relative(repoRoot, archivePath)),
        sha256: fileSha256(archivePath)
      })),
      {
        path: bundleName,
        sha256: null
      },
      {
        path: "SHA256SUMS",
        sha256: null
      }
    ],
    internalBundle: {
      name: bundleName,
      rootDirectory: bundleRootName,
      contents: {
        nativeAssets: inputs.nativeAssets.map((entry) => ({
          platform: entry.platform,
          binaryPath: normalizePath(path.relative(assetsDir, entry.binaryPath)),
          metadataPath: normalizePath(path.relative(assetsDir, entry.metadataPath))
        })),
        npmTarballs: inputs.npmTarballs.map((entry) => ({
          directory: entry.directory,
          packageName: entry.packageName,
          path: normalizePath(path.relative(packDir, entry.tarballPath)),
          sha256: fileSha256(entry.tarballPath)
        })),
        npmManifests: inputs.npmManifests.map((entry) => normalizePath(path.relative(npmManifestsDir, entry))),
        installManifests: inputs.installManifests.map((entry) =>
          normalizePath(path.relative(installManifestsDir, entry))
        )
      }
    }
  };
}

function buildInternalChecksums(rootDir) {
  const lines = listRelativeFiles(rootDir)
    .filter((file) => file !== "SHA256SUMS.internal")
    .sort()
    .map((file) => `${fileSha256(path.join(rootDir, ...file.split("/")))}  ${file}`);
  return `${lines.join("\n")}\n`;
}

function verifyNoInstallArchivesInBundle() {
  const files = listRelativeFiles(bundleRoot);
  const duplicatedArchives = files.filter(
    (file) => file.startsWith("dist-install/") && (file.endsWith(".tar.gz") || file.endsWith(".zip"))
  );
  if (duplicatedArchives.length > 0) {
    throw new Error(
      `Release bundle must not contain complete install archives:\n${duplicatedArchives
        .map((file) => `- ${file}`)
        .join("\n")}`
    );
  }
}

function copyIntoBundle(source, relativeDestination) {
  const destination = path.join(bundleRoot, ...relativeDestination.split("/"));
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.copyFileSync(source, destination);
}

function assertFile(filePath, label) {
  if (!fs.existsSync(filePath) || !fs.statSync(filePath).isFile()) {
    throw new Error(`Missing ${label}: ${path.relative(repoRoot, filePath)}`);
  }
}

function npmTarballName(packageName, packageVersion) {
  return `${packageName.replace(/^@/, "").replace("/", "-")}-${packageVersion}.tgz`;
}

function parseArgs(args) {
  const parsed = {
    help: false,
    version: undefined,
    assetsDir: undefined,
    packDir: undefined,
    npmManifestsDir: undefined,
    installManifestsDir: undefined,
    outDir: undefined
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
      case "--assets-dir":
        parsed.assetsDir = readArgValue(args, ++index, arg);
        break;
      case "--pack-dir":
        parsed.packDir = readArgValue(args, ++index, arg);
        break;
      case "--npm-manifests-dir":
        parsed.npmManifestsDir = readArgValue(args, ++index, arg);
        break;
      case "--install-manifests-dir":
        parsed.installManifestsDir = readArgValue(args, ++index, arg);
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

function printHelp() {
  console.log(`Usage: node scripts/generate-release-bundle.mjs [options]

Options:
  --version <version>              Override the package version. Defaults to Cargo.toml.
  --assets-dir <dir>               Native binary asset directory. Defaults to dist-assets.
  --pack-dir <dir>                 npm tarball directory. Defaults to dist-pack.
  --npm-manifests-dir <dir>        npm manifest directory. Defaults to dist-npm/manifests.
  --install-manifests-dir <dir>    install manifest directory. Defaults to dist-install/manifests.
  --out-dir <dir>                  Output directory. Defaults to repository root.
  -h, --help                       Show this help.
`);
}
