# Capabilities

A capability is an explicit permission for a workflow step to perform a side effect. No capability, no side effect — the VM enforces this unconditionally.

## The ten capabilities

| Capability | Effect | Example use |
|------------|--------|-------------|
| `net.fetch` | HTTP requests | Calling external APIs, webhooks |
| `llm.call` | LLM inference | GPT-4, Claude, local models |
| `time.now` | Current timestamp | Timestamping records |
| `rand.next` | Random numbers | Sampling, tie-breaking |
| `fs.read` | File system reads | Loading documents, configs |
| `fs.write` | File system writes | Writing reports, outputs |
| `db.query` | Database access | Reading/writing records |
| `db.mutate` | Database mutations | Write operations specifically |
| `actor.spawn` | Actor creation | Spawning parallel agents |
| `actor.send` | Inter-actor messaging | Coordinating actor state |

## Declaring capabilities

Capabilities are declared on functions using the `!{...}` annotation:

```ax
fn call_model(prompt: String) -> String !{llm.call} {
    // live mode: calls LLM
}

fn fetch_and_parse(url: String) -> String !{net.fetch} {
    // live mode: makes HTTP request
}
```

A function without a capability annotation is pure — it cannot perform I/O and its output depends only on its inputs.

## Policies

A policy is a set of allowed capabilities. It is specified at runtime, not in the workflow definition. This separation means the same workflow can run in restricted mode during testing and with full capabilities in production.

Built-in policies:

| Policy | Allowed capabilities |
|--------|---------------------|
| `allow-all` | All 10 capabilities |
| `deny-all` | None |
| `default` | None (same as deny-all) |

Pass a policy on the CLI:

```bash
boruna workflow run my-workflow/ --policy allow-all
```

In demo mode (no `--live` flag), capability calls are stubbed or skipped. In live mode, the policy is enforced against every capability call.

## Capability enforcement in the VM

Every capability call in compiled bytecode goes through the `CapabilityGateway`:

1. The VM encounters a capability opcode.
2. The gateway checks the active policy.
3. If the capability is not allowed, the VM returns a `CapabilityDenied` error immediately.
4. If allowed, the call is dispatched to the registered handler (real HTTP, LLM client, etc.).
5. The call and its result are written to the `EventLog`.

This enforcement happens at the bytecode level, before any handler executes. There is no way to bypass it from `.ax` code.

## Capabilities in workflow definitions

Workflow steps declare their required capabilities in `workflow.json`. This makes the capability surface area visible before execution:

```json
{
  "steps": [
    {
      "id": "analyze",
      "source": "steps/analyze.ax",
      "capabilities": ["llm.call"]
    }
  ]
}
```

The workflow validator checks that declared capabilities are consistent with the policy before the workflow runs.

## Capabilities in package manifests

Standard libraries that require capabilities declare them in `package.ax.json`:

```json
{
  "name": "std-http",
  "capabilities_required": ["net.fetch"]
}
```

This gives visibility into the transitive capability requirements of a dependency graph.

See also: [Determinism](./determinism.md), [Policies](../PLATFORM_GOVERNANCE.md)
