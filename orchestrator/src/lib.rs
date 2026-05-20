pub mod adapters;
pub mod audit;
pub mod cli;
pub mod conflict;
pub mod engine;
#[cfg(feature = "persist-sqlite")]
pub mod metrics;
pub mod patch;
#[cfg(feature = "persist-sqlite")]
pub mod persistence;
pub mod simulate;
pub mod storage;
pub mod workflow;

/// Highest workflow-DAG schema major version this build understands.
/// Re-exported from `crate::workflow::definition` for the public
/// crate surface. See `docs/spec/workflow-dag-1.0.md`.
pub use workflow::definition::WORKFLOW_DAG_SCHEMA_VERSION;
