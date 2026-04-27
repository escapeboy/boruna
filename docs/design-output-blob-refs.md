# Output Blob References — Sprint 0.5-S7

**Status:** Design (Think phase)
**Sprint:** `0.5-S7`
**Branch:** `sprint/0.5-s7-output-blob-refs`

## Forcing questions

### Who needs this? What are they doing today?

Two integrators hit the same pain:

1. **FleetQ-style users running LLM steps** — a single `llm.complete` step
   returning a 200 KB JSON response. Today that 200 KB sits inline in
   `step_checkpoints.output_json`, gets re-read by `advance_run_one_tick`,
   re-serialized into the wait-driver's `metadata` snapshot, copied into the
   coord HTTP `/api/runs/{id}/status` response, and re-written by the
   `step_input` builtin into the next step's source. The bytes traverse the
   tape ~5 times per workflow tick. At 200 KB this is annoying; at 5 MB it
   pegs CPU on the coordinator and drives the SQLite database past 1 GB
   inside a week of light traffic.

2. **Compliance / replay users** want to audit a workflow that produced a
   2 MB legal-review output. Right now the evidence bundle dumps the
   `metadata.json` with that 2 MB inline, and `evidence verify` re-reads
   the whole thing per step. Nothing technically broken — but the bundle
   becomes hard to git-diff, hard to email, and `metadata.json` stops
   being human-readable.

The workaround today is **truncation** in user-space — the workflow author
adds a step that summarises before the next stage. That works but means
the original output isn't preserved in the audit trail, which defeats the
point of the platform.

### What's the narrowest MVP someone would pay for?

A single threshold (`64 KiB`, configurable later) at which point the
`output_json` value is moved out of `step_checkpoints` into a content-
addressed blob store on disk, with a `step_checkpoints.output_blob_ref`
column carrying the `sha256` hash. A reader API
(`read_step_output(run_id, step_id) -> Option<String>`) transparently
resolves either source. Coord HTTP exposes
`GET /api/runs/{run_id}/blobs/{hash}` (bearer-gated, identical to the
existing auth surface) so distributed-mode workers and CI clients can
fetch the bytes when they need to.

That's it. **No streaming, no chunking, no garbage collection, no
deduplication across runs.** Each blob is a single UTF-8 file at
`<data-dir>/<env>/blobs/<aa>/<bbbb...>` (first two hex chars of the hash
shard the directory). Refcounting and GC are explicit non-goals — disk is
cheap, SHA-256 collisions don't happen, and the blobs survive run
deletion harmlessly until an operator manually clears the dir.

### What would make someone say "whoa"?

Two things, in order:

1. **Audit hashes don't change.** A workflow that produced a 200 KB
   inline output yesterday and the same 200 KB blob-ref-stored output
   today produces *the same* `output_hash` — because the hash was always
   over the bytes, not the row layout. The blob ref **IS** the hash.
   The whole hash chain Just Works without a single re-derivation. The
   only schema-version-keyed thing changing is the *storage location*,
   not the content contract.

2. **`evidence verify` still works on bundles produced before the sprint.**
   Pre-S7 bundles have inline `output_json` and the verifier re-hashes
   that. Post-S7 bundles include the blob bytes alongside `metadata.json`
   and the verifier re-hashes the resolved string. Same code path, same
   hash, both bundle shapes verifiable. No format version bump on the
   evidence bundle — only an additive `blobs/` subdirectory when the run
   used any.

### How does this compound over time?

- **Unblocks the LLM router** to handle real production traffic without
  cardinality concerns on the coord SQLite. Today nobody runs a workflow
  that emits more than ~50 KB outputs because it gets sluggish; after
  S7, that ceiling moves up by ~3 orders of magnitude.
- **Sets up `boruna evidence diff` (planned 0.2.x)** because once the
  bundle has a `blobs/` directory, a structural diff is `diff -r
  bundle-a/ bundle-b/` and the tool just compares hashes.
- **Removes the silent truncation pressure** on workflow authors. Without
  S7, sprint authors are doing dimensionality reduction in their workflow
  graph (LLM → summariser → next-step) purely to keep the audit trail
  small. With S7, they pipe full outputs and the platform handles the
  storage delta.

### What's intentionally out of scope?

Stated up front to keep the sprint small:

- **No streaming write.** The blob is written when the step completes,
  fully formed, single `write_all` call. Streaming output during step
  execution is a separate (much larger) sprint.
- **No GC / refcounting.** Blob files persist past run deletion. Operator
  can `rm -rf <data-dir>/<env>/blobs/` to reclaim disk; documented in
  the limitations doc.
- **No deduplication across the cluster.** Each coord/worker is the
  source of truth for the blobs it stored; cross-node fetch goes
  through the coord HTTP route.
- **No cross-run reference.** A blob ref is conceptually scoped to a
  single run for HTTP routing purposes (`/api/runs/{run_id}/blobs/...`).
  We could later promote to a global `/api/blobs/{hash}` once GC
  policy is decided, but starting run-scoped keeps the auth and
  routing trivial.
