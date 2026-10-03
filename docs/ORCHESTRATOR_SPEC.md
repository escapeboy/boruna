# Multi-Agent Orchestration Layer — Specification

## 1. Overview

The orchestrator enables parallel, safe, deterministic development on the Boruna codebase. It is a **separate tool** — it does not modify the compiler, VM, or framework runtime. It coordinates work by scheduling tasks as a DAG, enforcing a two-person rule (Implementer + Reviewer), managing file-level locks to prevent conflicts, and gating all changes through deterministic CI checks.

## 2. Work Graph Model

### 2.1 Nodes

Each node represents a unit of work:

```
WorkNode {
    id: String,              // unique identifier (e.g., "WN-001")
    description: String,     // human-readable summary
    inputs: Vec<String>,     // file paths or artifact IDs consumed
    outputs: Vec<String>,    // file paths or artifact IDs produced
    dependencies: Vec<String>, // IDs of nodes that must complete first
    owner_role: Role,        // Planner | Implementer | Reviewer
    tags: Vec<String>,       // e.g., ["compiler", "vm", "framework"]
    status: NodeStatus,
    assigned_to: Option<String>,
    patch_bundle: Option<String>, // path to .patchbundle.json
    review_result: Option<ReviewResult>,
}
```

### 2.2 Node States

```
pending  → ready → running → passed
                  ↘ blocked
                  ↘ failed
```

| State   | Meaning |
|---------|---------|
| pending | Dependencies not yet met |
| ready   | All dependencies passed; eligible for assignment |
| running | Assigned to a role; work in progress |
| blocked | Lock conflict or external dependency |
| failed  | Gate check failed; may retry |
| passed  | All gates passed; work accepted |

### 2.3 Edges

Directed edges encode dependency: `A → B` means B cannot start until A is `passed`. The graph must be a DAG (no cycles). The engine validates acyclicity on plan creation.

### 2.4 Scheduling

The engine uses topological sort to determine execution order:
1. Compute in-degree for each node.
2. Enqueue nodes with in-degree 0.
3. When a node passes, decrement successors' in-degree.
4. Nodes reaching in-degree 0 become `ready`.

Concurrency is bounded by `max_parallel` (the CLI uses 4). There is no automatic retry: a failed gate leaves the node `failed`, and re-running it is a manual step.

## 3. Roles

| Role | Responsibility |
|------|---------------|
| **Planner** | Creates the work graph (DAG), assigns tags, defines dependencies |
| **Implementer** | Produces patch bundles for assigned nodes |
| **Reviewer** | Reviews patch bundles, runs gate checks, approves or rejects |
| **Red-team** (optional) | Adversarial review — tries to break the change |

### 3.1 Two-Person Rule

Every node that modifies code requires two steps:
1. **Implement**: An Implementer produces a patch bundle and marks the node `running`.
2. **Review**: A Reviewer runs `boruna-orch review <bundle>`, which:
   - Validates bundle format
   - Runs deterministic gates (compile, test, replay)
   - Checks reviewer checklist items
   - Outputs `approve` or `reject`

A node can only reach `passed` if both steps succeed. The Implementer and Reviewer must be different agents (enforced by `assigned_to` field).

## 4. Artifact Types

### 4.1 Patch Bundle (`.patchbundle.json`)

```json
{
  "version": 1,
  "metadata": {
    "id": "PB-20260220-001",
    "intent": "Add list_set opcode for indexed mutation",
    "author": "agent-1",
    "timestamp": "2026-02-20T10:30:00Z",
    "touched_modules": ["boruna-bytecode", "boruna"],
    "risk_level": "low"
  },
  "patches": [
    {
      "file": "crates/llmbc/src/opcode.rs",
      "hunks": [
        {
          "start_line": 45,
          "old_text": "    ListPush,          // 0x83",
          "new_text": "    ListPush,          // 0x83\n    ListSet,           // 0x87"
        }
      ]
    }
  ],
  "expected_checks": {
    "compile": true,
    "test": true,
    "replay": true,
    "diagnostics_count": null
  },
  "reviewer_checklist": [
    "No new language features introduced",
    "Backward compatible with existing bytecode",
    "Tests cover happy path and error cases"
  ]
}
```

### 4.2 Diagnostics Report

Structured JSON output from `boruna-orch report --json`:

```json
{
  "graph_id": "G-001",
  "total_nodes": 5,
  "passed": 3,
  "failed": 1,
  "pending": 1,
  "nodes": [...],
  "locks": [...],
  "last_gate_results": {...}
}
```

