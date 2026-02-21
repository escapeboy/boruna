use boruna_bytecode::Value;
use serde_json;

use crate::effect::Effect;
use crate::error::FrameworkError;

/// Extract strings from a Value that is either List or Record{type_id:0xFFFF} (list literal).
fn extract_string_list(value: &Value) -> Vec<String> {
    let items = match value {
        Value::List(items) => items.as_slice(),
        Value::Record { type_id, fields, .. } if *type_id == 0xFFFF => fields.as_slice(),
        _ => return Vec::new(),
    };
    items.iter().filter_map(|v| {
        if let Value::String(s) = v { Some(s.clone()) } else { None }
    }).collect()
}

/// Application policy set — declares allowed capabilities and resource limits.
#[derive(Debug, Clone)]
pub struct PolicySet {
    /// Allowed capability names.
    pub capabilities: Vec<String>,
    /// Max effects per update cycle (0 = unlimited).
    pub max_effects_per_cycle: u64,
    /// Max VM steps per function call (0 = unlimited).
    pub max_steps: u64,
}

impl Default for PolicySet {
    fn default() -> Self {
        PolicySet {
            capabilities: Vec::new(),
            max_effects_per_cycle: 0,
            max_steps: 10_000_000,
        }
    }
}

impl PolicySet {
    /// Create a permissive policy.
    pub fn allow_all() -> Self {
        PolicySet {
            capabilities: vec![
                "net.fetch".into(),
                "fs.read".into(),
                "fs.write".into(),
                "db.query".into(),
                "ui.render".into(),
                "time.now".into(),
                "random".into(),
                "llm.call".into(),
                "actor.spawn".into(),
                "actor.send".into(),
            ],
            max_effects_per_cycle: 0,
            max_steps: 10_000_000,
        }
    }

    /// Parse a PolicySet from a VM Value returned by the policies() function.
    ///
    /// Expected: Record { capabilities: List<String>, max_effects: Int, max_steps: Int }
    pub fn from_value(value: &Value) -> Self {
        match value {
            Value::Record { fields, .. } => {
                let capabilities = match fields.first() {
                    Some(val) => extract_string_list(val),
                    _ => Vec::new(),
                };
                let max_effects = match fields.get(1) {
                    Some(Value::Int(n)) => *n as u64,
                    _ => 0,
                };
                let max_steps = match fields.get(2) {
                    Some(Value::Int(n)) => *n as u64,
                    _ => 10_000_000,
                };
                PolicySet { capabilities, max_effects_per_cycle: max_effects, max_steps }
            }
            _ => PolicySet::default(),
        }
    }

    /// Check if an effect is allowed by this policy.
    pub fn check_effect(&self, effect: &Effect) -> Result<(), FrameworkError> {
        let cap_name = effect.kind.capability_name();
        if !self.capabilities.is_empty() && !self.capabilities.iter().any(|c| c == cap_name) {
            return Err(FrameworkError::PolicyViolation(format!(
                "effect {:?} requires capability '{}' which is not in the policy",
                effect.kind, cap_name
            )));
        }
        Ok(())
    }

    /// Check if a batch of effects is within limits.
    pub fn check_batch(&self, effects: &[Effect]) -> Result<(), FrameworkError> {
        if self.max_effects_per_cycle > 0 && effects.len() as u64 > self.max_effects_per_cycle {
            return Err(FrameworkError::PolicyViolation(format!(
                "too many effects: {} exceeds limit of {}",
                effects.len(), self.max_effects_per_cycle
            )));
        }
        for effect in effects {
            self.check_effect(effect)?;
        }
        Ok(())
    }

    /// Produce a structured JSON diagnostic of this policy.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&serde_json::json!({
            "capabilities": self.capabilities,
            "max_effects_per_cycle": self.max_effects_per_cycle,
            "max_steps": self.max_steps,
        })).unwrap_or_default()
    }
}

/// Produce a structured JSON diagnostic from a FrameworkError.
pub fn error_to_json(err: &FrameworkError) -> String {
    let (kind, detail) = match err {
        FrameworkError::PolicyViolation(msg) => ("policy_violation", msg.clone()),
        FrameworkError::PurityViolation { name } => ("purity_violation", format!("function: {name}")),
        FrameworkError::MissingFunction(name) => ("missing_function", name.clone()),
        FrameworkError::Validation(msg) => ("validation", msg.clone()),
        FrameworkError::Effect(msg) => ("effect_error", msg.clone()),
        FrameworkError::State(msg) => ("state_error", msg.clone()),
        FrameworkError::MaxCyclesExceeded(n) => ("max_cycles_exceeded", format!("{n}")),
        FrameworkError::WrongArity { name, expected, got } => {
            ("wrong_arity", format!("{name}: expected {expected}, got {got}"))
        }
        FrameworkError::MissingType(t) => ("missing_type", t.clone()),
        FrameworkError::Compile(e) => ("compile_error", format!("{e}")),
        FrameworkError::Runtime(e) => ("runtime_error", format!("{e}")),
    };
    serde_json::to_string_pretty(&serde_json::json!({
        "error": kind,
        "detail": detail,
    })).unwrap_or_default()
}
