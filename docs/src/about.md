# About

Claude Code Rust is a native Rust terminal interface for Claude Code. It replaces the stock Node.js and React Ink terminal UI with a Ratatui-based binary while keeping Claude Code functionality routed through Anthropic's Agent SDK.

The goal is a faster, lower-memory terminal experience with reliable scrollback, direct terminal rendering, native input handling, and a project-local configuration surface.

## Project Status

The project is pre-1.0. The current release line is `0.13.x`, and the crate version is tracked in the root `Cargo.toml`.

The project is useful today, but the runtime still depends on the upstream Claude Agent SDK bridge. Startup readiness, authentication behavior, billing, model availability, and service limits are controlled by Anthropic.

## Relationship To Anthropic

This project is not affiliated with, endorsed by, or supported by Anthropic. It is a third-party terminal UI that talks to the official Agent SDK through a local TypeScript bridge. It is not a fork, copy, or port of Anthropic's Claude Code source.

For official Claude documentation, use the Claude documentation:

- [Claude Docs](https://claude.ai/docs)
- [Claude Code Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview)

## Billing Note

Because Claude Code Rust uses the Agent SDK, usage should be treated as Agent SDK usage. Anthropic has paused the previously announced Agent SDK credit change. For now, Agent SDK usage, including `claude -p` and third-party apps like this one, still draws from normal Claude subscription limits.

Check Anthropic's current support article before relying on billing assumptions:

- [Use the Claude Agent SDK with your Claude plan](https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan)
