# Test Plan — Output Blob References (Sprint 0.5-S7)

**Companion docs:** `docs/design-output-blob-refs.md`,
`docs/architecture-output-blob-refs.md`.

## Acceptance criteria (gates)

- All workspace tests green: `cargo test --workspace --features
  boruna-cli/serve`.
- Clippy clean: `cargo clippy --workspace --features boruna-cli/serve
  -- -D warnings`.
- Format clean: `cargo fmt --all -- --check`.
- All new tests below pass.
- No existing test regresses (i.e. small-output workflows still
  store inline; existing audit hashes unchanged).

## Unit tests — `BlobStore` (new file `blob_store.rs`)

| # | Name | Asserts |
|---|------|---------|
| 1 | `write_then_read_string_roundtrip` | Write known bytes; read_string returns identical String |
| 2 | `write_then_read_bytes_roundtrip` | Same with raw bytes |
| 3 | `bad_hash_rejected_on_write` | `write("not-hex")` → BadHash; no file created |
| 4 | `bad_hash_rejected_on_read` | `read_string("../etc/passwd")` → BadHash; no file accessed |
| 5 | `bad_hash_traversal_dots` | `write("../etc/whatever")` → BadHash |
| 6 | `bad_hash_uppercase` | hash with uppercase chars rejected (consistency with hex_only) |
| 7 | `bad_hash_short` | 63 chars rejected |
| 8 | `bad_hash_long` | 65 chars rejected |
| 9 | `not_found_returns_typed_error` | `read_string("aaaa..."  64 a's)` on empty store → NotFound |
| 10 | `idempotent_rewrite_same_hash` | Write same hash twice with same content; no error, single file |
| 11 | `atomic_via_tempfile_rename` | Inject I/O failure mid-write; verify final path absent (no half-file) |
| 12 | `exists_returns_true_after_write` | Write then `exists` → true |
| 13 | `exists_returns_false_for_missing` | Empty store, valid hash → false |
| 14 | `not_utf8_returns_typed_error` | Write raw non-UTF-8 bytes; read_string → NotUtf8 |
| 15 | `shard_directory_created_on_demand` | Write to `<root>/aa/...`; aa/ created with 0o700 perms |

## Unit tests — persistence (`mod.rs`)

| # | Name | Asserts |
|---|------|---------|
| 16 | `complete_step_cas_inline_below_threshold` | 32 KiB output → row.output_json populated, output_blob_ref NULL |
| 17 | `complete_step_cas_blob_above_threshold` | 100 KiB output → row.output_json NULL, output_blob_ref = output_hash, blob present on disk |
| 18 | `complete_step_cas_at_threshold_boundary` | exactly 64 KiB → inline; 64 KiB + 1 → blob (boundary off-by-one guard) |
| 19 | `read_step_output_inline` | Inline row → returns Some(json) |
| 20 | `read_step_output_blob` | Blob row → returns Some(json) after blob_store resolution; bytes equal original |
| 21 | `read_step_output_pending` | Row exists but status=Pending → returns Ok(None) |
| 22 | `read_step_output_inconsistent_returns_typed_error` | Force both columns set via raw SQL → PersistenceError::Inconsistent |
| 23 | `migration_v3_to_v4_adds_column` | Open v3 DB, run migration, schema_version = 4, column exists, existing rows have NULL ref |
| 24 | `migration_v3_to_v4_idempotent_on_v4` | Open v4 DB, run migration, no error, no row mutation |
| 25 | `migration_preserves_existing_inline_outputs` | v3 DB with inline rows → v4 reads return identical json |
| 26 | `audit_hash_unchanged_inline_vs_blob` | Same content, one stored inline (small) one stored as blob (forced) — output_hash byte-identical |

## Unit tests — workflow runner

