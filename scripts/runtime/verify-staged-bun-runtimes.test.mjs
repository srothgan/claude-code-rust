import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { resolveRepoRoot } from "../shared/repo-root.mjs";
import { PLATFORM_PACKAGES, readBunRuntimeManifest } from "../shared/npm-package-config.mjs";
import { verifyStagedBunRuntimes } from "./verify-staged-bun-runtimes.mjs";

const repoRoot = resolveRepoRoot(import.meta.url);
const fakeRuntimeSha256 = crypto.createHash("sha256").update("fake runtime").digest("hex");

function fakeRuntimeManifest() {
  const manifest = structuredClone(readBunRuntimeManifest(repoRoot));
  for (const asset of Object.values(manifest.assets)) {
    asset.binarySha256 = fakeRuntimeSha256;
  }
  return manifest;
}

function stageFakeRuntime(distPlatformDir, platformPackage) {
  const runtimePath = path.join(
    distPlatformDir,
    platformPackage.dir,
    "bin",
    platformPackage.bundledRuntimeName,
  );
  fs.mkdirSync(path.dirname(runtimePath), { recursive: true });
  fs.writeFileSync(runtimePath, "fake runtime", "utf8");
  fs.chmodSync(runtimePath, 0o755);
}

test("verifyStagedBunRuntimes accepts artifact tree rooted at dist-platform", () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-staged-runtime-"));
  try {
    const distPlatformDir = path.join(tempDir, "dist-platform");
    for (const platformPackage of PLATFORM_PACKAGES) {
      stageFakeRuntime(distPlatformDir, platformPackage);
    }

    verifyStagedBunRuntimes({
      distPlatformDir,
      manifest: fakeRuntimeManifest(),
      rootForMessages: tempDir,
    });
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("verifyStagedBunRuntimes rejects generic bun filenames in package bin dirs", () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-staged-runtime-"));
  try {
    const distPlatformDir = path.join(tempDir, "dist-platform");
    const platformPackage = PLATFORM_PACKAGES.find((entry) => entry.dir === "linux-x64-gnu");
    stageFakeRuntime(distPlatformDir, platformPackage);
    fs.writeFileSync(path.join(distPlatformDir, platformPackage.dir, "bin", "bun"), "bad", "utf8");

    assert.throws(
      () =>
        verifyStagedBunRuntimes({
          distPlatformDir,
          platformPackages: [platformPackage],
          manifest: fakeRuntimeManifest(),
          rootForMessages: tempDir,
        }),
      /forbidden generic Bun name/,
    );
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("verifyStagedBunRuntimes rejects root-level artifact extraction shape", () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-staged-runtime-"));
  try {
    const platformPackage = PLATFORM_PACKAGES.find((entry) => entry.dir === "darwin-arm64");
    stageFakeRuntime(tempDir, platformPackage);

    assert.throws(
      () =>
        verifyStagedBunRuntimes({
          distPlatformDir: path.join(tempDir, "dist-platform"),
          platformPackages: [platformPackage],
          manifest: fakeRuntimeManifest(),
          rootForMessages: tempDir,
        }),
      /missing staged runtime/,
    );
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("verifyStagedBunRuntimes rejects staged runtime hash mismatch", () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-staged-runtime-"));
  try {
    const distPlatformDir = path.join(tempDir, "dist-platform");
    const platformPackage = PLATFORM_PACKAGES.find((entry) => entry.dir === "linux-x64-gnu");
    stageFakeRuntime(distPlatformDir, platformPackage);
    const manifest = fakeRuntimeManifest();
    manifest.assets[platformPackage.dir].binarySha256 = "0".repeat(64);

    assert.throws(
      () =>
        verifyStagedBunRuntimes({
          distPlatformDir,
          platformPackages: [platformPackage],
          manifest,
          rootForMessages: tempDir,
        }),
      /staged runtime SHA256 mismatch/,
    );
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});
