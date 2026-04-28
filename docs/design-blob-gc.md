# Blob GC sweep — design

Sprint `W3-B`. Closes the 0.5-S7 accepted limitation around manual
blob cleanup.

## Problem

Sprint `0.5-S7` introduced output blob references: step outputs whose
JSON encoding exceeds 64 KiB are stored in a content-addressed blob
store at `<data-dir>/blobs/<aa>/<full-hash>` and referenced by
`step_checkpoints.output_blob_ref` in `runs.db`.

The S7 retro flagged orphan accumulation: a step that gets re-attempted
under retry can land a blob, then have its checkpoint row replaced
(see `upsert_step_checkpoint` semantics — `output_blob_ref =
excluded.output_blob_ref` overrides the previous value when the new
attempt succeeds inline). The previous blob file remains on disk with
no referencing row. Same pattern applies to any future workflow that
deletes runs (e.g. `runs prune`, not yet shipped).

S7 retro called this an accepted ~1% leak rate with manual cleanup
via `rm -rf <data-dir>/blobs/`. That manual fix is unsafe — it
indiscriminately deletes referenced blobs too. This sprint ships the
safe primitive.

## Library APIs

Three additions, all in `boruna-orchestrator::persistence`:

```rust
impl BlobStore {
    pub fn find_orphans(
        &self,
        referenced: &BTreeSet<String>,
    ) -> Result<Vec<String>, BlobStoreError>;

    pub fn delete(&self, hash: &str) -> Result<(), BlobStoreError>;
}

impl RunCheckpointStore {
    pub fn all_referenced_blob_hashes(
        &self,
    ) -> Result<BTreeSet<String>, PersistenceError>;
}
```

`find_orphans` walks the blobs/ tree, filters non-shard-shaped files
and stray `.tmp.<pid>.<seq>` writer remnants, and returns the
on-disk-but-not-referenced set.

`delete` is **idempotent** for `NotFound` — a delete of a hash that was
already deleted (or never existed) returns `Ok(())`. This is the
correct shape for a GC sweep that may race with another deleter or a
manual `rm`. The shard directory `<root>/<aa>/` is best-effort pruned
when it ends up empty after the delete; concurrent writers placing a
sibling are race-tolerated by the `ENOTEMPTY`-or-success contract of
`fs::remove_dir`.

`all_referenced_blob_hashes` is a streaming read: rows are not
materialized into a `Vec` and then deduped — they're inserted directly
into a `BTreeSet`. Malformed `output_blob_ref` values (non-64-hex)
are logged-and-skipped per project convention §1 rather than
poisoning the reachability set.

## CLI

```
boruna evidence gc-blobs [--data-dir <path>] [--dry-run] [--json]
```

- Resolves the data-dir using the same fallback chain as `workflow run`
  / `workflow resume` (flag → `BORUNA_DATA_DIR` → `./.boruna/data`).
- Hard-fails with a clear error if `runs.db` is missing — without the
  reachability set we'd treat every blob as an orphan, which is the
  exact deletion-safety footgun this sprint exists to close.
- Reports `{deleted, skipped, bytes_freed, dry_run, blob_root,
  referenced_count, orphans_found, errors}`. Per-blob delete errors
  go to `errors` and count as `skipped`, not fatal — one corrupt
  blob doesn't block the rest of the sweep.
- `--json` emits the structured report to stdout. Default is
  human-readable.

## TOCTOU window

The orphan check has a classic read-then-delete race:

```text
T0: GC reads referenced-set R from runs.db
T1: a new run starts; coordinator writes a checkpoint row referencing
    blob B (which was orphan-at-T0)
T2: GC's find_orphans returns B (not in R)
T3: GC deletes B → the new run's checkpoint now references a missing
    blob → `read_step_output` errors with NotFound
```

**What this sprint does to the window:**

The sprint relies on the *existing* SQLite concurrency story, which is
narrower than a full exclusive write lock:

- `RunCheckpointStore::open` opens a `Connection` with WAL, a 5s
  `busy_timeout`, and `BEGIN IMMEDIATE` retry semantics on writes
  (see persistence module docs). It does **not** take a SQLite-level
  `EXCLUSIVE` lock for the lifetime of the connection.
- The `all_referenced_blob_hashes` read sees a snapshot consistent
  with WAL frame visibility at read time. New writes may commit
  immediately after.
- Therefore: **the GC can race with concurrent workers writing new
  checkpoints**. The window is small (referenced-set read happens in
  a single `prepare`/`query_map` pass), but it is not zero.

**Recommended operator usage:**

> Run `gc-blobs` during a maintenance window, or while no
> coordinators / workers are actively writing to the same data-dir.
> The deletion-safety story degrades gracefully — a deleted-then-rewritten
> blob is restored on the next `complete_step_cas` of the same content
> (SHA-256 collision resistance + the `BlobStore::write` idempotent
> shortcut), and a workflow that lands on an orphan-deleted hash fails
> with a clean `read_step_output → BlobStore::NotFound` error rather
> than corruption.

A future sprint may wire a periodic background sweep into the coord
process; that wiring would naturally hold the same lock disciplines as
the lease-expiry sweeper. Out of scope here — this sprint ships the
operator primitive.

## Determinism note

`find_orphans` returns hashes in `BTreeSet::difference` order
(ascending lexicographic), so two GC runs over the same on-disk
state produce identical reports. The CLI's report fields are also
deterministic given the same inputs.

## Tests

- `BlobStore::find_orphans` unit tests cover empty / all-referenced /
  partial-orphan / fresh-root-with-no-blobs-dir / stray-junk-skip
  cases.
- `BlobStore::delete` unit tests cover happy-path, idempotent-missing,
  bad-hash, shard-prune, and shard-keep-on-sibling cases.
- `RunCheckpointStore::all_referenced_blob_hashes` tests cover
  empty / blob-offloaded / inline-only / malformed-row-skip cases.
- Integration tests cover the dry-run-keeps-everything and
  actual-delete paths against a real on-disk blob store.
- A CLI integration test exercises the end-to-end `evidence gc-blobs`
  command against a temp data-dir.
