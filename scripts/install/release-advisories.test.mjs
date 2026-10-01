import assert from "node:assert/strict";
import test from "node:test";
import { resolveRepoRoot } from "../shared/repo-root.mjs";
import {
  compareVersions,
  loadReleaseAdvisories,
  validateReleaseAdvisories,
} from "../shared/release-advisories.mjs";

const repoRoot = resolveRepoRoot(import.meta.url);
const validAdvisory = { start: "1.2.0", end: "1.4.8", summary: "Known issue (workaround: press Esc). Fixed in 1.4.9." };

test("checked-in release advisories are valid", () => {
  const advisories = loadReleaseAdvisories(repoRoot);

  assert.ok(advisories.length > 0, "expected at least one release advisory");
});

test("compareVersions orders version components numerically", () => {
  assert.equal(compareVersions("0.9.0", "0.12.0"), -1);
  assert.equal(compareVersions("0.14.10", "0.14.9"), 1);
  assert.equal(compareVersions("1.0.0", "1.0.0"), 0);
});

test("validateReleaseAdvisories accepts any key order", () => {
  const advisories = validateReleaseAdvisories({
    advisories: [{ summary: validAdvisory.summary, end: validAdvisory.end, start: validAdvisory.start }],
  });

  assert.deepEqual(advisories, [validAdvisory]);
});

test("validateReleaseAdvisories rejects entries the installers cannot handle", () => {
  for (const [name, advisory] of [
    ["missing key", { start: "1.2.0", end: "1.4.8" }],
    ["extra key", { ...validAdvisory, fixed: "1.4.9" }],
    ["prerelease bound", { ...validAdvisory, end: "1.4.8-rc.1" }],
    ["v-prefixed bound", { ...validAdvisory, start: "v1.2.0" }],
    ["inverted range", { ...validAdvisory, start: "1.5.0" }],
    ["empty summary", { ...validAdvisory, summary: "" }],
    ["quoted summary", { ...validAdvisory, summary: 'Press "Esc" first.' }],
    ["backslash summary", { ...validAdvisory, summary: "Use C:\\path." }],
    ["shell expansion summary", { ...validAdvisory, summary: "Costs $HOME." }],
    ["multi-line summary", { ...validAdvisory, summary: "First line.\nSecond line." }],
    ["padded summary", { ...validAdvisory, summary: " Known issue." }],
    ["oversized summary", { ...validAdvisory, summary: "a".repeat(201) }],
  ]) {
    assert.throws(() => validateReleaseAdvisories({ advisories: [advisory] }), Error, name);
  }
  assert.throws(() => validateReleaseAdvisories([validAdvisory]), Error, "top-level array");
  assert.throws(() => validateReleaseAdvisories({ advisories: [], other: true }), Error, "extra top-level key");
});
