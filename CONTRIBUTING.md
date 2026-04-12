# Contributing to claude_rust

Thank you for considering contributing to claude_rust! This document provides
guidelines and information for contributors.

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
- Node.js 18+ (for the in-repo agent bridge)
- npx (included with Node.js)

### Clone and Build

```bash
git clone https://github.com/srothgan/claude-code-rust.git
cd claude_rust
cargo build
```

### Run

```bash
cargo run

# Run with debug logging
RUST_LOG=debug cargo run
```

### Running CI Checks Locally

These match the checks in `.github/workflows/ci.yml`:

```bash
# Formatting
cargo fmt --all -- --check

# Linting
cargo clippy --all-targets --all-features -- -D warnings

# Tests
cargo test --all-features

# Lockfile integrity
cargo fetch --locked

# MSRV (requires the 1.88.0 toolchain)
cargo +1.88.0 check --all-features
```

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
2. `agent::client::BridgeClient` spawns `agent-sdk/dist/bridge.mjs` as a child
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
