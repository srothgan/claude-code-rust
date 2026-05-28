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

The package installs a `claude-rs` launcher. During `postinstall`, it downloads the matching prebuilt Rust binary from GitHub Releases into the package `vendor/` directory.

Supported published binary targets:

| Platform | Target |
| --- | --- |
| Linux x64 | `x86_64-unknown-linux-gnu` |
| Windows x64 | `x86_64-pc-windows-msvc` |
| macOS x64 | `x86_64-apple-darwin` |
| macOS arm64 | `aarch64-apple-darwin` |

The installer also tries to copy the current Node.js executable next to the Rust binary as `claude-rs-bridge-node` or `claude-rs-bridge-node.exe`. If that copy fails, the Rust binary falls back to `node` on `PATH`.

## Build From Source

Use this path when developing the project or testing a fork without installing a global `claude-rs` command:

```bash
git clone https://github.com/srothgan/claude-code-rust.git
cd claude-code-rust
npm ci --prefix agent-sdk
npm run build --prefix agent-sdk
cargo run 
```

## Install A Source Or Fork Build Globally

Use this path when you want a fully local build that can be launched as `claude-rs` from any directory. It installs through npm's global package location under the package name `claude-code-rust`, the same package name used by the published npm install. That means a later `npm install -g claude-code-rust` replaces the same global package and shim instead of competing with a separate `cargo install` binary on `PATH`.

Do not use `cargo install --path .` for the global command if you want this behavior. `cargo install` writes to Cargo's bin directory and can create a separate `claude-rs` earlier or later on `PATH`.

### Windows x64

```powershell
git clone https://github.com/<you>/claude-code-rust.git
cd claude-code-rust

npm ci --prefix agent-sdk
npm run build --prefix agent-sdk

cargo build --release --locked --target x86_64-pc-windows-msvc --bin claude-rs

$target = "x86_64-pc-windows-msvc"
New-Item -ItemType Directory -Force "vendor\$target" | Out-Null
Copy-Item "target\$target\release\claude-rs.exe" "vendor\$target\claude-rs.exe" -Force
Copy-Item (Get-Command node).Source "vendor\$target\claude-rs-bridge-node.exe" -Force

npm install -g . --ignore-scripts
claude-rs --version
```

### Linux x64

```bash
git clone https://github.com/<you>/claude-code-rust.git
cd claude-code-rust

npm ci --prefix agent-sdk
npm run build --prefix agent-sdk

target=x86_64-unknown-linux-gnu
cargo build --release --locked --target "$target" --bin claude-rs

mkdir -p "vendor/$target"
cp "target/$target/release/claude-rs" "vendor/$target/claude-rs"
chmod +x "vendor/$target/claude-rs"
cp "$(command -v node)" "vendor/$target/claude-rs-bridge-node"
chmod +x "vendor/$target/claude-rs-bridge-node"

npm install -g . --ignore-scripts
claude-rs --version
```

### macOS

For Apple Silicon:

```bash
target=aarch64-apple-darwin
```

For Intel macOS:

```bash
target=x86_64-apple-darwin
```

Then run:

```bash
git clone https://github.com/<you>/claude-code-rust.git
cd claude-code-rust

npm ci --prefix agent-sdk
npm run build --prefix agent-sdk

cargo build --release --locked --target "$target" --bin claude-rs

mkdir -p "vendor/$target"
cp "target/$target/release/claude-rs" "vendor/$target/claude-rs"
chmod +x "vendor/$target/claude-rs"
cp "$(command -v node)" "vendor/$target/claude-rs-bridge-node"
chmod +x "vendor/$target/claude-rs-bridge-node"

npm install -g . --ignore-scripts
claude-rs --version
```

The local global install relies on the same launcher shape as the published npm package:

- `bin/claude-rs.js` is installed as the global `claude-rs` shim.
- `vendor/<target>/claude-rs` or `vendor/<target>/claude-rs.exe` is the Rust binary the shim launches.
- `agent-sdk/dist/bridge.js` is included from the source checkout after `npm run build --prefix agent-sdk`.
- `claude-rs-bridge-node` is a copied Node runtime, matching the published installer behavior. If that file is missing, the Rust binary falls back to `node` on `PATH`.

To go back to the published package later:

```bash
npm install -g claude-code-rust
```

That overwrites the same global npm package and command shim.

## Manual Bridge Overrides

If you need to run a manually built binary outside the npm package layout, pass the bridge explicitly:

```bash
claude-rs --bridge-script /path/to/claude-code-rust/agent-sdk/dist/bridge.js
```

You can also set:

```bash
CLAUDE_RS_AGENT_BRIDGE=/path/to/agent-sdk/dist/bridge.js
```

If the bundled or system Node runtime needs an explicit override, set:

```bash
CLAUDE_RS_AGENT_BRIDGE_NODE=/path/to/node
```

## Troubleshooting

If `claude-rs` does not launch after npm install:

- Confirm the npm global bin directory is first on `PATH`.
- Remove stale global shims from older installs.
- Reinstall with `npm install -g claude-code-rust`.
- Confirm your platform is one of the published targets above.
- Confirm Node.js 18 or newer is available.
- Confirm Claude Code authentication exists at `~/.claude/config.json`.
- If running from source, confirm `agent-sdk/dist/bridge.js` exists.

When reporting install problems, include the install method, OS, terminal, `node --version`, `claude-rs --version`, the command you ran, and the exact error output.
