use boruna_bytecode::{Module, Value};

use crate::capability_gateway::{CapabilityGateway, Policy};
use crate::error::VmError;
use crate::replay::EventLog;
use crate::vm::{StepResult, Vm};

/// Message in an actor's mailbox.
#[derive(Debug, Clone)]
pub struct Message {
    pub from: u64,
    pub payload: Value,
}

/// Actor lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActorStatus {
    Runnable,
    Blocked,
    Completed,
    Failed,
}

/// An actor instance.
struct Actor {
    id: u64,
    vm: Vm,
    status: ActorStatus,
    parent: Option<u64>,
    children: Vec<u64>,
    result: Option<Value>,
}

/// Deterministic actor scheduler.
/// Round-robin scheduling with deterministic message delivery.
pub struct ActorSystem {
    actors: Vec<Actor>,
    next_id: u64,
    /// Pending messages collected during a round, delivered at round end.
    pending_messages: Vec<(u64, u64, Value)>, // (from, to, payload)
    /// Max rounds before giving up.
    max_rounds: u64,
    /// Steps budget per actor per round.
    budget_per_round: u64,
    /// The policy used for spawning child actor gateways.
    policy: Option<Policy>,
    /// Event log for the scheduler.
    event_log: EventLog,
}

impl Default for ActorSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl ActorSystem {
    pub fn new() -> Self {
        ActorSystem {
            actors: Vec::new(),
            next_id: 0,
            pending_messages: Vec::new(),
            max_rounds: 10_000,
            budget_per_round: 1000,
            policy: None,
            event_log: EventLog::new(),
        }
    }

    pub fn set_max_rounds(&mut self, max: u64) {
        self.max_rounds = max;
    }

    pub fn set_budget_per_round(&mut self, budget: u64) {
        self.budget_per_round = budget;
    }

