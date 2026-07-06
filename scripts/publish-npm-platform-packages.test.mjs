import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { PLATFORM_PACKAGES } from "./npm-package-config.mjs";
import {
  platformPublishPlan,
  tarballIntegrity,
} from "./publish-npm-platform-packages.mjs";

test("tarballIntegrity returns npm dist integrity format", () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-integrity-"));
  try {
    const tarball = path.join(tempDir, "pkg.tgz");
    fs.writeFileSync(tarball, "package bytes", "utf8");

    assert.equal(
      tarballIntegrity(tarball),
      `sha512-${crypto.createHash("sha512").update("package bytes").digest("base64")}`,
    );
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("platformPublishPlan covers only platform packages", () => {
  const plan = platformPublishPlan("dist-pack", "1.2.3");

  assert.equal(plan.length, PLATFORM_PACKAGES.length);
  assert.deepEqual(
    plan.map((entry) => entry.packageName),
    PLATFORM_PACKAGES.map((entry) => entry.packageName),
  );
  assert.equal(plan.some((entry) => entry.packageName === "claude-code-rust"), false);
});
