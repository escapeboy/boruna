use std::collections::BTreeMap;

use boruna_bytecode::{Capability, Value};
use boruna_vm::capability_gateway::{CapabilityGateway, CapabilityHandler, Policy};
use boruna_vm::replay::EventLog;

use crate::effect::{Effect, EffectKind};
use crate::error::FrameworkError;
use crate::runtime::AppMessage;

/// Trait for executing effects and producing callback messages.
pub trait EffectExecutor {
    fn execute(&mut self, effects: Vec<Effect>) -> Result<Vec<AppMessage>, FrameworkError>;
}

/// Mock executor for testing — returns deterministic stub results.
///
/// Uses `BTreeMap` (not `HashMap`) for deterministic iteration order.
pub struct MockEffectExecutor {
    /// Maps callback_tag → response value.
    responses: BTreeMap<String, Value>,
    /// Default response for tags not in the map.
    default_response: Value,
    /// Next actor ID for mock SpawnActor effects.
    next_mock_actor_id: u64,
}

impl Default for MockEffectExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl MockEffectExecutor {
    pub fn new() -> Self {
        MockEffectExecutor {
            responses: BTreeMap::new(),
            default_response: Value::String("mock_result".into()),
            next_mock_actor_id: 1,
        }
    }

    /// Set a specific response for a callback tag.
    pub fn set_response(&mut self, callback_tag: impl Into<String>, value: Value) {
        self.responses.insert(callback_tag.into(), value);
    }

    /// Set the default response for unknown callback tags.
    pub fn set_default_response(&mut self, value: Value) {
        self.default_response = value;
    }
}

impl EffectExecutor for MockEffectExecutor {
    fn execute(&mut self, effects: Vec<Effect>) -> Result<Vec<AppMessage>, FrameworkError> {
        let mut messages = Vec::new();
        for effect in effects {
            // EmitUi is fire-and-forget — no callback
            if effect.kind == EffectKind::EmitUi {
                continue;
            }

            // Use explicit response if one was set for this callback_tag
            if let Some(response) = self.responses.get(&effect.callback_tag) {
                messages.push(AppMessage::new(&effect.callback_tag, response.clone()));
                continue;
            }

            // Actor effects get deterministic mock responses
            let response = match effect.kind {
                EffectKind::SpawnActor => {
                    let id = self.next_mock_actor_id;
                    self.next_mock_actor_id += 1;
                    Value::ActorId(id)
                }
                EffectKind::SendToActor => Value::String("delivered".into()),
                _ => self.default_response.clone(),
            };
            messages.push(AppMessage::new(&effect.callback_tag, response));
        }
        Ok(messages)
    }
}

/// Host executor — dispatches effects to the capability gateway.
///
/// Each effect kind maps to a `Capability` variant. The gateway handles
/// policy enforcement, logging, and delegation to the handler.
pub struct HostEffectExecutor {
    gateway: CapabilityGateway,
    event_log: EventLog,
}

impl Default for HostEffectExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl HostEffectExecutor {
    /// Create with default mock handler and allow-all policy.
    pub fn new() -> Self {
        HostEffectExecutor {
            gateway: CapabilityGateway::new(Policy::allow_all()),
            event_log: EventLog::new(),
        }
    }

    /// Create with a specific policy and handler.
    pub fn with_handler(policy: Policy, handler: Box<dyn CapabilityHandler>) -> Self {
        HostEffectExecutor {
            gateway: CapabilityGateway::with_handler(policy, handler),
            event_log: EventLog::new(),
        }
    }

    /// Get the event log (for replay/inspection).
    pub fn event_log(&self) -> &EventLog {
        &self.event_log
    }
}

/// Map an EffectKind to the corresponding VM Capability.
fn effect_to_capability(kind: &EffectKind) -> Option<Capability> {
    match kind {
        EffectKind::HttpRequest => Some(Capability::NetFetch),
        EffectKind::DbQuery => Some(Capability::DbQuery),
        EffectKind::FsRead => Some(Capability::FsRead),
        EffectKind::FsWrite => Some(Capability::FsWrite),
        EffectKind::Timer => Some(Capability::TimeNow),
        EffectKind::Random => Some(Capability::Random),
        EffectKind::EmitUi => Some(Capability::UiRender),
        EffectKind::LlmCall => Some(Capability::LlmCall),
        EffectKind::SpawnActor => Some(Capability::ActorSpawn),
        EffectKind::SendToActor => Some(Capability::ActorSend),
    }
}

/// Build gateway args from the effect payload.
fn effect_args(effect: &Effect) -> Vec<Value> {
    match effect.kind {
        // Timer and Random take no args
        EffectKind::Timer | EffectKind::Random => vec![],
        // All others pass the payload
        _ => vec![effect.payload.clone()],
    }
}

impl EffectExecutor for HostEffectExecutor {
    fn execute(&mut self, effects: Vec<Effect>) -> Result<Vec<AppMessage>, FrameworkError> {
        let mut messages = Vec::new();
        for effect in effects {
            // EmitUi is fire-and-forget
            if effect.kind == EffectKind::EmitUi {
                let cap = Capability::UiRender;
                let args = vec![effect.payload.clone()];
                // Execute but don't produce callback
                let _ = self.gateway.call(&cap, &args, &mut self.event_log);
                continue;
            }

            let cap = match effect_to_capability(&effect.kind) {
                Some(c) => c,
                None => {
                    // Unsupported effect kind (e.g., SpawnActor) → deliver error
                    messages.push(AppMessage::new(
                        &effect.callback_tag,
                        Value::String(format!("unsupported effect: {}", effect.kind.as_str())),
                    ));
                    continue;
                }
            };

            let args = effect_args(&effect);
            match self.gateway.call(&cap, &args, &mut self.event_log) {
                Ok(result) => {
                    messages.push(AppMessage::new(&effect.callback_tag, result));
                }
                Err(err) => {
                    messages.push(AppMessage::new(
                        &effect.callback_tag,
                        Value::String(format!("effect error: {err}")),
                    ));
                }
            }
        }
        Ok(messages)
    }
}
