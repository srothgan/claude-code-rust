# Contributing to claude-code-rust

Thank you for considering contributing to claude-code-rust! This document provides guidelines and information for contributors.

## Code of Conduct

This project follows the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md).
By participating, you agree to uphold this code.

## How to Contribute

### Reporting Bugs

- Use the [Bug Report](../../issues/new?template=bug_report.yml) issue template
- Include reproduction steps, expected vs actual behavior, and environment details
- Run with `RUST_LOG=debug` and include relevant log output

### Suggesting Features

- Use the [Feature Request](../../issues/new?template=feature_request.yml) template
- Check existing issues and discussions first
- Describe the problem being solved, not just the desired solution

### Submitting Code

1. Fork the repository
2. Create a feature branch from `main`: `git checkout -b feat/my-feature`
3. Make your changes following the coding standards below
4. Add or update tests as appropriate
5. Ensure all checks pass:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test --all-features
   cargo fetch --locked
   ```
   If you have the MSRV toolchain (`1.88.0`) installed, also verify:
   ```bash
   cargo +1.88.0 check --all-features
   ```
6. Commit using [Conventional Commits](https://www.conventionalcommits.org/):
   ```
   feat: add keyboard shortcut for tool collapse
   fix: prevent panic on empty terminal output
   ```
7. Push to your fork and open a Pull Request against `main`
8. Fill out the PR summary, validation, and any relevant notes

## Development Setup

### Prerequisites

- Rust 1.88.0+ (install via https://rustup.rs)
- npm for contributor JavaScript tooling and package scripts.
- Bun for source runs, bridge runtime checks, and release packaging validation.

### Clone and Build

```bash
git clone https://github.com/srothgan/claude-code-rust.git
cd claude-code-rust
cargo build
```

### Run

```bash
cargo run

# Run with debug logging
RUST_LOG=debug cargo run
```

### Running CI Checks Locally

These match the core checks in `.github/workflows/pr.yml`:

```bash
# Formatting
cargo fmt --all -- --check

# Linting
cargo clippy --all-targets --all-features -- -D warnings

# Tests
cargo test --all-features

# Lockfile integrity
cargo fetch --locked

# Cargo dependency policy
cargo deny check bans licenses sources advisories

# MSRV (requires the 1.88.0 toolchain)
cargo +1.88.0 check --all-features
```

Additional GitHub-only checks include the separate PR title lint workflow,
CodeQL analysis, path-aware package layout validation, scheduled cross-platform
smoke tests, and release packaging validation.

### Release And Packaging Changes

Release workflow changes are maintainer-owned and should preserve the package architecture described in `docs/src/architecture.md`.

Important invariants:

- The root npm package must not use `postinstall` or install-time binary downloads.
- The root npm package owns the `claude-rs` bin through `bin/claude-rs.js`.
- Native binaries and private Bun runtimes live in platform-specific optional npm packages.
- Platform packages must not expose their own npm `claude-rs` bin; otherwise npm can link the payload package over the root resolver.
- Platform packages must publish before the root package for a given version.
- npm publication must use Trusted Publishing, not a checked-in token or `NPM_TOKEN`.
- Release artifacts should be generated, verified, packed, and smoke-tested before publication.

For local package-layout validation, use the platform mapping in `scripts/shared/npm-package-config.mjs` rather than duplicating package names in docs:

```bash
npm ci
npm ci --prefix agent-sdk
npm run build --prefix agent-sdk
node scripts/npm/generate-npm-packages.mjs
node scripts/npm/verify-npm-packages.mjs
node scripts/npm/smoke-npm-package-install.mjs --platform <platform> --real-binary --no-system-runtime
node scripts/npm/smoke-npm-package-install.mjs --platform <platform> --registry-smoke --real-binary --no-system-runtime
```

Do not trigger releases, create tags, or publish npm packages from contributor PRs.

## Coding Standards

- **Formatting**: Use `rustfmt` (configured via `rustfmt.toml`)
- **Linting**: `cargo clippy` must pass with zero warnings (configured via `clippy.toml` and `Cargo.toml` `[lints.clippy]`)
- **Naming**: Follow [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/naming.html)
- **Error handling**: Use `thiserror` for library errors, `anyhow` in main/app
- **Comments**: Only where the logic isn't self-evident
- **License headers**: Every new `.rs` file should include `// SPDX-License-Identifier: Apache-2.0`

## Architecture

The project is split into a Rust binary and an in-repo TypeScript bridge:

```
src/
├── main.rs          # Entry point – CLI parsing, tokio runtime + LocalSet
├── agent/           # Bridge spawning, NDJSON client, wire types, event handling
├── app/             # Application state, event loop, config, permissions, input
└── ui/              # Ratatui widgets – chat view, markdown, diffs, footer, themes

agent-sdk/
└── src/             # TypeScript NDJSON stdio bridge wrapping @anthropic-ai/claude-agent-sdk
```

**How the pieces connect:**

1. `main.rs` boots a `tokio::task::LocalSet` (required because the bridge child
   process handles are `!Send`) and hands control to `app::run_tui`.
2. `agent::client::BridgeClient` spawns `agent-sdk/dist/bridge.js` as a child
   process and communicates over **NDJSON on stdin/stdout**.
3. The Rust side sends `CommandEnvelope`s (start session, submit prompt,
   permission responses, …) and receives `EventEnvelope`s (assistant messages,
   tool calls, errors, …).
4. `app/` ties everything together: it owns the `App` state, routes terminal
   events and bridge events through `tokio::sync::mpsc` channels, and drives the
   TUI render loop.
5. `ui/` is a pure rendering layer built on **Ratatui + Crossterm** (cross-platform).

## License

By contributing, you agree that your contributions will be licensed under the
Apache-2.0 license, the same license as the project.
