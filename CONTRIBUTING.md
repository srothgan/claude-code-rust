# Contributing to claude-code-rust

Thank you for considering contributing to claude-code-rust! This document provides guidelines and information for contributors.

## Code of Conduct

This project follows the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md). By participating, you agree to uphold this code.

## How to Contribute

### Reporting Bugs

- Use the [Bug Report](https://github.com/srothgan/claude-code-rust/issues/new?template=bug_report.yml) issue template
- Include reproduction steps, expected vs actual behavior, and environment details
- Follow [Useful Issue Reports](docs/src/diagnostics.md#useful-issue-reports) for redacted diagnostics; review collected material before sharing it

### Suggesting Features

- Use the [Feature Request](https://github.com/srothgan/claude-code-rust/issues/new?template=feature_request.yml) template
- Check existing issues and discussions first
- Describe the problem being solved, not just the desired solution

### Before Starting Work

Read [AGENTS.md](AGENTS.md) and [the architecture documentation](docs/src/architecture.md) before implementing a change.

- **Issue first:** Find or create an issue and discuss the problem and proposed approach there. Opening an issue does not mean the implementation is approved.
- **Agree on scope:** Wait for explicit maintainer agreement before implementing the change or opening a PR. This applies to draft PRs too.
- **Keep scope and parallel work agreed:** Keep each PR focused on the agreed work item. Ask before adding features, splitting a feature across PRs, or opening multiple PRs in parallel. Address outstanding questions and review feedback before starting additional work.
- **Respect ownership and decisions:** Do not take on maintainer-owned tasks or revisit rejected approaches without explicit maintainer agreement.
- **Own your contribution:** Understand the code you submit, explain its behavior, and verify the actual workflow. Passing checks alone is insufficient. These requirements apply equally to AI-assisted contributions.

PRs opened without the required agreement may be closed without review.

### Submitting Code

Once the scope is agreed:

1. Fork the repository
2. Create a feature branch from `main`: `git checkout -b feat/my-feature`
3. Make your changes following the coding standards below
4. Add or update tests as appropriate
5. Run the checks for every affected area in [Test and validate changes](docs/src/testing.md), and report any checks you could not run.
6. Commit using [Conventional Commits](https://www.conventionalcommits.org/):
   ```
   feat: add keyboard shortcut for tool collapse
   fix: prevent panic on empty terminal output
   ```
7. Push to your fork and open a Pull Request against `main`
8. Link the issue and the maintainer comment agreeing to the scope, and fill out the PR summary, validation, and any relevant notes

### Labels

Labels use a `namespace: value` convention so they group visually and filter cleanly. Apply one `type:` label per issue or pull request. The exception is a dependency update that also carries a security fix, which takes `type: dependencies` and `type: security` together.

| Namespace | Values | Meaning |
| --- | --- | --- |
| `type:` | `bug`, `fix`, `feature`, `docs`, `refactor`, `security`, `dependencies`, `release` | What kind of work it is. Use `bug` for a reported defect and `fix` for the change that resolves one. |
| `area:` | `tui`, `core`, `agent-sdk`, `install`, `ci` | Which part of the project it touches. |
| `status:` | `triage`, `blocked`, `needs-info`, `discussion`, `wontfix` | Where it sits in the workflow. |

`good first issue` and `help wanted` stay unprefixed because GitHub uses them to populate the repository contribution page.

`status: triage` means an issue awaits initial maintainer assessment. Remove it after assessing the report and deciding its scope and classification; assignment and other labels do not establish that review is complete.

The `Issue Triage` workflow adds `status: triage` when an issue opens or reopens, including issues created through the CLI or API, and removes it on closure. Repository workflows using `GITHUB_TOKEN` apply that lifecycle themselves because those events do not trigger another workflow. Refreshing an existing issue does not restore triage.

Collaborators with write access can use these standalone issue comments:

- `/triaged`: Remove `status: triage` after assessment. This does not assign the issue or approve an implementation.
- `/duplicate #123`: Close the current issue as a duplicate of issue #123 in this repository, record GitHub's duplicate relationship, and remove triage. The target must be an existing issue, not a PR or the current issue; it may already be closed.

These commands ignore quoted text, code blocks, bot comments and PR threads. They make no changes for users without write access. Duplicate handling requires an explicit maintainer decision; the bot does not infer similarity or post acknowledgement comments.

Issue templates apply `type:` automatically, Dependabot applies `type: dependencies` plus the matching `area:` label from `.github/dependabot.yml`, and the weekly `Dependency Monitor` workflow applies `type: security` to the advisory issue it opens. `status: triage` is named in `.github/scripts/issue-triage.cjs` and `.github/workflows/dependency-monitor.yml`. Renaming any of those labels requires updating that config in the same change.

## Development and validation

- [Build and run locally](docs/src/development.md) covers prerequisites, dependency installation, source runs and bridge overrides.
- [Test and validate changes](docs/src/testing.md) maps each changed area to local checks and CI jobs.
- [Packaging and release invariants](docs/src/packaging.md) records artifact ownership and validation. Release workflow changes remain maintainer-owned; [Governance](docs/src/governance.md) owns publication approval policy.

## Coding Standards

- **Formatting**: Use `rustfmt` (configured via `rustfmt.toml`)
- **Linting**: `cargo clippy` must pass with zero warnings (configured via `clippy.toml` and `Cargo.toml` `[lints.clippy]`)
- **Naming**: Follow [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/naming.html)
- **Error handling**: Use `thiserror` for library errors, `anyhow` in main/app
- **Comments**: Only where the logic isn't self-evident
- **License headers**: Every new `.rs` file should include `// SPDX-License-Identifier: Apache-2.0`

## AI Agents

`AGENTS.md` holds the shared instructions for coding agents working in this repository. `CLAUDE.md` only imports it, so Claude Code and other agents follow the same rules.

Keep `AGENTS.md` limited to what applies to every contributor. Put your own tooling and preferences, such as personal MCP servers, skills, or search tools, in a `PERSONAL.md` at the repository root. The file is gitignored and optional. The Personalization section of `AGENTS.md` pulls it in: Claude Code imports it automatically, and other agents are told to read it when it exists.

## Architecture

Read the [architecture overview](docs/src/architecture.md) for component boundaries, one prompt's end-to-end flow, and where changes belong. [Subsystem ownership and invariants](docs/src/subsystems.md) records the detailed state, persistence and failure guarantees.

## License

By contributing, you agree that your contributions will be licensed under the Apache-2.0 license, the same license as the project.
