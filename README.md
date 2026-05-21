# Claude Code Rust

A native Rust terminal interface for Claude Code. Drop-in replacement for Anthropic's stock Node.js/React Ink TUI, built for performance and a better user experience.

[![npm version](https://img.shields.io/npm/v/claude-code-rust)](https://www.npmjs.com/package/claude-code-rust)
[![npm downloads](https://img.shields.io/npm/dm/claude-code-rust)](https://www.npmjs.com/package/claude-code-rust)
[![CI](https://github.com/srothgan/claude-code-rust/actions/workflows/ci.yml/badge.svg)](https://github.com/srothgan/claude-code-rust/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://www.apache.org/licenses/LICENSE-2.0)
[![Node.js](https://img.shields.io/badge/Node.js-%3E%3D18-green.svg)](https://nodejs.org/)

## About

Claude Code Rust replaces the stock Claude Code terminal interface with a native Rust binary built on [Ratatui](https://ratatui.rs/). It connects to the same Claude API through a local Agent SDK bridge. Core Claude Code functionality - tool calls, file editing, terminal commands, and permissions - works unchanged.

## Requisites

- Node.js 18+ (for the Agent SDK bridge)
- Existing Claude Code authentication (`~/.claude/config.json`)

## Install

### npm (global, recommended)

```bash
npm install -g claude-code-rust
```

The published package installs a `claude-rs` command and fetches the matching prebuilt release binary for your platform during install.

If `claude-rs` resolves to an older global shim, ensure your npm global bin directory comes first on `PATH` or remove the stale shim before retrying.

## Usage

```bash
claude-rs
```

> [!WARNING]
> **Agent SDK billing changes on June 15, 2026.** Anthropic says Agent SDK usage, `claude -p`, Claude Code GitHub Actions, and third-party Agent SDK apps will use a separate monthly Agent SDK credit instead of normal interactive Claude or Claude Code subscription limits. Because Claude Code Rust wraps the Agent SDK, treat usage through this project as Agent SDK usage. If that credit is exhausted, continued use may require enabling extra usage billed at standard API rates, or requests may pause until the credit refreshes.
>
> Sources:
> - [Anthropic support: Use the Claude Agent SDK with your Claude plan](https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan)
> - [ClaudeDevs announcement](https://x.com/ClaudeDevs/status/2054610152817619388)

## Why

The stock Claude Code TUI runs on Node.js with React Ink. This causes real problems:

- **Memory**: 200-400MB baseline vs ~20-50MB for a native binary
- **Startup**: 2-5 seconds vs under 100ms
- **Scrollback**: Broken virtual scrolling that loses history
- **Input latency**: Event queue delays on keystroke handling
- **Copy/paste**: Custom implementation instead of native terminal support

Claude Code Rust fixes all of these by compiling to a single native binary with direct terminal control via Crossterm.

## Custom Commands

Claude Code Rust adds project-local slash commands that set environment variables via `.claude/settings.local.json` without leaving the TUI. Changes apply on the next session.

| Command | Usage | Description |
|---------|-------|-------------|
| `/1m-context` | `/1m-context <enable\|disable\|status>` | Disable the 1 million token context window to improve model performance and prevent quality degradation on large context windows. |
| `/opus-version` | `/opus-version <4.5\|4.6\|4.7\|default\|status>` | Pin the Opus model version for the current folder. Useful for switching to 4.6 or 4.5 to avoid 4.7's tokenization issues. Use `default` to clear the pin. |

## Status

This project is pre-1.0 and under active development. See [CONTRIBUTING.md](CONTRIBUTING.md) for how to get involved.

## Limitations

Startup is still constrained by the upstream Claude Agent SDK runtime that this TUI wraps. The Rust interface itself is fast, but end-to-end readiness can still take noticeable time before a session is fully available. Improving that remains an active area of work.

## License

This project is licensed under the [Apache License 2.0](LICENSE). Apache-2.0 was chosen to keep usage and redistribution straightforward for individual users, downstream packagers, and commercial adopters.

## Disclaimer and Legal Notice

This project is not affiliated with, endorsed by, or supported by Anthropic.

A quick note on where this project stands, since I know people worry about this kind of thing: claude-code-rust is a terminal UI that I wrote from scratch in Rust. It is not a fork, copy or port of the latest Claude Code source leak -- it talks to Anthropic's official [Agent SDK](https://docs.anthropic.com/en/docs/agents-and-tools/claude-code/agent-sdk) as a runtime dependency instead, the same way any other third-party tool would. No Anthropic source code was read or used as reference at any point during development.

The project authenticates through your existing Claude Code account via the Agent SDK, and the Agent SDK's terms allow building on top of it. Billing, credits, limits, and overage behavior are controlled by Anthropic, including the Agent SDK credit change noted above. Other community projects do the same. As far as I can tell, using this project is fine -- but I am a single maintainer, not a lawyer. If anything changes on Anthropic's end, I will update this section and adjust the project accordingly.

This project's source code is licensed under [Apache-2.0](LICENSE). The Agent SDK itself is proprietary and governed by [Anthropic's Commercial Terms of Service](https://www.anthropic.com/legal/commercial-terms).

For official Claude documentation, see [https://claude.ai/docs](https://claude.ai/docs).
