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
- Redaction does not open a hole. The cross-check uses the chain's content hash, which a
  redacted entry keeps. A gate record carries no personal data, so redacting its entry and then
  dropping the record would hide an auto-approval; when the workflow declares confidence gates,
  every redacted policy entry must be accounted for by a record in `confidence_gates.json`.

## Known limits

- The CLI's `--record` bundle for a resumed run lists only the gates evaluated in that call.
  Use `boruna evidence create <run-id>` for a complete bundle, which rebuilds from the audit chain.
- `confidence_gate` is rejected with `--submit-only` and `coordinator`.
