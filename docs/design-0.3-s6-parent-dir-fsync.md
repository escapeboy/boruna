# Design — 0.3-S6: Parent-Directory fsync

**Status:** 2026-04-26
**Predecessor:** `0.3-S3` shipped atomic-rename for `DataStore::store_output` but explicitly documented the parent-directory fsync gap as "process-crash safe, NOT power-loss safe." This sprint closes that gap.

## Scope

After `tempfile::NamedTempFile::persist` returns, open the parent directory and `fsync` it so the rename's directory entry is journaled. Without this, on POSIX a crash between `persist()` returning and the dirent being flushed can leave the file referencing the old inode (or absent) even though `store_output` returned `Ok(())`.

**In scope:**

1. `DataStore::store_output` opens the parent dir after `persist` and calls `File::sync_all` (which on POSIX issues `fsync(2)` on the directory inode).
2. Update the method docstring honestly: "process-crash safe AND power-loss safe (POSIX local filesystems)."
3. Best-effort integration test that exercises the new code path (full crash-safety verification needs hardware-level support and isn't easily testable; we verify the calls happen and that the existing atomicity test still passes).
4. Update `CHANGELOG.md` `### Fixed` to mark the H1/C3 deferral closed.

**Out of scope:**

- fsync of the data_dir itself (parent of `runs/` and `runs.db`). The orchestrator creates that dir via `std::fs::create_dir_all` once at run start; SQLite's WAL journal handles its own durability. We're closing the gap in `store_output` specifically.
- Cross-process / NFS durability semantics. POSIX local FS only.
- Windows. Not a production target.

## Forcing questions (Think)

**Who needs this?** Operators running on real hardware where power loss / kernel panic is a real failure mode (production deployments, especially on edge / branch hardware without UPS). Today: `store_output` returning Ok doesn't guarantee the file survives a power cut.

**Narrowest MVP?** A workflow that runs to completion, then loses power, then recovers — and `boruna workflow show <run-id>` lists the right `output_hash` per step from disk. Today this can break for the most-recently-completed step.

**Compounds?** Closes the durability story for the persistence chassis. Future sprints (multi-process flock, distributed) won't have to re-derive what "Ok(())" means.

## Implementation

```rust
// In DataStore::store_output, after `tmp.persist(&target)`:
//
// Open the parent directory and fsync it. This makes the rename's
// directory entry durable — without it, POSIX permits the dirent to
// be lost on power loss even though the file's data blocks were
// flushed via tmp.as_file().sync_all().
let dir_handle = std::fs::File::open(&dir)?;
dir_handle.sync_all()?;
```

Three subtleties:
1. **Windows**: `File::sync_all` on a directory fails on Windows with `ERROR_INVALID_HANDLE`. Wrap in `cfg(unix)` so non-POSIX builds compile.
2. **NFS / fuse / network FS**: behavior varies. We document POSIX local FS as the supported case.
3. **Performance**: an extra fsync per `store_output` call. Each fsync is ~10ms on spinning disks, microseconds on SSDs. For the typical workflow (a handful of steps), the cost is negligible.

## Determinism contract

No change. The fsync is operational only — doesn't feed any hash. The on-disk bytes are unchanged.

## Acceptance criteria

- `cargo test --workspace` green including a new test that exercises the new fsync path (verifying it doesn't break atomicity).
- `cargo clippy -D warnings` clean.
- `cargo fmt --check` clean.
- `DataStore::store_output` docstring updated to remove the "NOT power-loss safe" disclaimer.
- `CHANGELOG.md` `### Fixed` notes the H1/C3 deferral closed.
