pub mod actor;
pub mod capability_gateway;
pub mod error;
#[cfg(feature = "http")]
pub mod http_handler;
pub mod replay;
#[cfg(feature = "telemetry")]
pub mod telemetry;
#[cfg(test)]
mod tests;
pub mod vm;

pub use actor::{ActorStatus, ActorSystem, Message};
pub use capability_gateway::{CapabilityGateway, NetPolicy, Policy, PolicyRule};
pub use error::VmError;
pub use replay::{EventLog, ReplayEngine};
#[cfg(feature = "telemetry")]
pub use telemetry::{init as init_telemetry, TelemetryHandle};
pub use vm::{SpawnRequest, StepResult, Vm};
