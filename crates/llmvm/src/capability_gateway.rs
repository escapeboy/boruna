use boruna_bytecode::{Capability, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::VmError;
use crate::replay::EventLog;

/// Policy rule for a capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    pub allow: bool,
    /// Maximum invocations allowed (0 = unlimited).
    pub budget: u64,
}

impl Default for PolicyRule {
    fn default() -> Self {
        PolicyRule {
            allow: true,
            budget: 0,
        }
    }
}

/// Policy configuration for the capability gateway.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Policy {
    pub rules: HashMap<String, PolicyRule>,
    /// Default rule for capabilities not explicitly listed.
    pub default_allow: bool,
}

impl Policy {
    /// Create a permissive policy that allows everything.
    pub fn allow_all() -> Self {
        Policy {
            rules: HashMap::new(),
            default_allow: true,
        }
    }

    /// Create a deny-all policy.
    pub fn deny_all() -> Self {
        Policy::default()
    }

    /// Allow a specific capability with an optional budget.
    pub fn allow(&mut self, cap: &Capability, budget: u64) -> &mut Self {
        self.rules.insert(
            cap.name().to_string(),
            PolicyRule {
                allow: true,
                budget,
            },
        );
        self
    }

    /// Deny a specific capability.
    pub fn deny(&mut self, cap: &Capability) -> &mut Self {
        self.rules.insert(
            cap.name().to_string(),
            PolicyRule {
                allow: false,
                budget: 0,
            },
        );
        self
    }
}

/// Capability gateway — all side effects go through here.
pub struct CapabilityGateway {
    policy: Policy,
    usage: HashMap<String, u64>,
    /// Host-provided handler for capability calls.
    handler: Box<dyn CapabilityHandler>,
}

/// Trait for host-provided capability implementations.
pub trait CapabilityHandler: Send {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String>;
}

/// Default handler that returns mock values (for testing / sandbox).
pub struct MockHandler;

impl CapabilityHandler for MockHandler {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
        match cap {
            Capability::TimeNow => Ok(Value::Int(1700000000)),
            Capability::Random => Ok(Value::Float(0.42)),
            Capability::NetFetch => {
                let url = args.first().map(|v| format!("{v}")).unwrap_or_default();
                Ok(Value::String(format!(
                    "{{\"mock\": true, \"url\": \"{url}\"}}"
                )))
            }
            Capability::FsRead => {
                let path = args.first().map(|v| format!("{v}")).unwrap_or_default();
                Ok(Value::String(format!("mock file content for {path}")))
            }
            Capability::FsWrite => Ok(Value::Bool(true)),
            Capability::DbQuery => Ok(Value::List(vec![])),
            Capability::UiRender => Ok(Value::Unit),
            Capability::LlmCall => {
                // Mock LLM returns a structured JSON object
                let mut result = std::collections::BTreeMap::new();
                result.insert("status".into(), Value::String("ok".into()));
                result.insert("mock".into(), Value::Bool(true));
                Ok(Value::Map(result))
            }
            Capability::ActorSpawn | Capability::ActorSend => {
                // Actor ops are handled at the opcode level, not through the gateway
                Ok(Value::Unit)
            }
        }
    }
}

/// Replay handler that returns values from a recorded log.
pub struct ReplayHandler {
    events: Vec<Value>,
    cursor: usize,
}

impl ReplayHandler {
    pub fn new(events: Vec<Value>) -> Self {
        ReplayHandler { events, cursor: 0 }
    }
}

impl CapabilityHandler for ReplayHandler {
    fn handle(&mut self, _cap: &Capability, _args: &[Value]) -> Result<Value, String> {
        if self.cursor < self.events.len() {
            let val = self.events[self.cursor].clone();
            self.cursor += 1;
            Ok(val)
        } else {
            Err("replay log exhausted".into())
        }
    }
}

impl CapabilityGateway {
    pub fn new(policy: Policy) -> Self {
        CapabilityGateway {
            policy,
            usage: HashMap::new(),
            handler: Box::new(MockHandler),
        }
    }

    /// Get the policy (for cloning into child actors).
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn with_handler(policy: Policy, handler: Box<dyn CapabilityHandler>) -> Self {
        CapabilityGateway {
            policy,
            usage: HashMap::new(),
            handler,
        }
    }

    /// Execute a capability call with policy enforcement.
    pub fn call(
        &mut self,
        cap: &Capability,
        args: &[Value],
        log: &mut EventLog,
    ) -> Result<Value, VmError> {
        let name = cap.name();

        // Check policy
        let rule = self.policy.rules.get(name);
        let allowed = match rule {
            Some(r) => r.allow,
            None => self.policy.default_allow,
        };
        if !allowed {
            return Err(VmError::CapabilityDenied(cap.clone()));
        }

        // Check budget
        let count = self.usage.entry(name.to_string()).or_insert(0);
        *count += 1;
        if let Some(r) = rule {
            if r.budget > 0 && *count > r.budget {
                return Err(VmError::CapabilityBudgetExceeded(cap.clone()));
            }
        }

        // Log the call
        log.log_cap_call(cap, args);

        // Invoke handler
        let result = self
            .handler
            .handle(cap, args)
            .map_err(|e| VmError::AssertionFailed(format!("capability error: {e}")))?;

        // Log the result
        log.log_cap_result(cap, &result);

        Ok(result)
    }

    pub fn usage(&self) -> &HashMap<String, u64> {
        &self.usage
    }
}
