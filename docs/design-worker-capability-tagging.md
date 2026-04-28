# Worker capability tagging / placement (sprint W3-A)

Status: shipped 2026-04-28
Roadmap entry: 0.5.0 — Distributed-execution closure
Related ADR: ADR 002 (atomic-upgrade rule, capability-set hash)

## Problem

ADR 002's atomic-upgrade rule rejects any worker whose capability
set hash does not match the coordinator's. This blocks
heterogeneous fleets — every worker MUST carry the same compiled
capability surface as the coord. Workers that genuinely cannot
speak `db.query` (no DB driver installed, sandboxed environment,
network-only edge worker) cannot register at all.

## Goal

Relax all-or-nothing matching to a SUBSET match: a worker may
advertise a subset of the coord's capability set; the coord
routes only steps whose required-capability set is a subset of
the worker's advertised set. Backwards-compatible — workers that
omit the new field behave as before (full-fleet workers).

## Non-goals

- This is a **placement filter ONLY**, not a security gate. The
  capability gateway in `boruna-vm` remains the authority. A
  worker that lies about its advertised capabilities is still
  denied at execution time by the policy check inside the VM.
- Per-step capability negotiation. The coord uses the
  workflow-level `Policy` to derive a step's required cap set;
  finer-grained per-step requirements are out of scope.
- Live re-advertisement. Workers advertise once, at registration.
  To change the advertised set, re-register.

## Wire shape

Additive change to `RegisterRequest` (existing field unchanged):

```rust
pub struct RegisterRequest {
    pub worker_id: Option<String>,
    pub capability_set_hash: String,
    /// Sprint W3-A — capability NAMES (not hashes) drawn from
    /// boruna_bytecode::Capability::ALL. None = full fleet.
    pub advertised_capabilities: Option<Vec<String>>,
}
```

Pre-W3-A workers serialize without the field; coord deserializes
to `None` (`#[serde(default)]`). Existing tests pass unchanged.

## Capability validation at register

The coord validates every name in `advertised_capabilities`
against `Capability::ALL` exact canonical names:

| name | id |
|------|----|
| `actor.send` | 9 |
| `actor.spawn` | 8 |
| `db.query` | 3 |
| `fs.read` | 1 |
| `fs.write` | 2 |
| `llm.call` | 7 |
| `net.fetch` | 0 |
| `random` | 6 |
| `step.input` | 10 |
| `time.now` | 5 |
| `ui.render` | 4 |

Aliases accepted by `Capability::from_name` (e.g. `"net"`,
`"db"`) are NOT accepted on the wire — the taxonomy must be
unambiguous.

Unknown names cause registration to fail with the new stable
error_kind:

```json
{
  "protocol_version": 1,
  "error_kind": "coord.unknown_capability",
  "message": "advertised capability \"foo.bar\" is not a known capability name; expected names from boruna_bytecode::Capability::ALL"
}
```

HTTP status: `400 Bad Request`. This entry joins the locked
`coord.*` taxonomy (project §2: stable forever).

## Placement filter

`WorkerSession` carries `advertised_capabilities:
Option<BTreeSet<String>>`. `BTreeSet` gives O(log N) subset
checks and deterministic iteration order.

`handle_claim` captures the worker's advertised set at session
lookup time and threads it through `try_claim_one →
find_one_pending_step`. The latter, while iterating Pending
steps, calls `required_capabilities_from_policy` on each step's
`policy_json` and skips steps whose required set is NOT a subset
of the worker's advertised set.

Filtering is done in Rust at the application layer, NOT in SQL —
the policy JSON parsing is too gnarly for SQL and the claim is
already a one-step-at-a-time poll. Performance is fine.

### Required-cap derivation

A capability is "required" if either:

1. It appears in `policy.rules` with `allow: true`. (Explicit
   allow.)
2. `policy.default_allow == true`, in which case ALL capabilities
   in `Capability::ALL` are potentially required — only a
   full-fleet worker (or one that advertises every name) can
   claim such a step.

Malformed JSON falls back to "no requirements known", which
remains safe because the VM gateway re-checks at execution.

### Backwards compat

Workers with `advertised_capabilities = None` skip the filter
entirely — they see every step (the pre-W3-A behavior). Existing
test fixtures use `default_allow:true` policies, which combined
with `None` advertised caps keeps every existing test passing
without modification.

## Observable behavior

### CLI

New flag on `boruna worker run`:

```
--advertise-caps net.fetch,db.query
```

Comma-separated capability names. Empty / absent → `None`
(full fleet). The flag is parsed by `worker::parse_advertise_caps`
which trims whitespace, drops empty fragments, and returns
`None` for empty input.

### Wire

Workers POST `/api/workers/register` with the new optional
field. Coord responds `200 OK` on success, `409 Conflict` on
binary mismatch (existing), `400 Bad Request` with
`error_kind: "coord.unknown_capability"` on unknown names.

`/api/work/claim` is unchanged at the URL level; the filter is
applied transparently using session state.

## Security posture

This is operational state per project §15. It does NOT feed any
hash. It does NOT replace the policy gateway. Specifically:

- A malicious worker that advertises `["fs.write", "net.fetch"]`
  but does NOT actually have a network stack will be ROUTED
  steps requiring `net.fetch`. When it attempts the call, the
  VM's CapabilityGateway invokes the registered handler; if the
  handler can't reach the network, the step fails at execution
  with a normal runtime error. The capability gateway's policy
  gate is unchanged.
- A malicious worker that advertises a SMALLER set than it
  actually carries is allowed — that just makes it useless for
  steps it could otherwise have run, which hurts only the
  liar.
- The atomic-upgrade rule (binary-mismatch rejection on
  `capability_set_hash`) still holds. A worker MUST be built
  from a binary whose capability surface matches the coord's.
  Subset advertisement narrows the routing, not the surface.

## Testing

Six new tests in `crates/llmvm-cli/src/coordinator.rs`:

- `register_with_subset_advertised_caps_succeeds`
- `register_rejects_unknown_capability_name`
- `claim_returns_only_steps_within_advertised_caps`
- `claim_skips_step_requiring_unadvertised_capability`
- `worker_without_advertised_caps_sees_all_steps` (backwards
  compat)
- `claim_skips_incompatible_step_but_claims_compatible_sibling`
  (adversarial-review case per project §29)

Plus two unit tests in `crates/llmvm-cli/src/worker.rs` for the
`--advertise-caps` flag parser.

## Stable taxonomy delta

Added: `coord.unknown_capability` (400). Locked under project
§2 (taxonomy strings stable forever). Documented inline in this
file; no other canonical taxonomy doc exists today.
