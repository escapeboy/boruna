# Security Policy

## Supported Versions

Only the latest major line is supported, and security fixes ship in its latest release.

| Version | Supported |
|---------|-----------|
| 4.x     | ✓ (latest release) |
| 3.x     | ✗ end of life since 2026-10-03 |
| 2.x     | ✗ end of life since 2026-07-18 |
| 1.x     | ✗ end of life since 2026-07-18 |
| 0.x     | ✗ |

See [`docs/lts.md`](docs/lts.md) for the full support and deprecation policy.

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

**Trust boundary for `providers.json`.** The provider configuration chooses where prompts go and,
with `kind: "command"`, which program runs for each `llm.call`. Anyone who can edit that file can
run programs as the user who runs Boruna. Boruna's capability policy gates the `llm.call` itself
but cannot constrain what an external program does once started. This is by design, not a
vulnerability; report cases where Boruna runs a program that the configuration did not name.

## Backport Policy

Security fixes are released as a new patch or minor release of the current major line.
There are no backports to older minor releases or to earlier major lines; upgrade to the
latest release to get a fix.

Severity follows [CVSS v4](https://www.first.org/cvss/v4-0/):

- **CRITICAL or HIGH** — fix released within 7 days of confirmed
  disclosure (or an interim advisory with mitigations if no fix is ready).
- **MEDIUM** — fix released within 30 days of confirmed disclosure.
- **LOW** — bundled with the next scheduled release.

Support-window definitions live in [`docs/lts.md`](docs/lts.md).

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
