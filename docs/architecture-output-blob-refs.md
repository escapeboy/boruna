# Architecture — Output Blob References (Sprint 0.5-S7)

**Status:** Plan phase
**Companion docs:** `docs/design-output-blob-refs.md` (Think), `docs/test-plan-output-blob-refs.md` (Test plan)

## Overview

A content-addressed local blob store moves large step outputs out of the
SQLite `step_checkpoints` row and into a sharded directory under
`<data-dir>/<env>/blobs/`. The row carries an `output_blob_ref` that
points back to the file. The audit hash chain is unchanged — the ref
**is** the existing `output_hash`.

## Data flow

### Write path (worker / in-process completion)

```
Step completes → output_json: String (any size)
       │
       ▼
output_hash = sha256(output_json.as_bytes())
       │
       ▼
if output_json.len() > BLOB_THRESHOLD (64 KiB):
    BlobStore::write(blobs_root, &output_hash, output_json.as_bytes())
        ├─ open  <blobs_root>/<aa>/<hash>.tmp
        ├─ write_all + sync_all
        └─ rename  <hash>.tmp → <hash>
    persistence::complete_step_cas(... output_json=None,
                                       output_blob_ref=Some(hash) ...)
else:
    persistence::complete_step_cas(... output_json=Some(json),
                                       output_blob_ref=None ...)
```

The blob is written **before** the row update so a successful CAS implies
the blob is present on disk. A worker crash between write + CAS leaves
an orphan blob on disk; the CAS re-attempt on retry rewrites the same
hash idempotently (same content → same path), so the orphan is
harmless. Documented limitation: orphan blobs accumulate at the
sub-1% level and require manual cleanup via `rm -rf
<data-dir>/<env>/blobs/`.

### Read path (resume, replay, dashboard, step_input builtin)

A new accessor `RunCheckpointStore::read_step_output(run_id, step_id) ->
Result<Option<String>, PersistenceError>` resolves either source:

```
let cp = self.get_step_checkpoint(run_id, step_id)?;
match (cp.output_json, cp.output_blob_ref) {
    (Some(json), None)  => Ok(Some(json)),
    (None, Some(hash))  => self.blob_store.read(&hash).map(Some),
    (None, None)        => Ok(None),  // step not yet completed
    (Some(_), Some(_))  => Err(PersistenceError::Inconsistent(...)),
}
```

The mutual-exclusion invariant is enforced by a SQL CHECK constraint on
the migration (see Schema below). All existing call sites that read
`cp.output_json` directly must switch to this accessor — there are 2
hot spots (`workflow/runner.rs:1640` resume, `workflow/data_flow.rs`
output piping); other reads are in test fixtures and will keep using
the inline column.

### Distributed-mode HTTP path

Worker → coord on completion: existing `POST /api/work/complete`
unchanged. Worker has the `output_json` bytes locally; sends them
inline. Coordinator receives, decides on size threshold, writes blob if
needed, persists via `complete_step_cas`. The 8 MiB body limit on
coord routes (set in `build_router`) caps the maximum step output
size; that cap is unchanged. (Increasing the body limit is a separate
sprint with security implications.)

CI runner → coord blob fetch:

```
GET /api/runs/{run_id}/blobs/{hash}
Authorization: Bearer <token>
→ 200 OK   Content-Type: application/octet-stream  (the bytes)
→ 401      coord.unauthorized
→ 400      coord.blobs.bad_hash       (hash not 64 hex chars)
→ 404      coord.blobs.not_found      (no blob with that hash for this run)
```

The route is **run-scoped** (path includes `run_id`) for two reasons:
(a) it gives the auth check a natural ownership boundary, and (b) it
sets up future cross-run dedup as an explicit additive change (a new
`/api/blobs/{hash}` route with different access control). The handler
cross-checks that the run actually has a checkpoint referencing this
hash before returning bytes — this prevents using the route as a
generic blob server.

### `RunStatusResponse` shape

The response from `GET /api/runs/{run_id}/status` is unchanged in
shape. Per-step status mapping continues to return `String` status
strings only — it never carried output bytes. Operators that want
the actual output of a completed step take the existing two-step path:
read step status from `/status`, then if interested fetch via
`/blobs/{hash}` (the hash is exposed via a new optional
`step_output_hashes: BTreeMap<String, String>` field, additive,
non-breaking — does not bump `protocol_version`).

## Schema migration v3 → v4

`orchestrator/src/persistence/schema_v3_to_v4.sql`:

