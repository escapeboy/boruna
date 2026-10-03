# Platform Governance

## Policy System

Boruna enforces policies at multiple levels:

### VM-Level Policy
The `Policy` struct controls capability access at the VM level:
- `rules: BTreeMap<String, PolicyRule>` — per-capability allow/deny with budget
- `default_allow: bool` — default behavior for undeclared capabilities
- `schema_version: u32` — for forward compatibility

Built-in policies: `Policy::allow_all()`, `Policy::deny_all()`.

### Framework-Level PolicySet
For framework apps, `PolicySet` adds application-specific constraints:
- `capabilities` — allowed capability names
- `max_effects_per_cycle` — rate limit on effects per update cycle (0 = unlimited)
- `max_steps` — max VM steps per function call (0 = unlimited)

### LLM Policy
`LlmPolicy` controls LLM-specific behavior:
- `total_token_budget` — total tokens allowed across all LLM calls (0 = unlimited)
- `max_calls` — maximum number of LLM calls (0 = unlimited)
- `allowed_models` — approved model names (empty = all allowed)
- `max_context_bytes` — per-call context limit in bytes (0 = unlimited)
- `prompt_allowlist` — allowed prompt IDs (empty = all allowed)

## Budget Enforcement

Budgets are enforced at the step level:
```json
{
  "budget": {
    "max_tokens": 10000,
    "max_calls": 5
  }
}
```

When a budget is exceeded, the step fails with an auditable error.

## Approval Gates

Workflow steps can be approval gates that pause execution:
```json
{
  "kind": "approval_gate",
  "required_role": "reviewer",
  "condition": "severity >= 3"
}
```

`condition` is informational; the runner does not enforce it.

When reached, the workflow pauses with status `Paused` and records an `ApprovalRequested` audit event.

An approval gate can also carry an optional `confidence_gate` (`source_step`, `calibration`, `alpha_permille`). When the source step's calibrated confidence score clears the threshold, the gate completes without a human; otherwise it pauses as above. It is rejected with `--submit-only`. See [workflow DAG spec](spec/workflow-dag-1.0.md) and `docs/design-conformal-gating.md`.

## Audit Log

Every workflow run produces a hash-chained audit log. Events include:
- `WorkflowStarted` — workflow and policy hashes
- `StepStarted` / `StepCompleted` / `StepFailed`
- `CapabilityInvoked` — with allow/deny decision
- `PolicyEvaluated` — rule evaluation details
- `BudgetConsumed` — token/call consumption
- `ApprovalRequested` / `ApprovalGranted` / `ApprovalDenied`
- `ExternalTriggerReceived` — external-trigger gate advanced via `boruna workflow trigger` (records the payload hash, not the payload)
- `WorkflowCompleted` — result hash and duration

Each entry's hash includes the previous entry's hash, forming a tamper-evident chain.

## RBAC (Gap)

Currently, policies are per-run rather than per-user. A full RBAC system is documented as a P1 gap in [`archive/ENTERPRISE_GAPS.md`](archive/ENTERPRISE_GAPS.md). The current model:
- Workflow author defines the policy
- CLI operator selects which policy to apply
- Approval gates specify a required role (string-based, not yet identity-verified)
