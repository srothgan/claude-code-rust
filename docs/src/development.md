# Build and run locally

Use this page to run a checkout without installing a global `claude-rs`. For contribution scope and review requirements, read [CONTRIBUTING.md](https://github.com/srothgan/claude-code-rust/blob/main/CONTRIBUTING.md). For an ordinary installation, use [Installation](installation.md).

## Prerequisites

| Tool | Requirement |
| --- | --- |
| Rust and Cargo | The checkout selects Rust 1.89 through `rust-toolchain.toml`. The declared minimum supported Rust version is 1.88.0. |
| Node.js and npm | Node.js 24 or newer for root and bridge tooling. CI uses Node.js 24. |
| Bun | Needed on PATH for debug source runs of the bridge; it is not the package manager. |
| Native C/C++ tools | Visual Studio Build Tools with the C++ workload and Windows SDK on Windows; Xcode Command Line Tools on macOS; GCC/Clang and platform development headers on Linux. |
| Claude CLI and account | Needed for interactive authentication and CLI-backed management. Pure Rust tests and the scripted terminal fixture do not require a model call. |

The native build tools compile dependencies including Oniguruma through `onig_sys`; no separate Oniguruma runtime installation is needed. Packaged user installations already contain their bridge runtime.

## Build From Source

Run these commands in a shell with Git, Cargo, npm and Bun available:

```bash
git clone https://github.com/srothgan/claude-code-rust.git
cd claude-code-rust
npm ci
npm run build --prefix agent-sdk
cargo run -- --help
```

The bridge in `agent-sdk/` is an npm workspace, so the root `npm ci` installs the dependencies of both from the single root lockfile. The bridge build creates `agent-sdk/dist/bridge.js`. The final command compiles the Rust app and prints CLI help without starting a session.

To start the interactive app from the same directory:

```bash
cargo run -- --no-update-check
```

This opens the TUI in the checkout. Authentication, project trust and session access follow the [first-session walkthrough](usage.md). Debug builds resolve the built bridge from the checkout and use Bun from PATH unless a runtime override is set.

## Make and check a change

1. Use the [architecture overview](architecture.md#where-a-change-belongs) to locate the owner of the behavior.
2. Edit that owner and cover the observable outcome with the relevant existing test layer.
3. After TypeScript changes, run `npm run build --prefix agent-sdk` before launching the app again. The app runs compiled JavaScript, so stale output hides source changes.
4. Run the focused tests, then the required checks from [Test and validate changes](testing.md).

For a bridge startup failure, check that `agent-sdk/dist/bridge.js` exists and Bun is available. `cargo run -- doctor` reports resolved paths; see [startup troubleshooting](troubleshooting.md#the-app-will-not-start-or-the-bridge-will-not-spawn). For detailed session logs, run `cargo run -- --enable-logs --diagnostics-preset session` and use the [diagnostics guide](diagnostics.md) to find the output.

## Release-mode source builds

Build the bridge and native binary from the repository root:

```bash
npm run build --prefix agent-sdk
cargo build --release --locked --bin claude-rs
```

Release-mode source binaries do not use the debug PATH fallback for Bun. Place the runtime beside the binary under its bundled name and pass the bridge script explicitly.

On macOS/Linux:

```bash
cp "$(command -v bun)" ./target/release/claude-rs-bridge-bun
./target/release/claude-rs --bridge-script ./agent-sdk/dist/bridge.js
```

On Windows PowerShell:

```powershell
Copy-Item (Get-Command bun).Source .\target\release\claude-rs-bridge-bun.exe
.\target\release\claude-rs.exe --bridge-script .\agent-sdk\dist\bridge.js
```

`cargo install --path .` installs only the Rust binary. It does not install the bridge, private Bun runtime or platform package layout. To test a packaged installation, follow [Packaging](packaging.md#validate-packaging-changes).

## Manual Bridge Overrides

Prefer an explicit script path when running a manually built binary:

```bash
claude-rs --bridge-script /path/to/claude-code-rust/agent-sdk/dist/bridge.js
```

Replace the example path with the built file. `CLAUDE_RS_AGENT_BRIDGE` is the environment alternative. Debug builds also accept `CLAUDE_RS_AGENT_BRIDGE_RUNTIME` to select a Bun executable. For a single source run on macOS/Linux:

```bash
CLAUDE_RS_AGENT_BRIDGE_RUNTIME="$(command -v bun)" cargo run
```

On PowerShell, set an override only for the development session and remove it when finished:

```powershell
$env:CLAUDE_RS_AGENT_BRIDGE_RUNTIME = (Get-Command bun).Source
cargo run
Remove-Item Env:\CLAUDE_RS_AGENT_BRIDGE_RUNTIME
```

Release npm installs ignore the runtime override and use the bundled `claude-rs-bridge-bun` executable from the platform package. Remove stale bridge-script overrides when returning to an installed release.
