-- v2 → v3 migration (sprint 0.5-S2a): adds the lease/claim columns
-- to step_checkpoints. These columns power the distributed-execution
-- claim CAS state machine from ADR 002.
--
-- All three columns are OPERATIONAL ONLY per project convention #15
-- — never feed audit hashes, never order replay-relevant queries.
--
-- ALTER TABLE ADD COLUMN with a constant DEFAULT is fast in SQLite
-- (no table rewrite). worker_id and lease_expires_at default to NULL
-- which the application code reads as "no current lease". claim_id
-- defaults to 0 which the application reads as "never claimed";
-- claim_step always allocates claim_id >= 1.
--
-- NOTE: this script runs INSIDE the migration transaction in init();
-- it is guarded by a column-presence check so re-running on a fresh
-- DB (where SCHEMA_V1_SQL already laid down the columns) is a no-op.

ALTER TABLE step_checkpoints ADD COLUMN worker_id        TEXT;
ALTER TABLE step_checkpoints ADD COLUMN lease_expires_at INTEGER;
ALTER TABLE step_checkpoints ADD COLUMN claim_id         INTEGER NOT NULL DEFAULT 0;
