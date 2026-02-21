# Boruna: Enterprise Execution Platform Migration

> **Status**: Plan — Awaiting approval
> **Date**: 2026-02-21
> **Type**: Major architectural repositioning
> **Scope**: Documentation, orchestrator, CLI, new crate, tests, CI

---

## Overview

Reposition Boruna from "a deterministic, capability-safe programming language" to **"a deterministic, policy-gated, auditable workflow execution runtime for enterprise AI workflows."** The language and VM remain — they become the execution substrate. The new surface area is: workflow DAGs, evidence bundles, governance enforcement, and compliance tooling.

This plan defines 5 implementation phases, each independently shippable and testable. Total estimated scope: ~4,000–6,000 lines of new Rust code + documentation + tests.

---

## Problem Statement

Boruna has all the foundational pieces of an enterprise workflow platform (deterministic VM, capability gating, event sourcing, replay, SHA-256 hashing, DAG scheduling) but they are fragmented across crates and framed as a "programming language." The market positioning, documentation, and developer experience do not communicate the enterprise value proposition.

**What exists today (and maps to enterprise concepts):**

| Existing Component | Enterprise Equivalent |
|---|---|
| `WorkGraph` + `Scheduler` (orchestrator) | Workflow DAG engine |
| `CapabilityGateway` + `Policy` + `PolicySet` + `LlmPolicy` | 3-layer policy enforcement |
| `EventLog` + `ReplayEngine` | Audit trail + replay verification |
| `PatchBundle` + SHA-256 hashing | Audited change management |
| `StateMachine` + `CycleRecord` | State tracking with time-travel |
| `EffectExecutor` + `MockEffectExecutor` | Side-effect management with mock mode |
| Package resolver + lockfiles | Dependency management with integrity |
| `BTreeMap` throughout | Deterministic ordering guarantee |

**What is missing:**

| Gap | Priority | Phase |
|---|---|---|
| Workflow runner (actually executes steps) | P0 | 2 |
| Evidence bundle generation | P0 | 3 |
| Hash-chained audit log | P0 | 3 |
| Schema versioning on all artifacts | P1 | 1 |
| Workflow definition format (enterprise-oriented) | P0 | 2 |
| Step-to-step typed data flow | P1 | 2 |
| Approval gates | P1 | 2 |
| Documentation repositioning | P0 | 4 |
| `HashMap` → `BTreeMap` in orchestrator (determinism bug) | P0 | 1 |
| RBAC / identity | P2 | Gap |
| Real HTTP/DB/Queue handlers | P2 | Gap |
| Daemon/service mode | P2 | Gap |
| Digital signatures on artifacts | P2 | Gap |
| Structured logging (tracing) | P1 | 3 |

---

## Technical Approach

### Design Decisions

**D1: Workflow definition format = JSON manifest + .ax step files**

Extend the existing `WorkGraph` struct in `orchestrator/src/engine/graph.rs` with enterprise workflow semantics. Each workflow is a directory:

```
workflows/my-workflow/
  workflow.json          # DAG definition, metadata, policy refs
  steps/
    fetch_data.ax        # Step implementation (Boruna source)
    transform.ax
    store_results.ax
  policy.json            # Policy for this workflow
```

`workflow.json` schema (new struct `WorkflowDef`):
```json
{
  "schema_version": 1,
  "name": "llm-code-review",
  "version": "1.0.0",
  "description": "Automated code review using LLM analysis",
  "steps": {
    "fetch": {
      "source": "steps/fetch_data.ax",
      "capabilities": ["net.fetch"],
      "inputs": {},
      "outputs": { "code": "String" },
      "timeout_ms": 30000,
      "retry": { "max_attempts": 2, "on_transient": true }
    },
    "analyze": {
      "source": "steps/transform.ax",
      "capabilities": ["llm.call"],
      "inputs": { "code": "fetch.code" },
      "outputs": { "review": "String", "score": "Int" },
      "budget": { "max_tokens": 10000 }
    },
    "approve": {
      "kind": "approval_gate",
      "required_role": "reviewer",
      "depends_on": ["analyze"],
      "condition": "analyze.score < 80"
    },
    "store": {
      "source": "steps/store_results.ax",
      "capabilities": ["db.query"],
      "inputs": { "review": "analyze.review" },
      "depends_on": ["approve"]
    }
  },
  "edges": [
    ["fetch", "analyze"],
    ["analyze", "approve"],
    ["approve", "store"]
  ]
}
```

