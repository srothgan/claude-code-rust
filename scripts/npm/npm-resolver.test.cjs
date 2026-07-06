const assert = require("node:assert/strict");
const path = require("node:path");
const test = require("node:test");

const {
  detectLinuxLibc,
  resolveInstall,
  selectTarget,
  supportedPlatformsText
} = require("../../bin/claude-rs.js");

function processLike(platform, arch, glibcVersionRuntime) {
  return {
    platform,
    arch,
    report: {
      getReport() {
        return {
          header: glibcVersionRuntime ? { glibcVersionRuntime } : {}
        };
      }
    }
  };
}

test("selectTarget resolves glibc Linux platform package", () => {
  const selected = selectTarget(processLike("linux", "x64", "2.39"));

  assert.equal(selected.error, undefined);
  assert.equal(selected.info.packageName, "@srothgan/claude-code-rust-linux-x64-gnu");
});

test("selectTarget rejects musl Linux without reinstall advice", () => {
  const selected = selectTarget(processLike("linux", "x64", undefined));

  assert.match(selected.error, /linux\/x64 musl is not supported/);
  assert.match(selected.error, /require glibc/);
  assert.doesNotMatch(selected.error, /reinstall/);
});

test("selectTarget lists supported platforms for unsupported host", () => {
  const selected = selectTarget(processLike("freebsd", "x64", undefined));

  assert.match(selected.error, /Unsupported platform\/arch/);
  assert.match(selected.error, /Supported platforms:/);
  assert.match(selected.error, /darwin:arm64/);
  assert.equal(supportedPlatformsText().includes("linux:x64 glibc"), true);
});

test("resolveInstall reports omitted optional dependency for supported platform", () => {
  const resolved = resolveInstall({
    processLike: processLike("linux", "arm64", "2.39"),
    requireResolve() {
      const error = new Error("missing");
      error.code = "MODULE_NOT_FOUND";
      throw error;
    }
  });

  assert.match(resolved.error, /@srothgan\/claude-code-rust-linux-arm64-gnu/);
  assert.match(resolved.error, /npm install --omit=optional/);
  assert.match(resolved.error, /npm config get omit/);
});

test("resolveInstall returns binary and bridge paths when package payload is complete", () => {
  const packageJsonPath = path.join("node_modules", "@srothgan", "claude-code-rust-darwin-arm64", "package.json");
  const resolved = resolveInstall({
    processLike: processLike("darwin", "arm64", undefined),
    dirname: path.join("node_modules", "claude-code-rust", "bin"),
    requireResolve(request) {
      assert.equal(request, "@srothgan/claude-code-rust-darwin-arm64/package.json");
      return packageJsonPath;
    },
    existsSync() {
      return true;
    }
  });

  assert.equal(resolved.error, undefined);
  assert.equal(resolved.binaryPath, path.join("node_modules", "@srothgan", "claude-code-rust-darwin-arm64", "bin", "claude-rs"));
  assert.equal(
    resolved.bundledBridgeScript,
    path.join("node_modules", "claude-code-rust", "agent-sdk", "dist", "bridge.js")
  );
});

test("detectLinuxLibc treats missing process report as musl-class unsupported libc", () => {
  assert.equal(detectLinuxLibc({ platform: "linux", arch: "x64" }), "musl");
});
