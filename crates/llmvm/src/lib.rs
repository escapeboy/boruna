pub mod actor;
pub mod capability_gateway;
pub mod error;
#[cfg(feature = "http")]
pub mod http_handler;
#[cfg(feature = "http")]
pub mod net_record_replay;
pub mod replay;
#[cfg(feature = "telemetry")]
pub mod telemetry;
#[cfg(test)]
mod tests;
pub mod vm;

pub use actor::{ActorStatus, ActorSystem, Message};
pub use capability_gateway::{CapabilityGateway, NetPolicy, Policy, PolicyRule};
pub use error::VmError;
#[cfg(feature = "http")]
pub use net_record_replay::{
    NetTape, NetTransaction, RecordingHttpHandler, ReplayingHttpHandler, TAPE_FORMAT_VERSION,
};
pub use replay::{EventLog, ReplayEngine};
#[cfg(feature = "telemetry")]
pub use telemetry::{init as init_telemetry, TelemetryHandle};
pub use vm::{SpawnRequest, StepResult, Vm};
