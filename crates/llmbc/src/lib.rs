pub mod capability;
pub mod module;
pub mod opcode;
#[cfg(test)]
mod tests;
pub mod value;

pub use capability::Capability;
pub use module::{BytecodeError, Function, Module};
pub use opcode::Op;
pub use value::Value;
