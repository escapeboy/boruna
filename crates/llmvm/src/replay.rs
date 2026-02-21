use boruna_bytecode::{Capability, Value};
use serde::{Deserialize, Serialize};

/// Current version of the EventLog format.
pub const EVENT_LOG_VERSION: u32 = 1;

/// Maximum supported version (for forward-compat rejection).
const MAX_SUPPORTED_VERSION: u32 = 1;

/// A single event in the execution log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    CapCall {
        capability: String,
        args: Vec<Value>,
    },
    CapResult {
        capability: String,
        result: Value,
    },
    ActorSpawn {
        actor_id: u64,
        function: String,
    },
    MessageSend {
        from: u64,
        to: u64,
        payload: Value,
    },
    MessageReceive {
        actor_id: u64,
        payload: Value,
    },
    UiEmit {
        tree: Value,
    },
    SchedulerTick {
        round: u64,
        active_actor: u64,
    },
}

/// Event log for recording and replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLog {
    /// Format version. Defaults to EVENT_LOG_VERSION for new logs.
    /// Missing in old JSON → serde default fills EVENT_LOG_VERSION.
    #[serde(default = "default_version")]
    version: u32,
    events: Vec<Event>,
}

fn default_version() -> u32 {
    EVENT_LOG_VERSION
}

impl EventLog {
    pub fn new() -> Self {
        EventLog {
            version: EVENT_LOG_VERSION,
            events: Vec::new(),
        }
    }

    /// Get the format version.
    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn log_cap_call(&mut self, cap: &Capability, args: &[Value]) {
        self.events.push(Event::CapCall {
            capability: cap.name().to_string(),
            args: args.to_vec(),
        });
    }

    pub fn log_cap_result(&mut self, cap: &Capability, result: &Value) {
        self.events.push(Event::CapResult {
            capability: cap.name().to_string(),
            result: result.clone(),
        });
    }

    pub fn log_actor_spawn(&mut self, actor_id: u64, function: &str) {
        self.events.push(Event::ActorSpawn {
            actor_id,
            function: function.to_string(),
        });
    }

    pub fn log_message_send(&mut self, from: u64, to: u64, payload: &Value) {
        self.events.push(Event::MessageSend {
            from,
            to,
            payload: payload.clone(),
        });
    }

    pub fn log_ui_emit(&mut self, tree: &Value) {
        self.events.push(Event::UiEmit { tree: tree.clone() });
    }

    pub fn log_message_receive(&mut self, actor_id: u64, payload: &Value) {
        self.events.push(Event::MessageReceive {
            actor_id,
            payload: payload.clone(),
        });
    }

    pub fn log_scheduler_tick(&mut self, round: u64, active_actor: u64) {
        self.events.push(Event::SchedulerTick {
            round,
            active_actor,
        });
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// Serialize the log to JSON.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| e.to_string())
    }

    /// Deserialize from JSON.
    ///
    /// Rejects logs with version > MAX_SUPPORTED_VERSION.
    /// Missing version field defaults to EVENT_LOG_VERSION (backward compat).
    pub fn from_json(json: &str) -> Result<Self, String> {
        let log: EventLog = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if log.version > MAX_SUPPORTED_VERSION {
            return Err(format!(
                "unsupported EventLog version {}: max supported is {}",
                log.version, MAX_SUPPORTED_VERSION
            ));
        }
        Ok(log)
    }

    /// Extract capability results for replay.
    pub fn capability_results(&self) -> Vec<Value> {
        self.events.iter()
            .filter_map(|e| match e {
                Event::CapResult { result, .. } => Some(result.clone()),
                _ => None,
            })
            .collect()
    }
}

/// Replay engine: re-executes bytecode using recorded capability results.
pub struct ReplayEngine;

impl ReplayEngine {
    /// Verify that a replay produces the same event sequence.
    pub fn verify(original: &EventLog, replay: &EventLog) -> ReplayResult {
        let orig_caps: Vec<_> = original.events().iter()
            .filter(|e| matches!(e, Event::CapCall { .. }))
            .collect();
        let replay_caps: Vec<_> = replay.events().iter()
            .filter(|e| matches!(e, Event::CapCall { .. }))
            .collect();

        if orig_caps.len() != replay_caps.len() {
            return ReplayResult::Diverged {
                reason: format!(
                    "different number of capability calls: {} vs {}",
                    orig_caps.len(),
                    replay_caps.len()
                ),
            };
        }

        // Compare capability call sequences
        for (i, (o, r)) in orig_caps.iter().zip(replay_caps.iter()).enumerate() {
            if let (Event::CapCall { capability: oc, args: oa },
                    Event::CapCall { capability: rc, args: ra }) = (o, r) {
                if oc != rc || oa != ra {
                    return ReplayResult::Diverged {
                        reason: format!(
                            "capability call #{i} differs: {oc}({oa:?}) vs {rc}({ra:?})"
                        ),
                    };
                }
            }
        }

        ReplayResult::Identical
    }

    /// Verify that ALL events match (not just CapCall).
    /// Compares CapCall, ActorSpawn, MessageSend, MessageReceive, SchedulerTick.
    pub fn verify_full(original: &EventLog, replay: &EventLog) -> ReplayResult {
        let orig = original.events();
        let repl = replay.events();

        if orig.len() != repl.len() {
            return ReplayResult::Diverged {
                reason: format!(
                    "different event count: {} vs {}",
                    orig.len(),
                    repl.len()
                ),
            };
        }

        for (i, (o, r)) in orig.iter().zip(repl.iter()).enumerate() {
            let o_json = serde_json::to_string(o).unwrap_or_default();
            let r_json = serde_json::to_string(r).unwrap_or_default();
            if o_json != r_json {
                return ReplayResult::Diverged {
                    reason: format!(
                        "event #{i} differs: {o_json} vs {r_json}"
                    ),
                };
            }
        }

        ReplayResult::Identical
    }
}

#[derive(Debug)]
pub enum ReplayResult {
    Identical,
    Diverged { reason: String },
}
