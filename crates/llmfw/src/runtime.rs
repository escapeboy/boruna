use std::collections::HashMap;

use boruna_bytecode::{Module, Value};
use boruna_vm::capability_gateway::{CapabilityGateway, Policy};
use boruna_vm::vm::Vm;

use crate::effect::{parse_update_result, Effect};
use crate::error::FrameworkError;
use crate::policy::PolicySet;
use crate::state::StateMachine;

/// Message delivered to the update() function.
#[derive(Debug, Clone)]
pub struct AppMessage {
    pub tag: String,
    pub payload: Value,
}

impl AppMessage {
    pub fn new(tag: impl Into<String>, payload: Value) -> Self {
        AppMessage { tag: tag.into(), payload }
    }

    /// Convert to a VM Value (Record with fields [tag, payload]).
    pub fn to_value(&self) -> Value {
        Value::Record {
            type_id: 0,
            fields: vec![
                Value::String(self.tag.clone()),
                self.payload.clone(),
            ],
        }
    }
}

/// Snapshot of one cycle for replay/inspection.
#[derive(Debug, Clone)]
pub struct CycleRecord {
    pub cycle: u64,
    pub message: AppMessage,
    pub state_before: Value,
    pub state_after: Value,
    pub effects: Vec<Effect>,
    pub ui_tree: Option<Value>,
}

/// The application runtime — drives the init → update → effects → view cycle.
pub struct AppRuntime {
    module: Module,
    state_machine: StateMachine,
    policy: PolicySet,
    fn_map: HashMap<String, u32>,
    cycle_log: Vec<CycleRecord>,
    max_cycles: u64,
}

impl AppRuntime {
    /// Create a new AppRuntime from a compiled module.
    pub fn new(module: Module) -> Result<Self, FrameworkError> {
        // Build function index map
        let mut fn_map = HashMap::new();
        for (i, f) in module.functions.iter().enumerate() {
            fn_map.insert(f.name.clone(), i as u32);
        }

        // Verify required functions exist
        if !fn_map.contains_key("init") {
            return Err(FrameworkError::MissingFunction("init".into()));
        }
        if !fn_map.contains_key("update") {
            return Err(FrameworkError::MissingFunction("update".into()));
        }
        if !fn_map.contains_key("view") {
            return Err(FrameworkError::MissingFunction("view".into()));
        }

        // Run init() to get initial state (init may use capabilities)
        let init_state = Self::call_function(&module, &fn_map, "init", vec![], false)?;

        // Run policies() if it exists
        let policy = if fn_map.contains_key("policies") {
            let policy_val = Self::call_function(&module, &fn_map, "policies", vec![], true)?;
            PolicySet::from_value(&policy_val)
        } else {
            PolicySet::allow_all()
        };

        let state_machine = StateMachine::new(init_state);

        Ok(AppRuntime {
            module,
            state_machine,
            policy,
            fn_map,
            cycle_log: Vec::new(),
            max_cycles: 100_000,
        })
    }

    /// Get the current application state.
    pub fn state(&self) -> &Value {
        self.state_machine.current()
    }

    /// Get the current cycle number.
    pub fn cycle(&self) -> u64 {
        self.state_machine.cycle()
    }

    /// Get the cycle log.
    pub fn cycle_log(&self) -> &[CycleRecord] {
        &self.cycle_log
    }

    /// Get the policy.
    pub fn policy(&self) -> &PolicySet {
        &self.policy
    }

    /// Get the state machine (for inspection/testing).
    pub fn state_machine(&self) -> &StateMachine {
        &self.state_machine
    }

    /// Send a message to the application.
    /// Runs: update(state, msg) → execute effects → view(new_state)
    /// Returns: (new_state, effects, ui_tree)
    pub fn send(&mut self, msg: AppMessage) -> Result<(Value, Vec<Effect>, Option<Value>), FrameworkError> {
        if self.state_machine.cycle() >= self.max_cycles {
            return Err(FrameworkError::MaxCyclesExceeded(self.max_cycles));
        }

        let state_before = self.state_machine.current().clone();

        // Call update(state, msg) — PURE: no capabilities allowed
        let msg_value = msg.to_value();
        let update_result = Self::call_function(
            &self.module,
            &self.fn_map,
            "update",
            vec![state_before.clone(), msg_value],
            true,
        )?;

        // Parse the UpdateResult
        let (new_state, effects) = parse_update_result(&update_result)
            .ok_or_else(|| FrameworkError::Effect(
                "update() must return a Record with [state, effects] fields".into()
            ))?;

        // Validate effects against policy
        self.policy.check_batch(&effects)?;

        // Transition state
        self.state_machine.transition(new_state.clone());

        // Call view(state) — PURE: no capabilities allowed
        let ui_tree = Self::call_function(
            &self.module,
            &self.fn_map,
            "view",
            vec![new_state.clone()],
            true,
        )?;

        let ui_value = Some(ui_tree.clone());

        // Log the cycle
        self.cycle_log.push(CycleRecord {
            cycle: self.state_machine.cycle(),
            message: msg,
            state_before,
            state_after: new_state.clone(),
            effects: effects.clone(),
            ui_tree: Some(ui_tree),
        });

        Ok((new_state, effects, ui_value))
    }

