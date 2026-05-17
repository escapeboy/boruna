# Skill: Workflows

A Boruna workflow is a DAG of steps defined in a `workflow.json` file inside a
workflow directory. Each step compiles to bytecode and runs on the VM under a
capability policy. Every run can produce a hash-chained evidence bundle.

## Directory layout

```
my_workflow/
  workflow.json        # the DAG definition
  steps/
    fetch_data.ax      # one .ax source per "source" step
    transform.ax
```

## workflow.json shape

```
{
  "schema_version": 1,
  "name": "my_workflow",
  "version": "1.0.0",
  "description": "...",
  "steps": {
    "fetch":     { "kind": "source", "source": "steps/fetch_data.ax",
                   "capabilities": ["net.fetch"], "depends_on": [] },
    "transform": { "kind": "source", "source": "steps/transform.ax",
                   "depends_on": ["fetch"] }
  },
  "edges": [["fetch", "transform"]]
}
```

## Step kinds

- `source` — a step backed by a `.ax` source file.
- `approval_gate` — pauses for a human decision by `required_role`.
- `external_trigger` — waits for an external event.

## Dependencies

A step runs after every step it depends on. Dependencies come from a step's
`depends_on` list and from the global `edges` list — both are honored. The
workflow must be a DAG; a cycle is a validation error.

## Commands

```
boruna workflow validate <dir>            # check the DAG is well-formed
boruna workflow graph <dir> --json        # nodes, edges, topo order, roots, leaves
boruna workflow run <dir> --policy allow-all --record
```

## Inspecting graph structure as an agent

`boruna workflow graph <dir> --json` returns:

- `nodes` — each step with `kind`, `capabilities`, `depends_on`
- `edges` — explicit edge pairs
- `topological_order` — execution order
- `roots` — steps with no dependencies (entry points)
- `leaves` — steps nothing depends on (terminal outputs)
- `is_dag` — `false` if the graph contains a cycle

Use this to understand a workflow before modifying it: read the graph, find
the step you need, check its dependencies, then edit the relevant `.ax` file.
