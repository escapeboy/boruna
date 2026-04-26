# Limitations

Boruna has real constraints. This document describes them clearly, so you can make an informed decision about whether it fits your use case.

## Language limitations

**No mutable variables.** `.ax` variables are immutable. State transitions use record spread (`State { ..old, field: new_value }`). This is intentional for determinism, but requires a different style than imperative code.

**No loops.** `.ax` does not have `for` or `while` loops. Use recursion or standard library functions. The step-limit mechanism prevents infinite loops from hanging execution, but deep recursion can hit the limit.

**No generics.** Types in `.ax` are concrete at definition time. There is no generic type system. This keeps the language simple but limits abstraction.

**No imports.** `.ax` files cannot import other `.ax` files. Standard library access is through the package system, not file imports. Large workflows are composed at the workflow DAG level, not at the language level.

**String processing is limited.** The standard library provides basic string operations, but `.ax` is not designed for complex text transformation. Use LLM capabilities for natural language processing; use compiled Rust for heavy text manipulation.

## VM limitations

**Step limit is a blunt instrument.** The `--step-limit` flag prevents runaway execution but does not provide fine-grained CPU time control. A step that does 10M arithmetic operations may run longer than expected before hitting the limit.

**Wave-based concurrent execution, not full DAG parallelism.** The runner processes steps in topological waves with `--concurrency <N>` workers per wave (sprint `0.3-S4`). A slow step at level N blocks fast steps at level N+1 even if they don't actually depend on the slow one. A full DAG-based scheduler (no wave boundaries) is not yet implemented.

**Actor system is single-process.** Actors run in the same OS process with round-robin scheduling. There is no distributed actor execution.

**Memory is unbounded within a step.** The VM does not enforce memory limits on individual step execution. A step that allocates large lists or maps may use significant memory.

## Capability limitations

**The HTTP handler requires a feature flag.** Real HTTP calls require building with `--features boruna-cli/http`. This is a build-time decision, not a runtime one.

**LLM calls use Bring Your Own Handler (BYOH).** The `llm.call` capability is declared and enforced by the VM, and `boruna-effect` provides prompt / cache / context primitives — but no default network-calling handler ships in core. Wire your provider (OpenAI, Anthropic, vLLM, Ollama, custom router) by implementing `CapabilityHandler` and passing it to `CapabilityGateway::with_handler`. See [LLM Integration Guide](./guides/llm-integration.md) for the contract, examples, and rationale. A reference OpenAI handler lives at `examples/llm_handlers/openai/`.

**No streaming.** Capability calls are synchronous and blocking. LLM responses must complete before the step continues. This is unsuitable for streaming chat interfaces.

**SSRF protection is allowlist-based.** The HTTP handler rejects private IP ranges and localhost, but allowlisting specific domains requires extending the `NetPolicy` struct. There is no UI for this.

## Workflow limitations

**No async steps.** Steps run synchronously. A step that needs to wait for an external event (webhook callback, human approval via an external system) cannot be expressed natively. Approval gates pause the workflow synchronously and require an operator (`boruna workflow approve`) to advance — this is intentional, but does not generalize to webhook-triggered resumption. On the 0.3.0 roadmap.

**`.ax` step-input access not yet wired through.** Workflow steps can declare `inputs: { document: "ingest.result" }` and the runner validates references at compile time, but `.ax` step bodies cannot yet *read* those resolved input values at runtime — steps remain self-contained today. The persisted output flows correctly between waves (downstream steps see upstream outputs in the data store), but the language-level access requires compiler work (`Op::CapCall` emission for capability-annotated functions). On the 0.3.0 roadmap.

## Evidence and audit limitations

**Evidence bundles are local files.** There is no built-in mechanism to ship evidence bundles to a remote store. You must handle this yourself (copy to S3, push to a document store, etc.).

**LLM response reproducibility is not guaranteed.** Evidence bundles capture LLM responses for replay, but if the LLM provider changes their model weights, a replay may produce different outputs if the real capability is used. Replay with recorded responses is always reproducible.

**No evidence bundle encryption.** Evidence bundles are written as plaintext JSON. If they contain sensitive data (customer information, proprietary prompts), you must handle encryption at the storage layer.

## Operational limitations

**No web UI.** There is no dashboard for workflow history, run status, or evidence inspection. All interaction is via the CLI.

**No authentication.** Boruna has no built-in access control. Access to the CLI and evidence bundles depends on your file system permissions.

**No multi-tenancy.** Boruna is designed for single-team deployment. Running workflows for multiple isolated teams requires OS-level separation.

**Minimum Rust version: 1.75.0.** Teams running older Rust toolchains will need to upgrade.

## What to do if these limitations block you

File an issue at https://github.com/escapeboy/boruna. Limitations that frequently block real use cases will be prioritized in the roadmap.

See also: [Roadmap](./roadmap.md), [Stability](./stability.md)
