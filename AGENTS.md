# AGENTS.md

## Working Style

- Be precise, concise, and evidence-based. Avoid vague statements and unnecessary long explanations.
- Inspect the relevant code, configuration, tests, and existing patterns before changing behavior.
- For clear, scoped implementation requests, proceed without asking for another approval.
- For open-ended feature or design requests with more than one viable approach, present the options and a recommendation before editing.
- Ask for clarification only when ambiguity materially affects behavior, scope, data, security, or an irreversible/external action.
- Verify assumptions that could affect correctness. When version- or API-specific behavior matters, prefer official documentation.
- Keep changes focused on the requested work. Avoid unrelated refactors or cleanup.
- Prefer established repository patterns unless there is a concrete reason to change them.

## Discussing

Focus on substance over praise. Skip unnecessary compliments, engage critically with my ideas, question my assumptions, identify my biases, and offer counterpoints when relevant. Don't shy away from disagreement, and ensure that any agreements are grounded in reason and evidence.

## Change Discipline

### Complete the relevant change

Understand which parts of the system the requested behavior actually crosses and update all affected parts.

Depending on the task, this may include runtime behavior, domain logic, persistence, schemas, generated artifacts, external interfaces, tests, documentation, or obsolete code.

Do not treat this as a mechanical checklist. Update what is relevant to the real workflow and no more.

### Follow the real workflow

Reason from the actual entry point through downstream behavior instead of modifying only the directly mentioned function or file.

Where relevant, consider:

- success and empty cases;
- partial and failure paths;
- retry or reuse behavior;
- boundary conditions;
- persistence or serialization round-trips;
- downstream consumers.

Do not add handling for hypothetical states that are not reachable through the supported workflow unless explicitly requested.

## Architecture

### One authority per invariant

Each important validation, invariant, transition, relationship, decision, or derived value should have one authoritative owner.

Do not duplicate the same semantic rule across callers and callees, services and domain objects, persistence and runtime models, generated and handwritten models, or multiple helpers.

Boundary validation may validate a relationship or input at the boundary, but should not reimplement invariants already guaranteed by the authoritative object.

### Avoid parallel or driftable state

Do not introduce unnecessary:

- copied semantic fields;
- mirrored models;
- duplicated derived values;
- alternative decision paths;
- compatibility paths that keep two implementations active;
- multiple implementations of the same calculation or rule.

Representations at different layers may differ in shape, but they should agree on the same semantics and source of truth.

### Fix root causes

When a defect comes from incorrect ownership, duplicated state, an inconsistent invariant, or a broken workflow, fix that cause rather than layering guards, filters, fallbacks, validators, or special cases around the symptom.

Additional defensive checks are appropriate when they protect a real boundary or failure mode, not when they compensate for an unresolved architectural problem.

## Replacement and Cleanup

When behavior is replaced, remove the obsolete implementation instead of leaving parallel architectures active.

Check for stale:

- validation;
- helpers;
- branches;
- fields or enums;
- imports or parameters;
- comments or diagnostics;
- tests;
- compatibility paths.

Do not remove an important behavioral guarantee merely because its previous implementation was replaced. Move the guarantee and its regression coverage to the new authoritative owner.

Keep cleanup scoped to code made obsolete by the requested change unless broader cleanup is explicitly requested.

## Testing and Verification

- Add regression coverage for changed behavior when appropriate.
- Prefer tests of the authoritative behavior and real workflow over tests that only exercise isolated helpers.
- Define each test by the intended user or domain guarantee and the concrete defect it must catch. A passing test must distinguish the required behavior from a plausible broken implementation.
- Derive expected results from requirements or independent fixtures, not from the production helper or calculation being tested.
- Assert observable outcomes at the relevant boundary: persisted data, emitted commands, delivered notifications, interaction responses, or the relevant rendered region. Check the actual target and seed the state named by the test.
- Use exact string assertions when the string itself is required output, such as copied content, time formatting, protocol bytes, or explicitly required UI wording. Do not lock down incidental prose, punctuation, or complete footer text.
- For rendering tests, verify association, selection, visibility, clipping, and controls in their relevant rows or regions. Whole-screen substring searches alone do not prove those guarantees.
- Synchronize process tests using correlated events or persisted outcomes when available. Do not use cosmetic success messages or arbitrary sleeps as evidence that an operation completed.
- Keep fixtures independent between tests. Use the lowest test layer that exercises the required path, and retain functional coverage of important cross-layer workflows. Duplicate coverage at multiple layers must protect distinct boundaries.
- Preserve meaningful negative guarantees, such as preventing writes, duplicate alerts, or disclosure of private data. Do not add tests solely to prove that removed code or a superseded implementation is absent.
- Use targeted checks while iterating, then run the relevant broader verification before considering the work complete.
- Never claim that a test, lint, formatting, generation, build, or other verification step passed unless it was actually run.
- If a relevant check cannot be run, state that clearly.
- Before finishing, review the complete diff for:
  - missing requirements;
  - duplicated semantic authority;
  - stale or parallel behavior;
  - inconsistent representations or contracts;
  - missing generated artifacts or other required follow-up;
  - accidental unrelated changes.
- Do not consider the work complete while a known reachable defect or duplicated semantic authority remains in the changed behavior.

## Permissions and Side Effects

Require explicit user intent before actions with meaningful external, destructive, or irreversible side effects, such as:

- committing or pushing;
- publishing npm packages, creating tags, or triggering releases;
- changing GitHub state such as PRs, issues, labels, or repository settings;
- deleting or overwriting files outside the scope of the requested change;
- other actions that cannot be safely undone locally.

