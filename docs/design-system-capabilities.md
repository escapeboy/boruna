# Design: files, clock and random numbers, plus the remaining 3.x gaps

Status: accepted (2026-10-03). Target release: 3.8.0, language 1.3.

## Problem

After 3.7.0 a `.ax` program can call the network (`net_fetch`, `net_request`) and an LLM
(`llm_call`), but four capabilities that already exist in the bytecode and the policy
catalog have no language built-in and no real implementation: `fs.read`, `fs.write`,
`time.now`, `random`. Only the mock handler answers them, with fixed values. Several other
gaps are left over from earlier releases:

- `workflow eval --providers-a/--providers-b` loads both files but runs both sides on the
  mock, so it compares mock against mock.
- `workflow resume` and `workflow schedule` accept `--live` but not `--providers`, so a
  resumed or scheduled run cannot reach the configured LLM providers.
- A `match` arm cannot use a negative integer literal (`-1 => ...` is a parse error).
- `lang check` does not warn when a reassignment changes a variable's type or when a
  `while` condition is not a `Bool`.

## Decisions (from the user, 2026-10-03)

| Question | Decision |
|---|---|
| File access scope | Explicit list of folders in the policy (`fs_policy.allowed_roots`). No list means no access. Without `--live` the mock still answers. |
| Clock and random | Real values under `--live`, captured in the event log so `boruna replay` returns the same values. Without `--live`, fixed mock values as today. |
| New type checks | E009 warning now, error at the next major (4.0), the same path as `mut`. |
| Bedrock against real AWS | Not done. It stays verified against the AWS SigV4 test vectors only, and the docs say so. |

## Who needs it

- **Developers** writing `.ax` steps that read an input file, write a report, or stamp a
  time. Today they have to fake those through `step_input` or an external script.
- **Auditors**: every file read, write, clock read and random draw becomes a capability
  call. It is gated by the policy, counted in the audit log (`CapabilityInvoked`), and its
  result is in the event log for replay.
- **Teams comparing models**: `workflow eval` actually calls the two providers.

## Scope

In scope: the four built-ins, the real handler, the policy field, CLI and workflow-runner
wiring, the eval/resume/schedule provider wiring, negative integer patterns, the two E009
warnings, docs, and release 3.8.0.

Out of scope: directory listing, delete, append; a seeded random source; any change to the
mock values' meaning for framework apps beyond the unit noted in the architecture doc.
