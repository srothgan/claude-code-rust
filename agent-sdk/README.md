# claude-rs agent-sdk bridge

NDJSON stdio bridge that connects the Rust TUI (`claude-code-rust`) with `@anthropic-ai/claude-agent-sdk`. Spawned as a child process by the Rust binary and communicates via line-delimited JSON envelopes over stdin/stdout.

## Local build

```bash
npm install
npm run build
```

Build output is written to `dist/bridge.mjs`.

## License

This bridge is part of the `claude-code-rust` project and is licensed under
the Apache License 2.0. See the repository root [LICENSE](../LICENSE).
