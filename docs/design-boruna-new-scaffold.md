# `boruna new` — interactive scaffold

**Sprint:** `W3-C`
**Status:** shipped on master.
**Code:** `crates/llmvm-cli/src/scaffold.rs`, `Command::New` in `crates/llmvm-cli/src/main.rs`.

## What it does

`boruna new` is a thin, interactive front end on top of the existing
`boruna_tooling::templates` engine. It walks the user through:

1. Picking a template (numbered list of available manifests).
2. Picking a target directory (default `./<template-name>`).
3. Filling in each manifest variable, prompting with the description
   and offering a default if the manifest provides one.
4. Confirming a summary before any files are written.

After confirmation it calls `templates::apply_template` and writes the
generated `.ax` file to the chosen target dir.

## Why it exists

The DX backlog deferred from 0.2.0 listed `boruna new` as one of the
two highest-leverage friction reducers (alongside `boruna fmt`).
`boruna template apply` already exists, but it requires the user to
know:

- which template they want,
- the exact name of every variable in that template's manifest,
- the comma/equals format for `--args`.

For a first-time user the discovery cost is real. The interactive
scaffold removes it without adding new template surface area, new
manifest fields, or a new template engine — it is a UX wrapper, not a
new subsystem.

## Non-goals

- **No TUI library.** Plain stdin line-reading. `crossterm`, `dialoguer`,
  `inquire`, etc. would each pull a tree of dependencies and add a
  novel attack surface for a feature that prints questions and reads
  lines. The current implementation is ~200 lines of Rust with one new
  symbol (`run_new`).
- **No saved-answers / config file.** Out of scope. If a user wants
  reproducible scaffolding they pass `--var key=value` flags or call
  `boruna template apply` directly; both are scriptable.
- **No template authoring UI.** Templates are still authored by hand
  under `templates/<name>/` with a `template.json` and an
  `app.ax.template`. The scaffold consumes those; it does not write
  them.
- **No new variables on existing templates.** `template.json` files
  under `templates/*` are read-only for this sprint.

## Design decisions

### Generic over `BufRead` + `Write`

`run_new<R: BufRead, W: Write>(...)` takes the reader and writer as
parameters. The binary path passes `stdin().lock()` and
`stdout().lock()`; the unit tests pass `Cursor::new(b"...")` and a
`Vec<u8>`. This is the only practical way to test interactive prompt
logic without spawning a subprocess and managing a pty. Costs nothing
in production.

### `--no-input` is loud, not silent

CI-safe behaviour was an explicit acceptance criterion. With
`--no-input`, the command:

- uses the manifest default when one is declared,
- errors with a precise message (`'foo' has no default and was not
  supplied via --var`) when a required variable has no default,
- never picks the first option in a list,
- still requires a positional template name (no auto-pick).

Silent fallbacks were considered and rejected: a user who runs the
command in CI and gets a working but wrong scaffold has a much worse
day than a user who gets a clear error and adds a `--var`.

### `--force` for non-empty targets

By default the command refuses to write into an existing non-empty
directory. The expected workflow is `boruna new` → fresh dir, then
`cd` and edit. `--force` is a separate flag because an unintended
overwrite is the dominant failure mode for scaffolding tools and we'd
rather the user opt in.

### Output path

The template engine emits exactly one file (`<template>_app.ax`)
today. The scaffold writes that file into the target dir and prints
the next steps. If the engine grows multi-file output later, the
scaffold's `written_files: Vec<PathBuf>` already accommodates it
without an API change.

## Tests

Seven unit tests in `scaffold::tests` cover:

- `scaffold_with_all_args_no_input_succeeds` — happy-path CI mode.
- `scaffold_prompts_for_missing_variables` — verifies prompts get
  emitted and answers flow through.
- `scaffold_refuses_overwrite_without_force`.
- `scaffold_force_overwrites_target`.
- `scaffold_no_input_errors_on_missing_default` — the CI-safety
  guarantee.
- `scaffold_invalid_template_name_errors_clearly`.
- `scaffold_uses_default_when_user_presses_enter` — manifest defaults
  in interactive mode.

All tests use temp dirs and `Cursor`-backed readers; none touch real
stdin or the real `templates/` directory.

## What did not change

- `boruna_tooling::templates` — no new public API, no new fields on
  `TemplateManifest` or `ArgSpec`. The `default: Option<Value>` field
  already existed on `ArgSpec`; the scaffold is the first caller to
  use it.
- `boruna template list` and `boruna template apply` — unchanged. The
  new command supplements rather than replaces them.
- Templates under `templates/*` — read-only for this sprint.