**Rationale**: JSON is machine-parseable for CI/CD, human-readable for auditors, and already used by the package system. The `.ax` step files leverage the existing compiler pipeline. This avoids extending the language grammar.

**D2: Data flow between steps = serialized `Value` in a run-specific directory**

Each step's outputs are serialized as Boruna `Value` (JSON) to `<run-dir>/outputs/<step-id>/<output-name>.json`. Step inputs reference these by `"<step-id>.<output-name>"`. The runner deserializes them and passes them as function arguments to the step's `fn main()`.

**Rationale**: File-based data flow is deterministic, inspectable, and naturally captured in evidence bundles. It reuses the existing `Value` serialization infrastructure.

**D3: Approval gates = special node type in the DAG**

Add `StepKind::ApprovalGate` variant. When the runner encounters an approval gate:
1. Serialize workflow state to disk
2. Print a message: `"Awaiting approval for step '<id>' (role: <role>). Run: boruna workflow approve <run-id> <step-id>"`
3. Exit with a special exit code (e.g., 42) indicating "paused"
4. `boruna workflow approve <run-id> <step-id>` resumes execution from the serialized state

**Rationale**: Stateless CLI-first approach. No daemon required. Integrates with any external approval system (Slack bot, web UI, CI pipeline) that can call the CLI.

**D4: Evidence bundle = directory with JSON files + checksums**

Not a tar archive initially — a flat directory that can be inspected with standard tools. Archive support can be added later.

```
evidence/<run-id>/
  manifest.json           # BundleManifest with all hashes
  workflow.json            # Frozen workflow definition
  policy.json              # Policy snapshot
  lockfile.json            # Package lockfile snapshot
  event-log.json           # Full EventLog
  audit-log.jsonl          # Hash-chained audit entries
  outputs/                 # Per-step outputs
    fetch/code.json
    analyze/review.json
  checksums.sha256         # SHA-256 of every file
  env-fingerprint.json     # Environment metadata
```

**Rationale**: Flat directory is easier to debug and integrate than archives. SHA-256 checksums provide tamper evidence. JSONL audit log is append-only and grep-friendly.

**D5: HashMap → BTreeMap fix in orchestrator (prerequisite)**

The `Scheduler` in `orchestrator/src/engine/mod.rs` uses `HashMap` and `HashSet` for topological sort computation. The `LockTable` in `orchestrator/src/conflict/mod.rs` also uses `HashMap`. These must be changed to `BTreeMap`/`BTreeSet` before any "deterministic platform" claim.

Additionally, `Policy.rules` in `crates/llmvm/src/capability_gateway.rs:28` uses `HashMap<String, PolicyRule>` and `usage` on line 76 uses `HashMap<String, u64>`. Both must become `BTreeMap`.

---

## Implementation Phases

### Phase 1: Foundation (schema versioning + determinism fix)

**Goal**: Add version fields to all serializable schemas, fix HashMap violations, establish the versioning contract.

**Files to modify:**

| File | Change |
|---|---|
| `orchestrator/src/engine/mod.rs` | Replace `HashMap`/`HashSet` with `BTreeMap`/`BTreeSet` (lines 5, 24, 33, 167, 168) |
| `orchestrator/src/engine/graph.rs` | Add `schema_version: u32` field to `WorkGraph` (default 1) |
| `orchestrator/src/conflict/mod.rs` | Replace `HashMap` with `BTreeMap` (line 7) |
| `crates/llmvm/src/capability_gateway.rs` | Replace `HashMap` with `BTreeMap` on lines 4, 28, 76. Add `version: u32` to `Policy` |
| `crates/llmfw/src/policy.rs` | Add `schema_version: u32` to `PolicySet` (default 1) |
| `crates/llm-effect/src/policy.rs` | Add `schema_version: u32` to `LlmPolicy` (default 1) |

**New files:**

| File | Purpose |
|---|---|
| `crates/llmbc/src/schema_version.rs` | `SchemaVersion` struct (major, minor, patch) + compatibility checking |

