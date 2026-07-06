#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { PLATFORM_PACKAGES, readBunRuntimeManifest } from "./npm-package-config.mjs";
import {
  BUN_EMBEDDED_POLYFILLS,
  BUN_LINKED_LIBRARIES,
  buildThirdPartyNotices
} from "./third-party-notices.mjs";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..");

export function verifyThirdPartyNoticeCoverage({
  manifest = readBunRuntimeManifest(repoRoot),
  notices,
  noticesPath,
} = {}) {
  const noticeText = notices ?? (noticesPath ? fs.readFileSync(noticesPath, "utf8") : buildThirdPartyNotices({ manifest }));
  const failures = [];

  expectIncludes(noticeText, `Bun version: ${manifest.version}`, "Bun version", failures);
  expectIncludes(noticeText, "https://bun.com/docs/project/license", "Bun license docs URL", failures);
  expectIncludes(noticeText, "https://github.com/oven-sh/WebKit", "WebKit source URL", failures);
  expectIncludes(noticeText, "LGPL-2", "WebKit LGPL notice", failures);
  expectIncludes(noticeText, "Apache-2.0", "project Apache-2.0 notice", failures);
  expectIncludes(noticeText, "MIT", "Bun MIT notice", failures);

  for (const platformPackage of PLATFORM_PACKAGES) {
    const asset = manifest.assets?.[platformPackage.dir];
    if (!asset) {
      failures.push(`manifest is missing ${platformPackage.dir}`);
      continue;
    }
    expectIncludes(noticeText, platformPackage.dir, `${platformPackage.dir} package directory`, failures);
    expectIncludes(noticeText, asset.assetName, `${platformPackage.dir} Bun asset`, failures);
    expectIncludes(noticeText, asset.archiveBinaryPath, `${platformPackage.dir} archive binary path`, failures);
    expectIncludes(noticeText, asset.sha256, `${platformPackage.dir} source archive SHA256`, failures);
  }

  for (const [linkedLibrary] of BUN_LINKED_LIBRARIES) {
    expectIncludes(noticeText, linkedLibrary, `linked library ${linkedLibrary}`, failures);
  }

  for (const polyfill of BUN_EMBEDDED_POLYFILLS) {
    expectIncludes(noticeText, polyfill, `embedded polyfill ${polyfill}`, failures);
  }

  if (failures.length > 0) {
    throw new Error(`Third-party notice verification failed:\n- ${failures.join("\n- ")}`);
  }
}

function expectIncludes(haystack, needle, label, failures) {
  if (!haystack.includes(needle)) {
    failures.push(`missing ${label}: ${needle}`);
  }
}

function printHelp() {
  console.log(`Usage: node scripts/verify-third-party-notices.mjs

Verifies the generated third-party notice content covers the bundled Bun version,
asset names, checksums, WebKit LGPL/source references, linked libraries, and
embedded polyfills.
`);
}

async function main() {
  if (process.argv.includes("--help") || process.argv.includes("-h")) {
    printHelp();
    return;
  }
  verifyThirdPartyNoticeCoverage();
  console.log("Verified third-party notice coverage");
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  });
}
