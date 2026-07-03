# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| latest  | Yes       |
| < latest | No (upgrade to latest) |

## Reporting a Vulnerability

**Please do NOT report security vulnerabilities through public GitHub issues.**

Instead, use [GitHub Security Advisories](https://github.com/srothgan/claude-code-rust/security/advisories/new)
to report vulnerabilities privately.

Please include:

1. Description of the vulnerability
2. Steps to reproduce
3. Potential impact
4. Suggested fix (if any)

## Response Timeline

- **Acknowledgment**: Within 48 hours
- **Initial assessment**: Within 1 week
- **Fix and disclosure**: Coordinated with reporter, typically within 30 days

## Scope

This policy covers the `claude-rs` binary and its direct dependencies. Vulnerabilities
in the upstream Agent SDK (`@anthropic-ai/claude-agent-sdk`) or Claude API should be
reported to their respective maintainers.

## Security Measures

- Rust dependency policy is enforced with Cargo Deny for advisories, licenses,
  duplicate-crate policy, and allowed sources.
- Cargo Deny advisories run on a scheduled dependency monitor workflow and are
  kept non-blocking so newly published advisories can be triaged deliberately.
- Dependency updates are managed via Dependabot, with Agent SDK migration
  tracked separately through an issue-based monitor.
- GitHub dependency graph, Dependabot alerts, secret scanning, and push
  protection are enabled for repository-level coverage.
- PRs require the `pr-gate` status check and separate semantic PR title lint.
