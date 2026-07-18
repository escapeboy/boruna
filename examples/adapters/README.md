# Boruna as an auditable execution cell

These adapters show how to call Boruna from an external agent framework
(LangGraph, Temporal, OpenAI Agents SDK, …) as a **deterministic, auditable
execution cell**: you hand Boruna an `.ax` program, it runs it under a
capability policy, and it hands back not just the result but a **verifiable
execution record**.

The tool that makes this work is **`boruna_run_sealed`** on Boruna's MCP
server. It runs the program, captures the VM's `EventLog`, re-executes it a
second time feeding the recorded capability results back, and compares the two
logs (`ReplayEngine::verify_full`). You get:

| field | meaning |
|-------|---------|
| `result` | the program's return value |
| `replay_verified` | `true` only if the second run reproduced every event identically |
| `capability_calls` | ordered list of capability calls the run made |
| `event_log` | the full event log (capability calls/results, actor events, UI emits, contract checks) |
| `event_log_sha256` | SHA-256 of the canonical event log — a stable **seal handle** to pin |
| `steps` | deterministic step count |

## What "sealed" means (and does not)

The seal here is a **replay-verified event log**, *not* a signed evidence
bundle. That is deliberate and honest:

- A single MCP call is a stateless `source`-string invocation. It has no
  workflow definition, no data store, and no output directory.
- A full **signed, hash-chained evidence bundle** is a workflow-directory
  artifact produced by the **orchestrator**, not by a single run. Produce one
  with:

  ```bash
  boruna workflow run <workflow-dir> --policy allow-all --record
  boruna evidence verify <bundle-dir>
  ```

`boruna_run_sealed` returns the strongest artifact a single run genuinely
produces — a deterministic replay proof plus a digest — and says so in its
`seal.note`. It never fabricates a bundle. If you need the signed bundle, use
the workflow path above.

## Registering the MCP server

Boruna ships an MCP server binary, `boruna-mcp`, speaking JSON-RPC over stdio.

`.mcp.json` (Claude Code) or equivalent:

```json
{
  "mcpServers": {
    "boruna": {
      "command": "cargo",
      "args": ["run", "--bin", "boruna-mcp", "--manifest-path", "/path/to/ai-lang/Cargo.toml"],
      "env": {}
    }
  }
}
```

Or, with the binary on `PATH`, just `command: "boruna-mcp"`.

## Calling the tool

Arguments:

```jsonc
{
  "source": "fn main() -> Int { 2 + 40 }\n",  // required, .ax source (≤ 1 MB)
  "policy": "allow-all",                        // optional: "allow-all" | "deny-all" | policy object
  "max_steps": 10000000                          // optional deterministic ceiling
}
```

Success response (abridged):

```jsonc
{
  "success": true,
  "protocol_version": 1,
  "result": 42,
  "steps": 6,
  "replay_verified": true,
  "replay_divergence_reason": null,
  "event_count": 0,
  "capability_calls": [],
  "event_log": { "version": 2, "events": [], "truncated": false },
  "event_log_sha256": "5df6e0e2761359d30a8275058e299fcc0381534545f55cf43e41983f5d4c9456",
  "seal": {
    "kind": "replay-verified-event-log",
    "verified": true,
    "digest_alg": "sha256",
    "digest": "5df6…9456",
    "note": "This seal is a deterministic replay proof over the VM EventLog … not a signed evidence bundle …"
  }
}
```

Domain errors come back as `success: false` with a stable `error_kind`
(`parse_error`, `runtime_error`, `capability_denied`, `invalid_policy`, …) —
they are returned as successful tool responses, not MCP transport errors, so
callers branch on the JSON.

## The adapters

- **`langgraph_node.py`** — a LangGraph node that calls `boruna_run_sealed` and
  writes `{result, replay_verified}` into graph state so downstream nodes can
  branch on reproducibility.
- **`temporal_activity.py`** — a Temporal **Activity** wrapping the same call
  (external I/O belongs in an Activity, not in deterministic workflow code),
  returning a small result the workflow can persist or assert on.

Both mark their MCP-client plumbing (`call_mcp_tool`) as **pseudo-code** — wire
it to your MCP client of choice (e.g. the `mcp` Python SDK's stdio client, or
LangChain's MCP adapters). Everything else is real.

### CLI fallback

If you would rather not run the MCP server, the CLI covers the run itself
(without the replay-verified envelope):

```bash
boruna run program.ax --policy allow-all
```

For the full audited path with a signed bundle, use `boruna workflow run
--record` + `boruna evidence verify` as shown above.
