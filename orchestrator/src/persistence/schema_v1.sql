-- Boruna persistent workflow checkpoint schema, v1.
-- Applied on every connection via execute_batch with IF NOT EXISTS so re-open
-- is idempotent. See docs/design-persistence-store.md and ADR 001.
--
-- Determinism contract column annotations:
--   REPLAY-VERIFIED — the column's value participates in audit hashes /
--                     replay comparison. MUST be identical across replays.
--   OPERATIONAL ONLY — the column carries operational metadata only.
--                      MUST NOT feed an audit hash; MUST NOT order replay
--                      queries.

-- Single-row table holding the schema version. The CHECK constraint pins
-- the only allowed primary key to 1, so the table is structurally
-- single-row. The version column itself can be any integer; reading it
-- (vs. inserting it) is what the migration runner uses.
CREATE TABLE IF NOT EXISTS schema_version (
    id      INTEGER PRIMARY KEY CHECK (id = 1),
    version INTEGER NOT NULL
);

-- INSERT OR IGNORE means: on a fresh DB this writes (1, 1); on an existing
-- DB it's a no-op (the (1, ...) row already exists). The migration check
-- in init() then reads `version` and either accepts or rejects.
INSERT OR IGNORE INTO schema_version (id, version) VALUES (1, 1);

CREATE TABLE IF NOT EXISTS runs (
    run_id        TEXT PRIMARY KEY,
    workflow_name TEXT NOT NULL,
    workflow_hash TEXT NOT NULL,                  -- REPLAY-VERIFIED
    status        TEXT NOT NULL,                  -- terminal values feed replay
    started_at    INTEGER NOT NULL,               -- OPERATIONAL ONLY (unix ms)
    updated_at    INTEGER NOT NULL,               -- OPERATIONAL ONLY (unix ms)
    policy_json   TEXT NOT NULL,                  -- REPLAY-VERIFIED
    metadata_json TEXT NOT NULL DEFAULT '{}'      -- caller-defined
);

CREATE TABLE IF NOT EXISTS step_checkpoints (
    run_id      TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    step_id     TEXT NOT NULL,
    status      TEXT NOT NULL,                    -- terminal values feed replay
    output_json TEXT,                             -- REPLAY-VERIFIED
    output_hash TEXT,                             -- REPLAY-VERIFIED
    started_at  INTEGER,                          -- OPERATIONAL ONLY
    ended_at    INTEGER,                          -- OPERATIONAL ONLY
    error_msg   TEXT,                             -- OPERATIONAL ONLY
    PRIMARY KEY (run_id, step_id)
);

CREATE INDEX IF NOT EXISTS idx_runs_status ON runs(status);

-- Partial index for "what's blocked / running across all runs?" queries the
-- dashboard (0.4-S7) and the scheduler (0.3-S7) will need. Keeps the index
-- small (most rows have terminal status and don't need a global view).
CREATE INDEX IF NOT EXISTS idx_step_checkpoints_active
    ON step_checkpoints(status)
    WHERE status IN ('awaiting_approval', 'running');