    /// Send a message and execute effects, returning callback messages.
    ///
    /// This extends `send()` by passing the returned effects through an
    /// `EffectExecutor`, producing callback messages for the next cycle.
    /// Returns: (new_state, callback_messages, ui_tree)
    pub fn send_with_executor(
        &mut self,
        msg: AppMessage,
        executor: &mut dyn crate::executor::EffectExecutor,
    ) -> Result<(Value, Vec<AppMessage>, Option<Value>), FrameworkError> {
        let (state, effects, ui) = self.send(msg)?;
        let callbacks = executor.execute(effects)?;
        Ok((state, callbacks, ui))
    }

    /// Call view() on the current state (without updating). PURE.
    pub fn view(&self) -> Result<Value, FrameworkError> {
        let state = self.state_machine.current().clone();
        Self::call_function(&self.module, &self.fn_map, "view", vec![state], true)
    }

    /// Get the state snapshot as JSON.
    pub fn snapshot(&self) -> String {
        self.state_machine.snapshot()
    }

    /// Time-travel: rewind to a previous cycle.
    pub fn rewind(&mut self, cycle: u64) -> Result<(), FrameworkError> {
        self.state_machine.rewind(cycle)
    }

    /// Get the state diff between current and a previous cycle.
    pub fn diff_from(&self, cycle: u64) -> Vec<crate::state::StateDiff> {
        self.state_machine.diff_from_cycle(cycle)
    }

    /// Helper: call a named function in the module with given args.
    /// `pure` = true uses deny-all capability policy (for update/view).
    fn call_function(
        module: &Module,
        fn_map: &HashMap<String, u32>,
        name: &str,
        args: Vec<Value>,
        pure: bool,
    ) -> Result<Value, FrameworkError> {
        let &func_idx = fn_map.get(name)
            .ok_or_else(|| FrameworkError::MissingFunction(name.into()))?;

        let policy = if pure { Policy::deny_all() } else { Policy::allow_all() };

        if args.is_empty() {
            let gateway = CapabilityGateway::new(policy);
            let mut module_copy = module.clone();
            module_copy.entry = func_idx;
            let mut vm = Vm::new(module_copy, gateway);
            vm.run().map_err(|e| {
                if pure { Self::wrap_purity_error(name, e) } else { FrameworkError::Runtime(e) }
            })
        } else {
            let mut wrapper = module.clone();
            let mut wrapper_code = Vec::new();

            for arg in &args {
                let idx = wrapper.add_const(arg.clone());
                wrapper_code.push(boruna_bytecode::Op::PushConst(idx));
            }

            wrapper_code.push(boruna_bytecode::Op::Call(func_idx, args.len() as u8));
            wrapper_code.push(boruna_bytecode::Op::Ret);

            let wrapper_fn = boruna_bytecode::Function {
                name: "__fw_wrapper__".into(),
                arity: 0,
                locals: 0,
                code: wrapper_code,
                capabilities: Vec::new(),
                match_tables: Vec::new(),
            };
            let wrapper_idx = wrapper.add_function(wrapper_fn);
            wrapper.entry = wrapper_idx;

            let gateway = CapabilityGateway::new(policy);
            let mut vm = Vm::new(wrapper, gateway);
            vm.run().map_err(|e| {
                if pure { Self::wrap_purity_error(name, e) } else { FrameworkError::Runtime(e) }
            })
        }
    }

    /// Convert a VM capability-denied error into a PurityViolation.
    fn wrap_purity_error(name: &str, err: boruna_vm::VmError) -> FrameworkError {
        match &err {
            boruna_vm::VmError::CapabilityDenied(_) | boruna_vm::VmError::CapabilityBudgetExceeded(_) => {
                FrameworkError::PurityViolation { name: name.into() }
            }
            _ => FrameworkError::Runtime(err),
        }
    }
}
