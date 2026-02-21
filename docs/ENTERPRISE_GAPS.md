# Enterprise Gaps

Known gaps between current capabilities and full enterprise platform requirements.

## P0 — No P0 gaps currently

## P1 — High Priority

### RBAC / Identity System
- **What blocks us**: No user authentication; `Policy` is per-run, not per-user. Approval gates specify roles as strings without identity verification.
- **Minimal required change**: Add identity provider integration and role-based policy selection.
- **Proposed direction**: JWT-based identity tokens passed at workflow start; roles resolved from token claims; approval gates verify identity against required role.

### Real HTTP Handler
- **What blocks us**: Only `MockHandler` exists for `NetFetch` capability. Cannot make actual HTTP requests.
- **Minimal required change**: Implement a real HTTP handler behind the `CapabilityGateway`.
- **Proposed direction**: Add `HttpHandler` that respects policy allowlists (domain, path, method) and records request/response in EventLog for replay.

### Real DB Handler
- **What blocks us**: Only `MockHandler` exists for `DbQuery` capability.
- **Minimal required change**: Implement a database handler with connection pooling.
- **Proposed direction**: Add `DbHandler` supporting PostgreSQL/SQLite via connection string in policy; parameterized queries only; record results for replay.

### Secrets Management
- **What blocks us**: No secure secrets store. Environment variables are not acceptable for production.
- **Minimal required change**: Add a `Capability::SecretRead` with sealed handler.
- **Proposed direction**: Integration with external secret managers (Vault, AWS Secrets Manager); secrets injected at runtime via handler, never persisted in evidence bundles.

### Structured Logging
- **What blocks us**: No JSON structured logs. Only stderr text output.
- **Minimal required change**: Add `tracing` crate integration with JSON output.
- **Proposed direction**: `tracing-subscriber` with JSON formatter; log levels configurable via env/CLI; per-step span context.

### Conditional Branching in DAGs
- **What blocks us**: Workflow DAG is unconditional. No choice/switch/if nodes.
- **Minimal required change**: Add a `StepKind::Conditional` variant with a predicate expression.
- **Proposed direction**: Evaluate predicate against previous step outputs; skip branches where predicate is false; mark skipped steps in audit log.

## P2 — Medium Priority

### Queue Capability
- **What blocks us**: No `Capability::Queue` variant in bytecode.
- **Minimal required change**: Add queue read/write capability with mock handler.
- **Proposed direction**: `queue.publish` and `queue.subscribe` capabilities; integrate with RabbitMQ/SQS via handler.

### Digital Signatures
- **What blocks us**: Evidence bundles use SHA-256 hashes but no cryptographic signatures.
- **Minimal required change**: Add Ed25519 signing of bundle manifests.
- **Proposed direction**: CLI generates keypair; `boruna evidence sign` signs manifest; `boruna evidence verify --key` checks signature.

### Daemon/Service Mode
- **What blocks us**: CLI-only execution. No persistent execution queue or API.
- **Minimal required change**: Add a long-running process with HTTP API.
- **Proposed direction**: `boruna serve` mode with REST API for workflow submission, status polling, and evidence retrieval.

### Multi-tenancy
- **What blocks us**: No namespace or tenant isolation.
- **Minimal required change**: Add tenant context to workflow runs.
- **Proposed direction**: Tenant ID in run metadata; per-tenant evidence directories; tenant-scoped policies.

### Webhook Capability
- **What blocks us**: No inbound webhook handler.
- **Minimal required change**: Add webhook receiver in daemon mode.
- **Proposed direction**: `boruna serve` handles webhook endpoints; maps webhooks to workflow triggers.

### Metrics Export
- **What blocks us**: No metrics collection beyond what's in evidence bundles.
- **Minimal required change**: Add Prometheus/OpenTelemetry metrics.
- **Proposed direction**: Instrument runner with counters/histograms; expose `/metrics` endpoint in daemon mode.
