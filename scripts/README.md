# Release Tooling Scripts

The scripts directory is grouped by release phase. These files are part of the
active release tooling and are validated by PR, release, or nightly workflows.

## Runtime

- `runtime/stage-bun-runtime.mjs` is a maintainer entry point. It validates the
  Bun runtime manifest and downloads verified private Bun runtime binaries into
  `dist-platform/`.
- `runtime/verify-staged-bun-runtimes.mjs` is a release and nightly verification
  entry point for staged runtime binaries.
- `runtime/bun-runtime-manifest.json` records the bundled Bun source and binary
  checksums.
- `runtime/verify-staged-bun-runtimes.test.mjs` covers staged-runtime verifier
  behavior in PR validation.

## npm

- `npm/generate-npm-packages.mjs` is a maintainer and workflow entry point for
  creating `dist-npm/` package directories.
- `npm/verify-npm-packages.mjs` is a maintainer and workflow entry point for
  checking generated package contents, manifests, runtime provenance, and
  third-party notice coverage.
- `npm/smoke-npm-package-install.mjs` is a maintainer and workflow entry point
  for packed npm install smoke tests.
- `npm/dry-run-npm-publish.mjs` and `npm/publish-npm-platform-packages.mjs` are
  release publication entry points.
- `npm/*.test.mjs` and `npm/npm-resolver.test.cjs` are PR validation tests for
  npm packaging, install smoke behavior, resolver behavior, and publication
  guardrails.

## Install

- `install/generate-install-archives.mjs` is a maintainer and workflow entry
  point for creating public install archives in `dist-install/`.
- `install/verify-install-archives.mjs` is a maintainer and workflow entry point
  for verifying install archive contents and manifests.
- `install/smoke-install-archive.mjs` is a maintainer and workflow entry point
  for archive extraction and startup smoke tests.
- `install/install-progress.test.mjs` executes the Unix installer with redirected output and local release fixtures to verify download transport settings, diagnostic summaries, progress suppression, and cleanup.
- `install/install-version-guard.test.mjs` verifies Unix same-version no-op,
  explicit reinstall, update, exact-version, and `latest` metadata behavior.
- `install/test-install-progress.ps1` validates the PowerShell spinner, fixed-width download progress, streaming transfer, and output-helper behavior without executing the installer body.
- `install/test-install-version-guard.ps1` validates PowerShell version metadata,
  decision precedence, and release-download boundaries under Windows PowerShell.
- `install/release-advisories.json` lists known issues by release range. Both installers read it; see Release Advisories.
- `install/release-advisories.test.mjs` validates the checked-in advisories in PR validation.
- `install/install.sh` and `install/install.ps1` are maintained public installer
  assets. Keep syntax checks and help text current before publishing them.

### Release Advisories

`install/release-advisories.json` is the single source for "this release has a known issue" notices. Each entry has three keys:

- `start` and `end`: the first and last affected release, both inclusive, as plain `MAJOR.MINOR.PATCH`.
- `summary`: the complete one-line user-facing text, at most 200 characters, using only letters, digits, spaces, and `. , : ; ( ) / + _ @ # = -`. `install.sh` reads the file without a JSON parser and the text is pasted into a shell command, so quotes, backslashes, and `$` are rejected by the test.

The installers fetch the file from `main` after the release is selected and print `Known issue in claude-rs <version>: <summary>` for every entry whose range contains the selected version. Prerelease and build suffixes are ignored when comparing. The check only warns: it never blocks or prompts, it is skipped when the selected version is already installed, and a fetch or parse failure is ignored. Because the file is read from `main`, a merged entry takes effect immediately for every installer run, without a release.

To add an advisory, append an entry, run `node --test scripts/install/release-advisories.test.mjs`, and merge. After the fixed release is published, show the same text to npm users of the affected versions by deprecating the range on the root package only:

```sh
npm deprecate "claude-code-rust@>=<start> <=<end>" "<summary> Upgrade: npm install -g claude-code-rust@latest"
```

## Release

- `release/generate-release-bundle.mjs` is a release and nightly entry point for
  assembling the internal release bundle from native assets, npm tarballs,
  install archives, manifests, and checksums.
- `release/verify-release-bundle.mjs` is the paired release-bundle verifier.

The release bundle remains generated in release and nightly workflows. PR
validation intentionally stops at npm packages and install archives unless a
lightweight fixture is added for `dist-assets`, build metadata, npm tarballs,
npm manifests, install archives, and install manifests.

## Shared

- `shared/npm-package-config.mjs` is the package and platform source of truth.
- `shared/third-party-notices.mjs` builds bundled runtime notices.
- `shared/verify-third-party-notices.mjs` validates notice coverage and is run
  directly in PR validation.
- `shared/install-archive-common.mjs` contains archive helpers.
- `shared/release-advisories.mjs` loads and validates `install/release-advisories.json`.
- `shared/repo-root.mjs` resolves the repository root for scripts in any phase
  directory.

Shared helpers are imported by phase scripts. Do not run helper-only modules as
release steps unless they expose their own command-line interface.

## Quality

- `quality/jscpd-warning-summary.mjs` is used by the nightly duplicate-code
  warning scan through `npm run quality:duplicates:summary`.

## Workflow Usage

- PR package-layout validation runs runtime manifest checks, third-party notice
  verification, script tests, runtime staging, npm package generation,
  npm package verification, npm install smoke tests, install archive generation,
  install archive verification, and a Linux install-archive smoke test.
- Release and nightly workflows run the full package assembly path, including
  runtime staging, npm packages, install archives, release bundle generation,
  package smoke tests on every target platform, publication dry-runs, and
  release-bundle verification.

## Local Validation

```sh
node scripts/runtime/stage-bun-runtime.mjs --check
node scripts/shared/verify-third-party-notices.mjs
node --test scripts/npm/npm-resolver.test.cjs scripts/npm/smoke-npm-package-install.test.mjs scripts/runtime/verify-staged-bun-runtimes.test.mjs scripts/npm/dry-run-npm-publish.test.mjs scripts/npm/publish-npm-platform-packages.test.mjs scripts/install/release-advisories.test.mjs
shellcheck -s sh scripts/install/install.sh
pwsh -NoLogo -NoProfile -File scripts/install/install.ps1 -Help
pwsh -NoLogo -NoProfile -File scripts/install/test-install-progress.ps1
pwsh -NoLogo -NoProfile -File scripts/install/test-install-version-guard.ps1
node scripts/runtime/stage-bun-runtime.mjs --download
node scripts/npm/generate-npm-packages.mjs --mock-native-binary
node scripts/npm/verify-npm-packages.mjs
node scripts/npm/smoke-npm-package-install.mjs
node scripts/install/generate-install-archives.mjs --mock-binaries
node scripts/install/verify-install-archives.mjs
node --test scripts/install/install-progress.test.mjs scripts/install/install-version-guard.test.mjs
node scripts/install/smoke-install-archive.mjs --platform linux-x64-gnu
```

On Windows, also run the progress-helper test under Windows PowerShell 5.1:

```powershell
powershell -NoLogo -NoProfile -File scripts/install/test-install-progress.ps1
powershell -NoLogo -NoProfile -File scripts/install/test-install-version-guard.ps1
```

Repository-wide validation after script changes:

```sh
cargo fmt --all -- --check
cargo check --offline
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```
