pub mod capability;
pub mod module;
pub mod opcode;
#[cfg(test)]
mod tests;
pub mod value;

pub use capability::{
    capability_set_report, compute_capability_set_hash, Capability, CapabilityIdentity,
    CapabilitySetReport, CAPABILITY_REPORT_PROTOCOL_VERSION,
};
pub use module::{BytecodeError, Function, Module};
pub use opcode::Op;
pub use value::Value;

/// Frozen bytecode specification version.
///
/// This is the public, semver-like format identifier of the Boruna bytecode
/// surface (opcode discriminants, capability IDs, value variants, module
/// header layout, determinism contract). It is **distinct** from the
/// per-module wire-format byte (`module::VERSION`), which tracks
/// internal-encoding changes.
///
/// Locked by [`docs/spec/bytecode-1.0.md`](../../../docs/spec/bytecode-1.0.md)
/// (sprint `W9-A`). A 1.x VM accepts any 1.y bytecode module; a 2.0 module
/// MUST be rejected with a typed error.
pub const BYTECODE_VERSION: &str = "1.0";