| # | Name | Asserts |
|---|------|---------|
| 27 | `resume_restores_blob_output_into_data_store` | Crash + resume of run with completed large step; data_store has the value; downstream step_input sees it |
| 28 | `step_input_reads_blob_in_process` | In-process two-step workflow, step1 produces 100 KiB; step2 references step1.result; receives full bytes |
| 29 | `large_output_persists_as_blob_in_workflow_run_result` | `WorkflowRunResult.steps[0].output_hash` populated; output_json field omitted/absent (consistent with how API exposes refs) |
| 30 | `synthetic_approval_completion_under_threshold_inline` | 0.5-S6 synthetic completion at line 1191 of runner.rs is small; stays inline (regression: doesn't accidentally hit the blob path) |

## Unit tests — coordinator HTTP

| # | Name | Asserts |
|---|------|---------|
| 31 | `handle_complete_inline_below_threshold` | Worker POST /api/work/complete with 32 KiB body → 200 OK; row.output_json populated; no blob file |
| 32 | `handle_complete_blob_above_threshold` | Worker POST /api/work/complete with 100 KiB body → 200 OK; row.output_blob_ref populated; blob present at `<data_dir>/<env>/blobs/<aa>/<hash>` |
| 33 | `get_blob_returns_bytes_for_referenced_hash` | After 32, GET /api/runs/{id}/blobs/{hash} with valid bearer → 200 application/octet-stream; body byte-equal to written output |
| 34 | `get_blob_unauthorized_no_bearer` | GET without Authorization → 401 coord.unauthorized |
| 35 | `get_blob_unauthorized_wrong_token` | GET with wrong bearer → 401 coord.unauthorized |
| 36 | `get_blob_bad_hash_short` | GET /blobs/abc → 400 coord.blobs.bad_hash |
| 37 | `get_blob_bad_hash_traversal` | GET /blobs/..%2Fetc%2Fpasswd → 400 coord.blobs.bad_hash |
| 38 | `get_blob_bad_hash_uppercase` | GET /blobs/ABCD... → 400 coord.blobs.bad_hash |
| 39 | `get_blob_not_found_unknown_hash` | GET with valid-looking but unreferenced hash → 404 coord.blobs.not_found |
| 40 | `get_blob_not_found_other_run` | GET for run B targeting a hash referenced only by run A → 404 coord.blobs.not_found (run-scope enforcement) |
| 41 | `run_status_includes_step_output_hashes_for_completed_blob_steps` | GET /api/runs/{id}/status → response includes step_output_hashes map populated |
| 42 | `run_status_omits_step_output_hashes_for_pending_steps` | A pending step has no entry in the map |
| 43 | `protocol_version_unchanged_for_status_response` | RunStatusResponse.protocol_version == 1; additive field doesn't bump version (per convention §4) |

## Integration tests — `cli_coordinator_worker.rs` (existing file)

| # | Name | Asserts |
|---|------|---------|
| 44 | `e2e_large_output_blob_roundtrip` | Submit run, worker completes step with 200 KiB LLM-style payload, CI client polls status, fetches blob, verifies bytes match what worker sent |
| 45 | `e2e_two_step_workflow_pipes_large_blob_through_step_input` | step1 emits 100 KiB; step2 reads via step_input; e2e completes successfully |

## Unit tests — audit / evidence

| # | Name | Asserts |
|---|------|---------|
| 46 | `evidence_bundle_writes_sidecar_blobs_for_large_outputs` | Build bundle from a run with one large step → bundle dir has `blobs/<aa>/<hash>` file with the bytes |
| 47 | `evidence_bundle_omits_sidecar_for_small_outputs` | Run with only inline outputs → no `blobs/` directory in bundle |
| 48 | `verify_bundle_resolves_sidecar_blobs` | Verify post-S7 bundle with sidecar blob → PASSES (hash chain matches) |
| 49 | `verify_bundle_pre_s7_inline_still_passes` | Verify a pre-S7 bundle (no `blobs/` dir, all inline) → PASSES (backward compat) |
| 50 | `verify_bundle_tampered_blob_fails` | Bundle with sidecar blob whose bytes have been mutated → verify fails with hash mismatch |
| 51 | `verify_bundle_missing_sidecar_blob_fails_loudly` | Bundle metadata references a hash; sidecar file deleted → verify returns specific error pointing to missing blob |

## Unit tests — dashboard

| # | Name | Asserts |
|---|------|---------|
| 52 | `dashboard_renders_blob_placeholder_for_large_output` | Per-run detail page with one large completed step → HTML contains `[stored as blob: ...]` link to /api/runs/{id}/blobs/{hash} |
| 53 | `dashboard_renders_inline_for_small_output` | Per-run detail with small output → HTML still contains the actual JSON value |

## Adversarial cases (per project convention §29)

The reviewer (ce-correctness-reviewer + ce-data-integrity-guardian + ce-security-reviewer)
should specifically construct these scenarios:

- **TOCTOU between blob write and CAS row update.** Worker writes blob,
  another worker takes lease (lease-expired), the second worker writes
  a *different-content* same-hash output (impossible by SHA-256, but if
  the content were different the second write would clobber). Verify
  the design relies on hash uniqueness for safety.
- **Half-written blob due to disk full.** Force EIO mid-`write_all`;
  verify temp file remains `.tmp`, never gets renamed; `read_string`
  on the hash returns NotFound.
- **Path traversal via `run_id` in the route.** `GET /api/runs/..%2F..%2Fetc%2Fpasswd/blobs/{hash}` —
  Axum's path matcher should reject; verify with explicit test.
- **Hash mismatch attack.** Hash in path is valid hex but doesn't match
  the blob content on disk (somehow corrupted); operator's `read_string`
  succeeds but the verifier's re-hash fails. Document this as a
  storage-corruption signal: the hash chain detects it.
- **8 MiB body limit interaction.** Worker tries to POST a 9 MiB
  output → existing 413 fires before we ever reach the threshold gate.
  No regression; document the interaction.
- **Concurrent blob writes for the same hash.** Two workers complete
  the *same* step (both legitimately reattempt with same content);
  both `write` calls succeed; final file is byte-identical; no race.
- **Cross-run blob fetch denial.** Run A has blob `abc...`. Run B has
  no checkpoints. `GET /api/runs/<B>/blobs/<abc>` → 404, not 200.
- **Blob ref column visible in dashboard list endpoint.** The slim
  `RunSummary` for /api/runs (list) intentionally omits sensitive
  fields per convention §0.4-S16. New field must be examined: is
  `output_blob_ref` exposed in list, detail, both, neither?
  Decision: same policy as `output_hash` — exposed in detail only.
  (Hash already fine in detail; ref is the same hash.)

## Performance sanity checks (not gates, but worth running)

- 1 MiB output completion roundtrip < 50 ms wall on dev hardware.
- 100 concurrent reads of the same 200 KiB blob from coord HTTP
  serve at > 5K req/s on a single worker process (sanity that
  filesystem read isn't a bottleneck; this is just file IO + send).

These are not in CI; an operator runs them once during S7 review
and pastes the numbers in the retro.

## Test count

- Unit: 53 (15 blob_store, 11 persistence, 4 runner, 13 coord, 6
  audit, 2 dashboard, 2 adversarial integration).
- Integration: 2 (cli_coordinator_worker).
- Total new: **55**.

Existing tests passing post-S7 should match the post-0.5-S6 count
plus 55 = **381 orchestrator + 55 = TBD**, plus equivalent additions
to cli unit tests for the coord handlers (~13 new) bringing
cli unit from 54 → 67, cli_coordinator_worker integration from 26 → 28.
