# Installation

## Prerequisites

Claude Code Rust needs:

- Installed Claude Code CLI for authentication. Use `claude auth login` before starting, or run `/login` inside `claude-rs`.
- Rust 1.88.0 or newer, Node.js 24/npm, and Bun only when building or running from source.

## Install From npm

The recommended npm install path is the root package:

```bash
npm install -g claude-code-rust
claude-rs --version
claude-rs
```

The npm package owns the `claude-rs` command, selects the matching platform payload with npm optional dependencies, and includes the private Bun runtime used by the Agent SDK bridge. A separate Bun install is not required for npm installs. No install-time binary download or `postinstall` script is used.

Supported npm platforms are Linux x64/arm64 with glibc, Windows x64/arm64, and macOS x64/arm64.

## Troubleshooting npm Installs

If npm omitted optional dependencies, the launcher cannot find the native platform package. Check your npm config and reinstall:

```bash
npm config get omit
npm install -g claude-code-rust
```

If the resolver reports a missing platform package, npm optional dependencies were likely omitted. Check `npm config get omit` and reinstall without `--omit=optional`.

Linux npm packages currently require glibc. On musl-based distributions, build from source until a matching npm package is available.

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

Maintainer and source-build npm tooling targets Node.js 24. Packaged npm installs use the bundled private Bun runtime for the Agent SDK bridge.

Debug builds resolve `agent-sdk/dist/bridge.js` from the checkout after the bridge is built. They use `bun` from `PATH` unless `CLAUDE_RS_AGENT_BRIDGE_RUNTIME` points at a specific Bun executable.

For a release-mode source binary, build the bridge and binary, then provide both an explicit bridge script and a Bun runtime using the bundled-runtime filename. Release-mode source binaries do not use the debug PATH fallback for Bun.

```bash
npm ci --prefix agent-sdk
npm run build --prefix agent-sdk
cargo build --release --locked --bin claude-rs
cp "$(command -v bun)" ./target/release/claude-rs-bridge-bun
./target/release/claude-rs --bridge-script ./agent-sdk/dist/bridge.js
```

On Windows, copy `bun.exe` next to the binary as `claude-rs-bridge-bun.exe`, then run:

```powershell
Copy-Item (Get-Command bun).Source .\target\release\claude-rs-bridge-bun.exe
.\target\release\claude-rs.exe --bridge-script .\agent-sdk\dist\bridge.js
```

Do not use `cargo install --path .` if you want to test the npm install shape. `cargo install` writes only the Rust binary to Cargo's bin directory and does not install the bundled Agent SDK bridge, private Bun runtime, or platform package layout.

## Manual Bridge Overrides

If you need to run a manually built binary outside the npm package layout, pass the bridge explicitly:

```bash
claude-rs --bridge-script /path/to/claude-code-rust/agent-sdk/dist/bridge.js
```

You can also set:

```bash
CLAUDE_RS_AGENT_BRIDGE=/path/to/agent-sdk/dist/bridge.js
```

Debug builds can use a local runtime override while developing the bridge:

```bash
CLAUDE_RS_AGENT_BRIDGE_RUNTIME=/path/to/bun
```

Release npm installs ignore runtime overrides and use the bundled `claude-rs-bridge-bun` executable from the platform package.

## Reporting Install Problems

Include:

- install method
- OS and architecture
- terminal
- `npm --version`
- `npm config get omit`
- `claude-rs --version`
- `claude-rs doctor --json`
- the command you ran
- the exact error output
