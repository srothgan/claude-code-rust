# claude-rs agent-sdk bridge

NDJSON stdio bridge that connects the Rust TUI (`claude-code-rust`) with `@anthropic-ai/claude-agent-sdk`. Spawned as a child process by the Rust binary and communicates via line-delimited JSON envelopes over stdin/stdout.

Published npm platform packages run this bridge with the private `claude-rs-bridge-bun` runtime bundled beside the native binary. Local contributor builds still use npm/TypeScript tooling to build `dist/bridge.js`.

## Local build

Local maintainer builds require Node.js 24. The bridge is an npm workspace of the repository root, so install dependencies from the root lockfile:

```bash
npm ci            # from the repository root
npm run build     # from agent-sdk/
```

Build output is written to `dist/bridge.js`.

## License

This bridge is part of the `claude-code-rust` project and is licensed under
the Apache License 2.0. See the repository root [LICENSE](../LICENSE).
