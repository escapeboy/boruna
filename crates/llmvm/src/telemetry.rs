//! OpenTelemetry exporter integration for capability spans.
//!
//! Sprint 0.4-S5 (FleetQ #9). The `telemetry` Cargo feature gates this
//! entire module. When the feature is on AND the `OTEL_EXPORTER_OTLP_ENDPOINT`
//! environment variable is set, [`init`] wires an OTLP-over-HTTP exporter
//! into the global `tracing` subscriber registry. Capability spans
//! (`boruna.cap` with `cap.name`, `bytes_in`, `bytes_out`, `cap.budget_remaining`,
//! `error.kind` attributes) emitted from `CapabilityGateway::call` are then
//! exported to the configured OTel collector.
//!
//! When the env var is unset, [`init`] returns a no-op handle — Boruna
//! behaves identically to a non-telemetry build, with the spans simply
//! dropped because no subscriber was installed. This is the documented
//! activation contract: env-var IS the activation signal, no Boruna-specific
//! flag.
//!
//! **Determinism contract (per ADR 001):** spans are operational metadata
//! ONLY. Their content (durations, byte counts) MUST NOT feed an
//! `EventLog`, `AuditLog`, or `EvidenceBundle`. A replayed run produces
//! identical replay state but may produce different span durations on a
//! faster/slower host — by design.
//!
//! See `docs/design-otel.md` for the full design.

use std::env;

use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::Resource;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const OTLP_ENDPOINT_ENV: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
const SERVICE_NAME_ENV: &str = "OTEL_SERVICE_NAME";
const DEFAULT_SERVICE_NAME: &str = "boruna";

/// Handle returned by [`init`]. On `Drop`, flushes pending spans to the
/// OTLP exporter (if one was installed). Hold this for the lifetime of the
/// binary; drop right before exit so all in-flight spans flush.
///
/// When the OTLP endpoint env var was unset at `init` time, this is a
/// `Disabled` variant and `Drop` is a no-op.
#[must_use = "drop the TelemetryHandle at end of main to flush pending spans"]
pub struct TelemetryHandle {
    state: HandleState,
}

enum HandleState {
    /// `OTEL_EXPORTER_OTLP_ENDPOINT` was unset → no exporter installed.
    Disabled,
    /// Exporter installed; provider held for shutdown-on-drop.
    Enabled {
        provider: opentelemetry_sdk::trace::TracerProvider,
    },
}

/// Initialize the OTel exporter from environment variables.
///
/// Reads:
/// - `OTEL_EXPORTER_OTLP_ENDPOINT` — required to enable exporting.
///   When unset, returns `TelemetryHandle::Disabled` and Boruna behaves
///   identically to a non-telemetry build.
/// - `OTEL_SERVICE_NAME` — optional; defaults to `"boruna"`.
///
/// Returns `Err` only on a hard configuration failure (e.g. the OTLP
/// builder rejects the supplied endpoint format). A missing endpoint is
/// NOT an error — it's the documented "telemetry off" state.
///
/// **Idempotency:** safe to call only ONCE per process. Subsequent calls
/// either fail (subscriber already installed) or — worse — install a
/// second subscriber. Document and enforce in the CLI integration that
/// `init` runs exactly once during startup.
pub fn init() -> Result<TelemetryHandle, String> {
    let endpoint = match env::var(OTLP_ENDPOINT_ENV) {
        Ok(v) if !v.is_empty() => v,
        _ => return Ok(TelemetryHandle { state: HandleState::Disabled }),
    };

    let service_name = env::var(SERVICE_NAME_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_string());

    // Set up the global OTel propagator so trace context can flow if anyone
    // ever wires Boruna into a parent trace. Cheap; no effect when unused.
    global::set_text_map_propagator(TraceContextPropagator::new());

    // Build the OTLP HTTP exporter. We pin to HTTP/proto (vs. gRPC) because
    // the http-proto feature has no native dep beyond reqwest, keeping the
    // musl static-link story intact (per ADR 001).
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(&endpoint)
        .with_protocol(Protocol::HttpBinary)
        .build()
        .map_err(|e| format!("failed to build OTLP exporter for '{endpoint}': {e}"))?;

    let resource = Resource::new(vec![KeyValue::new("service.name", service_name)]);

    let provider = opentelemetry_sdk::trace::TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_resource(resource)
        .build();

    use opentelemetry::trace::TracerProvider as _;
    let tracer = provider.tracer("boruna");

    // Install the OTel layer into tracing's global subscriber registry.
    // `try_init` returns Err if a subscriber was already installed by
    // someone else — which we treat as a hard config failure since the
    // CLI is supposed to be the single owner of the global subscriber.
    tracing_subscriber::registry()
        .with(OpenTelemetryLayer::new(tracer))
        .try_init()
        .map_err(|e| {
            format!(
                "failed to install global tracing subscriber (already initialized?): {e}"
            )
        })?;

    Ok(TelemetryHandle {
        state: HandleState::Enabled { provider },
    })
}

impl Drop for TelemetryHandle {
    fn drop(&mut self) {
        if let HandleState::Enabled { provider } = &self.state {
            // Best-effort flush. Errors during shutdown can't be propagated
            // (Drop returns ()); log to stderr and continue. Same pattern as
            // the recording-handler save-on-drop in `net_record_replay`.
            for result in provider.force_flush() {
                if let Err(e) = result {
                    eprintln!("warning: OTel span flush failed: {e}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Disabled` handle Drop is a clean no-op (no panic, no allocations
    /// to release). This is the only piece of the telemetry surface that's
    /// safely testable as a unit test: the global subscriber + tokio
    /// runtime initialization can only be exercised by integration-style
    /// tests in a binary, since the OTel SDK is process-singleton.
    ///
    /// **Why no env-var test:** an earlier draft tested
    /// `init_without_endpoint_returns_disabled_handle` by mutating
    /// `OTEL_EXPORTER_OTLP_ENDPOINT` with `unsafe { env::remove_var(...) }`.
    /// Cargo runs unit tests in parallel by default, and POSIX `setenv`/
    /// `getenv` is not thread-safe — the mutation is documented unsafe in
    /// Rust 2024 specifically because of this libc data race. Removed the
    /// test rather than introduce UB-on-parallel-test risk.
    #[test]
    fn disabled_handle_drop_is_a_no_op() {
        let h = TelemetryHandle {
            state: HandleState::Disabled,
        };
        drop(h);
    }
}
