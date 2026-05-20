# Design — `boruna repl` (interactive evaluation of `.ax` modules)

## Status

Planned. Borrowed from Quint (`quint repl`). Source: research_quint_borrowable_ideas_2026-05-20.md, rec #1.

## Context

Boruna's iteration loop today is **edit → `cargo run -- run file.ax` → wait for full compile**.
For interactive exploration of a module (call a function, inspect a value, exercise a non-deterministic
branch), this is the wrong cycle time. Quint's most-praised feature is its REPL — engineers do most
spec-debugging there, not via `quint run`. Boruna has zero equivalent.

The actor-system sprint (Feb 2026) already wired `Vm::execute_bounded(budget) -> StepResult`, giving
us the primitive for stepwise, resumable VM evaluation. The pieces exist.

## Why

The 1.4.0 retro and the v1.5.0 deferred items both list `boruna new` (interactive scaffold) and
`boruna run --watch` as DX gaps. A REPL subsumes both: a hot VM with line editing replaces both
"scaffold a thing fast" and "re-run on file change." It is also the single highest-ROI improvement
for the LSP audience — agents and humans can drive the REPL via stdin pipes.

## Goals

1. `boruna repl [file.ax]` boots a prompt with the module's top-level definitions in scope.
2. Each REPL line is parsed as a Quint-style expression *or* a meta-command (`:load`, `:type`, `:reset`, `:step`, `:quit`).
3. Expressions are typechecked against the loaded module's symbol table, compiled into a one-shot
   thunk, executed on a persistent `Vm`, and the result printed via the existing `Value::pretty`.
4. Errors recover the prompt — a type error or runtime panic does not kill the session.
5. The REPL session ID is logged as part of the existing `EventLog` so REPL-driven actions are
   replayable (compliance-aligned).

## Non-goals

- Apalache-style symbolic state exploration. The REPL evaluates concrete expressions only.
- Multi-line edit mode. Single-line per input; multi-line specs go in files.
- Hot-reload of modified `.ax` files. Use `:load` to re-import.
- Shell-style history navigation across sessions (in-session arrow-up only, via `rustyline`).
- `cargo install`-style binary distribution beyond the existing `cargo install --path crates/llmvm-cli`.

## Forcing questions

**Who needs this? What are they doing today?**
A `.ax` library author exploring how `Result::map` behaves on the `Err` branch is the canonical
user. Today they write a `fn main() { let r = …; if r is Err { … } }`, run `cargo run -- run scratch.ax`,
read stdout, edit, repeat. Cycle time ~5 seconds compile + 2 seconds run. Per investigation, 20+
times an hour. With a REPL: line-edit and read result, ~50ms.

**What's the narrowest MVP someone would pay for?**
The "boot file → eval one expression → exit" path. If `echo "1 + 2" | boruna repl file.ax` prints `3`
and exits zero, the rest is incremental polish.

**What would make someone say "whoa"?**
Loading a workflow `.ax` file and being able to drive its `init() / update(state, msg)` interactively
from the REPL — calling `update(s, Msg::Increment)` and seeing the new state immediately. That's
the spec-debugging experience.

**How does this compound over time?**
The REPL becomes the substrate for `:test`, `:trace`, `:replay <bundle>`, `:run-simulator` —
every future DX feature lands cheaper. It also unblocks an embedded REPL in the LSP (Jupyter-style
inline evaluation in VS Code).

## Scope

| In | Out |
|---|---|
| `boruna repl [file.ax]` subcommand | Multi-line input editor (yet) |
| Meta-commands: `:load`, `:reload`, `:type`, `:reset`, `:step`, `:env`, `:help`, `:quit` | `:edit` (open $EDITOR) |
| Persistent VM with execute_bounded for `:step` | Distributed/coordinator-aware REPL |
| Pretty-printed values with truncation (long lists, deep records) | Color/syntax highlighting (separate concern) |
| `rustyline` line editing + history file at `~/.boruna/repl-history` | Tab-completion (later — needs LSP integration) |
| Capability policy from `--policy` flag, same shape as `boruna run` | New capability classes for REPL |

## Decisions

1. **Dependency:** `rustyline` (already widely used; MIT). No alternative; the alternatives don't
   match the project's "default-features = false" §6 convention as cleanly.
2. **State model:** REPL holds one `Module`, one persistent `Vm`, and one persistent capability
   gateway. Re-loading a file replaces the module but does NOT reset the VM unless `:reset` is given.
3. **Errors:** all errors print and return the prompt. No exit-code propagation from individual evals.
   `Ctrl-D` cleanly exits with 0. `Ctrl-C` aborts current eval, returns to prompt.
4. **Determinism:** REPL inputs are non-deterministic by definition (operator-driven). Per §15,
   any EventLog entries emitted from REPL evaluations are tagged `phase: "repl"` so replay
   verification skips them. Tracked under operational-only state.
5. **Policy default:** REPL defaults to `deny-all` (safe by default). User must pass `--policy`
   explicitly to enable side-effects.

## Risks

- **VM not designed for repeated thunks.** The current `Vm::run()` is one-shot — runs a `main`
  function to completion. Compiling expressions as anonymous thunks and reusing the VM may surface
  latent stack/heap-state assumptions. Mitigation: integration test that runs 1000 sequential
  evals and asserts no memory growth.
- **Symbol-scope merging.** When a `:load` introduces a name that collides with an existing one,
  do we shadow or reject? Decision deferred to architecture doc; current lean: shadow with a
  warning.
- **Telemetry overhead.** Every REPL keystroke shouldn't spawn an OTel span. The CLI's existing
  `init_telemetry` is keyed on `--otel-*` flags; default off, no risk.

## Open questions for the architecture phase

- Where does `repl.rs` live — `crates/llmvm-cli/src/repl/` or `crates/llmvm-cli/src/repl.rs`?
- Does typechecking happen per-line, or do we pre-typecheck the loaded module and reuse the inference
  context for line expressions?
- What does `:step <action>` look like for an Elm-style framework app vs. a flat `.ax` module?