    /// Spawn the root actor from a module.
    pub fn spawn_root(&mut self, module: Module, gateway: CapabilityGateway) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.policy = Some(gateway.policy().clone());
        let mut vm = Vm::new(module, gateway);
        vm.set_actor_id(id);
        vm.next_spawn_id = self.next_id;
        self.actors.push(Actor {
            id,
            vm,
            status: ActorStatus::Runnable,
            parent: None,
            children: Vec::new(),
            result: None,
        });
        id
    }

    /// Send a message to an actor (external API).
    pub fn send(&mut self, to: u64, msg: Message) -> Result<(), VmError> {
        self.pending_messages.push((msg.from, to, msg.payload));
        Ok(())
    }

    /// Run the root actor to completion (single-actor mode, backward compat).
    pub fn run_single(&mut self) -> Result<Value, VmError> {
        if self.actors.is_empty() {
            return Ok(Value::Unit);
        }
        self.actors[0].vm.run()
    }

    /// Run all actors to completion using deterministic round-robin scheduling.
    pub fn run(&mut self) -> Result<Value, VmError> {
        if self.actors.is_empty() {
            return Ok(Value::Unit);
        }

        // Set up root actor's entry function
        let entry = self.actors[0].vm.module().entry;
        self.actors[0].vm.set_entry_function(entry)?;

        for round in 0..self.max_rounds {
            // Build run queue: all runnable actor indices
            let run_queue: Vec<usize> = self
                .actors
                .iter()
                .enumerate()
                .filter(|(_, a)| a.status == ActorStatus::Runnable)
                .map(|(i, _)| i)
                .collect();

            // Check termination: no runnable actors and no pending messages
            if run_queue.is_empty() && self.pending_messages.is_empty() {
                let any_blocked = self.actors.iter().any(|a| a.status == ActorStatus::Blocked);
                if any_blocked {
                    return Err(VmError::Deadlock);
                }
                // All actors completed — return root result
                return self.root_result();
            }

            if run_queue.is_empty() && !self.pending_messages.is_empty() {
                // All actors blocked but messages pending — deliver and continue
                self.deliver_messages();
                self.wake_blocked_actors();
                continue;
            }

            // Phase 1: Execute each runnable actor for budget steps
            for &actor_idx in &run_queue {
                let actor_id = self.actors[actor_idx].id;
                self.event_log.log_scheduler_tick(round, actor_id);

                // Set next_spawn_id so child IDs are deterministic
                self.actors[actor_idx].vm.next_spawn_id = self.next_id;

                let step_result = self.actors[actor_idx]
                    .vm
                    .execute_bounded(self.budget_per_round);

                match step_result {
                    StepResult::Completed(val) => {
                        self.actors[actor_idx].status = ActorStatus::Completed;
                        self.actors[actor_idx].result = Some(val);
                    }
                    StepResult::Yielded { .. } => {
                        // Still runnable, will execute next round
                    }
                    StepResult::Blocked => {
                        self.actors[actor_idx].status = ActorStatus::Blocked;
                    }
                    StepResult::Error(e) => {
                        self.actors[actor_idx].status = ActorStatus::Failed;
                        // Cascade failure to all descendants
                        let children = self.actors[actor_idx].children.clone();
                        self.cascade_failure(&children);
                        // Notify parent with error message
                        if let Some(parent_id) = self.actors[actor_idx].parent {
                            let actor_id = self.actors[actor_idx].id;
                            let err_str = format!("{}", e);
                            self.pending_messages.push((
                                actor_id,
                                parent_id,
                                Value::Err(Box::new(Value::String(err_str))),
                            ));
                        }
                        // If root actor fails, propagate the error
                        if actor_idx == 0 {
                            return Err(e);
                        }
                    }
                }

                // Collect spawn requests (discard if actor failed)
                let spawn_requests = self.actors[actor_idx].vm.drain_spawn_requests();
                if self.actors[actor_idx].status == ActorStatus::Failed {
                    // Don't spawn children for crashed actors
                    let outgoing = self.actors[actor_idx].vm.drain_outgoing_messages();
                    let _ = outgoing; // discard outgoing from failed actor
                    continue;
                }
                let parent_id = self.actors[actor_idx].id;
                for req in spawn_requests {
                    let child_id = self.next_id;
                    self.next_id += 1;
                    let module = self.actors[actor_idx].vm.module().clone();
                    let func_name = module
                        .functions
                        .get(req.func_idx as usize)
                        .map(|f| f.name.as_str())
                        .unwrap_or("unknown");
                    self.event_log.log_actor_spawn(child_id, func_name);
                    let policy = self.policy.clone().unwrap_or_default();
                    let gateway = CapabilityGateway::new(policy);
                    let mut child_vm = Vm::new(module, gateway);
                    child_vm.set_actor_id(child_id);
                    child_vm
                        .set_entry_function(req.func_idx)
                        .expect("invalid function index in spawn request");
                    self.actors.push(Actor {
                        id: child_id,
                        vm: child_vm,
                        status: ActorStatus::Runnable,
                        parent: Some(parent_id),
                        children: Vec::new(),
                        result: None,
                    });
                    // Track child in parent
                    self.actors[actor_idx].children.push(child_id);
                }

                // Collect outgoing messages
                let outgoing = self.actors[actor_idx].vm.drain_outgoing_messages();
                let sender_id = self.actors[actor_idx].id;
                for (target_id, payload) in outgoing {
                    self.pending_messages.push((sender_id, target_id, payload));
                }
            }

            // Phase 2: Deliver pending messages in deterministic order
            self.deliver_messages();

            // Phase 3: Wake blocked actors with non-empty mailboxes
            self.wake_blocked_actors();
        }

        Err(VmError::MaxRoundsExceeded(self.max_rounds))
    }

    /// Deliver pending messages sorted by (target_id, sender_id) for determinism.
    fn deliver_messages(&mut self) {
        // Sort for deterministic delivery order
        self.pending_messages
            .sort_by_key(|&(from, to, _)| (to, from));
        let messages: Vec<_> = self.pending_messages.drain(..).collect();
        for (from, to, payload) in messages {
            self.event_log.log_message_send(from, to, &payload);
            if let Some(actor) = self
                .actors
                .iter_mut()
                .find(|a| a.id == to && a.status != ActorStatus::Failed)
            {
                self.event_log.log_message_receive(to, &payload);
                actor.vm.deliver_message(Message { from, payload });
            }
        }
    }

    /// Cascade failure to all descendants of failed actors.
    fn cascade_failure(&mut self, child_ids: &[u64]) {
        for &child_id in child_ids {
            if let Some(actor) = self.actors.iter_mut().find(|a| a.id == child_id) {
                actor.status = ActorStatus::Failed;
                let grandchildren = actor.children.clone();
                if !grandchildren.is_empty() {
                    self.cascade_failure(&grandchildren);
                }
            }
        }
    }

    /// Move blocked actors with non-empty mailboxes back to runnable.
    fn wake_blocked_actors(&mut self) {
        for actor in self.actors.iter_mut() {
            if actor.status == ActorStatus::Blocked && actor.vm.has_messages() {
                actor.status = ActorStatus::Runnable;
            }
        }
    }

    /// Get the root actor's result.
    fn root_result(&self) -> Result<Value, VmError> {
        match &self.actors[0].result {
            Some(val) => Ok(val.clone()),
            None => Ok(Value::Unit),
        }
    }

    /// Get the root actor's VM (for inspection).
    pub fn root_vm(&self) -> Option<&Vm> {
        self.actors.first().map(|a| &a.vm)
    }

    /// Get UI output from the root actor.
    pub fn root_ui_output(&self) -> Vec<Value> {
        self.actors
            .first()
            .map(|a| a.vm.ui_output.clone())
            .unwrap_or_default()
    }

    /// Get the number of actors in the system.
    pub fn actor_count(&self) -> usize {
        self.actors.len()
    }

    /// Get the scheduler's event log.
    pub fn event_log(&self) -> &EventLog {
        &self.event_log
    }
}
