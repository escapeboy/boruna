# Boruna Documentation

## Start here

- [Quickstart](./QUICKSTART.md) — build, run a workflow, inspect an evidence bundle
- [FAQ](./faq.md) — common questions answered directly

## Concepts

Core ideas that make Boruna work:

- [Determinism](./concepts/determinism.md) — why same inputs → same outputs, and how it's enforced
- [Capabilities](./concepts/capabilities.md) — declaring and gating side effects
- [Evidence Bundles](./concepts/evidence-bundles.md) — tamper-evident audit logs and replay

## Guides

Task-oriented walkthroughs:

- [Your First Workflow](./guides/first-workflow.md) — build a two-step workflow from scratch

## Reference

Complete API and format documentation:

- [CLI Reference](./reference/cli.md) — all `boruna` commands and options
- [.ax Language Reference](./reference/ax-language.md) — syntax, types, capabilities

## Product

- [Stability](./stability.md) — what is stable, experimental, and planned
- [Roadmap](./roadmap.md) — 0.2.0 through 1.0.0
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
