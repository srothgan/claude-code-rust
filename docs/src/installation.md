# Installation

## User Prerequisite

Install the Claude Code CLI before using Claude Code Rust. Authentication is not required before installation or startup; if you are not already signed in, use `/login` inside `claude-rs` or run `claude auth login` when needed.

The recommended script install includes the application and its runtime dependencies. It does not require a Rust toolchain, Node.js, npm, or a separate Bun installation.

## Install Script

Install scripts are available in GitHub Releases starting with `v0.14.0` and are the recommended install path. They install a self-contained release without requiring npm, Node.js, or Bun on the user's machine.

The scripts download a complete release archive from GitHub, verify the release archive integrity, install the native binary with the bundled private Bun runtime, Agent SDK bridge, and production `node_modules`, then run a quiet `claude-rs --version` check. Strict runtime diagnostics are available with the opt-in verify flag.

Interactive terminals display live progress for longer installation steps. Redirected output and CI remain plain and log-friendly, and `NO_COLOR` disables colored status output.

**macOS/Linux:**

```bash
curl -fsSL https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.sh | sh
```

**Windows PowerShell:**

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm 'https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.ps1' | iex"
```

**The default Unix install layout is:**

```text
${XDG_DATA_HOME:-$HOME/.local/share}/claude-rs/
$HOME/.local/bin/claude-rs
```

The app directory contains `claude-rs`, `claude-rs-bridge-bun`, `agent-sdk/`, and `node_modules/`. The file in `$HOME/.local/bin` is a launcher script that executes the app binary.

**The default Windows install layout is:**

```text
%LOCALAPPDATA%\Programs\claude-rs\
```

The Windows app directory is added to the user `Path` unless path modification is disabled.

## Install From npm

npm remains supported for users who prefer package-manager ownership of the global command:

```bash
npm install -g claude-code-rust
claude-rs --version
claude-rs
```

The npm option requires Node.js 24 and npm to install and run its JavaScript launcher. The package owns the `claude-rs` command, selects the matching platform payload with npm optional dependencies, and includes the private Bun runtime used by the Agent SDK bridge. A Rust toolchain and separate Bun installation are not required. No install-time binary download or `postinstall` script is used.

Supported npm platforms are Linux x64/arm64 with glibc, Windows x64/arm64, and macOS x64/arm64.

### Pinning a Release

Use `CLAUDE_RS_RELEASE` to install a specific release:

```bash
curl -fsSL https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.sh | CLAUDE_RS_RELEASE=v0.14.0 sh
```

```powershell
$env:CLAUDE_RS_RELEASE = "v0.14.0"
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm 'https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.ps1' | iex"
```

The version can include or omit the `v` prefix.

### Custom Install Locations

On macOS/Linux, pass installer flags after `sh -s --`:

```bash
curl -fsSL https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.sh | sh -s -- --install-dir "$HOME/.local/share/claude-rs" --bin-dir "$HOME/.local/bin"
```

For non-interactive Unix installs:

```bash
curl -fsSL https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.sh | sh -s -- --yes
```

The Unix installer also accepts:

- `--release <version>`
- `--install-dir <dir>`
- `--bin-dir <dir>`
- `--yes` or `-y`
- `--non-interactive`
- `--no-modify-path`
- `--verify`
- `--run`
- `--remove-npm`
- `--keep-npm`
- `--uninstall`
- `--update`

When using the PowerShell one-liner, configure the installer with environment variables because arguments cannot be passed through `iex`:

```powershell
$env:CLAUDE_RS_INSTALL_DIR = "$env:LOCALAPPDATA\Programs\claude-rs"
$env:CLAUDE_RS_NO_MODIFY_PATH = "1"
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm 'https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.ps1' | iex"
```

If you download the script first, PowerShell flags are also available:

```powershell
.\install.ps1 -Release v0.14.0 -InstallDir "$env:LOCALAPPDATA\Programs\claude-rs" -NoModifyPath
```

PowerShell also supports `-Uninstall` when the script is downloaded first. With the one-liner, use `CLAUDE_RS_UNINSTALL`:

```powershell
$env:CLAUDE_RS_UNINSTALL = "1"
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm 'https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.ps1' | iex"
```

Other PowerShell flags available when the script is downloaded first:

- `-Yes`
- `-Verify`
- `-Run`
- `-RemoveNpm`
- `-KeepNpm`
- `-Update`

With the PowerShell one-liner, use the matching environment variables:

- `CLAUDE_RS_VERIFY=1`
- `CLAUDE_RS_RUN=1`
- `CLAUDE_RS_REMOVE_NPM=1`
- `CLAUDE_RS_KEEP_NPM=1`

After a successful install, an interactive run asks whether to start `claude-rs` immediately. This runs the installed binary directly, so it works even before a new shell picks up PATH changes. Use `--run`, `-Run`, or `CLAUDE_RS_RUN=1` to start it automatically after install.

When the startup update screen offers **Install update**, it detects whether the running executable is owned by a script or npm install and uses the same method. Script updates replace the existing app directory while preserving its launcher and PATH configuration. If the executable is not in a recognized install layout, the screen offers separate script and npm choices instead of guessing from PATH order.

### Supported Script Platforms

Install archives are published for Linux x64/arm64 with glibc, Windows x64/arm64, and macOS x64/arm64.

Linux musl distributions are not supported by the install archives yet. Use npm if your platform has a matching package, or build from source.

The scripts do not require user-installed Node.js or Bun. If npm is available and a global `claude-code-rust` install is present, the installer reports it and can remove it after explicit confirmation so the script install owns `claude-rs` on `PATH`. If the selected release does not contain install archives, the installer exits with:

```text
install script is currently not available for this release
```

### Switching Install Methods

`claude-rs` is resolved by normal `PATH` order. npm and script installs use different app layouts, and one method does not automatically own files created by the other. If both are installed, whichever `claude-rs` appears first on `PATH` runs.

To see every visible `claude-rs` on macOS/Linux:

```bash
command -v claude-rs
which -a claude-rs
```

To see every visible `claude-rs` on Windows:

```powershell
Get-Command claude-rs -All
```

To switch from npm to the install script, either let the installer prompt you or pass the explicit removal flag:

```bash
curl -fsSL https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.sh | sh -s -- --remove-npm
```

```powershell
$env:CLAUDE_RS_REMOVE_NPM = "1"
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm 'https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.ps1' | iex"
Remove-Item Env:\CLAUDE_RS_REMOVE_NPM
```

To switch from the install script back to npm, uninstall the script layout first:

```bash
curl -fsSL https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.sh | sh -s -- --uninstall
npm install -g claude-code-rust
```

```powershell
$env:CLAUDE_RS_UNINSTALL = "1"
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm 'https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.ps1' | iex"
Remove-Item Env:\CLAUDE_RS_UNINSTALL
npm install -g claude-code-rust
```

The script uninstall path removes the script install directory, removes the Unix launcher when it points at that directory, and removes installer-managed PATH entries where supported. It refuses to delete an app directory that does not look like a `claude-code-rust` script install.

The installers do not silently remove the other install method because that would delete files outside their ownership and can be surprising in managed environments. For non-interactive installs, use `--remove-npm` / `CLAUDE_RS_REMOVE_NPM=1` when you want the script installer to remove the npm install, or `--keep-npm` / `CLAUDE_RS_KEEP_NPM=1` when you want it kept without prompting.

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

Source development requires Rust 1.88.0 or newer, Node.js 24 with npm, and Bun. These are developer toolchain requirements, not requirements for script-installed users.

```bash
git clone https://github.com/srothgan/claude-code-rust.git
cd claude-code-rust
npm ci --prefix agent-sdk
npm run build --prefix agent-sdk
cargo run
```

Maintainer and source-build npm tooling targets Node.js 24. Packaged installs use the bundled private Bun runtime for the Agent SDK bridge.

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
- `npm --version`, for npm installs
- `npm config get omit`, for npm installs
- `claude-rs --version`
- `claude-rs doctor --json`
- the command you ran
- the exact error output
