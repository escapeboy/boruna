pub mod vm;
pub mod capability_gateway;
pub mod actor;
pub mod replay;
pub mod error;
#[cfg(test)]
mod tests;

pub use vm::{Vm, StepResult, SpawnRequest};
pub use capability_gateway::{CapabilityGateway, Policy, PolicyRule};
pub use actor::{ActorSystem, ActorStatus, Message};
pub use replay::{EventLog, ReplayEngine};
pub use error::VmError;
