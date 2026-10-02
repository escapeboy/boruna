# Test plan: calibrated confidence for approval gates

| Layer | File | Covers |
|---|---|---|
| Maths | `orchestrator/src/confidence.rs` | Hand-computed threshold (490 for the 100-example split), monotone in alpha, order independent, too few wrong examples, no wrong examples, all wrong, boundary decisions, bad inputs, `GateRecord::verify` detects tampering, empirical false approval rate on held-out data stays within 5 permille of alpha |
| Wiring | `orchestrator/src/workflow/confidence_gate.rs` | Score must be an `Int` in range, missing/bad calibration is an error, path traversal rejected, audit round trip |
| Validator | `orchestrator/src/workflow/validator.rs` | Bad alpha, unknown / non-source / non-dependency score step, calibration paths that leave the directory |
| Runner | `orchestrator/tests/confidence_gate.rs` | Auto-approve and continue, same output hash as a human approval, below threshold pauses, exact threshold approves, non-Int and out-of-range scores pause, missing calibration fails, gate without the field always pauses, serialization unchanged without the field, concurrent waves, submit-only refused with no run created |
| CLI | `crates/llmvm-cli/tests/cli_confidence_gate.rs` | Persisted run to bundle to verify; flipped decision, swapped calibration and dropped file all rejected; escalate, approve, resume records the decision once and verifies; submit-only refused |
| CLI | `crates/llmvm-cli/tests/cli_confidence.rs` | `confidence threshold` JSON, decision, never, bad inputs |
| CI | `examples/workflows/confidence_gated_review` | Validated, run and bundle-verified by the existing example loop |

| Verify | `orchestrator/src/audit/verify.rs`, `audit/log.rs` | Forged record rejected, any redacted policy entry rejected (with or without a record file), duplicate records rejected, gate entries refused by `redact`, other entries still redactable |

Attacks replayed by hand against the real binary across three independent review rounds:
flipped decision, swapped or edited calibration, dropped file (with and without recomputed
manifest), forged score under an unrelated redaction, dropped record behind a redacted gate
entry, and the same with the declaration stripped from `workflow.json`. All rejected.

Mutation check done by hand: making `decide` ignore the threshold fails four tests
(runner, wiring, maths unit, empirical rate).

Gates: `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D warnings`,
`cargo test --workspace`.
