-- v1 → v2 migration (sprint 0.3-S11): adds the `attempt_count` column
-- to `step_checkpoints` so the runner can record how many attempts each
-- step took. Existing rows default to 1 — a reasonable assumption since
-- prior code only persisted terminal state once per step (no mid-retry
-- writes), so any persisted Completed/Failed checkpoint represents a
-- single-attempt result from the operator's audit perspective.
--
-- SQLite ALTER TABLE ADD COLUMN with a constant DEFAULT is fast (no
-- table rewrite) and additive — the column appears at the end of the
-- table's storage order. Backfill is implicit via DEFAULT.
--
-- NOTE: this script runs INSIDE the migration transaction in init();
-- it must be idempotent OR guarded by the version check. The
-- conditional guard lives in mod.rs (`if v < 2 { ... }`), so this
-- file does NOT need to handle "column already exists" — by the time
-- it runs, the column genuinely doesn't exist on the target DB.

ALTER TABLE step_checkpoints ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 1;