**Tests to add/update:**
- Orchestrator determinism test: run `ready_nodes()` 100 times, assert identical order
- Schema version backward compatibility: serialize v1 structs, add version field, deserialize from old JSON with `#[serde(default)]`
- `HashMap` → `BTreeMap` migration: ensure all existing orchestrator tests still pass

**Acceptance criteria:**
- All 504+ existing tests pass
- `cargo clippy --workspace -- -D warnings` clean
- No `HashMap`/`HashSet` in orchestrator crate
- Every serializable schema struct has a `schema_version` or `version` field

---

### Phase 2: Workflow Engine (definition + validator + runner)

**Goal**: Implement `WorkflowDef`, `WorkflowValidator`, and `WorkflowRunner` that can execute a multi-step DAG with typed data flow and approval gates.

**New files:**

| File | Purpose |
|---|---|
| `orchestrator/src/workflow/mod.rs` | Module root |
| `orchestrator/src/workflow/definition.rs` | `WorkflowDef`, `StepDef`, `StepKind`, `DataRef`, `RetryPolicy`, `ApprovalGate` structs |
| `orchestrator/src/workflow/validator.rs` | `WorkflowValidator`: DAG acyclicity, capability existence, input/output type checking, policy compatibility |
| `orchestrator/src/workflow/runner.rs` | `WorkflowRunner`: execute steps in topological order, manage data flow, enforce policies, handle approval gates, produce event logs |
| `orchestrator/src/workflow/data_flow.rs` | `DataStore`: serialize/deserialize step outputs, resolve `DataRef` inputs |
| `orchestrator/src/workflow/evidence.rs` | `EvidenceBundleBuilder`: accumulate evidence during a run |

**Key structs:**

```rust
// orchestrator/src/workflow/definition.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDef {
    pub schema_version: u32,
    pub name: String,
    pub version: String,
    pub description: String,
    pub steps: BTreeMap<String, StepDef>,
    pub edges: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepDef {
    pub kind: StepKind,
    pub capabilities: Vec<String>,
    pub inputs: BTreeMap<String, String>,   // name -> "step_id.output_name"
    pub outputs: BTreeMap<String, String>,   // name -> type
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub retry: Option<RetryPolicy>,
    #[serde(default)]
    pub budget: Option<StepBudget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum StepKind {
    #[serde(rename = "source")]
    Source { source: String },
    #[serde(rename = "approval_gate")]
    ApprovalGate {
        required_role: String,
        #[serde(default)]
        condition: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    #[serde(default)]
    pub on_transient: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepBudget {
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub max_calls: Option<u64>,
}
```

**Runner behavior:**

