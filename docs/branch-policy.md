# Branch Policy

Boruna maintains two long-lived branches:

```
master  ──●──●──●──●── (1.x LTS line; reader constants frozen)
           \
            \
0.7.x   ────●──●──●──── (parallel; speculative surfaces; may break wire shape)
```

## `master`

- The 1.x LTS line. Receives all LTS-compatible feature work and
  patches.
- The four versioned spec reader constants
  (`AX_LANGUAGE_VERSION`, `BYTECODE_VERSION`,
  `WORKFLOW_DAG_SCHEMA_VERSION`, `BUNDLE_FORMAT_VERSION`) MUST NOT
  change on this branch.
- See [`lts.md`](./lts.md) §B for the precise stable surface.
- All PRs that target users running 1.x land here.

## `0.7.x`

- A parallel speculative branch cut off the `v1.0.0` GA tag.
- Receives work that would break the LTS contract — e.g. wire-format
  changes (`protocol_version: 2`), capability negotiation overhauls,
  new auth shapes that supersede the bearer token, etc.
- Anything landed here is **not** part of any 1.x release. Operators
  running production should pin to a `1.x` tag, not a `0.7.x` build.
- The version line in `Cargo.toml` is `0.7.0-dev` to make this
  unambiguous.

## Cross-merging

- **Never** merge `0.7.x` wholesale into `master`. The two branches
  diverge by design.
- A feature stabilized on `0.7.x` that we want on `master` must be
  re-landed on `master` as an **additive, LTS-compatible** design.
  That re-landing is a separate task with its own PR, its own design
  doc, and its own deprecation/migration plan if it touches an
  existing surface.
- Cherry-picks of pure bug fixes are allowed in either direction
  when the fix applies cleanly and does not change a public surface.

## Branch protection

Both branches are protected on GitHub:

- CI must pass before merge.
- At least one approving review is required for non-admin merges.
- Force-pushes are disallowed.
- Repository administrators may override these rules in coordinated
  release operations (tag cuts, retroactive CHANGELOG fixes).

## Tags

- `v1.0.x`, `v1.y.z` tags are cut from `master`.
- `0.7.x` produces no tagged releases until the parallel work
  graduates back to a numbered branch.
