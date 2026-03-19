# Your First Workflow

This guide walks through creating a simple two-step workflow from scratch, running it, and inspecting the evidence it produces.

**Time required:** ~15 minutes
**Prerequisites:** Boruna built (`cargo build --workspace`)

## What you'll build

A workflow that fetches a number and doubles it — two steps connected in sequence. Simple enough to fit in your head, realistic enough to demonstrate every core concept: DAG definition, `.ax` step code, capability policies, and evidence.

## Step 1: Create the workflow directory

```bash
mkdir -p my-first-workflow/steps
cd my-first-workflow
```

## Step 2: Define the workflow DAG

Create `workflow.json`:

```json
{
  "id": "double-it",
  "name": "Double It",
  "description": "Fetch a number, double it, return the result.",
  "steps": [
    {
      "id": "fetch",
      "name": "Fetch Number",
      "source": "steps/fetch.ax",
      "capabilities": []
    },
    {
      "id": "double",
      "name": "Double It",
      "source": "steps/double.ax",
      "capabilities": [],
      "depends_on": ["fetch"]
    }
  ],
  "edges": [
    { "from": "fetch", "to": "double" }
  ]
}
```

The `depends_on` field defines the DAG edge. `double` runs after `fetch` completes.

## Step 3: Write the step files

**`steps/fetch.ax`** — returns a number (demo mode; live mode would call net.fetch):

```ax
fn get_number() -> Int {
    42
}

fn main() -> Int {
    let n: Int = get_number()
    n
}
```

**`steps/double.ax`** — doubles the input:

```ax
fn double(n: Int) -> Int {
    n * 2
}

fn main() -> Int {
    let input: Int = 42
    let result: Int = double(input)
    result
}
```

## Step 4: Validate the workflow

Before running, validate the DAG structure:

```bash
cd ..
cargo run --bin boruna -- workflow validate my-first-workflow/

# Expected output:
# Workflow 'double-it': valid
# Steps: 2
# Edges: 1
# Topological order: fetch → double
# No cycles detected.
```

Validation checks: all referenced step files exist, the graph is acyclic, and every `depends_on` refers to a real step.

## Step 5: Run the workflow

```bash
cargo run --bin boruna -- workflow run my-first-workflow/ --policy allow-all

# Expected output:
# Running workflow: double-it
#
#   [1/2] fetch     → ok
#   [2/2] double    → ok
#
# Workflow completed in 0.01s
# Output: 84
```

## Step 6: Run with evidence recording

```bash
cargo run --bin boruna -- workflow run my-first-workflow/ --policy allow-all --record

# Expected output:
# Running workflow: double-it
# ...
# Workflow completed in 0.01s
# Bundle written to: .boruna/runs/20260319-120000-abc12/
```

## Step 7: Inspect the evidence

```bash
cargo run --bin boruna -- evidence inspect .boruna/runs/20260319-120000-abc12/

# Expected output:
# Run ID:     20260319-120000-abc12
# Workflow:   double-it
# Started:    2026-03-19T12:00:00Z
# Completed:  2026-03-19T12:00:00Z
# Policy:     allow-all
# Steps:      2 completed, 0 failed
#
# Step Results:
#   fetch    → ok  (0.0s)
#   double   → ok  (0.0s)
#
# Chain:      valid (2 entries, no gaps)
```

## Step 8: Verify the evidence bundle

```bash
cargo run --bin boruna -- evidence verify .boruna/runs/20260319-120000-abc12/

# Expected output:
# Chain integrity: VALID
# All step hashes: MATCH
# Environment fingerprint: PRESENT
# Verification: PASSED
```

That's a complete workflow: defined, validated, executed, recorded, and verified.

## What just happened

1. The workflow runner read `workflow.json` and sorted steps topologically.
2. Each `.ax` file was compiled to bytecode and run on the VM.
3. Every step's output was captured and written to the evidence bundle.
4. The audit log was hash-chained so no entry can be modified undetected.
5. `evidence verify` confirmed the chain was unbroken.

## Next steps

- Add a real capability: modify `steps/fetch.ax` to declare `!{net.fetch}` and run with `--live`
- See a production-scale example: [examples/workflows/llm_code_review](../../examples/workflows/llm_code_review/)
- Add an approval gate: [customer support triage workflow](../../examples/workflows/customer_support_triage/)
- Read the [.ax language reference](../reference/ax-language.md)