Project-specific restrictions may tighten these defaults.

## Project-Specific Instructions

### Toolchain

- Rust crate `claude-code-rust` (binary `claude-rs`, edition 2024) built with Cargo. `rust-toolchain.toml` pins channel `1.89`; the MSRV in `Cargo.toml` is `1.88.0`.
- TypeScript bridge in `agent-sdk/` and the root npm package both use npm with Node `>=24`. Each has its own `package-lock.json`; install with `npm ci` (root) and `npm ci --prefix agent-sdk`.
- Bun is only the bundled bridge runtime for source runs and release packaging. It is not the package manager.

### Validation commands

Run the Rust checks after Rust changes. They match the core jobs in `.github/workflows/pr.yml`:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo fetch --locked
```

Run the bridge checks from `agent-sdk/` after bridge changes. They match `.github/workflows/_agent-sdk.yml`:

```bash
npm run build
npm run test
npm run lint
npm run knip
npm run audit
```

Run these only when the change touches the matching area:

- Dependency graph (`Cargo.toml`, `Cargo.lock`, `deny.toml`): `cargo deny check bans licenses sources advisories`.
- MSRV-sensitive code, if the toolchain is installed: `cargo +1.88.0 check --all-features`.
- Duplicate code in `src`, `agent-sdk/src`, or `bin`: `npm run quality:duplicates` from the repository root.
- npm packaging, installers, or `scripts/`: the `npm-package-layout` job in `.github/workflows/pr.yml` is the canonical command list.

### Repository conventions

- Every new `.rs` file starts with `// SPDX-License-Identifier: Apache-2.0`.
- Clippy denies `unwrap_used`, `expect_used`, `panic`, and `exit` in production code (`Cargo.toml` `[lints.clippy]`; tests are exempted through `clippy.toml`). Use `thiserror` for library errors and `anyhow` in main/app code.
- When outdated, dead, or currently unused production code must remain temporarily during a change, mark it with `#[allow(dead_code)]` and a short removal note. Never hide dead or unused production code behind `#[cfg(test)]`; remove it before finalizing when possible.
- The Rust side and the bridge talk NDJSON over stdio. A change to the wire contract must update both `src/agent/wire.rs` and `agent-sdk/src/types.ts` in the same change.
- The `@anthropic-ai/claude-agent-sdk` version is pinned identically in the root `package.json` and `agent-sdk/package.json`; bump both together with their lockfiles.
- PR titles are linted by `.github/workflows/pr-title.yml`. Allowed types, also used for commit messages: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `chore`, `ci`, `revert`. Scope is optional.

### Generated artifacts

- `agent-sdk/dist/` is `tsc` build output and is gitignored. Rebuild it with `npm run build --prefix agent-sdk` after bridge changes; the app spawns `agent-sdk/dist/bridge.js`, so a stale build hides bridge changes at runtime.
- `dist-npm/`, `dist-install/`, `dist-pack/`, `docs/book/`, and `jscpd-report/` are generated by repository scripts and gitignored. Do not edit them by hand.
- There are no database schemas or migrations.

### Diagnostics

- When logging is enabled without `--log-file`, the default filename is timestamped per run: `claude-rs-YYYYMMDDTHHMMSSZ-pPID-rRUN.log`.

### Additional project restrictions

- Never open, create, close, update, or otherwise operate on GitHub PRs, issues, or repository settings from this workspace. If a PR is needed, create only local draft text such as `pr.md` when explicitly requested; the user owns all GitHub actions. Pushing commits is covered by the Git section.
- Do not trigger releases, create tags, or publish npm packages. Release workflow changes are maintainer-owned and must preserve the invariants in `CONTRIBUTING.md` and `docs/src/architecture.md`.
- Do not terminate the bridge runtime process `claude-rs-bridge-bun.exe`. When cleaning up development processes, stop only PIDs started in the current session.

Prefer existing CLI commands and repository scripts when they provide the canonical workflow.

When moving or renaming tracked files, preserve history with `git mv` when appropriate.

## Git

Read-only Git inspection such as `git status`, `git diff`, `git log`, and `git show` is allowed.

Commit and push only when the user asks for it in that turn, never as a side effect of finishing a task. One approval covers that commit or push only; it does not carry over to later ones.

Do not merge, rebase, create/delete/rename branches, switch branches, or rewrite history unless explicitly requested.

When writing or suggesting a commit message:

- describe the net changes against `HEAD`, not the local implementation path;
- use imperative mood, present tense, and concise language;
- use the format `<type>(<scope>): <message>` with a type from the PR title list in Repository conventions;
- include 2–5 precise bullet points in the body;
- do not mention temporary or uncommitted planning files unless they are part of the intended change;
- do not add a `Co-Authored-By` trailer, a generated-by footer, or any other agent attribution.

Example:

```text
chore(agent-sdk): align integration with 0.3.286

- pin root and bridge dependencies to SDK 0.3.286
- propagate new command, lifecycle, MCP, task, artifact, and paste metadata
- handle conversation replacement identity and cache invalidation
- align CLI rendering and ownership decisions with SDK metadata
- add Rust and bridge regression coverage for migrated contracts
```

## Personalization

Personal, untracked instructions live in `PERSONAL.md` at the repository root. The file is optional and gitignored. If it exists, read it and apply it in addition to this document. It may add personal tooling and preferences; it does not relax the restrictions above.

@PERSONAL.md
