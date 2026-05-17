# Skill: The boruna CLI

The `boruna` binary is the single toolchain entry point. Every inspection
command accepts `--json` for machine-readable output. For the full reference
see `docs/reference/cli.md`.

## Compile and run

```
boruna compile app.ax              # -> app.axbc bytecode
boruna run app.ax                  # compile + execute
boruna run app.ax --policy allow-all
boruna ast app.ax                  # dump the AST as JSON
boruna inspect app.axbc            # inspect a compiled bytecode file
```

## Diagnostics and repair

```
boruna lang check app.ax --json    # structured diagnostics
boruna lang repair app.ax          # apply suggested fixes
boruna lang codes --json           # registry of all diagnostic codes
```

## Inspection (agent-friendly, all support --json)

```
boruna doctor --json               # environment + toolchain health
boruna size app.ax --json          # bytecode artifact size report
boruna workflow graph <dir> --json # DAG facts: nodes, edges, topo order
boruna skills list                 # embedded agent documentation
```

## Workflows

```
boruna workflow validate <dir>           # validate a workflow.json DAG
boruna workflow run <dir> --policy allow-all
boruna workflow graph <dir> --json       # inspect DAG structure
```

## Evidence (compliance / audit)

```
boruna evidence verify <bundle-dir>      # verify a hash-chained bundle
boruna evidence inspect <bundle-dir> --json
```

## Templates and scaffolding

```
boruna template list
boruna template apply crud-admin --args "entity_name=products"
boruna new                               # interactive project scaffold
```

## Exit codes

`0` success. `1` is the common failure code (compile error, diagnostics with
errors, validation failure, unknown skill). Some commands use additional codes
documented in `docs/reference/cli.md` — for example `workflow wait` uses `3`
for a budget timeout.
