import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { resolveRepoRoot } from "./repo-root.mjs";
import { PLATFORM_PACKAGES } from "./npm-package-config.mjs";

const repoRoot = resolveRepoRoot(import.meta.url);

function readTomlSection(toml, sectionName) {
  const sectionHeader = new RegExp(`^\\s*\\[${sectionName}\\]\\s*$`);
  const anySectionHeader = /^\s*\[/;
  const sectionLines = [];
  let inSection = false;

  for (const line of toml.split(/\r?\n/)) {
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

function readDenyGraphTargets(denyTomlPath) {
  const graphBody = readTomlSection(fs.readFileSync(denyTomlPath, "utf8"), "graph");
  if (!graphBody) {
    throw new Error(`Could not find [graph] section in ${denyTomlPath}`);
  }

  const targetsMatch = graphBody.match(/targets\s*=\s*\[([^\]]*)\]/);
  if (!targetsMatch) {
    throw new Error(`Could not find targets array in the [graph] section of ${denyTomlPath}`);
  }

  return [...targetsMatch[1].matchAll(/"([^"]+)"/g)].map((entry) => entry[1]);
}

test("deny.toml checks every released Rust target", () => {
  const denyTargets = readDenyGraphTargets(path.join(repoRoot, "deny.toml"));
  const releaseTargets = PLATFORM_PACKAGES.map((platformPackage) => platformPackage.rustTarget);

  assert.deepEqual(
    [...denyTargets].sort(),
    [...releaseTargets].sort(),
    "deny.toml [graph] targets must match PLATFORM_PACKAGES[].rustTarget in scripts/shared/npm-package-config.mjs",
  );
});

test("deny.toml lists each target exactly once", () => {
  const denyTargets = readDenyGraphTargets(path.join(repoRoot, "deny.toml"));

  assert.equal(new Set(denyTargets).size, denyTargets.length, "deny.toml [graph] targets must not contain duplicates");
});
