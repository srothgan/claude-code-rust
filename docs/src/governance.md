# Governance

This page records repository governance rules for maintainers and contributors.
It describes who stewards the project, how conduct concerns are reported, how the repository is protected, how release authority is gated, how sponsorship is configured, and which GitHub settings are expected to remain in place.

## Project Stewardship

The project is maintained by Simon Rothgang, who is currently the only maintainer and the only Community Moderator.

Repository rules, workflow permissions, release publication, and code of conduct enforcement are all owned by that single role. The project does not have an independent escalation contact today. If a concern involves the maintainer directly, there is no separate internal channel to route it to, and this page does not pretend otherwise.

## Community Conduct and Reporting

The [Code of Conduct](https://github.com/srothgan/claude-code-rust/blob/main/CODE_OF_CONDUCT.md) is the policy and the source of truth for what is expected, what is restricted, and how violations are addressed. This section only describes how to reach the maintainer privately.

Conduct concerns must **not** be reported through public GitHub issues, GitHub Discussions, or GitHub Security Advisories. Those channels are public or reserved for security vulnerabilities, and none of them are an appropriate place for a report involving a person.

Report a concern by email to the Community Moderator, Simon Rothgang:

- [Send a Code of Conduct Report](mailto:simonrothgang@icloud.com?subject=Code%20of%20Conduct%20Report)
- If no email client is configured, write to `simonrothgang@icloud.com` with the subject `Code of Conduct Report`.

Include whatever you can of the following. Only what is needed to understand and respond to the report is expected:

- what happened
- where and when it happened
- relevant links, messages, or screenshots
- who was involved
- any immediate safety or confidentiality needs

Reports are handled privately by the Community Moderator. This page does not promise absolute confidentiality or a fixed response time; the enforcement process and the possible outcomes are described in the Code of Conduct.

## Repository Safeguards

### Branch Protection

The `main` branch is protected by an active GitHub repository ruleset rather than classic branch protection.

The ruleset requires pull requests before merging to `main` and requires these status checks to pass:

- `pr-gate`
- `Lint PR title`

Pull request reviews are required. Review conversations must be resolved before merge, and new reviewable commits dismiss previous approvals.

### GitHub Actions Permissions

Repository default `GITHUB_TOKEN` permissions are expected to be read-only. Workflows should request broader permissions only where a job needs them.

Examples of intentionally elevated job permissions include:

- provenance attestations for release artifacts
- OIDC tokens for npm Trusted Publishing and GitHub Pages deployment
- GitHub Release creation and publication
- issue creation or update from scheduled dependency monitoring

Do not rely on repository-wide write permissions for normal CI behavior.

## Release Authority

Release workflow changes are maintainer-owned because they can affect package contents, provenance, publication, and user installation paths.

The `npm-release` environment gates publication jobs. It requires reviewer approval before npm publication or GitHub Release publication can proceed.

npm publication must use Trusted Publishing. The release process must not use a
checked-in npm token or a long-lived `NPM_TOKEN`.

## Funding

Sponsorship runs through [GitHub Sponsors](https://github.com/sponsors/srothgan). `.github/FUNDING.yml` lists that account, and GitHub reads the file to render the Sponsor button on the repository.

No other funding platform is configured, and the project does not accept sponsorship through any channel that is not listed in that file. Adding or changing a funding target is a maintainer-owned change.

### Tiers

Tiers are configured on the GitHub Sponsors account rather than per repository, so they are not specific to this project and may apply to any project the maintainer publishes. Each tier is standalone: a higher tier does not include the benefits of the ones below it.

| Amount | Benefit | Monthly | One-time |
| --- | --- | --- | --- |
| $5 | Named in the release notes | While the sponsorship is active | 30 days |
| $25 | Logo on the maintainer's GitHub profile | While the sponsorship is active | 30 days |
| $50 | Logo at the top of the project README | While the sponsorship is active | 30 days |
| $100 | Logo on the project website | While the sponsorship is active | 30 days |

Monthly and one-time tiers cost the same at every level. One-time placements run for 30 days from the day the placement goes live rather than from the payment date.

Sponsors send their logo and link after sponsoring, by email to the address given in the tier welcome message. Logo placements are added within five working days of receiving usable files, and are removed when a monthly sponsorship is cancelled or a one-time placement reaches the end of its 30 days.

### Placement Policy

Placement is at maintainer discretion. Logos unrelated to software development, or that imply endorsement by or affiliation with Anthropic, are declined and the sponsorship refunded. The [About](about.md) page states the project's relationship to Anthropic, and sponsor placement must not contradict it.

Sponsorship does not buy review priority, merge priority, or influence over the roadmap or governance rules on this page.

## Governance Changes

Changes to repository rules, deployment environments, workflow permissions, npm Trusted Publishing configuration, or release publication behavior require explicit maintainer review.

When changing governance-sensitive files or settings, summarize the impact in the pull request and include verification of the relevant GitHub setting after the change is applied.
