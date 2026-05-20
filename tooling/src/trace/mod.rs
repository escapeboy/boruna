//! Trace export formats.
//!
//! Boruna's internal trace representation lives in `trace2tests` (regression test
//! generation) and `orchestrator::audit` (evidence bundles). This module is the
//! *export* layer — it converts Boruna's internal shapes to community-standard
//! formats consumed by the broader formal-methods ecosystem.
//!
//! Currently exports:
//! - [`itf`] — Informal Trace Format v0.15
//!   (<https://apalache-mc.org/docs/adr/015adr-trace.html>)
//!
//! There is no import direction. Boruna's internal formats remain authoritative.

pub mod audit_to_itf;
pub mod itf;
