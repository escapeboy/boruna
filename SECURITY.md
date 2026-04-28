# Security Policy

## Supported Versions

Only the current release receives security patches.

| Version | Supported |
|---------|-----------|
| 0.1.x   | ✓         |

Once 1.0 ships, the long-term-support contract in [`docs/lts.md`](docs/lts.md)
takes effect: 1.x is supported actively for 18 months from 1.0 GA and receives
security fixes for 24 months. The 0.x line is EOL on 1.0 GA.

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Report using [GitHub Security Advisories](https://github.com/escapeboy/boruna/security/advisories/new).

Include in your report:
- Description of the vulnerability
- Steps to reproduce
- Potential impact and affected versions

## Response Timeline

- **Acknowledgment**: within 48 hours
- **Initial triage**: within 5 business days
- **Status updates**: every 7 days until resolved
- **Target resolution**: within 90 days for critical issues

## Scope

**In scope**: `boruna-vm` (capability gateway, replay engine), `boruna-compiler`, `boruna-orchestrator`
(workflow runner, evidence bundle verification), `boruna-mcp` server.

**Out of scope**: example files, documentation, third-party dependencies.

## Backport Policy

Security fixes are backported to every supported 1.y minor line for which
the vulnerability applies. Fix versions are cut as patch releases (e.g.
`1.3.4`) on each affected line; patch releases contain the security fix
and any trivially related test or doc changes only.

Severity follows [CVSS v4](https://www.first.org/cvss/v4-0/):

- **CRITICAL or HIGH** — fix released within 7 days of confirmed
  disclosure (or an interim advisory with mitigations if no fix is ready).
- **MEDIUM** — fix released within 30 days of confirmed disclosure.
- **LOW** — bundled with the next scheduled patch release on each
  supported line.

Pre-1.0, only the latest 0.x release receives fixes. Full backport contract
and support-window definitions live in [`docs/lts.md`](docs/lts.md).

## Disclosure Policy

We follow coordinated disclosure:
1. Reporter submits privately via GitHub Security Advisories
2. We confirm the issue and assess severity
3. We develop and test a fix
4. We release the fix and publish a security advisory
5. Reporter is credited (unless they prefer anonymity)

## Safe Harbor

Good-faith security research conducted in accordance with this policy constitutes authorized testing.
We will not pursue legal action for responsible vulnerability disclosure.
