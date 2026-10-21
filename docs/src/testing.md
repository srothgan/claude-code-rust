# Test and validate changes

Run commands from the repository root unless a section says otherwise. Complete [development setup](development.md) first. Start with the smallest test that exercises the changed behavior, then run the relevant broader checks before considering the change complete.

## Choose checks for your change

| Changed area | Required local checks | CI counterpart |
| --- | --- | --- |
| Rust | Format, Clippy, all-feature tests and locked dependency fetch | Rustfmt, Clippy, Test Ubuntu, Lockfile in `pr.yml` |
| Terminal input, layout or lifecycle | Rust checks plus relevant real-terminal regressions | Terminal tests in `pr.yml` |
| TypeScript bridge | Build, tests, lint, Knip and direct-dependency audit | `_agent-sdk.yml` |
| Rust/bridge protocol | Both language suites and the affected end-to-end behavior | Rust and Agent SDK jobs |
| Cargo dependency graph | Dependency policy and advisory checks | Cargo Deny jobs in `pr.yml` |
| Production code in `src`, `agent-sdk/src` or `bin` | Duplicate scan in addition to the area's checks | Duplicate Code in `pr.yml` |
| npm packages, installers or packaging scripts | Full package-layout validation | `npm-package-layout` in `pr.yml` |
| Documentation | Book build, links, moved anchors and relevant examples | `docs.yml`; CLI table coverage in Rust tests |

The workflow files are the executable authority for CI job contents: [pr.yml](https://github.com/srothgan/claude-code-rust/blob/main/.github/workflows/pr.yml), [_agent-sdk.yml](https://github.com/srothgan/claude-code-rust/blob/main/.github/workflows/_agent-sdk.yml), and [docs.yml](https://github.com/srothgan/claude-code-rust/blob/main/.github/workflows/docs.yml). PR title lint and CodeQL also run in GitHub. A passing local suite does not establish that those remote checks passed.

## Rust

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features -- --test-threads=1
cargo fetch --locked
```

Run these sequentially. The repository's `.cargo/config.toml` limits compilation to eight jobs; that limit controls compiler concurrency, not test execution threads. Override it with `-j N` or `CARGO_BUILD_JOBS` when needed. The serial test invocation above follows this workspace's validation instructions; the general Ubuntu CI suite uses Cargo's default test parallelism, while terminal CI explicitly runs serially.

For a focused integration suite:

```bash
cargo test --all-features --test cli_startup -- --test-threads=1
cargo test --all-features --test config_flow -- --test-threads=1
cargo test --all-features --test slash_input_flow -- --test-threads=1
```

Choose the relevant suite rather than running all three during every iteration. Unit tests also live beside their implementation; Cargo accepts a test-name filter for targeted runs.

If your change is sensitive to the minimum supported Rust version and that toolchain is installed, run:

```bash
cargo +1.88.0 check --all-features
```

For changes to `Cargo.toml`, `Cargo.lock` or `deny.toml`:

```bash
cargo deny check bans licenses sources advisories
```

This requires `cargo-deny`. CI runs policy checks generally and gates advisories on dependency-graph changes; the scheduled dependency monitor also reports advisories.

## Terminal regressions

These tests exercise a real pseudo-terminal using the Node fixture in `tests/fixtures/fake-bridge.js`. They do not require a live Claude account.

```bash
node --check tests/fixtures/fake-bridge.js
cargo test --locked --all-features --test terminal_resize -- --test-threads=1
```

The system clipboard regression is ignored by default because it overwrites the clipboard. Run it only when that side effect is acceptable:

```bash
cargo test --locked --all-features --test terminal_resize -- --ignored system_clipboard --test-threads=1
```

Linux CI supplies a display through Xvfb for that test. Keep tests synchronized on emitted events or persisted results; a cosmetic status message or an arbitrary delay does not prove that an operation finished.

## TypeScript bridge

Run from `agent-sdk/` after installing its dependencies:

```bash
npm run build
npm run test
npm run lint
npm run knip
npm run audit
```

`npm run test` builds and runs the Node test suite. The separate build command is also useful before launching the Rust app: it runs `agent-sdk/dist/bridge.js`, so stale output can hide a TypeScript change. CI builds once and invokes the same compiled tests directly.

`npm run audit` is the repository's direct-dependency gate. CI also collects `npm run audit:raw` as a non-blocking report; the two commands have different roles.

## Duplicate code and packaging

With root npm dependencies installed, run:

```bash
npm run quality:duplicates
```

For packaging changes, follow the complete `npm-package-layout` job linked above. It includes runtime staging, script tests, installer syntax and behavior, generated package/archive verification, and smoke tests. See [Packaging and release invariants](packaging.md) before changing those scripts. A mock native payload verifies package layout, not real application startup.

## Build and preview documentation

Use mdBook 0.5.3, matching the docs workflow:

```bash
cargo install mdbook --version 0.5.3 --locked
mdbook build docs
mdbook serve docs --open
```

Install mdBook only if it is missing or you need to match the CI version. `build` writes generated HTML to `docs/book/`; do not edit it. `serve` keeps a preview running until you stop it with `Ctrl+C`.

Check the changed pages in the preview, including navigation, tables, code blocks and links. When moving a section, update incoming links and `SUMMARY.md` to its canonical destination. A successful book build does not check that every local or external link works or that examples are correct.

The CLI table is checked against Clap with:

```bash
cargo test --all-features --test docs_cli_contract
```

That test checks documented long-option membership and duplicates. It does not validate descriptions, defaults, option interactions or examples; compare those with the parser and relevant startup tests.

## Maintain the documentation

| Fact or procedure | Canonical page |
| --- | --- |
| Product installation and removal | [Installation](installation.md) |
| First completed conversation | [Your first session](usage.md) |
| Conversation tasks | [Work with conversations](conversations.md) |
| Launch syntax and option lifetime | [CLI reference](cli.md) |
| Configuration editing | [Configure claude-rs](configure.md) |
| Keys, scopes and persistence | [Settings reference](settings.md) |
| MCP and plugin procedures | [Integrations](integrations.md) |
| Logs and diagnostic bundles | [Diagnostics](diagnostics.md) |
| Local development and validation | [Development](development.md) and this page |
| System flow and internal invariants | [Architecture](architecture.md) and its subsystem reference |
| Artifact layout and packaging rules | [Packaging](packaging.md) |
| Contribution process and project policy | Root `CONTRIBUTING.md` and [Governance](governance.md) |

Other pages should summarize and link instead of maintaining competing detailed procedures. Keep Markdown paragraphs and individual list items on one source line. Preserve the changelog include and edit historical release notes only under the repository's explicit policy.

For a substantial rewrite, review three reader outcomes: a new user can finish and resume a conversation; an existing user can find a precise setting or recovery procedure; and a contributor can locate the behavior, its tests and the required checks. Record which workflows were executed and which were only checked against source. Authentication, external service behavior and untested platforms remain unverified until exercised.
