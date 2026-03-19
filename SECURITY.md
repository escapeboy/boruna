# Security Policy

## Supported Versions

Only the current release receives security patches.

| Version | Supported |
|---------|-----------|
| 0.1.x   | ✓         |

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