### 4.3 Trace Hash

A stable hash produced by `boruna framework trace-hash` used to verify determinism. Stored per-node as part of gate results.

## 5. Conflict Rules

### 5.1 Module-Level Locking

For MVP, locking operates at the crate/module level:

| Lock Target | Granularity | Example |
|------------|-------------|---------|
| Crate | Entire crate directory | `crates/llmbc` |
| Example | Example directory | `examples/admin_crud` |
| Doc | Single file | `docs/reference/ax-language.md` |

### 5.2 Lock Lifecycle

1. When a node transitions to `running`, locks are acquired for all `outputs`.
2. If any lock is held by another node, the requesting node becomes `blocked`.
3. Locks are released when the node reaches `passed` or `failed`.
4. There is no stale-lock timeout and no CLI command to force-release a lock. The library exposes `LockTable::force_release(module)` for callers that need it.

### 5.3 Conflict Detection

Before applying a patch bundle:
1. Check that no locked module overlaps with `touched_modules`.
2. Verify file checksums match expected state (patches apply cleanly).
3. If conflict detected, the apply operation fails and the node becomes `blocked`.

## 6. Deterministic Gating Process

Every patch bundle must pass these gates in order:

| Gate | Command | Pass Criteria |
|------|---------|---------------|
| 1. Compile | `cargo build --workspace` | Exit code 0 |
| 2. Test | `cargo test --workspace` | All tests pass |
| 3. Replay | `cargo run -p boruna-cli -- framework trace-hash <file>` | Hash matches expected |
| 4. Lint (optional) | `cargo clippy --workspace` | No errors (no adapter yet; not run by `apply`/`review`) |

Gate results are recorded per-node:

```json
{
  "node_id": "WN-001",
  "gates": {
    "compile": { "status": "pass", "duration_ms": 3200 },
    "test": { "status": "pass", "duration_ms": 8100, "total": 179, "passed": 179 },
    "replay": { "status": "pass", "hash": "a1b2c3d4e5f6g7h8" }
  }
}
```

If any gate fails, the node becomes `failed` and the patch bundle is rolled back (best-effort).

## 7. Storage

MVP uses local JSON files under `orchestrator/storage/`:

```
orchestrator/storage/
  graphs/
    G-001.json        # work graph
  bundles/
    PB-*.patchbundle.json
  locks/
    locks.json        # active lock table
  gates/
    WN-001.gate.json  # per-node gate results
```

## 8. CLI Commands

| Command | Description |
|---------|-------------|
| `boruna-orch plan <spec.json>` | Create DAG from a plan specification |
| `boruna-orch next --role <role>` | Assign next ready node for the given role |
| `boruna-orch apply <bundle.patchbundle.json>` | Apply patch bundle, run gates |
| `boruna-orch review <bundle.patchbundle.json>` | Review bundle: validate + gates + checklist |
| `boruna-orch status` | Show current graph state |
| `boruna-orch report --json` | Machine-readable summary of graph + gates |

## 9. Adapter Interface

Adapters wrap existing tooling as "judges":

```rust
trait GateAdapter {
    fn name(&self) -> &str;
    fn run(&self, context: &GateContext) -> GateResult;
}
```

Built-in adapters:
- `CompileAdapter` — runs `cargo build --workspace`
- `TestAdapter` — runs `cargo test --workspace`, parses test counts
- `ReplayAdapter` — runs `cargo run -p boruna-cli -- framework trace-hash`, compares hashes
- `DiagAdapter` — runs `cargo run -p boruna-cli -- framework diag`, captures JSON output (informational, always passes)
- `PackageVerifyAdapter` — runs `cargo run -p boruna-pkg -- verify`
- `PackageResolveAdapter` — runs `cargo run -p boruna-pkg -- resolve`
- `LlmMockGateAdapter` — fails if `LLM_BACKEND=external`, otherwise runs `cargo test -p boruna-effect`

`apply` and `review` build their gate list from the bundle's `expected_checks`: `CompileAdapter`, `TestAdapter` and `ReplayAdapter` (the CLI passes no expected hashes, so the replay gate reports `skip`). The other adapters are library API and are not wired into the CLI.

## 10. Non-Goals (MVP)

- Network-distributed agents (local-only for MVP)
- Git integration (manual commits; orchestrator doesn't touch git)
- AST-level patch granularity (file-level hunks for MVP)
- Red-team role automation (manual for now)
- Real-time collaboration (sequential role handoff)