```sql
-- v3 → v4 migration (sprint 0.5-S7): adds the output_blob_ref column
-- to step_checkpoints. Powers content-addressed offloading of large
-- step outputs.
--
-- output_blob_ref is REPLAY-VERIFIED — it equals the existing
-- output_hash whenever set, by construction in complete_step_cas.
-- Replay reads the bytes via either the inline output_json or by
-- resolving the ref through the blob store, then re-hashes for
-- comparison.
--
-- ALTER TABLE ADD COLUMN with a constant DEFAULT (NULL) is fast in
-- SQLite — no table rewrite. Existing v3 rows have output_blob_ref
-- NULL meaning "inline".
--
-- NOTE: this script runs INSIDE the migration transaction in init();
-- guarded by a column-presence check so re-running on a fresh DB is a no-op.

ALTER TABLE step_checkpoints ADD COLUMN output_blob_ref TEXT;
```

A SQL CHECK enforcing the mutual exclusion (`output_json IS NULL OR
output_blob_ref IS NULL`) cannot be added via `ALTER TABLE` in SQLite,
so the invariant is enforced at the application layer in
`complete_step_cas` and the read accessor. A defensive consistency
check on read returns `PersistenceError::Inconsistent` if both
columns are set.

The application reads through the existing `parse_step_checkpoint`
helper, which gains an `output_blob_ref: Option<String>` field.
Existing pre-v4 databases get the column via the migration; the
column-presence check pattern is reused (per `column_exists` helper).

## Blob store layout

```
<data-dir>/<env>/blobs/
├── 00/
│   └── 00aa11bb22cc...
├── 01/
│   └── 0123456789ab...
├── ...
└── ff/
```

- Sharded by first two hex chars (256 buckets) — keeps `ls` reasonable
  on dirs with millions of blobs.
- Filename is the full 64-char SHA-256 hex (no extension).
- One blob is one UTF-8-encoded `output_json` string; reading the
  file as bytes and re-encoding as a Rust `String` round-trips
  losslessly because the bytes are already valid UTF-8 (it was a
  `String` before serialization).

API surface (`orchestrator/src/persistence/blob_store.rs`):

```rust
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    pub fn open(root: PathBuf) -> io::Result<Self> { ... }

    /// Write `bytes` to `<root>/<aa>/<hash>` atomically via tempfile +
    /// rename. Caller passes the precomputed hash; this function does
    /// NOT hash the bytes (avoids a redundant pass — caller already
    /// has it for the audit chain).
    pub fn write(&self, hash: &str, bytes: &[u8]) -> Result<(), BlobStoreError>;

    /// Read `<root>/<aa>/<hash>` as a UTF-8 string. Returns
    /// BlobStoreError::NotFound if absent.
    pub fn read_string(&self, hash: &str) -> Result<String, BlobStoreError>;

    /// Read raw bytes. Used by the coord HTTP route which streams
    /// octet-stream and doesn't need to validate UTF-8.
    pub fn read_bytes(&self, hash: &str) -> Result<Vec<u8>, BlobStoreError>;

    /// Existence check without read. Used by the dashboard's "view
    /// blob" affordance to render a placeholder when the blob is
    /// missing (cleaned up out-of-band).
    pub fn exists(&self, hash: &str) -> bool;
}

pub enum BlobStoreError {
    BadHash,              // not 64 hex chars
    NotFound,
    NotUtf8,              // read_string only; bytes aren't valid UTF-8
    Io(io::Error),
}
```

Hash validation reuses the existing hex-only pattern: `len() == 64 &&
all are 0-9a-f`. Anything else returns `BadHash` BEFORE touching the
filesystem (path-traversal defense, per project precedent in
`LlmCache` and `ContextStore`).

## Threshold

Constant `BLOB_THRESHOLD: usize = 64 * 1024;` defined in
`orchestrator/src/persistence/mod.rs`. Hard-coded for the sprint, not
exposed via config or `Policy`. Justification in the design doc.
A single `pub` constant means tests can import it, and a future
sprint that introduces configurability has a single migration point.

## Determinism contract (column annotations)

Per project convention §15, the new column is documented in **4
places**:

1. **Struct doc on `StepCheckpoint`**: REPLAY-VERIFIED, equals the
   existing `output_hash` whenever set.
2. **Field doc on `output_blob_ref`**: same, with explicit reference
   to the mutual-exclusion invariant.
3. **Schema SQL comment** in `schema_v3_to_v4.sql` and updated
   header in `schema_v1.sql` (forward-included for fresh databases).
4. **`docs/reference/cli.md` or `docs/reference/persistence.md`**:
   table of replay-verified vs. operational-only columns gets a new
   row.

## Audit + evidence bundle path

The `audit::log` module's `StepCompleted` event already carries
`output_hash: String` (orchestrator/src/audit/log.rs:26). It does
NOT carry `output_json`. The hash chain therefore needs **zero
changes** — the event is the same shape with the same hash. ✓

`evidence::EvidenceBundleBuilder` writes a per-run directory with
`metadata.json`, `audit_log.json`, and the workflow's persisted
checkpoints. With S7, large outputs in the persisted checkpoints
are stored as refs. Two paths:

