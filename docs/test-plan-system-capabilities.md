# Test plan: files, clock and random numbers

Architecture: [architecture-system-capabilities.md](architecture-system-capabilities.md).

## Compiler (`crates/llmc/tests/capability_builtins.rs`, parser tests)

- Each new built-in compiles to `CapCall` with the right capability and arity.
- A call without the declared capability fails with "capability not declared".
- Wrong arity is a compile error.
- A program-defined `time_now` shadows the built-in.
- `match x { -1 => .., 0 => .., _ => .. }` compiles and picks the `-1` arm at run time.

## Handler (`crates/llmvm/src/system_handler.rs` unit tests)

- Read inside a root returns the contents.
- Read outside every root is denied: absolute path, `../` escape, and a symlink inside the
  root that points outside (unix only).
- No `fs_policy` means every read and write is denied, with a message naming
  `fs_policy.allowed_roots`.
- A file larger than `max_read_bytes` is denied before reading.
- Write inside a root creates the file. Write to a missing parent or outside the roots is
  denied.
- `time.now` is within a second of `SystemTime::now()`.
- `random_int(5, 5)` returns 5. 1000 draws of `random_int(1, 6)` all fall in `[1, 6]`.
  `lo > hi` is an error. No-arg random is a Float in `[0, 1)`.
- Other capabilities reach the fallback.

## Policy (`policy_validate` tests)

- `fs_policy` with roots parses. An empty root list and an unknown key are rejected.
- A policy without `fs_policy` serializes byte-for-byte as before (hash unchanged).

## CLI (`crates/llmvm-cli/tests/cli_system_capabilities.rs`)

- `run --live` with a policy file whose root is a temp dir: `fs_write` then `fs_read`
  round-trips, and a path outside the root fails with a policy error.
- `run --live --record log.json`, then `replay` with the log, gives the same `time_now` /
  `random_int` result.
- Without `--live`: mock values (`1700000000000`, `lo`).
- `workflow eval --live` against two fake OpenAI-compatible servers: each server receives
  its side's requests.
- `workflow resume --providers` is accepted, and the provider is used on the resumed step.

## Diagnostics (tooling)

- `let mut x: Int = 1; x = "a"` gives an E009 warning.
- `while 1 { }` gives an E009 warning.
- `while i < 3 { }` and same-type reassignment give no warning.

## Gates

`cargo fmt --check`; `cargo clippy --workspace --all-targets -D warnings`, both with and
without `--features boruna-vm/http,boruna-cli/http`; `cargo test --workspace`; the http
CLI tests; `scripts/check-doc-examples.py` and `scripts/check-site-links.py`.
