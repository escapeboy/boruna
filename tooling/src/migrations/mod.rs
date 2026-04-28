//! Migration tooling beta (sprint `W5-C`).
//!
//! Provides migrators that upgrade pre-1.0 Boruna artifacts to the
//! current on-disk format. Each migrator inspects the input, decides
//! whether a change is required, and either rewrites the artifact or
//! reports a no-op. Two flags shape the operator-facing semantics:
//!
//! - `dry_run`: report the planned change WITHOUT touching disk.
//! - `in_place`: rewrite the input artifact directly. When `false`
//!   (the default), the migrator writes a `<path>.migrated` sibling so
//!   operators can diff before swapping.
//!
//! Coverage today (0.5.0 → 0.6.0/1.0.0):
//!
//! - [`evidence_bundle`] — synthesize a missing `bundle.json` summary
//!   for legacy bundles produced before sprint `W1-C` introduced the
//!   bundle.json requirement.
//! - [`workflow_json`] — add `schema_version: 1` to workflow.json files
//!   produced before sprint `W4` introduced the versioned DAG schema.
//!
//! Persistence-store schema migrations (`runs.db`) are explicitly out
//! of scope for the beta — they will land in a follow-up sprint when
//! the next breaking schema change ships.

pub mod evidence_bundle;
pub mod workflow_json;

use std::fmt;

/// Outcome of a migrator invocation.
#[derive(Debug, Clone)]
pub struct MigrationReport {
    /// Migrator name (e.g., `"evidence-bundle"`).
    pub kind: String,
    /// Was the input already at the target format?
    pub no_op: bool,
    /// Did dry-run mode prevent any disk write?
    pub dry_run: bool,
    /// Path the migrator wrote (or WOULD have written, in dry-run).
    /// `None` for no-op migrations.
    pub written_path: Option<std::path::PathBuf>,
    /// Human-readable change summary (one line per change applied).
    pub changes: Vec<String>,
}

impl fmt::Display for MigrationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = if self.dry_run { "[dry-run] " } else { "" };
        if self.no_op {
            return write!(
                f,
                "{}{}: no-op (already at target format)",
                prefix, self.kind
            );
        }
        writeln!(f, "{}{} migration:", prefix, self.kind)?;
        for change in &self.changes {
            writeln!(f, "  - {change}")?;
        }
        if let Some(p) = &self.written_path {
            let verb = if self.dry_run { "would write" } else { "wrote" };
            write!(f, "  {} {}", verb, p.display())?;
        }
        Ok(())
    }
}

/// Errors a migrator can return.
#[derive(Debug)]
pub enum MigrationError {
    /// I/O failure (read, write, mkdir).
    Io(std::io::Error),
    /// Input was malformed (rejected at parse — convention §1).
    Malformed(String),
    /// Input is at a future schema this migrator cannot downgrade.
    UnsupportedFutureVersion(String),
    /// Synthesized output failed self-validation.
    ValidationFailed(String),
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MigrationError::Io(e) => write!(f, "io error: {e}"),
            MigrationError::Malformed(m) => write!(f, "malformed input: {m}"),
            MigrationError::UnsupportedFutureVersion(m) => {
                write!(f, "future schema, downgrade not supported: {m}")
            }
            MigrationError::ValidationFailed(m) => write!(f, "validation failed: {m}"),
        }
    }
}

impl std::error::Error for MigrationError {}

impl From<std::io::Error> for MigrationError {
    fn from(e: std::io::Error) -> Self {
        MigrationError::Io(e)
    }
}
