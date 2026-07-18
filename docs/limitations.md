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

**Wall-clock-keyed enforcement is non-deterministic on failure.** Limits like `max_wall_ms` are wall-clock-keyed: a workflow that completes within budget produces deterministic output, but one that times out may finish on a fast machine and time out on a slow one. Documented per integrator surface (limits, OTel spans).

## Evidence and audit limitations

**Evidence bundles are local files; remote storage is operator-owned.** Evidence bundles write to `<data-dir>/runs/<run-id>/`. Pluggable storage adapters (S3 / object storage / document store) are roadmap 0.7.x or 1.x. Today, ship bundles to remote storage with your own pipeline (rsync, S3 upload, etc.).

**LLM response reproducibility is not guaranteed.** Evidence bundles capture LLM responses for replay, but if the LLM provider changes their model weights, a replay may produce different outputs if the real capability is used. Replay with recorded responses (sprint 0.5-S7 of FleetQ track) is always reproducible.

**Evidence bundle encryption KEK is operator-managed.** Sprint W6-B added envelope AES-256-GCM encryption — operators supply the KEK via env var or CLI flag. Boruna does not ship key management (no HSM/KMS integration); KEK lifecycle (storage, rotation, sealing) is the operator's responsibility. Key rotation tooling is roadmap.

**Plaintext bundle.json metadata.** Even with `--encrypt-bundle`, the top-level `bundle.json` manifest is plaintext (chicken-and-egg with the wrapped DEK). It carries `format_version`, `boruna_version`, `run_id`, `workflow_hash`, and the wrapped DEK envelope. Run identifiers may be visible to a bundle inspector even when payload bytes are encrypted.

## Operational limitations

**Multi-tenancy is environment-namespaced, not cryptographically isolated.** The `--env` flag (sprint 0.4-S14) namespaces the data-dir and Prometheus labels per-environment. This separates run histories but does not provide cryptographic isolation between tenants — that requires OS-level separation or per-tenant deployments.

**Minimum Rust version: 1.75.0.** Teams running older Rust toolchains will need to upgrade.

## What to do if these limitations block you

File an issue at https://github.com/escapeboy/boruna. Limitations that frequently block real use cases will be prioritized in the roadmap.

See also: [Roadmap](./roadmap.md), [Stability](./stability.md)
