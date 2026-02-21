use serde::{Deserialize, Serialize};
use std::fmt;

/// Capabilities that bytecode can request.
/// All side effects go through this interface.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability {
    NetFetch,
    FsRead,
    FsWrite,
    DbQuery,
    UiRender,
    TimeNow,
    Random,
    LlmCall,
    ActorSpawn,
    ActorSend,
}

impl Capability {
    pub fn from_id(id: u32) -> Option<Self> {
        match id {
            0 => Some(Capability::NetFetch),
            1 => Some(Capability::FsRead),
            2 => Some(Capability::FsWrite),
            3 => Some(Capability::DbQuery),
            4 => Some(Capability::UiRender),
            5 => Some(Capability::TimeNow),
            6 => Some(Capability::Random),
            7 => Some(Capability::LlmCall),
            8 => Some(Capability::ActorSpawn),
            9 => Some(Capability::ActorSend),
            _ => None,
        }
    }

    pub fn id(&self) -> u32 {
        match self {
            Capability::NetFetch => 0,
            Capability::FsRead => 1,
            Capability::FsWrite => 2,
            Capability::DbQuery => 3,
            Capability::UiRender => 4,
            Capability::TimeNow => 5,
            Capability::Random => 6,
            Capability::LlmCall => 7,
            Capability::ActorSpawn => 8,
            Capability::ActorSend => 9,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Capability::NetFetch => "net.fetch",
            Capability::FsRead => "fs.read",
            Capability::FsWrite => "fs.write",
            Capability::DbQuery => "db.query",
            Capability::UiRender => "ui.render",
            Capability::TimeNow => "time.now",
            Capability::Random => "random",
            Capability::LlmCall => "llm.call",
            Capability::ActorSpawn => "actor.spawn",
            Capability::ActorSend => "actor.send",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "net.fetch" | "net" => Some(Capability::NetFetch),
            "fs.read" => Some(Capability::FsRead),
            "fs.write" => Some(Capability::FsWrite),
            "db.query" | "db" => Some(Capability::DbQuery),
            "ui.render" | "ui" => Some(Capability::UiRender),
            "time.now" | "time" => Some(Capability::TimeNow),
            "random" => Some(Capability::Random),
            "llm.call" | "llm" => Some(Capability::LlmCall),
            "actor.spawn" | "actor_spawn" => Some(Capability::ActorSpawn),
            "actor.send" | "actor_send" => Some(Capability::ActorSend),
            _ => None,
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}
