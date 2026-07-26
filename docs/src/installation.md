# Installation

## User Prerequisite

The Claude Code CLI must be installed as fallback for some SDK-unsupported features. See [anthropics/claude-code](https://github.com/anthropics/claude-code) for how to install it.

The recommended script install includes the application and its runtime dependencies. It does not require a Rust toolchain, Node.js, npm, or a separate Bun installation.

## Install Script

Install scripts are available in GitHub Releases starting with `v0.14.0` and are the recommended install path. They install a self-contained release without requiring npm, Node.js, or Bun on the user's machine.

The scripts download a complete release archive from GitHub, verify the release archive integrity, install the native binary with the bundled private Bun runtime, Agent SDK bridge, and production `node_modules`, then run a quiet `claude-rs --version` check. Download diagnostics and strict runtime diagnostics are available with the opt-in verify flag.

Interactive terminals display a fixed-width 10-cell progress bar while downloading the release archive and a spinner for other longer installation steps. The verify flag adds transferred size, total size, average speed, and ETA to the live download bar, followed by the final transfer size, elapsed time, average speed, and HTTP status. Redirected output and CI remain plain and log-friendly, and `NO_COLOR` disables colored status output.

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

### Reinstalling the Same Version

Before downloading release checksums or an archive, the installer compares the selected release with the version recorded in an existing script-owned install at the configured install directory. It does not compare with an npm install or another `claude-rs` found on `PATH`, because those may represent a different installation method or location.

When the selected version is already installed, an interactive installation asks whether to reinstall it and defaults to no. Declining is a successful no-op and leaves the existing files unchanged. In CI or another non-interactive environment, the same-version check also exits successfully without reinstalling.

Use `--yes` or `-Yes` to approve an intentional same-version reinstall, for example to repair an installation:

```bash
curl -fsSL https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.sh | sh -s -- --release v0.14.0 --yes
```

```powershell
.\install.ps1 -Release v0.14.0 -Yes
```

Update mode always treats an already-installed selected version as a successful no-op, even though update mode otherwise runs non-interactively. With the default `latest` selection, the installer must first request GitHub Release metadata to resolve the release tag; the same-version guard still runs before downloading `SHA256SUMS` or the release archive.

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

Other PowerShell flags available when the script is downloaded first:

- `-Yes`
- `-Verify`
- `-Run`
- `-RemoveNpm`
- `-KeepNpm`
- `-Uninstall`
- `-Update`

With the PowerShell one-liner, use the matching environment variables:

- `CLAUDE_RS_VERIFY=1`
- `CLAUDE_RS_RUN=1`
- `CLAUDE_RS_REMOVE_NPM=1`
- `CLAUDE_RS_KEEP_NPM=1`
- `CLAUDE_RS_UNINSTALL=1`

After a successful install, an interactive run asks whether to start `claude-rs` immediately. This runs the installed binary directly, so it works even before a new shell picks up PATH changes. Use `--run`, `-Run`, or `CLAUDE_RS_RUN=1` to start it automatically after install.

When the startup update screen offers **Install update**, it detects whether the running executable is owned by a script or npm install and uses the same method. Script updates replace the existing app directory while preserving its launcher and PATH configuration. If the executable is not in a recognized install layout, the screen offers separate script and npm choices instead of guessing from PATH order.

### Supported Script Platforms

Install archives are published for Linux x64/arm64 with glibc, Windows x64/arm64, and macOS x64/arm64.

Linux musl distributions are not supported by the install archives yet. Use npm if your platform has a matching package, or [build from source](development.md).

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

To switch from the install script back to npm, [uninstall](#uninstall) the script layout first, then install from npm:

```bash
npm install -g claude-code-rust
```

The installers do not silently remove the other install method because that would delete files outside their ownership and can be surprising in managed environments. For non-interactive installs, use `--remove-npm` / `CLAUDE_RS_REMOVE_NPM=1` when you want the script installer to remove the npm install, or `--keep-npm` / `CLAUDE_RS_KEEP_NPM=1` when you want it kept without prompting.

If the wrong `claude-rs` runs after switching methods, see [Troubleshooting](troubleshooting.md).

## Uninstall

Remove a script install on macOS/Linux with `--uninstall`:

```bash
curl -fsSL https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.sh | sh -s -- --uninstall
```

On Windows, PowerShell supports `-Uninstall` when the script is downloaded first:

```powershell
.\install.ps1 -Uninstall
```

With the PowerShell one-liner, arguments cannot be passed through `iex`, so use `CLAUDE_RS_UNINSTALL`:

```powershell
$env:CLAUDE_RS_UNINSTALL = "1"
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm 'https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.ps1' | iex"
Remove-Item Env:\CLAUDE_RS_UNINSTALL
```

The script uninstall path removes the script install directory, removes the Unix launcher when it points at that directory, and removes installer-managed PATH entries where supported. It refuses to delete an app directory that does not look like a `claude-code-rust` script install.

An npm install is owned by npm and is removed with npm:

```bash
npm uninstall -g claude-code-rust
```

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
