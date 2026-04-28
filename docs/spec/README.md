# Boruna Versioned Specifications

This directory holds **formal, versioned** specifications for the surfaces Boruna commits to keeping stable.

Each spec carries a `language_version` / `format_version` / `protocol_version` field in its front matter. Implementations against a `1.x` spec MUST keep working against any later `1.y` (`y >= x`).

## Current specs

| Surface | Latest | Status | File |
|---------|:------:|:------:|------|
| `.ax` language | 1.0 | stable | [ax-language-1.0.md](./ax-language-1.0.md) |
| Evidence bundle format | 1.0 | stable | [evidence-bundle-1.0.md](./evidence-bundle-1.0.md) |

Future entries (planned, not yet frozen):

- `bytecode-1.0.md` — the binary opcode set, capability ID table, and module format. The informal version lives at [`docs/bytecode-spec.md`](../bytecode-spec.md) and is the source for the future formal freeze.
- `workflow-dag-1.0.md` — the `workflow.json` schema with `schema_version`.

## Authoring rules

1. Specs are **prescriptive**, not descriptive. They are the authority. Reference docs (under `docs/reference/`) and concept docs (under `docs/concepts/`) are *interpretive*.
2. Each spec MUST declare its version, status, and last-revised date in YAML front matter.
3. Each spec MUST include a backwards-compatibility commitment for its current major line.
4. Once a spec at version `M.N` is shipped in a release tag, it is **frozen**. Corrections that change behavior require bumping to `M.(N+1)` (additive) or `(M+1).0` (breaking).
5. Frozen specs MAY be edited only for clarifications that do not change observable conformance — typo fixes, wording, examples.

## Versioning policy

`MAJOR.MINOR` decimal:

- **Major bump (1.0 → 2.0)** — breaking change. A `1.x` program may stop working.
- **Minor bump (1.0 → 1.1)** — additive only. Every `1.0` program still works.

There is no patch version on specs; clarifying edits keep the same minor.

## Cross-links

- Stability tiers across the codebase: [`../stability.md`](../stability.md)
- Roadmap (which specs are planned): [`../roadmap.md`](../roadmap.md)
- User-friendly references (not specs): [`../reference/`](../reference/)
