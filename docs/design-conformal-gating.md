# Design: calibrated confidence for approval gates

Status: sprint 2 of 2 from the agentlanguages.dev borrow review (2026-10-02).
Sprint 1 was [agent docs emit/pack](design-agent-docs-emit.md).
Borrowed from: Quasar (arXiv:2506.12202) and conformal selection (Jin and Candès).
This replaces the "no conformal prediction, deferred" decision in
`docs/design-llm-typed-effect.md`.

## Problem

An `approval_gate` always pauses for a human. When a model step is right 95% of the time,
that is a lot of review for answers nobody would change. Boruna already has everything
downstream of the decision: a gate that pauses, approve/reject/resume, and sealed
evidence. What was missing is a calibrated number that decides when pausing is not needed.

## What is guaranteed (read this before relying on it)

For a new case whose answer is wrong, the chance that the gate lets it through without a
human is at most `alpha`. This is the false approval rate. It holds for any calibration size
(no asymptotics), if the wrong calibration examples and the new wrong case are
exchangeable.

It is not "at most `alpha` of the approved answers are wrong". That share also depends on how
often the step is wrong to begin with. Example from `examples/workflows/confidence_gated_review`
at alpha 10%: the calibration set would approve 61 answers and 9 of those were wrong (15% of
approved), because 9 of the 104 wrong answers (8.7%) fall above the threshold. Both numbers
are true. The guarantee is the 8.7% side. `boruna confidence threshold` prints both.

It does not hold under drift: if live scores no longer resemble the calibration set, the
guarantee is gone. Recalibrate from recent labelled cases.

## Decisions

| Question | Decision | Why |
|---|---|---|
| Criterion | Conformal p-value over the wrong calibration examples: threshold is the smallest score `t` with `(wrong_at_or_above_t + 1) / (wrong_total + 1) <= alpha` | Exact, finite-sample, integer arithmetic, replayable. A first draft used all examples and a "wrong and approved" loss. A hand check showed it would approve the top slice even when every calibration answer was wrong, so it was replaced |
| Too few wrong examples | Threshold `never` | Needs `1000 / alpha_permille - 1` wrong examples (19 at 5%). No wrong examples at all also means `never`, not "approve everything" |
| Score type | `Int` permille 0..=1000 in the source step's `result` | `Record` fields are positional, floats are not reproducible across machines, and a step output is one value. Put the confidence in its own step |
| Bad or missing score | Escalate to a human | Fail closed |
| Bad or missing calibration file | Fail the run | A typo must not silently disable a gate |
| New audit event? | No. Reuse `PolicyEvaluated` with `rule = "confidence_gate"` and the record as JSON | A new variant would break older readers of the log |
| Where the detail lives | `confidence_gates.json` and `confidence/<step>.calibration.json` in the bundle | Same pattern as `model_invoking_steps.json`. Checksummed and covered by `bundle_hash` |
| Existing workflow hashes | Unchanged | The new field is skipped when absent |
| Coordinator / `--submit-only` | Rejected at submit | The wait driver would pause every such gate. A clear error beats quietly different behavior |

## Scope

In: the pure maths (`orchestrator/src/confidence.rs`), the gate field and validation, the
runner hook shared by the sequential and concurrent paths, evidence and verification,
`boruna confidence threshold`, one example.

Out, with reasons:

- Coordinator support. `advance_run_one_tick` has no data store and mints approval tokens
  before opening gates. It needs its own design.
- Producing the score. How a step derives a confidence (sample agreement, a verifier
  model) is workflow logic, not engine logic.
- Abstention as a fourth step outcome. The gate escalating to a human already covers it.
- Behavioral certificates (Aver) and type-guided sampling (MoonBit). Unchanged from sprint 1.
