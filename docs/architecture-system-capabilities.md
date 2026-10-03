# Architecture: files, clock and random numbers

Design: [design-system-capabilities.md](design-system-capabilities.md).

## Language (compiler, language 1.3)

New entries in `CAPABILITY_BUILTINS` (`crates/llmc/src/typeck.rs`). Calls compile to
`Op::CapCall(cap, arity)`, as for the 1.2 built-ins. The calling function must declare
the capability (E007), and a function the program defines with the same name shadows the
built-in.

| Built-in | Capability | Returns |
|---|---|---|
| `fs_read(path: String)` | `fs.read` | `String`: the file's UTF-8 contents |
| `fs_write(path: String, content: String)` | `fs.write` | `Bool`: `true` once written (failures are runtime errors) |
| `time_now()` | `time.now` | `Int`: Unix time in milliseconds |
| `random_int(lo: Int, hi: Int)` | `random` | `Int`: uniform in `[lo, hi]`, both ends included |

The parser accepts `-` followed by an integer literal as an integer pattern.

## Policy

`Policy` gains `fs_policy: Option<FsPolicy>`, serialized only when present
(`skip_serializing_if`), so the bytes and hash of every existing policy stay the same.

```json
"fs_policy": { "allowed_roots": ["./data", "/srv/reports"], "max_read_bytes": 10485760 }
```

- `allowed_roots`: folders a path must fall inside. Relative roots resolve against the
  working directory of the process. Required and non-empty when `fs_policy` is present.
- `max_read_bytes`: optional, default 10 MiB.

The strict validator (`policy_validate.rs`) accepts the field with `deny_unknown_fields`
and rejects an empty root list. `docs/reference/policy.schema.json` documents it.

## Handler

`boruna_vm::system_handler::SystemHandler { fs: Option<FsPolicy>, fallback }` handles
`fs.read`, `fs.write`, `time.now` and `random`, and passes everything else to `fallback`.
It needs no cargo feature.

- **Path check**: canonicalize every root and the target. For reads, the target must
  exist. For writes, the parent is canonicalized, the final component must be a normal
  file name, and an existing symlink at the target is resolved and checked too. The
  resolved path must start with a resolved root. This blocks `..` and escapes through
  symlinks.
- **Read**: the size is checked against `max_read_bytes` before reading. Non-UTF-8
  content is an error.
- **Clock**: `SystemTime::now()` in milliseconds.
- **Random**: `rand_core::OsRng` (already a workspace dependency, works on every platform
  we ship) with rejection sampling, so there is no modulo bias. With `(lo, hi)` it returns
  an Int and `lo > hi` is an error. With no args (framework effects) it returns a Float in
  `[0, 1)`.

The gateway still does policy and budget checks first. The VM event log records each
result, which is what `boruna run --record` and `boruna replay` already use, so replay
needs no new code.

## Wiring

`--live` wraps the live handler in `SystemHandler`, outermost:
- **CLI `make_gateway`**: the live path, the `--record-net-to` path, and the
  `--live`-without-`http` fallback.
- **Workflow runner `compile_and_run_step`**: the live path, inside `StepInputHandler`.

Mock changes, so the built-ins type-check at run time without `--live`:
- `time.now` returns `1700000000000` (ms; was seconds).
- `random` with `(lo, hi)` returns `lo`; with no args it still returns `0.42`.

## LLM providers on eval, resume, schedule

- `workflow eval` gains `--live`. Each side loads its file with `provider_registry::load`
  and passes it as `RunOptions.llm_providers`. Without `--live` it prints a warning that
  both sides use the mock.
- `ResumeOptions` gains `llm_providers`, copied into the synthesized `RunOptions`.
- `workflow resume` and `workflow schedule` gain `--providers`.

## Diagnostics (tooling)

In `check_types_in_stmt`:
- **`Stmt::Assign`**: if the target has a known type in `env` and the value's inferred
  type differs, report an E009 warning.
- **`Stmt::While`**: if the condition's inferred type is known and is not `Bool`, report
  an E009 warning.
