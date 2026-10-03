# Architecture: calibrated confidence for approval gates

```
confidence.rs            pure: CalibrationSet, risk_threshold, decide, GateRecord::verify
workflow/definition.rs   ConfidenceGate (on StepKind::ApprovalGate), GateEvaluation,
                         WorkflowRunResult.confidence_gates
workflow/validator.rs    InvalidConfidenceGate, is_safe_relative_path
workflow/confidence_gate.rs
                         evaluate() (reads calibration + score), audit_event(),
                         records_from_audit(), evaluations_from_run()
workflow/runner.rs       try_auto_approve_gate()  <- one function, called from both
                         execute_steps (sequential) and execute_steps_concurrent;
                         record_confidence_gate / has_gate_record (persisted);
                         reject_confidence_gates_for_coordinator;
                         create_bundle adds the gates
audit/evidence.rs        add_confidence_gates(), registered in bundle.json components
audit/verify.rs          verify_confidence_gates()
llmvm-cli                boruna confidence threshold; workflow run --record adds the gates
```

## Flow at a gate

1. The gate becomes ready. `try_auto_approve_gate` runs before the pause.
2. Persisted runs: if the audit chain already holds a decision for this gate (a resume),
   keep it and pause. A gate that auto-approved is already `Completed` and never gets here.
3. Read `<source_step>.result` from the data store. Read and validate the calibration file.
4. Compute the threshold and decide. Record it: audit event plus calibration text in the run
   metadata (one compare-and-swap write for persisted runs), or on the result for in-process.
5. Auto-approved: store the same empty-map output a human approval stores, mark `Completed`,
   continue. Anything else: pause exactly as before.

## Why this is safe

- Auto-approval and human approval produce the same output hash, so downstream bundles match.
- No clock, randomness or float anywhere in the decision path. Scores, alpha and thresholds
  are integers. The audit event carries no timestamps.
- `evidence verify` recomputes the decision from the embedded calibration file and
  cross-checks the audit chain both ways, so a flipped decision, a swapped calibration, or a
  dropped `confidence_gates.json` is rejected.
- Gate decisions cannot be redacted. A redacted audit entry is only a content hash: its event
  is a non-authoritative placeholder, so it cannot be told apart from a hidden gate decision.
  Gate records carry no personal data, so `boruna evidence redact` refuses them and
  `evidence verify` rejects any redacted policy entry (`PolicyEvaluated`). Only this feature
  emits that event. The cross-check also uses the chain's content hash, so a forged score or
  threshold is caught either way.

## Known limits

- The CLI's `--record` bundle for a resumed run lists only the gates evaluated in that call.
  Use `boruna evidence create <run-id>` for a complete bundle, which rebuilds from the audit chain.
- `confidence_gate` is rejected with `--submit-only` and `coordinator`.

## What verification does not protect against

`evidence verify` is a consistency check plus tamper evidence, not a lock.

- Someone who can write the whole bundle directory and recompute every hash can rebuild
  everything, including the audit chain (unkeyed SHA-256). Only an external anchor stops that:
  an anchored or signed `bundle_hash`, or an anchored `audit_log_hash` for the chain.
- An anchored `audit_log_hash` alone does not cover files outside the chain, and a redaction
  marker is not bound by the chain. A person with write access who hand-marks an entry as
  redacted and rewrites its placeholder to another event type is not caught by `verify`
  (the bundle's own `redact` command refuses gate decisions). Anchor `bundle_hash` or sign the
  manifest if the bundle can pass through untrusted hands.
- `manifest.workflow_hash` is not compared with `workflow.json` (also true before this
  feature).