- **No size-limit-exceeded contract change.** The existing
  `output_bytes` policy limit fires as before — a step that produces
  10 MB still gets `error_kind: "limit_exceeded", phase:
  "serialization"` if its policy says so. The blob path exists for the
  range *between* "we'd rather not inline this" and "we explicitly
  refuse this", which today is everything ≥ 64 KiB and ≤ the policy
  ceiling.

### Threshold rationale

Why 64 KiB?

- `output_json` of < 16 KiB is essentially-free in SQLite (single page).
- 16–64 KiB starts crossing page boundaries; still cheap.
- > 64 KiB the row crosses multiple pages, the row size starts to bias
  query planner decisions on `step_checkpoints` queries, and we're at
  the size where copying it into `metadata.json` snapshots becomes a
  visible memory bump per tick.
- 64 KiB is also the SQLite default `cache_size` boundary in many
  default deployments — keeps the hot path on cached pages.

Configurable post-MVP via an env var or `Policy` field if a user has
a different operating point in mind. **Hard-coded for the sprint** to
avoid the configuration-surface footgun (per project convention §1:
reject at parse, don't silently override).

## Constraints anchored before Plan phase

1. **Schema migration v3 → v4.** Add `output_blob_ref TEXT NULL` to
   `step_checkpoints` via `ALTER TABLE ADD COLUMN`. Constant DEFAULT
   means no table rewrite. Migration goes in `schema_v3_to_v4.sql` and
   `init()` follows the existing column-presence-check pattern.

2. **Determinism: blob ref is REPLAY-VERIFIED.** It's the SHA-256 of
   the UTF-8 bytes of `output_json` — same as `output_hash` already.
   Critically: a row can have **either** `output_json` set (small
   outputs) **or** `output_blob_ref` set (large outputs) but the
   `output_hash` is always populated and is always the same hash.
   Replay reads by hash — doesn't care which physical row column
   carries the bytes.

3. **Atomic write.** Blob file is written via `tempfile + rename` to
   avoid half-written files in the case of a crash mid-write. The
   row is updated **after** the rename succeeds (so a present
   `output_blob_ref` always points to a complete file).

4. **Path-traversal hardening.** Blob filename is the hex-only
   `output_hash` — already validated by the project's hex-only
   accessor pattern (per existing `LlmCache` and `ContextStore`
   precedent). Reject anything else with `coord.blobs.bad_hash`.

5. **Small-output path is unchanged.** Outputs < 64 KiB still go
   inline as today. Zero behavior change for existing users.
   `read_step_output` checks the column with the value (mutually
   exclusive) and dispatches.

6. **Distributed-mode flow.** Worker completes a large step → POST
   `/api/runs/{id}/complete` body still carries inline JSON (worker
   has the bytes; we don't add streaming). Coord receives, writes
   the blob locally, persists the ref. CI client wanting to read
   the output calls `GET /api/runs/{id}/status` (returns ref) then
   `GET /api/runs/{id}/blobs/{hash}` to fetch bytes. **Two-RPC
   read.** Acceptable; the alternative (inline-large in status JSON)
   defeats the point.

7. **Auth: `coord.blobs.unauthorized` reuses the existing
   `auth_middleware` 401.** No new error_kind for the auth case;
   only `coord.blobs.not_found` and `coord.blobs.bad_hash` are
   new. (Per convention §2 we lock these strings forever.)

## Risks identified up front

- **Race between blob write and worker crash.** If the worker writes
  the blob, then crashes before persisting `output_blob_ref` in the
  CAS update — the next worker reattempt produces a new run with the
  same content (and same hash, by determinism). Re-write of the same
  file at the same path is idempotent (same content). No leak; no
  divergence.
- **Coord crash during upload.** The blob write happens BEFORE the
  CAS row update; the row update is the commit point. If coord
  crashes between write and CAS, the orphan blob lives on disk; not
  referenced; harmless (covered by no-GC policy above; doc limitation).
- **Cross-coord ref drift in HA mode (future).** Out of scope for
  S7 because HA isn't here yet; flag in the architecture doc as a
  known constraint that HA will need to address.

## Acceptance criteria (hand-off to Plan phase)

A successful S7 produces:

- A new persistence column + migration, no audit-hash change.
- A reader API that resolves either inline or blob-stored outputs
  transparently.
- A blob writer triggered above the 64 KiB threshold.
- A coord HTTP route that serves blobs with bearer auth.
- Worker-side `step_input` builtin behavior unchanged (it reads
  through the same accessor).
- Dashboard renders large outputs by showing a `[stored as blob:
  abc123…]` indicator with a link to the blob HTTP route.
- New error_kind taxonomy locked: `coord.blobs.not_found`,
  `coord.blobs.bad_hash`. Documented in
  `docs/reference/mcp-server.md` and the coord taxonomy section.
- 12+ new unit tests across persistence, coord, and the wait driver.
- Convention §15 column annotation in 4 places (struct doc, field
  doc, schema SQL comment, docs/reference table).
- Limitations doc updated: GC policy, cross-coord drift, no
  streaming.
- CHANGELOG `[Unreleased]` entry.
