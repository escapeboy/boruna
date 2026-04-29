# Boruna Documentation

## Start here

- [Quickstart](./QUICKSTART.md) — build, run a workflow, inspect an evidence bundle
- [FAQ](./faq.md) — common questions answered directly

## Concepts

Core ideas that make Boruna work:

- [Determinism](./concepts/determinism.md) — why same inputs → same outputs, and how it's enforced
- [Capabilities](./concepts/capabilities.md) — declaring and gating side effects
- [Evidence Bundles](./concepts/evidence-bundles.md) — tamper-evident audit logs and replay
- [Bundle Storage](./concepts/bundle-storage.md) — local + remote (S3/GCS/Azure) destinations for evidence bundles

## Guides

Task-oriented walkthroughs:

- [Your First Workflow](./guides/first-workflow.md) — build a two-step workflow from scratch
- [LLM Integration](./guides/llm-integration.md) — Bring Your Own Handler model: how to wire OpenAI / Anthropic / vLLM / custom routers via the `CapabilityHandler` trait
- [Coordinator HA](./guides/coord-ha.md) — multi-coord deployment topologies, health endpoint, worker URL failover
- [Coordinator mTLS](./guides/coord-mtls.md) — X.509 client certs, cert generation recipe, identity reconciliation
- [Worker Capability Tagging](./guides/worker-capability-tagging.md) — heterogeneous fleet placement
- [Migration](./guides/migration.md) — `boruna migrate` for upgrading legacy bundles and workflow files

## Standard Libraries

Reference pages for all 11 built-in `std-*` packages:

- [std-ui](./reference/stdlib/std-ui.md) — declarative UI primitives
- [std-forms](./reference/stdlib/std-forms.md) — model-driven form engine
- [std-authz](./reference/stdlib/std-authz.md) — role and permission enforcement
- [std-http](./reference/stdlib/std-http.md) — typed HTTP effect wrappers
- [std-db](./reference/stdlib/std-db.md) — typed database query helpers
- [std-sync](./reference/stdlib/std-sync.md) — offline queue and conflict resolution
- [std-validation](./reference/stdlib/std-validation.md) — composable validation rules
- [std-routing](./reference/stdlib/std-routing.md) — declarative routing model
- [std-storage](./reference/stdlib/std-storage.md) — typed local persistence abstraction
- [std-notifications](./reference/stdlib/std-notifications.md) — notification queue and toast helpers
- [std-testing](./reference/stdlib/std-testing.md) — test helpers for framework apps

See [Stdlib Libraries](./STD_LIBRARIES_SPEC.md) for the full spec and [stdlib-graduation-tracker.md](./stdlib-graduation-tracker.md) for graduation status.

## Reference

Complete API and format documentation:

- [CLI Reference](./reference/cli.md) — all `boruna` commands and options
- [.ax Language Reference](./reference/ax-language.md) — syntax, types, capabilities (informal narrative; see also `spec/ax-language-1.0.md` for the frozen formal spec)
- [MCP Server Tool Reference](./reference/mcp-server.md) — wire contract for all `boruna-mcp` tools (parameters, return shapes, `error_kind` values)
- [Capability Policy Schema](./reference/policy-schema.md) — structured `policy` parameter for `boruna_run` and the CLI

## Versioned Specifications

Frozen contracts (LTS-protected at 1.0 GA):

- [`.ax` Language 1.0](./spec/ax-language-1.0.md) — formal grammar, type rules, capability semantics
- [Workflow DAG 1.0](./spec/workflow-dag-1.0.md) — `workflow.json` schema with `schema_version: 1`
- [Evidence Bundle 1.0](./spec/evidence-bundle-1.0.md) — `bundle.json` manifest format and encryption envelope
- [Spec Index](./spec/README.md) — versioning policy and reader contract

## Product

- [LTS Contract](./lts.md) — support windows, deprecation policy, security backport SLAs (effective at 1.0 GA)
- [Performance](./PERFORMANCE.md) — baseline numbers and 1.x performance budget commitments
- [Stability](./stability.md) — what is stable, experimental, and planned
- [Roadmap](./roadmap.md) — 0.2.0 through 1.0.0 and beyond
- [Limitations](./limitations.md) — real constraints, stated honestly

## Specifications

Deep technical documentation:

- [Language Guide](./language-guide.md) — extended .ax language documentation
- [Determinism Contract](./DETERMINISM_CONTRACT.md) — formal determinism specification
- [Framework Spec](./FRAMEWORK_SPEC.md) — Elm-architecture app protocol
- [Compliance Evidence](./COMPLIANCE_EVIDENCE.md) — evidence bundle format specification
- [Platform Governance](./PLATFORM_GOVERNANCE.md) — enterprise execution governance model
- [Enterprise Platform Overview](./ENTERPRISE_PLATFORM_OVERVIEW.md) — workflow engine architecture
- [Security Model](./SECURITY_MODEL.md) — threat model and security properties
- [Operations Guide](./OPERATIONS.md) — deployment and operational guidance
- [Package Spec](./PACKAGE_SPEC.md) — `package.ax.json` format and registry protocol
- [Stdlib Libraries](./STD_LIBRARIES_SPEC.md) — standard library APIs
- [Diagnostics and Repair](./DIAGNOSTICS_AND_REPAIR.md) — `lang check` and `lang repair`
- [Effects Guide](./EFFECTS_GUIDE.md) — LLM effect integration
- [Actors Guide](./ACTORS_GUIDE.md) — actor system and multi-agent patterns
- [Testing Guide](./TESTING_GUIDE.md) — testing `.ax` programs and workflows
- [Trace to Tests](./TRACE_TO_TESTS.md) — generating regression tests from traces

## Examples

- [LLM Code Review](../examples/workflows/llm_code_review/) — linear 3-step workflow
- [Document Processing](../examples/workflows/document_processing/) — fan-out parallelism
- [Customer Support Triage](../examples/workflows/customer_support_triage/) — approval gate
