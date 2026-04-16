#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const packageDir = resolve(scriptDir, "..");
const packageJsonPath = resolve(packageDir, "package.json");
const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf8"));

const declaredDependencies = new Set(
  Object.keys({
    ...(packageJson.dependencies ?? {}),
    ...(packageJson.devDependencies ?? {}),
    ...(packageJson.optionalDependencies ?? {}),
    ...(packageJson.peerDependencies ?? {}),
  }),
);

const auditArgs = ["audit", "--json", "--audit-level=moderate"];
const auditCommand =
  process.platform === "win32"
    ? {
        command: process.env.ComSpec || "cmd.exe",
        args: ["/d", "/s", "/c", `npm ${auditArgs.join(" ")}`],
      }
    : {
        command: "npm",
        args: auditArgs,
      };

const audit = spawnSync(auditCommand.command, auditCommand.args, {
  cwd: packageDir,
  encoding: "utf8",
});

if (audit.error) {
  console.error(`failed to run npm audit: ${audit.error.message}`);
  process.exit(1);
}

const stdout = audit.stdout?.trim();
if (!stdout) {
  if (audit.stderr?.trim()) {
    console.error(audit.stderr.trim());
  } else {
    console.error("npm audit did not return JSON output");
  }
  process.exit(1);
}

let report;
try {
  report = JSON.parse(stdout);
} catch (error) {
  console.error("failed to parse npm audit JSON output");
  console.error(stdout);
  process.exit(1);
}

const vulnerabilities =
  report.vulnerabilities && typeof report.vulnerabilities === "object"
    ? report.vulnerabilities
    : {};

function normalizeViaEntries(via) {
  if (Array.isArray(via)) {
    return via;
  }
  return via === undefined ? [] : [via];
}

function advisoryObjectsForPackage(packageName, vulnerability) {
  return normalizeViaEntries(vulnerability.via).filter(
    (entry) =>
      entry &&
      typeof entry === "object" &&
      !Array.isArray(entry) &&
      typeof entry.name === "string" &&
      entry.name === packageName,
  );
}

function viaPackageNames(vulnerability) {
  return normalizeViaEntries(vulnerability.via)
    .map((entry) => {
      if (typeof entry === "string") {
        return entry;
      }
      if (entry && typeof entry === "object" && !Array.isArray(entry) && typeof entry.name === "string") {
        return entry.name;
      }
      return null;
    })
    .filter((entry) => typeof entry === "string");
}

const directFindings = [];
const ignoredMetaVulnerabilities = [];

for (const [packageName, vulnerability] of Object.entries(vulnerabilities)) {
  if (!declaredDependencies.has(packageName)) {
    continue;
  }

  const advisories = advisoryObjectsForPackage(packageName, vulnerability);
  if (advisories.length > 0) {
    directFindings.push({
      packageName,
      severity: vulnerability.severity ?? "unknown",
      advisories,
      fixAvailable: vulnerability.fixAvailable ?? false,
    });
    continue;
  }

  ignoredMetaVulnerabilities.push({
    packageName,
    severity: vulnerability.severity ?? "unknown",
    via: viaPackageNames(vulnerability),
  });
}

if (directFindings.length === 0) {
  console.log("No direct dependency advisories at moderate or higher severity.");
  if (ignoredMetaVulnerabilities.length > 0) {
    console.log("Ignored direct dependency meta-vulnerabilities caused by transitive packages:");
    for (const finding of ignoredMetaVulnerabilities) {
      const via = finding.via.length > 0 ? finding.via.join(", ") : "unknown";
      console.log(`- ${finding.packageName} (${finding.severity}) via ${via}`);
    }
  }
  process.exit(0);
}

console.error("Direct dependency advisories found:");
for (const finding of directFindings) {
  const firstAdvisory = finding.advisories[0];
  const title =
    firstAdvisory && typeof firstAdvisory.title === "string"
      ? firstAdvisory.title
      : "security advisory";
  const range =
    firstAdvisory && typeof firstAdvisory.range === "string" && firstAdvisory.range.length > 0
      ? firstAdvisory.range
      : "unknown range";
  const url =
    firstAdvisory && typeof firstAdvisory.url === "string" ? firstAdvisory.url : "no URL provided";
  console.error(`- ${finding.packageName} (${finding.severity})`);
  console.error(`  ${title}`);
  console.error(`  affected range: ${range}`);
  console.error(`  fix available: ${String(finding.fixAvailable)}`);
  console.error(`  ${url}`);
}
process.exit(1);
