# Worker capability tagging — operator guide

Sprint `W3-A` (0.5.0). Lets you run a heterogeneous fleet of
Boruna workers where each worker advertises a SUBSET of the
coordinator's capability set.

Use this when:

- A subset of your workers cannot speak certain capabilities
  (e.g. an edge worker has no DB connection, a sandboxed runner
  has no filesystem write). Today these workers can't even
  register because `coord.binary_mismatch` rejects them.
- You want network-bound work routed to network-capable nodes
  and DB-bound work routed to DB-capable nodes, without manually
  splitting workflows.

## Quick start

Start the coord as usual:

```sh
boruna coordinator serve --data-dir /var/lib/boruna --port 8090
```

Start a worker that ONLY handles `net.fetch` and `db.query`:

```sh
boruna worker run \
    --coordinator http://coord:8090 \
    --advertise-caps net.fetch,db.query
```

Start a second worker as a full-fleet worker (the default —
omit the flag):

```sh
boruna worker run --coordinator http://coord:8090
```

The coord routes:

- Steps whose policy requires only `net.fetch` and/or `db.query`
  → either worker (the more selective one is eligible).
- Steps whose policy requires e.g. `fs.write` → ONLY the
  full-fleet worker.

## Flag reference

```
--advertise-caps <COMMA_LIST>
```

Comma-separated capability names from
`boruna_bytecode::Capability::ALL`:

```
net.fetch, fs.read, fs.write, db.query, ui.render,
time.now, random, llm.call, actor.spawn, actor.send, step.input
```

Whitespace around each name is trimmed. Empty input or absent
flag → full-fleet behavior.

Aliases accepted in the local CLI policy parser (`"net"`,
`"db"`, `"ui"`, `"time"`, `"llm"`) are NOT accepted on the
wire — the wire taxonomy uses canonical names only.

## Errors

### `coord.unknown_capability` (400)

The worker tried to register with a capability name not in
`Capability::ALL`. Most common cause: typo (`net.fetc`).

```sh
$ boruna worker run --coordinator http://coord:8090 \
    --advertise-caps net.fetc,db.query
register 400 Bad Request: coord.unknown_capability \
    (advertised capability "net.fetc" is not a known capability name; \
     expected names from boruna_bytecode::Capability::ALL)
```

Fix the typo and re-run.

### `coord.binary_mismatch` (409)

UNCHANGED behavior from before W3-A. The atomic-upgrade rule
still applies — a worker MUST be built from a binary whose
`capability_set_hash` matches the coord's. Capability tagging
does NOT relax this; it only narrows the routing of an already-
matching binary.

## What is NOT a security gate

Read carefully. Capability tagging is a **placement filter
ONLY**.

- A worker that lies (advertises capabilities it can't
  actually fulfill) WILL be routed steps it can't run. When
  the step executes, the VM's `CapabilityGateway` invokes the
  registered handler; if the handler can't reach the network
  (or DB, or filesystem), the step fails with a normal runtime
  error and the orchestrator handles the failure per the
  workflow's retry policy.
- The capability gateway in `boruna-vm` is the security
  boundary. Capability tagging does NOT bypass, replace, or
  weaken it.
- Tagging is OPERATIONAL state — it does not feed any hash, is
  not part of the evidence bundle, and is not replay-verified.

If you need a security gate (deny `fs.write` on this worker
even if the policy allows it), use the policy mechanism, not
capability tagging.

## Choosing what to advertise

A pragmatic mapping:

| Worker role | Suggested `--advertise-caps` |
|-------------|------------------------------|
| Edge / network-only | `net.fetch,llm.call,step.input` |
| DB-bound | `db.query,step.input` |
| Local CI runner | `fs.read,fs.write,step.input` |
| Generalist | (omit flag — full fleet) |

`step.input` is almost always required because most workflow
steps consume upstream outputs through the
`step_input(name) -> String` builtin.

## Backwards compatibility

Pre-W3-A workers (binary `<` W3-A) serialize `RegisterRequest`
without the `advertised_capabilities` field; the W3-A coord
deserializes the missing field to `None` (full fleet). No
operator action is required when upgrading the coord ahead of
the workers.

W3-A workers connecting to a pre-W3-A coord serialize the new
field, and a pre-W3-A coord ignores unknown JSON fields by
default. The pre-W3-A coord will treat the worker as a full-
fleet worker (no filter applied) — degraded behavior, but
graceful. Upgrade the coord first when rolling out W3-A.

## Internals (for the curious)

The coord derives a step's required capability set from the
workflow's serialized `Policy` JSON:

- Capabilities listed in `policy.rules` with `allow: true` are
  required.
- If `policy.default_allow == true`, ALL capabilities are
  potentially required (only a full-fleet worker is eligible).
- Malformed JSON → "no requirements known" → step is eligible
  for any worker (the VM gateway re-checks at execution).

The check is `required.is_subset(advertised)`. Fine-grained
per-step requirement metadata is out of scope for W3-A.