1. **Embed** — bundle includes the resolved bytes as inline strings
   in `metadata.json`. Simplest. Bundle size grows.
2. **Sidecar** — bundle includes a `blobs/` subdir alongside
   `metadata.json`. Each blob ref in the metadata still points by
   hash; verifier resolves bundle-local then falls back to the
   data-dir blob store.

S7 ships **option 2** (sidecar), specifically because git-diffing
or emailing a 5 MB `metadata.json` is the exact pain we set out to
fix. The verifier (`verify::verify_bundle`) gains a parameter for
the sidecar dir; pre-S7 bundles without `blobs/` continue to verify
via the inline path.

The bundle format gains an additive `blobs/` subdirectory; this
does NOT bump `format_version` because old verifiers ignore the
unknown directory and old bundles still verify against the
upgraded verifier (it falls back to inline). Documented in
`docs/concepts/evidence-bundles.md`.

## Components touched

| Component | File | Change |
|-----------|------|--------|
| Persistence schema | `orchestrator/src/persistence/schema_v3_to_v4.sql` (new) | Add column |
| Persistence schema | `orchestrator/src/persistence/schema_v1.sql` | Forward-add column for fresh DBs |
| Persistence | `orchestrator/src/persistence/mod.rs` | `BLOB_THRESHOLD`, accessor, CAS update, `parse_step_checkpoint` field, migration runner |
| Blob store | `orchestrator/src/persistence/blob_store.rs` (new) | Full file |
| Workflow runner | `orchestrator/src/workflow/runner.rs:1640` (resume read) | Switch to accessor |
| Workflow data flow | `orchestrator/src/workflow/data_flow.rs` | Step_input reader uses accessor |
| Workflow runner | `orchestrator/src/workflow/runner.rs` (in-process complete path) | Threshold gate + blob write |
| Coord HTTP | `crates/llmvm-cli/src/coordinator.rs::handle_complete` | Threshold gate + blob write at completion |
| Coord HTTP | `crates/llmvm-cli/src/coordinator.rs` | New `handle_get_blob` handler + route |
| Coord HTTP | `crates/llmvm-cli/src/coordinator.rs::RunStatusResponse` | Add `step_output_hashes: BTreeMap<String, String>` (additive, optional) |
| Wait driver | `orchestrator/src/workflow/runner.rs::advance_run_one_tick` | Read large outputs via accessor when consulting `step_input` chain |
| Dashboard | `crates/llmvm-cli/src/dashboard.rs` (per-run detail) | Render `[stored as blob: <hash>]` link to coord blob route |
| Audit / evidence | `orchestrator/src/audit/evidence.rs` | Sidecar `blobs/` write |
| Audit / evidence | `orchestrator/src/audit/verify.rs` | Sidecar fallback resolution |
| Docs | `docs/reference/mcp-server.md` | New `coord.blobs.*` taxonomy strings |
| Docs | `docs/concepts/evidence-bundles.md` | Sidecar layout, backward compat |
| Docs | `docs/limitations.md` | Orphan blobs, GC policy, cross-coord HA |
| Changelog | `CHANGELOG.md` | `[Unreleased] ### Added` |

## Error_kind taxonomy (locked at first ship)

Per project convention §2, these strings are stable forever:

- `coord.blobs.not_found` — 404 — the run has no checkpoint
  referencing the requested hash, or the blob file is missing on disk.
- `coord.blobs.bad_hash` — 400 — the hash path segment is not 64
  lowercase hex chars.

Auth failures continue to return `coord.unauthorized` (existing).
Body-limit failures continue to return Axum's `413` for the
`POST /api/work/complete` route as before.

## Build sequence

1. Schema migration + `BlobStore` module (no dependents yet).
2. Persistence accessor (`read_step_output`) + `complete_step_cas`
   threshold logic.
3. Workflow runner resume path uses accessor.
4. Workflow data flow `step_input` resolver uses accessor.
5. In-process completion path (workflow runner) writes blobs.
6. Coord HTTP `handle_complete` threshold gate.
7. Coord HTTP `handle_get_blob` route.
8. Coord HTTP `RunStatusResponse.step_output_hashes` field.
9. Dashboard placeholder render.
10. Audit evidence sidecar write + verify fallback.
11. Docs + CHANGELOG + limitations.
12. Tests across the above (see test plan).

This sequence keeps each step independently testable and avoids a
big-bang merge — each step compiles cleanly without the next.

## Out of scope (re-stated from design)

- Streaming, GC, dedup, cross-run refs, threshold configurability.
- HA-aware cross-coord blob resolution (0.6.x territory).
- Compression (CPU cost vs disk savings unclear without benchmarks
  on real LLM-output corpora).
