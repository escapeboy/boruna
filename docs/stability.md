# Stability and Maturity

Boruna is at version **4.2.0**. It is a local engine and CLI. Since 3.0.0
there is no server component.

Only the latest major line (4.x) is supported. 1.x, 2.x and 3.x are end
of life. The support, compatibility and deprecation rules are in
[`lts.md`](./lts.md). The **Stable** tier below is what [`lts.md`](./lts.md)
§B covers. Experimental and Alpha components may change in minor releases.

## Current status

Suitable for:

- Evaluation, proof-of-concept and internal tooling
- Audit-sensitive AI pipelines that run locally
- Production use of the Stable surfaces, after checking
  [`PERFORMANCE.md`](./PERFORMANCE.md) and
  [`limitations.md`](./limitations.md) against your workload

Not suitable for:

- Environments that require a third-party security audit. No external
  audit has been done.
- Anything that needs a hosted server, distributed workers or a web
  dashboard. These were removed in 3.0.0.

## Stability tiers

### Stable (covered by [`lts.md`](./lts.md) §B)

- **`.ax` language 2.0**: syntax, type system, pattern matching, records,
  enums, `let mut`, loops with `break` / `continue`, capability built-ins. Spec:
  [`spec/ax-language-1.0.md`](./spec/ax-language-1.0.md).
- **VM execution**: bytecode format 1.1, capability enforcement,
  determinism.
- **Workflow DAG schema 1**: `workflow.json` with `schema_version: 1`.
  Spec: [`spec/workflow-dag-1.0.md`](./spec/workflow-dag-1.0.md).
- **Evidence bundle format 1.1**: hash-chained audit log, `bundle.json`,
  optional AES-256-GCM encryption, verifiable redaction. Reads 1.0
  bundles. From 4.1, a bundle whose approvals carry a reason (or whose
  rejections carry a name) is format 2.0, which 4.0 refuses as unsupported;
  4.1 reads 1.x and 2.x. Spec: [`spec/evidence-bundle-1.0.md`](./spec/evidence-bundle-1.0.md).
- **Capability set**: additions only within the major line.
- **CLI commands**: `run`, `compile`, `workflow validate/run/approve/reject/trigger/resume`,
  `evidence inspect/verify/gc-blobs/rotate-kek`, `migrate`, `new`,
  `lang check/repair`, `template list/apply`.
- **Bundle storage adapters**: local filesystem, and S3, GCS and Azure
  Blob behind the `s3`, `gcs` and `azure` cargo features.
- **MCP tool responses**: `protocol_version: 1` on every response from
  the 14 `boruna-mcp` tools.
- **Standard libraries**: 13 `std-*` packages. See [`lts.md`](./lts.md)
  §B.7 and [`stdlib-graduation-tracker.md`](./stdlib-graduation-tracker.md).

CLI commands not in the list above are Experimental.

### Experimental (may change in minor releases)

- **Actor system**: spawning, message passing, supervision.
- **Multi-agent orchestration**: the `boruna-orch` binary.
- **Package system**: the `boruna-pkg` manifest format and registry.
- **App templates**: variable names and generated code.
- **`trace2tests`**: test generation and minimization.
- **Migration tooling**: `boruna migrate` is beta and covers pre-1.0
  artifacts only.
- **`std-guard`** standard library (added in 3.1.0).
- **Evidence extras**: `evidence attest`, `anchor`, `report`, `otel`,
  `redact`, `diff` and `create`.
- **Confidence gates**: `confidence_gate` on approval gates and
  `boruna confidence threshold` (3.3.0).
- **Other commands**: `fmt` (v1 strips comments), `repl`, `simulate`,
  `literate`, `skills`, `doctor`, `size`, `capability`, `metrics`,
  `policy`, `lang codes/caps`, `workflow show/list/schedule/eval/find/graph`.
- **`lex_full()`**: the lexer API that keeps comments and whitespace.

### Alpha (expect breaking changes)

- **`--live` network calls**: the `http` cargo feature for `net.fetch`,
  SSRF policy and response handling.
- **`replay`**: replaying a recorded event log.
- **`framework test`**: the message protocol for testing framework apps.

### Planned (see [`roadmap.md`](./roadmap.md))

- External security audit of the VM and capability enforcement
- `boruna fmt` v2 that keeps comments

## Versioning policy

Boruna follows [Semantic Versioning](https://semver.org):

- **Patch** (4.0.x): bug fixes and security fixes.
- **Minor** (4.x.0): new features. Stable surfaces stay compatible.
  Experimental and Alpha components may change. Deprecations are
  announced here.
- **Major** (x.0.0): may remove deprecated features. The previous major
  line is end of life on the day the new major ships.

The language, bytecode, workflow schema and bundle format have their own
version numbers, separate from the release version. See [`lts.md`](./lts.md)
§B.

## Rust toolchain

Boruna builds on stable Rust. CI uses the current stable toolchain. No
minimum Rust version is declared.

## Security

See [`SECURITY.md`](../SECURITY.md) for how to report a vulnerability and
[`lts.md`](./lts.md) §D for which releases get security fixes.
