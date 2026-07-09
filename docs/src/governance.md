# Governance

This page records repository governance rules for maintainers and contributors.
It describes how the repository is protected, how release authority is gated, and
which GitHub settings are expected to remain in place.

## Branch Protection

The `main` branch is protected by an active GitHub repository ruleset rather
than classic branch protection.

The ruleset requires pull requests before merging to `main` and requires these
status checks to pass:

- `pr-gate`
- `Lint PR title`

Pull request reviews are required. Review conversations must be resolved before
merge, and new reviewable commits dismiss previous approvals.

## GitHub Actions Permissions

Repository default `GITHUB_TOKEN` permissions are expected to be read-only.
Workflows should request broader permissions only where a job needs them.

Examples of intentionally elevated job permissions include:

- provenance attestations for release artifacts
- OIDC tokens for npm Trusted Publishing and GitHub Pages deployment
- GitHub Release creation and publication
- issue creation or update from scheduled dependency monitoring

Do not rely on repository-wide write permissions for normal CI behavior.

## Release Governance

Release workflow changes are maintainer-owned because they can affect package
contents, provenance, publication, and user installation paths.

The `npm-release` environment gates publication jobs. It requires reviewer
approval before npm publication or GitHub Release publication can proceed.

npm publication must use Trusted Publishing. The release process must not use a
checked-in npm token or a long-lived `NPM_TOKEN`.

## Maintainer Changes

Changes to repository rules, deployment environments, workflow permissions, npm
Trusted Publishing configuration, or release publication behavior require
explicit maintainer review.

When changing governance-sensitive files or settings, summarize the impact in
the pull request and include verification of the relevant GitHub setting after
the change is applied.
