# Design — `boruna simulate --witnesses` (witness queries)

## Status

Planned. Depends on `boruna simulate` (design-boruna-simulate.md). Borrowed from Quint's
`quint run --witnesses=...`. Source: research_quint_borrowable_ideas_2026-05-20.md, rec #5.

## Context

An invariant says "this must hold in EVERY state." A witness says "this must be POSSIBLE in
some state." Witnesses are a practical proxy for liveness properties — they answer questions
like "in our SOC2 workflow, do we ever actually hit the rejection path?" or "under random worker
failure, can any approval gate stay pending longer than 24h?"

Quint reports witnesses as percentages of traces in which the predicate held: `alice_more_than_bob
was witnessed in 7094 trace(s) out of 10000 explored (70.94%)`. This is dramatically cheaper than
true liveness verification and good enough for compliance audits.

## Why

Once `boruna simulate` (design-boruna-simulate.md) lands, witnesses are 100 extra lines of code
and yield a meaningful new compliance question. Boruna customers want to make positive claims
("the auto-approve path is exercised in N% of routine cases"), not just negative ones ("no
violation found").

## Goals

1. `boruna simulate <dir> --witnesses=expr1,expr2` reports trace-frequency for each witness
   predicate across the explored runs.
2. Witness output: `<name> was witnessed in 7094 trace(s) out of 10000 explored (70.94%)`,
   matching Quint's UX so external docs/training apply.
3. Witnesses do NOT cause simulator failure. They are informational. `--require-witness=<name>`
   is a separate flag that DOES fail if frequency is below a threshold.
4. Witness predicates have the same `.ax`-expression-over-workflow-data-store grammar as
   invariants (rec'd in design-boruna-simulate.md §3).

## Non-goals

- Independent simulator — witnesses are a feature of `boruna simulate`, not a separate command.
- Per-state visualization of where witnesses fired (handled by ITF trace export).
- LTL temporal operators in witness expressions (out of scope; concrete `.ax` boolean only).

## Forcing questions

**Who needs this? What are they doing today?**
The same compliance engineer from `simulate`. Today they have no way to ask "does any of my
10K simulator runs actually exercise the rejection-then-retry path?" — they just hope coverage is
broad enough. With witnesses: they get a concrete percentage.

**What's the narrowest MVP someone would pay for?**
A single `--witnesses` flag accepting one expression and printing the count. Multi-witness
follows trivially.

**What would make someone say "whoa"?**
Combining `--witnesses` with `--out-itf` so each witnessing trace is exported with the witness
expression highlighted. (Out of scope this design; future polish.)

**How does this compound over time?**
The same predicate infrastructure powers `--require-witness` (compliance assertion: "this path
MUST be exercised") and downstream witness-coverage dashboards.

## Scope

| In | Out |
|---|---|
| `--witnesses=expr1,expr2,...` reports counts | Per-state diff of where each witness fired |
| `--require-witness=name=0.5` fails if frequency < threshold | Liveness/temporal operators |
| Sharing invariant evaluation harness with `--invariant` | Independent witness CLI |

## Decisions

1. **Evaluation point:** witnesses are checked at *every* state of the simulator, not just
   final state. A witness with even one hit in one state of one trace counts that trace as a
   witnessing trace.
2. **Naming:** witnesses can be named (`--witnesses=ack_path=expr,reject_path=expr`) or unnamed
   (positional; auto-numbered).
3. **Output format:** human format mirrors Quint verbatim (lower learning curve for transferring
   users); `--json` emits `{"witnesses": [{"name": "...", "trace_count": 7094, "total": 10000,
   "frequency": 0.7094}]}`.
4. **Performance:** witnesses share the same per-state callback hook as invariants; cost is one
   extra `.ax` evaluation per state. Marginal.

## Risks

- **Predicate explosion.** A user passes 50 witnesses; each is evaluated at every state. With
  10K traces × 20 steps × 50 witnesses = 10M `.ax` evals. Mitigation: warn in stderr if witness
  count exceeds 10; document a `--max-witnesses` cap.

## Open questions for the architecture phase

- Should `--require-witness` failures produce evidence bundles (like `--invariant` failures do)?
  Lean: no, just a non-zero exit + stderr listing.
