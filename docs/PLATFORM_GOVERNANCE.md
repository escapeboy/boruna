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
- `max_cycles` — maximum update cycles
- `allowed_effects` — effect type allowlist
- `max_effects_per_cycle` — rate limit on effects

### LLM Policy
`LlmPolicy` controls LLM-specific behavior:
- `total_token_budget` — total tokens allowed
- `max_context_tokens` — per-request context limit
- `model_allowlist` — approved model identifiers
- `cache_policy` — caching behavior (Always, Never, Conditional)

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

When reached, the workflow pauses with status `Paused` and records an `ApprovalRequested` audit event.

## Audit Log

Every workflow run produces a hash-chained audit log. Events include:
- `WorkflowStarted` — workflow and policy hashes
- `StepStarted` / `StepCompleted` / `StepFailed`
- `CapabilityInvoked` — with allow/deny decision
- `PolicyEvaluated` — rule evaluation details
- `BudgetConsumed` — token/call consumption
- `ApprovalRequested` / `ApprovalGranted` / `ApprovalDenied`
- `WorkflowCompleted` — result hash and duration

Each entry's hash includes the previous entry's hash, forming a tamper-evident chain.

## RBAC (Gap)

Currently, policies are per-run rather than per-user. A full RBAC system is documented as a P1 gap in `ENTERPRISE_GAPS.md`. The current model:
- Workflow author defines the policy
- CLI operator selects which policy to apply
- Approval gates specify a required role (string-based, not yet identity-verified)
