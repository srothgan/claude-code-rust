# Claude Code Rust

A native Rust terminal interface for Claude Code. Drop-in replacement for Anthropic's stock Node.js/React Ink TUI, built for performance and a better user experience.

[![npm version](https://img.shields.io/npm/v/claude-code-rust)](https://www.npmjs.com/package/claude-code-rust)
[![npm downloads](https://img.shields.io/npm/dm/claude-code-rust)](https://www.npmjs.com/package/claude-code-rust)
[![CI](https://github.com/srothgan/claude-code-rust/actions/workflows/ci.yml/badge.svg)](https://github.com/srothgan/claude-code-rust/actions/workflows/ci.yml)
[![License: AGPL-3.0-or-later](https://img.shields.io/badge/License-AGPL--3.0--or--later-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)
[![Node.js](https://img.shields.io/badge/Node.js-%3E%3D18-green.svg)](https://nodejs.org/)

## About

Claude Code Rust replaces the stock Claude Code terminal interface with a native Rust binary built on [Ratatui](https://ratatui.rs/). It connects to the same Claude API through a local Agent SDK bridge. Core Claude Code functionality - tool calls, file editing, terminal commands, and permissions - works unchanged.

## Requisites

- Node.js 18+ (for the Agent SDK bridge)
- Existing Claude Code authentication (`~/.claude/config.json`)

## Install

### pnpm (global, recommended)

```bash
pnpm add -g claude-code-rust
```

The published package installs a `claude-rs` command and downloads the matching
prebuilt release binary for your platform during `postinstall`.

## Usage

```bash
claude-rs
```

## Why

The stock Claude Code TUI runs on Node.js with React Ink. This causes real problems:

- **Memory**: 200-400MB baseline vs ~20-50MB for a native binary
- **Startup**: 2-5 seconds vs under 100ms
- **Scrollback**: Broken virtual scrolling that loses history
- **Input latency**: Event queue delays on keystroke handling
- **Copy/paste**: Custom implementation instead of native terminal support

Claude Code Rust fixes all of these by compiling to a single native binary with direct terminal control via Crossterm.

## Architecture

Three-layer design:

**Presentation** (Rust/Ratatui) - Single binary with an async event loop (Tokio) handling keyboard input and bridge client events concurrently. Virtual-scrolled chat history with syntax-highlighted code blocks.

**Agent SDK Bridge** (stdio JSON envelopes) - Spawns `agent-sdk/dist/bridge.js` as a child process and communicates via line-delimited JSON envelopes over stdin/stdout. Bidirectional streaming for user messages, tool updates, and permission requests.

**Agent Runtime** (Anthropic Agent SDK) - The TypeScript bridge drives `@anthropic-ai/claude-agent-sdk`, which manages authentication, session/query lifecycle, and tool execution.

## Known Limitations

- The config view includes the Settings tab but the Status, Usage, and MCP tabs are not yet implemented.

## Status

This project is pre-1.0 and under active development. See [CONTRIBUTING.md](CONTRIBUTING.md) for how to get involved.

## License

This project is licensed under the [GNU Affero General Public License v3.0 or later](LICENSE).
This license was chosen because Claude Code is not open-source and this license allows everyone to use it while stopping Anthropic from implementing it in their closed-source version.

By using this software, you agree to the terms of the AGPL-3.0. If you modify this software and make it available over a network, you must offer the source code to users of that service.

## Disclaimer

This project is an unofficial terminal UI for Claude Code and is not affiliated with, endorsed by, or supported by Anthropic.
Use it at your own risk.
For official Claude documentation, see [https://claude.ai/docs](https://claude.ai/docs).
