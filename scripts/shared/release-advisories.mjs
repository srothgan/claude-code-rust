import fs from "node:fs";
import path from "node:path";

export const RELEASE_ADVISORIES_RELATIVE_PATH = "scripts/install/release-advisories.json";

const ADVISORY_KEYS = ["end", "start", "summary"];
const PLAIN_VERSION = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/u;
// install.sh has no JSON parser and the summary is pasted into an npm deprecate
// command, so summaries are limited to characters that need no escaping in JSON,
// awk, POSIX sh, or PowerShell double quotes.
const SUMMARY_TEXT = /^[A-Za-z0-9 .,:;()/+_@#=-]+$/u;
const SUMMARY_MAX_LENGTH = 200;

export function compareVersions(left, right) {
  const leftParts = left.split(".").map(Number);
  const rightParts = right.split(".").map(Number);
  for (let index = 0; index < 3; index += 1) {
    if (leftParts[index] !== rightParts[index]) {
      return leftParts[index] < rightParts[index] ? -1 : 1;
    }
  }
  return 0;
}

export function validateReleaseAdvisories(document) {
  if (!document || typeof document !== "object" || Array.isArray(document)) {
    throw new Error("release advisories must be a JSON object");
  }
  if (Object.keys(document).join(",") !== "advisories" || !Array.isArray(document.advisories)) {
    throw new Error('release advisories must contain exactly one "advisories" array');
  }
  return document.advisories.map((advisory, index) => {
    const label = `advisory ${index + 1}`;
    if (!advisory || typeof advisory !== "object" || Array.isArray(advisory)) {
      throw new Error(`${label} must be an object`);
    }
    if (Object.keys(advisory).sort().join(",") !== ADVISORY_KEYS.join(",")) {
      throw new Error(`${label} must contain exactly the keys start, end, and summary`);
    }
    const { start, end, summary } = advisory;
    for (const [name, version] of [["start", start], ["end", end]]) {
      if (typeof version !== "string" || !PLAIN_VERSION.test(version)) {
        throw new Error(`${label} ${name} must be a plain MAJOR.MINOR.PATCH version`);
      }
    }
    if (compareVersions(start, end) > 0) {
      throw new Error(`${label} start ${start} is greater than end ${end}`);
    }
    if (typeof summary !== "string" || !SUMMARY_TEXT.test(summary) || summary !== summary.trim()) {
      throw new Error(`${label} summary must be one trimmed line of letters, digits, spaces, and . , : ; ( ) / + _ @ # = -`);
    }
    if (summary.length > SUMMARY_MAX_LENGTH) {
      throw new Error(`${label} summary exceeds ${SUMMARY_MAX_LENGTH} characters`);
    }
    return { start, end, summary };
  });
}

export function loadReleaseAdvisories(repoRoot) {
  const text = fs.readFileSync(path.join(repoRoot, RELEASE_ADVISORIES_RELATIVE_PATH), "utf8");
  return validateReleaseAdvisories(JSON.parse(text));
}
