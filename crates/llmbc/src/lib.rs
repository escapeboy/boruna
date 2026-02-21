pub mod opcode;
pub mod module;
pub mod value;
pub mod capability;
#[cfg(test)]
mod tests;

pub use opcode::Op;
pub use module::{Module, Function, BytecodeError};
pub use value::Value;
pub use capability::Capability;
