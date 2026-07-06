import path from "node:path";
import { resolveRepoRoot } from "./repo-root.mjs";
import { PLATFORM_PACKAGES, readBunRuntimeManifest } from "./npm-package-config.mjs";

const repoRoot = resolveRepoRoot(import.meta.url);

export const BUN_LINKED_LIBRARIES = [
  ["BoringSSL", "several licenses"],
  ["brotli", "MIT"],
  ["libarchive", "several licenses"],
  ["lol-html", "BSD 3-Clause"],
  ["mimalloc", "MIT"],
  ["picohttp", "Perl License or MIT"],
  ["zstd", "BSD License or GPLv2"],
  ["simdutf", "Apache 2.0"],
  ["tinycc", "LGPL v2.1"],
  ["uSockets", "Apache 2.0"],
  ["zlib-ng", "zlib"],
  ["c-ares", "MIT"],
  ["libicu", "ICU license"],
  ["libbase64", "BSD 2-Clause"],
  ["libuv", "MIT on Windows"],
  ["libdeflate", "MIT"],
  ["uWebSockets fork", "Apache 2.0"],
  ["TigerBeetle IO code", "Apache 2.0"]
];

export const BUN_EMBEDDED_POLYFILLS = [
  "assert",
  "browserify-zlib",
  "buffer",
  "constants-browserify",
  "crypto-browserify",
  "domain-browser",
  "events",
  "https-browserify",
  "os-browserify",
  "path-browserify",
  "process",
  "punycode",
  "querystring-es3",
  "stream-browserify",
  "stream-http",
  "string_decoder",
  "timers-browserify",
  "tty-browserify",
  "url",
  "util",
  "vm-browserify"
];

export function buildThirdPartyNotices({
  manifest = readBunRuntimeManifest(repoRoot),
  platformPackages = PLATFORM_PACKAGES
} = {}) {
  const assetRows = platformPackages.map((platformPackage) => {
    const asset = manifest.assets?.[platformPackage.dir];
    if (!asset) {
      throw new Error(`Bun runtime manifest is missing ${platformPackage.dir}`);
    }

    return [
      `| \`${platformPackage.dir}\``,
      `\`${asset.assetName}\``,
      `\`${asset.archiveBinaryPath}\``,
      `\`${asset.sha256}\` |`
    ].join(" | ");
  });

  const linkedLibraryRows = BUN_LINKED_LIBRARIES.map(
    ([library, license]) => `| ${library} | ${license} |`
  );
  const embeddedPolyfillRows = BUN_EMBEDDED_POLYFILLS.map((polyfill) => `| ${polyfill} | MIT |`);

  return `# Third-Party Notices

This npm platform package redistributes a private Bun executable used only to
run the bundled Agent SDK bridge for \`claude-code-rust\`. The executable is
renamed to \`claude-rs-bridge-bun\` or \`claude-rs-bridge-bun.exe\`; it is not
exposed as a user-facing \`bun\` command.

The \`claude-code-rust\` project is licensed under Apache-2.0.

## Bundled Bun Runtime

- Bun version: ${manifest.version}
- Upstream project: https://github.com/oven-sh/bun
- Bun license documentation: https://bun.com/docs/project/license
- Bun license: MIT
- Bun checksum source: ${manifest.checksumSourceUrl}

Bundled Bun release assets:

| Platform package | Bun release asset | Archive binary path | Source archive SHA256 |
| --- | --- | --- | --- |
${assetRows.join("\n")}

Release package manifests record the upstream asset URL, archive path, source
archive SHA256, and packaged runtime SHA256 for each platform package.

## JavaScriptCore And WebKit

Bun statically links JavaScriptCore and WebKit components. Bun's license page
identifies JavaScriptCore, WebKit, and WebCore files as LGPL-2 licensed and
describes the relinking obligation for static LGPL linkage.

Bun's patched WebKit source is published at:

https://github.com/oven-sh/WebKit

Downstream redistribution of the platform packages should preserve this notice,
the Bun license information, and the WebKit source/relinking reference.

## Statically Linked Libraries

Bun's license page lists these statically linked libraries and license notices:

| Library | License notice |
| --- | --- |
${linkedLibraryRows.join("\n")}

## Embedded Polyfills

Bun embeds these compatibility polyfill packages into its binary and injects
them if imported. Bun's license page lists each as MIT licensed:

| Package | License |
| --- | --- |
${embeddedPolyfillRows.join("\n")}

## Additional Credits

Bun's license page also credits esbuild-derived source code in Bun's JS
transpiler, CSS lexer, and Node.js module resolver.
`;
}
