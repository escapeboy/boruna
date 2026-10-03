# Summary

<!--
The published pages of the docs site, in navigation order. Paths are repository paths, so a
link between two published pages works the same on GitHub and on the site. A page that is not
listed here is not published; a listed page that does not exist fails the build.
Internal notes (design-*, architecture-*, test-plan-*, retros, archive/) stay in the repo only.
-->

[Introduction](README.md)

# Get started

- [Quickstart](docs/QUICKSTART.md)
- [Your first workflow](docs/guides/first-workflow.md)
- [Limitations](docs/limitations.md)
- [FAQ](docs/faq.md)

# Concepts

- [Determinism](docs/concepts/determinism.md)
- [Capabilities](docs/concepts/capabilities.md)
- [Evidence bundles](docs/concepts/evidence-bundles.md)
- [Evidence bundle threat model](docs/concepts/threat-model.md)
- [Runtime execution provenance](docs/concepts/runtime-execution-provenance.md)
- [Bundle storage](docs/concepts/bundle-storage.md)

# Guides

- [Compliance evidence](docs/COMPLIANCE_EVIDENCE.md)
- [LLM integration](docs/guides/llm-integration.md)
- [Model evaluation](docs/guides/model-eval.md)
- [Testing apps](docs/TESTING_GUIDE.md)
- [Traces to regression tests](docs/TRACE_TO_TESTS.md)
- [App template](docs/APP_TEMPLATE.md)
- [Framework effects](docs/EFFECTS_GUIDE.md)
- [Actors](docs/ACTORS_GUIDE.md)
- [Operations](docs/OPERATIONS.md)
- [Language server (LSP)](docs/guides/lsp.md)
- [Migration tooling](docs/guides/migration.md)
- [Bundle storage on S3](docs/guides/bundle-storage-s3.md)
- [Bundle storage on Google Cloud Storage](docs/guides/bundle-storage-gcs.md)
- [Bundle storage on Azure](docs/guides/bundle-storage-azure.md)
- [KEK rotation](docs/guides/kek-rotation.md)

# Reference

- [CLI guide](docs/reference/cli.md)
- [Command reference (generated)](docs/reference/cli-commands.md)
- [.ax language](docs/reference/ax-language.md)
- [MCP server](docs/reference/mcp-server.md)
- [Policy schema](docs/reference/policy-schema.md)
- [Capability identity](docs/reference/capability-identity.md)
- [Diagnostic codes](docs/reference/diagnostic-codes.md)
- [Diagnostics and repair](docs/DIAGNOSTICS_AND_REPAIR.md)
- [Error kinds](docs/reference/error-kinds.md)
- [Framework API](docs/FRAMEWORK_API.md)
- [Compliance workflow templates](docs/reference/compliance/README.md)
- [Standard libraries]()
  - [std-authz](docs/reference/stdlib/std-authz.md)
  - [std-db](docs/reference/stdlib/std-db.md)
  - [std-forms](docs/reference/stdlib/std-forms.md)
  - [std-http](docs/reference/stdlib/std-http.md)
  - [std-json](docs/reference/stdlib/std-json.md)
  - [std-llm](docs/reference/stdlib/std-llm.md)
  - [std-notifications](docs/reference/stdlib/std-notifications.md)
  - [std-routing](docs/reference/stdlib/std-routing.md)
  - [std-storage](docs/reference/stdlib/std-storage.md)
  - [std-sync](docs/reference/stdlib/std-sync.md)
  - [std-testing](docs/reference/stdlib/std-testing.md)
  - [std-ui](docs/reference/stdlib/std-ui.md)
  - [std-validation](docs/reference/stdlib/std-validation.md)

# Specifications

- [Overview](docs/spec/README.md)
- [.ax language](docs/spec/ax-language-1.0.md)
- [Bytecode](docs/spec/bytecode-1.0.md)
- [Workflow DAG](docs/spec/workflow-dag-1.0.md)
- [Evidence bundle](docs/spec/evidence-bundle-1.0.md)
- [Runtime provenance predicate](docs/spec/runtime-provenance-predicate-1.0.md)
- [Framework (App protocol)](docs/FRAMEWORK_SPEC.md)
- [Orchestrator](docs/ORCHESTRATOR_SPEC.md)
- [Packages](docs/PACKAGE_SPEC.md)
- [Determinism contract](docs/DETERMINISM_CONTRACT.md)

# Project

- [Platform overview](docs/ENTERPRISE_PLATFORM_OVERVIEW.md)
- [Security model](docs/SECURITY_MODEL.md)
- [Platform governance](docs/PLATFORM_GOVERNANCE.md)
- [Performance](docs/PERFORMANCE.md)
- [Roadmap](docs/roadmap.md)
- [Security policy](SECURITY.md)
- [Changelog](CHANGELOG.md)
