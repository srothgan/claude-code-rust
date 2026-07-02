# Installation

## Prerequisites

Claude Code Rust needs:

- Node.js 18 or newer for the Agent SDK bridge runtime.
- Existing Claude Code authentication, currently read from `~/.claude/config.json`.
- Rust 1.88.0 or newer only when building from source.

## Install From npm

The recommended install path is the published npm package:

```bash
npm install -g claude-code-rust
claude-rs --version
claude-rs
```

The npm package installs a small launcher plus a platform-specific optional dependency containing the prebuilt Rust binary for your OS and architecture. No install-time binary download or `postinstall` script is used.

Supported npm platform packages:

| Platform | Package |
| --- | --- |
| Linux x64 glibc | `@srothgan/claude-code-rust-linux-x64-gnu` |
| Windows x64 | `@srothgan/claude-code-rust-win32-x64-msvc` |
| macOS x64 | `@srothgan/claude-code-rust-darwin-x64` |
| macOS arm64 | `@srothgan/claude-code-rust-darwin-arm64` |

The root package exposes the global `claude-rs` command. The launcher resolves the matching platform package, passes the bundled Agent SDK bridge path to the Rust binary, and forwards CLI arguments unchanged.

## Troubleshooting npm Installs

If npm omitted optional dependencies, the launcher cannot find the native platform package. Check your npm config and reinstall:

```bash
npm config get omit
npm install -g claude-code-rust
```

Avoid installing with `--omit=optional`; that prevents npm from installing the native binary package.

If `claude-rs` resolves to an older global shim, ensure your npm global bin directory comes first on `PATH` or remove the stale shim before retrying.

## Build From Source

Use this path when developing the project or testing a fork without installing a global `claude-rs` command:

```bash
git clone https://github.com/srothgan/claude-code-rust.git
cd claude-code-rust
npm ci --prefix agent-sdk
npm run build --prefix agent-sdk
cargo run
```

Debug builds resolve `agent-sdk/dist/bridge.js` from the checkout after the bridge is built.

For a release-mode source binary:

```bash
npm ci --prefix agent-sdk
npm run build --prefix agent-sdk
cargo build --release --locked --bin claude-rs
./target/release/claude-rs --bridge-script ./agent-sdk/dist/bridge.js
```

On Windows, run `.\target\release\claude-rs.exe --bridge-script .\agent-sdk\dist\bridge.js`.

## Install A Source Or Fork Build Globally

For a local global install that mirrors the published npm layout, generate local npm tarballs and install the root tarball together with the matching platform tarball.

Build and stage the native binary under `dist-platform/<platform>/bin/`:

| Platform directory | Rust target | Binary |
| --- | --- | --- |
| `linux-x64-gnu` | `x86_64-unknown-linux-gnu` | `claude-rs` |
| `win32-x64-msvc` | `x86_64-pc-windows-msvc` | `claude-rs.exe` |
| `darwin-x64` | `x86_64-apple-darwin` | `claude-rs` |
| `darwin-arm64` | `aarch64-apple-darwin` | `claude-rs` |

Then run:

```bash
npm ci
npm ci --prefix agent-sdk
npm run build --prefix agent-sdk
node scripts/generate-npm-packages.mjs
node scripts/verify-npm-packages.mjs
node scripts/smoke-npm-package-install.mjs --platform <platform> --real-binary
npm install -g ./dist-pack/claude-code-rust-<version>.tgz ./dist-pack/<platform-package-tarball>.tgz
```

The smoke command packs the generated packages into `dist-pack/` before installing them in a temporary project.

Do not use `cargo install --path .` if you want to test the npm install shape. `cargo install` writes only the Rust binary to Cargo's bin directory and does not install the bundled Agent SDK bridge or platform package layout.

## Manual Bridge Overrides

If you need to run a manually built binary outside the npm package layout, pass the bridge explicitly:

```bash
claude-rs --bridge-script /path/to/claude-code-rust/agent-sdk/dist/bridge.js
```

You can also set:

```bash
CLAUDE_RS_AGENT_BRIDGE=/path/to/agent-sdk/dist/bridge.js
```

If Node needs an explicit override, set:

```bash
CLAUDE_RS_AGENT_BRIDGE_NODE=/path/to/node
```

## Reporting Install Problems

Include:

- install method
- OS and architecture
- terminal
- `node --version`
- `npm config get omit`
- `claude-rs --version`
- the command you ran
- the exact error output
