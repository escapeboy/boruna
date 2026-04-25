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
