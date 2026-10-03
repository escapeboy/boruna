# confidence_gated_review

A step reports how confident it is. A calibrated gate lets confident answers through without
a human and sends the rest to a reviewer.

```
score ──▶ review (approval_gate + confidence_gate) ──▶ publish
```

- `score` returns an `Int` in permille (0..=1000). Here it is a fixed 870 so the example is
  deterministic. A real workflow derives it from the model output.
- `review` carries `confidence_gate` with `alpha_permille: 100` and `calibration.json`,
  200 labelled past cases (score, was the answer right).
- `publish` runs once the gate completes, automatically or after `boruna workflow approve`.

```bash
boruna confidence threshold examples/workflows/confidence_gated_review/calibration.json \
  --alpha-permille 100 --score 870
boruna workflow run examples/workflows/confidence_gated_review --policy allow-all --record
```

The threshold is 673, so 870 completes the gate with no human. Change `steps/score.ax` to
return 500 and the run pauses at `review`.

## What the guarantee means

At alpha 10%, a wrong answer is auto-approved with probability at most 10%. Here 9 of the 104
wrong calibration answers score above 673 (8.7%).

That is not the same as "10% of approved answers are wrong". The calibration set approves 61
answers and 9 of them were wrong, which is 15% of the approved ones. The guarantee is about
wrong answers getting through, and it assumes live scores look like the calibration set.
Recalibrate from recent labelled cases.

## Evidence

`--record` writes `confidence_gates.json` (the decision, score, threshold) and
`confidence/review.calibration.json` (the exact file used) into the bundle.
`boruna evidence verify` recomputes the decision from them and checks it against the audit log.

Not supported with `--submit-only` or the coordinator.