1. Load `WorkflowDef` from `workflow.json`
2. Validate DAG (reuse existing Kahn's algorithm from `orchestrator/src/engine/mod.rs`)
3. Compute topological order
4. For each step in order:
   a. If `StepKind::ApprovalGate` and condition met → serialize state, exit with code 42
   b. Resolve input `DataRef`s from `DataStore`
   c. Compile `.ax` source file via `boruna_compiler::compile()`
   d. Create `Vm::new(module, gateway)` with step-specific policy
   e. Execute with `Vm::run()` (or `execute_bounded()` for timeout)
   f. Capture `EventLog` entries
   g. Serialize outputs to `DataStore`
   h. Record step evidence (input hash, output hash, capabilities used, tokens consumed, latency)
5. On step failure: check retry policy, retry or mark failed + block dependents
6. On workflow completion: finalize evidence bundle

**Tests:**
- Linear workflow (3 steps): fetch → transform → store (mock mode)
- Diamond workflow: A → B, A → C, B → D, C → D
- Approval gate: verify runner exits with code 42, resume produces correct result
- Policy denial: step requests forbidden capability → workflow fails with audit entry
- Budget exceeded: LLM step exceeds token budget → fail with audit entry
- Data flow: step B reads step A's output correctly
- Retry: transient failure retries, permanent failure does not

**Acceptance criteria:**
- `boruna workflow validate <dir>` validates a workflow directory
- `boruna workflow run <dir> --policy <policy>` executes in mock mode
- `boruna workflow run <dir> --policy <policy> --record` produces evidence
- All new tests pass + existing 504+ tests pass

---

### Phase 3: Evidence & Audit (bundles + hash-chained audit log)

**Goal**: Every workflow run produces a self-contained evidence bundle with a hash-chained audit log.

**New files:**

| File | Purpose |
|---|---|
| `orchestrator/src/audit/mod.rs` | Module root |
| `orchestrator/src/audit/log.rs` | `AuditLog`, `AuditEntry`, `AuditEvent` — hash-chained append-only log |
| `orchestrator/src/audit/evidence.rs` | `EvidenceBundle`, `BundleManifest` — evidence directory builder |
| `orchestrator/src/audit/fingerprint.rs` | `EnvFingerprint` — Boruna version, Rust version, OS, platform |
| `orchestrator/src/audit/verify.rs` | `verify_bundle()` — verify checksums, chain integrity, replay correctness |

**Key structs:**

```rust
// orchestrator/src/audit/log.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub sequence: u64,
    pub prev_hash: String,
    pub event: AuditEvent,
    pub entry_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditEvent {
    WorkflowStarted { workflow_hash: String, policy_hash: String },
    StepStarted { step_id: String, input_hash: String },
    StepCompleted { step_id: String, output_hash: String, duration_ms: u64 },
    StepFailed { step_id: String, error: String },
    CapabilityInvoked { step_id: String, capability: String, allowed: bool },
    PolicyEvaluated { step_id: String, rule: String, decision: String },
    BudgetConsumed { step_id: String, tokens: u64, remaining: u64 },
    ApprovalRequested { step_id: String, role: String },
    ApprovalGranted { step_id: String, approver: String },
    ApprovalDenied { step_id: String, reason: String },
    WorkflowCompleted { result_hash: String, total_duration_ms: u64 },
}

// orchestrator/src/audit/evidence.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    pub schema_version: u32,
    pub run_id: String,
    pub workflow_name: String,
    pub workflow_hash: String,
    pub policy_hash: String,
    pub event_log_hash: String,
    pub audit_log_hash: String,
    pub file_checksums: BTreeMap<String, String>,
    pub env_fingerprint: EnvFingerprint,
    pub started_at: String,
    pub completed_at: String,
    pub bundle_hash: String,
}
```

**Tests:**
- Audit log chain integrity: append 100 entries → `verify()` succeeds
- Audit log tamper detection: modify one entry → `verify()` fails
- Evidence bundle completeness: run workflow → verify all expected files present
- Evidence bundle determinism: run same workflow twice → identical bundle hashes (excluding timestamps)
- Evidence bundle replay: load bundle → replay from event log → identical results
- Verify command: `boruna evidence verify <dir>` succeeds for valid bundle, fails for tampered

**Acceptance criteria:**
- Every `boruna workflow run --record` produces an evidence directory
- `boruna evidence verify <dir>` validates bundle integrity
- Hash chain in audit log is cryptographically verifiable
- Evidence bundle includes: manifest, workflow def, policy, event log, audit log, per-step outputs, checksums, env fingerprint

---

### Phase 4: Documentation & Examples

**Goal**: Rewrite README, create 5 enterprise docs, add 3 example workflows, write gap documentation.

**Files to create/modify:**

| File | Action |
|---|---|
| `README.md` | **Rewrite** — reposition as enterprise execution platform |
| `docs/ENTERPRISE_PLATFORM_OVERVIEW.md` | **Create** — source of truth for the platform vision |
| `docs/PLATFORM_GOVERNANCE.md` | **Create** — policies, RBAC (documented as gap), audit, budgets |
| `docs/OPERATIONS.md` | **Create** — deploy, run, observe, replay, verify |
| `docs/SECURITY_MODEL.md` | **Create** — capabilities, isolation, secrets (gap), threat model |
| `docs/COMPLIANCE_EVIDENCE.md` | **Create** — evidence bundles, audit logs, replay verification |
| `docs/ENTERPRISE_GAPS.md` | **Create** — all gaps with priority, blocker, direction |
| `examples/workflows/llm_code_review/` | **Create** — LLM-powered code review workflow |
| `examples/workflows/document_processing/` | **Create** — document ingestion + classification workflow |
| `examples/workflows/customer_support_triage/` | **Create** — support ticket triage with approval gate |

**README.md new structure:**
1. One-liner: "Boruna is a deterministic execution platform for enterprise AI workflows."
2. What it does (workflow execution, policy enforcement, audit trails, replay)
3. Quick start (install, run a workflow, verify evidence)
4. Example workflow (code review pipeline)
5. Key guarantees (determinism, policy gating, auditability, replay)
6. Architecture overview (workflow → compiler → VM → evidence)
7. CLI reference (workflow, evidence, policy commands)
8. Enterprise documentation links
9. For LLMs and Coding Agents section (updated)

**Example workflows:**

1. **`llm_code_review/`** — Linear workflow: fetch code → LLM analysis → generate report. Demonstrates: LLM capability, budget enforcement, mock mode.

2. **`document_processing/`** — Fan-out workflow: ingest document → parallel (classify, extract entities, summarize) → merge results → store. Demonstrates: parallelism, data flow, DB capability.

3. **`customer_support_triage/`** — Approval-gated workflow: receive ticket → LLM triage → approval gate (if high severity) → route to team. Demonstrates: approval gates, conditional execution, audit trail.

**`docs/ENTERPRISE_GAPS.md` entries (known gaps):**

| Gap | Priority | Blocker |
|---|---|---|
| RBAC / Identity System | P1 | No user authentication; `Policy` is per-run, not per-user |
| Real HTTP Handler | P1 | Only `MockHandler` exists for `NetFetch` |
| Real DB Handler | P1 | Only `MockHandler` exists for `DbQuery` |
| Queue Capability | P2 | No `Capability::Queue` variant in bytecode |
| Secrets Management | P1 | No secure secrets store; env vars are not acceptable |
| Digital Signatures | P2 | Evidence bundles have SHA-256 but no Ed25519 signatures |
| Daemon/Service Mode | P2 | CLI-only; no persistent execution queue or API |
| Structured Logging (tracing) | P1 | No JSON structured logs; needed for production observability |
| Multi-tenancy | P2 | No namespace or tenant isolation |
| Conditional Branching in DAGs | P1 | Workflow DAG is unconditional; no choice/switch nodes |
| Webhook Capability | P2 | No inbound webhook handler |

**Acceptance criteria:**
- README clearly says "execution platform for enterprise AI workflows"
- All 5 enterprise docs exist and are interlinked
- 3 example workflows exist with `workflow.json` + step `.ax` files
- All 3 examples pass `boruna workflow validate`
- All 3 examples run in mock mode via `boruna workflow run --policy allow-all`
- `docs/ENTERPRISE_GAPS.md` lists all known gaps with P0/P1/P2 priority

---

### Phase 5: Test Matrix & CI

**Goal**: Comprehensive test coverage for enterprise features + unified CI script.

**New files:**

| File | Purpose |
|---|---|
| `orchestrator/src/workflow/tests.rs` | Unit tests for workflow engine |
| `orchestrator/src/audit/tests.rs` | Unit tests for audit log and evidence |
| `tests/enterprise/mod.rs` | Integration tests |
| `tests/enterprise/determinism.rs` | Determinism test suite |
| `tests/enterprise/replay.rs` | Replay test suite |
| `tests/enterprise/policy.rs` | Policy enforcement test suite |
| `tests/enterprise/evidence.rs` | Evidence bundle test suite |
| `tests/enterprise/schema_compat.rs` | Schema compatibility tests |
| `scripts/ci.sh` | Unified CI script |

**Test matrix:**

| Category | Test | Validates |
|---|---|---|
| **Determinism** | Same workflow + inputs → identical evidence hash | End-to-end determinism |
| **Determinism** | Same workflow + inputs → identical trace hash | Trace stability |
| **Determinism** | Same workflow + inputs → identical audit log hash | Audit log stability |
| **Replay** | Record → replay → identical outcomes | Replay correctness |
| **Replay** | Replay mode does not invoke capabilities | Replay isolation |
| **Policy** | Deny forbidden capability → step fails + audit entry | Capability enforcement |
| **Policy** | Exceed budget → step fails + audit entry | Budget enforcement |
| **Policy** | LLM model not allowlisted → fail + audit entry | Model allowlist |
| **Auditability** | Evidence bundle has all required files | Bundle completeness |
| **Auditability** | Tamper with one file → verify fails | Tamper detection |
| **Auditability** | Audit log chain → verify succeeds | Chain integrity |
| **Schema** | Deserialize v1 JSON with v2 struct | Backward compatibility |
| **Schema** | Golden file comparison | Schema stability |
| **Packaging** | Same deps → identical lockfile | Lock determinism |
| **Packaging** | Tampered package → integrity fail | Package integrity |
| **Integration** | Run workflow mock → evidence → replay | End-to-end happy path |
| **Integration** | Run workflow with policy deny → audit log | End-to-end denial path |
| **Integration** | LLM mock → cache hit → budget tracking | LLM governance |

**`scripts/ci.sh`:**

```bash
#!/usr/bin/env bash
set -euo pipefail

echo "=== Format check ==="
cargo fmt --all -- --check

echo "=== Clippy (zero warnings) ==="
cargo clippy --workspace -- -D warnings

echo "=== Unit tests ==="
cargo test --workspace

echo "=== Integration tests ==="
cargo test --test enterprise

echo "=== Workflow examples validation ==="
cargo run --bin boruna -- workflow validate examples/workflows/llm_code_review/
cargo run --bin boruna -- workflow validate examples/workflows/document_processing/
cargo run --bin boruna -- workflow validate examples/workflows/customer_support_triage/

echo "=== Workflow examples execution (mock mode) ==="
cargo run --bin boruna -- workflow run examples/workflows/llm_code_review/ --policy allow-all --record
cargo run --bin boruna -- workflow run examples/workflows/document_processing/ --policy allow-all --record
cargo run --bin boruna -- workflow run examples/workflows/customer_support_triage/ --policy allow-all --record

echo "=== All checks passed ==="
```

**Acceptance criteria:**
- `scripts/ci.sh` runs all checks in a single command
- All tests in the test matrix pass
- CI coverage includes: fmt, clippy, unit tests, integration tests, example validation, example execution
- `.github/workflows/ci.yml` updated to also run `scripts/ci.sh` (or its equivalent steps)

---

## CLI Changes

Add new subcommands to the existing CLI at `crates/llmvm-cli/src/main.rs`:

```
boruna workflow validate <dir>           # Validate workflow definition
boruna workflow run <dir> [--policy P]   # Execute workflow
  --policy <path|allow-all|deny-all>     # Policy to apply
  --record                               # Produce evidence bundle
  --mock                                 # Use mock handlers (no real IO)
  --output <dir>                         # Evidence output directory
boruna workflow approve <run-id> <step>  # Approve a paused gate
boruna workflow status <run-id>          # Show execution status
boruna workflow replay <evidence-dir>    # Replay from evidence

boruna evidence verify <dir>             # Verify evidence bundle integrity
boruna evidence inspect <dir>            # Show evidence bundle summary
boruna evidence diff <dir1> <dir2>       # Compare two evidence bundles

boruna policy check <file>               # Validate a policy file
boruna policy show <workflow> <policy>   # Show effective policy for workflow
```

---

## Alternative Approaches Considered

**A1: New `boruna-workflow` crate instead of extending `boruna-orchestrator`**

Rejected. The orchestrator already has `WorkGraph`, `Scheduler`, `PatchBundle`, `LockTable`, and `Store`. The workflow engine is a natural extension of the orchestrator. Creating a separate crate would duplicate the DAG infrastructure.

**A2: Workflow definition in `.ax` source code instead of JSON**

Rejected. JSON is machine-parseable, auditor-friendly, and does not require compiler changes. Step implementations remain in `.ax`, keeping the language relevant. JSON also aligns with enterprise tooling expectations (schema validation, policy scanning, CI integration).

**A3: Cedar policy integration immediately**

Deferred. The existing 3-layer policy system (`Policy` + `PolicySet` + `LlmPolicy`) is sufficient for the initial repositioning. Cedar integration is documented as a future enhancement in `ENTERPRISE_GAPS.md`. The native policy system is simpler to understand and audit.

**A4: Tar/gzip evidence bundles instead of flat directories**

Deferred. Flat directories are easier to debug, inspect with standard tools, and integrate with CI. Archive support can be added as an optional `--archive` flag later.

**A5: Structured logging (tracing crate) in Phase 1**

Deferred to Gap. Adding `tracing` changes all crates' logging behavior. The initial repositioning can succeed with the existing approach. Structured logging is listed as P1 in `ENTERPRISE_GAPS.md`.

---

## Risk Analysis & Mitigation

| Risk | Severity | Likelihood | Mitigation |
|---|---|---|---|
| Scope creep from "enterprise" expectations | High | High | `ENTERPRISE_GAPS.md` sets honest boundaries. Documentation says "execution platform" not "operations platform." |
| Workflow runner complexity | High | Medium | Start with synchronous single-threaded execution. Parallel + async is a future iteration. |
| Breaking existing tests | Medium | Medium | Phase 1 fixes are purely additive (`#[serde(default)]` for version fields) or mechanical (`HashMap` → `BTreeMap`). Run full test suite after each change. |
| Evidence bundle hash instability | Medium | Medium | Use `BTreeMap` for all serialization. Exclude timestamps from hash computation. Add golden-file tests. |
| Approval gate UX friction | Medium | Low | CLI-based approval is adequate for initial release. Document the pattern clearly. |

---

## Dependency Graph

```
Phase 1: Foundation
  ├── HashMap → BTreeMap fix (orchestrator, VM)
  └── Schema versioning (all crates)
          │
          v
Phase 2: Workflow Engine
  ├── WorkflowDef + validator
  ├── WorkflowRunner + data flow
  └── Approval gates
          │
          v
Phase 3: Evidence & Audit
  ├── Hash-chained audit log
  ├── Evidence bundle builder
  └── Verify command
          │
          v
Phase 4: Documentation & Examples
  ├── README rewrite
  ├── 5 enterprise docs
  ├── 3 example workflows
  └── ENTERPRISE_GAPS.md
          │
          v
Phase 5: Test Matrix & CI
  ├── Determinism tests
  ├── Replay tests
  ├── Policy tests
  ├── Evidence tests
  ├── Schema compat tests
  └── scripts/ci.sh
```

Phases 4 and 5 can be partially parallelized — example workflows (Phase 4) inform integration tests (Phase 5), but documentation and gap docs can be written while tests are being implemented.

---

## Definition of Done

1. README + docs clearly describe Boruna as an enterprise execution platform (not "language")
2. `WorkflowDef` format exists with validator and runner
3. Every workflow run with `--record` generates an evidence bundle
4. Policy + budgets + allowlists are enforced and produce audit entries
5. Replay is proven working and blocks external IO
6. 3 enterprise workflows in `examples/workflows/` pass CI
7. All gaps documented in `docs/ENTERPRISE_GAPS.md`
8. `scripts/ci.sh` runs all checks and passes
9. All 504+ existing tests continue to pass
10. Zero clippy warnings, zero fmt issues

---

## References

### Internal
- `orchestrator/src/engine/graph.rs` — existing `WorkGraph`, `WorkNode`, `NodeStatus`
- `orchestrator/src/engine/mod.rs` — existing `Scheduler` with Kahn's algorithm
- `crates/llmvm/src/capability_gateway.rs` — `CapabilityGateway`, `Policy`, `PolicyRule`
- `crates/llmvm/src/replay.rs` — `EventLog`, `ReplayEngine`
- `crates/llmfw/src/policy.rs` — `PolicySet`
- `crates/llm-effect/src/policy.rs` — `LlmPolicy`
- `packages/src/spec/mod.rs` — `PackageManifest`, `Lockfile`
- `docs/DETERMINISM_CONTRACT.md` — determinism guarantees
- `docs/ORCHESTRATOR_SPEC.md` — orchestrator specification

### External
- [Temporal Architecture](https://github.com/temporalio/temporal/blob/main/docs/architecture/README.md) — workflow/activity separation pattern
- [Restate: Durable Execution from First Principles](https://www.restate.dev/blog/building-a-modern-durable-execution-engine-from-first-principles) — journal-based recovery
- [Demystifying Determinism in Durable Execution](https://jack-vanlightly.com/blog/2025/11/24/demystifying-determinism-in-durable-execution) — determinism scope analysis
- [Cedar Policy Language](https://docs.cedarpolicy.com/) — future ABAC integration
- [OpenTelemetry GenAI Agent Spans](https://opentelemetry.io/docs/specs/semconv/gen-ai/gen-ai-agent-spans/) — observability conventions
- [petgraph crate](https://docs.rs/petgraph) — DAG primitives
- [sha2 crate](https://docs.rs/sha2) — already in use for integrity hashing
