use std::collections::VecDeque;
use std::time::Instant;

use boruna_bytecode::{Capability, Module, Op, Value};

use crate::actor::Message;
use crate::capability_gateway::CapabilityGateway;
use crate::error::VmError;
use crate::replay::EventLog;

const MAX_STACK: usize = 4096;
const MAX_CALL_DEPTH: usize = 256;
/// How often to check `max_wall_ms` during execution.
/// Checking on every step would be a measurable overhead; once per N steps is
/// cheap and keeps wall-clock granularity below ~1 ms for typical workloads.
/// Documented in `docs/design-resource-limits.md`.
const WALL_TIME_CHECK_EVERY: u64 = 1024;

/// Result of bounded execution — the VM may complete, yield, or block.
#[derive(Debug)]
pub enum StepResult {
    /// Program completed with a return value.
    Completed(Value),
    /// Budget exhausted; VM state preserved for resumption.
    Yielded { steps_used: u64 },
    /// Blocked on ReceiveMsg with empty mailbox.
    Blocked,
    /// Runtime error.
    Error(VmError),
}

/// A pending spawn request created by SpawnActor opcode.
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub func_idx: u32,
}

/// A call frame on the call stack.
#[derive(Debug, Clone)]
struct CallFrame {
    func_idx: u32,
    ip: usize,
    stack_base: usize,
    locals: Vec<Value>,
}

/// The Boruna virtual machine.
pub struct Vm {
    module: Module,
    stack: Vec<Value>,
    call_stack: Vec<CallFrame>,
    globals: Vec<Value>,
    gateway: CapabilityGateway,
    event_log: EventLog,
    step_count: u64,
    max_steps: u64,
    /// Wall-clock limit in milliseconds (None = no limit).
    /// Checked every `WALL_TIME_CHECK_EVERY` steps inside `execute`.
    max_wall_ms: Option<u64>,
    /// Set when `run` / `execute` starts. Used to compute elapsed wall-clock.
    /// `None` outside of an active execution.
    start_time: Option<Instant>,
    /// UI tree emitted by EmitUi instructions.
    pub ui_output: Vec<Value>,
    /// Trace log for debugging.
    pub trace: Vec<String>,
    pub trace_enabled: bool,
    /// Capability calls made during the current execute_bounded slice.
    /// Drained by `take_last_cap_events` between slices (T-2.2).
    last_cap_events: Vec<&'static str>,
    // --- Actor fields ---
    /// Which actor this VM belongs to (0 = root/default).
    actor_id: u64,
    /// Incoming message queue (populated by scheduler).
    pub(crate) mailbox: VecDeque<Message>,
    /// Outgoing messages queued by SendMsg (drained by scheduler).
    outgoing_messages: Vec<(u64, Value)>,
    /// Pending spawn requests from SpawnActor (drained by scheduler).
    spawn_requests: Vec<SpawnRequest>,
    /// Next actor ID to assign for spawns (set by scheduler).
    pub(crate) next_spawn_id: u64,
    /// Budget for bounded execution (None = unbounded/legacy).
    budget: Option<u64>,
    /// Step count at budget start.
    budget_start: u64,
    /// True when this VM is being driven by an [`ActorSystem`] scheduler.
    /// Determines `Op::ReceiveMsg`'s empty-mailbox behavior:
    ///   - `true` (actor mode) → rewind IP and yield
    ///     [`VmError::MailboxEmpty`] so the scheduler can deliver a
    ///     message and re-execute the op.
    ///   - `false` (standalone) → push [`Value::Unit`] and continue,
    ///     mirroring [`Vm::run`]'s legacy non-actor semantics.
    ///
    /// Defaults to `false`. [`ActorSystem`] sets it to `true` when
    /// scheduling. Reviewed in 0.4-S6 — the prior signal was
    /// `budget.is_some()`, which conflated "in actor scheduler"
    /// (correctly blocks) with "in slice-bounded streaming progress
    /// loop" (must mirror legacy behavior). The conflation made
    /// `boruna_run`'s streaming and non-streaming paths diverge for
    /// any program emitting `Op::ReceiveMsg` outside an actor system.
    in_actor_context: bool,
}

impl Vm {
    pub fn new(module: Module, gateway: CapabilityGateway) -> Self {
        let global_count = module.globals.len();
        Vm {
            module,
            stack: Vec::with_capacity(256),
            call_stack: Vec::new(),
            globals: vec![Value::Unit; global_count],
            gateway,
            event_log: EventLog::new(),
            step_count: 0,
            max_steps: 10_000_000,
            max_wall_ms: None,
            start_time: None,
            ui_output: Vec::new(),
            trace: Vec::new(),
            trace_enabled: false,
            last_cap_events: Vec::new(),
            actor_id: 0,
            mailbox: VecDeque::new(),
            outgoing_messages: Vec::new(),
            spawn_requests: Vec::new(),
            next_spawn_id: 0,
            budget: None,
            budget_start: 0,
            in_actor_context: false,
        }
    }

    /// Mark this VM as being driven by an [`ActorSystem`] scheduler so
    /// `Op::ReceiveMsg` blocks on an empty mailbox instead of falling
    /// through to `Value::Unit` (sprint `0.4-S6`). Called by
    /// `ActorSystem::run`.
    pub fn set_in_actor_context(&mut self, in_context: bool) {
        self.in_actor_context = in_context;
    }

    /// Start the wall-clock timer if not already running (sprint `0.4-S6`).
    /// Callers driving the VM through [`Self::execute_bounded`] in a
    /// loop should call this BEFORE [`Self::set_entry_function`] when
    /// they want timing semantics that match [`Self::run`] — `vm.run`
    /// initializes `start_time` before its `call_function(entry, ...)`
    /// setup, so the wall-time budget covers the entry-frame
    /// allocation. `execute_bounded` initializes `start_time` lazily on
    /// the first slice, after `set_entry_function` has already run, so
    /// without this hook the bounded path leaks the entry-setup time
    /// from `max_wall_ms` accounting.
    pub fn start_timer(&mut self) {
        if self.start_time.is_none() {
            self.start_time = Some(Instant::now());
        }
    }

    /// Drain capability call names accumulated since the last call (T-2.2).
    /// Called between execute_bounded slices to surface cap events in
    /// MCP progress notifications.
    pub fn take_last_cap_events(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut self.last_cap_events)
    }

    pub fn set_max_steps(&mut self, max: u64) {
        self.max_steps = max;
    }

    /// Set a wall-clock execution limit in milliseconds.
    /// Pass `None` to disable. Checked every `WALL_TIME_CHECK_EVERY` steps.
    ///
    /// **Honoured by both `Vm::run()` and `Vm::execute_bounded()`** — both set
    /// `start_time` before invoking the execute loop.
    ///
    /// **Wall-clock-keyed.** A script that completes within the limit is
    /// deterministic; a script that hits the limit may complete on a fast
    /// machine and time out on a slow one. Use `set_max_steps` for the
    /// deterministic ceiling; use this as an operational guardrail.
    ///
    /// **NEVER call from a code path that feeds an `EventLog`, `AuditLog`, or
    /// `EvidenceBundle`.** A `WallTimeExceeded` error is wall-clock-keyed and
    /// would corrupt replay verification across hosts. The orchestrator's
    /// `Runner` deliberately does not call this; only the MCP `boruna_run`
    /// path (one-shot user-driven execution, no audit hashing) does.
    ///
    /// **Does NOT interrupt blocking capability calls.** A single CapCall to
    /// a slow handler (LLM, HTTP, DB) executes synchronously inside one VM
    /// step — the wall-time check fires only between steps, so a 30-second
    /// LLM call with `max_wall_ms: 100` will block for 30 seconds and only
    /// then trigger the limit on the next step. For per-capability time
    /// budgets, use `NetPolicy.timeout_ms` (already supported) for net.fetch
    /// and equivalent per-handler controls when they ship for other caps.
    pub fn set_max_wall_ms(&mut self, max_ms: Option<u64>) {
        self.max_wall_ms = max_ms;
    }

    pub fn event_log(&self) -> &EventLog {
        &self.event_log
    }

    pub fn step_count(&self) -> u64 {
        self.step_count
    }

    /// Run from the module entry point.
    pub fn run(&mut self) -> Result<Value, VmError> {
        let entry = self.module.entry;
        // Start the wall-clock timer before any user code executes — gives the
        // tightest accounting and ensures the limit covers the entry call too.
        self.start_time = Some(Instant::now());
        let result = (|| {
            self.call_function(entry, vec![])?;
            self.execute()
        })();
        // Clear the timer so a subsequent reuse of the VM doesn't accidentally
        // measure against a stale start.
        self.start_time = None;
        result
    }

    /// Run with bounded execution budget. Returns StepResult.
    /// Call `set_entry_function()` first, then call this repeatedly.
    ///
    /// `max_wall_ms` (if set via `set_max_wall_ms`) is honoured — `start_time`
    /// is initialized on the first call and reused across yields, so the
    /// wall-time ceiling spans the full multi-step execution rather than
    /// resetting per slice.
    pub fn execute_bounded(&mut self, budget: u64) -> StepResult {
        // Initialize start_time on the first slice; preserve it on subsequent
        // slices so the wall-time budget spans the entire bounded execution.
        if self.start_time.is_none() {
            self.start_time = Some(Instant::now());
        }
        self.budget = Some(budget);
        self.budget_start = self.step_count;
        let result = self.execute();
        self.budget = None;
        match result {
            Ok(val) => {
                // Completed — clear the timer for any future reuse.
                self.start_time = None;
                StepResult::Completed(val)
            }
            Err(VmError::BudgetExhausted) => StepResult::Yielded {
                steps_used: self.step_count - self.budget_start,
            },
            Err(VmError::MailboxEmpty) => StepResult::Blocked,
            Err(e) => {
                // Errored out — clear the timer.
                self.start_time = None;
                StepResult::Error(e)
            }
        }
    }

    /// Set up the entry function for bounded execution.
    pub fn set_entry_function(&mut self, func_idx: u32) -> Result<(), VmError> {
        if self.call_stack.is_empty() {
            self.call_function(func_idx, vec![])?;
        }
        Ok(())
    }

    /// Get the module (for cloning into child actors).
    pub fn module(&self) -> &Module {
        &self.module
    }

    /// Get/set actor ID.
    pub fn actor_id(&self) -> u64 {
        self.actor_id
    }
    pub fn set_actor_id(&mut self, id: u64) {
        self.actor_id = id;
    }

    /// Deliver a message to this VM's mailbox.
    pub fn deliver_message(&mut self, msg: Message) {
        self.mailbox.push_back(msg);
    }

    /// Check if mailbox has messages.
    pub fn has_messages(&self) -> bool {
        !self.mailbox.is_empty()
    }

    /// Drain outgoing messages (called by scheduler after each step).
    pub fn drain_outgoing_messages(&mut self) -> Vec<(u64, Value)> {
        std::mem::take(&mut self.outgoing_messages)
    }

    /// Drain spawn requests (called by scheduler after each step).
    pub fn drain_spawn_requests(&mut self) -> Vec<SpawnRequest> {
        std::mem::take(&mut self.spawn_requests)
    }

    /// Set up a function call.
    fn call_function(&mut self, func_idx: u32, args: Vec<Value>) -> Result<(), VmError> {
        let func = self
            .module
            .functions
            .get(func_idx as usize)
            .ok_or(VmError::InvalidFunction(func_idx))?;

        if self.call_stack.len() >= MAX_CALL_DEPTH {
            return Err(VmError::StackOverflow(MAX_CALL_DEPTH));
        }

        let mut locals = vec![Value::Unit; func.locals as usize];
        for (i, arg) in args.into_iter().enumerate() {
            if i < locals.len() {
                locals[i] = arg;
            }
        }

        let frame = CallFrame {
            func_idx,
            ip: 0,
            stack_base: self.stack.len(),
            locals,
        };
        self.call_stack.push(frame);
        Ok(())
    }

    /// Main execution loop.
    fn execute(&mut self) -> Result<Value, VmError> {
        loop {
            self.step_count += 1;
            if self.step_count > self.max_steps {
                return Err(VmError::ExecutionLimitExceeded(self.max_steps));
            }
            // Wall-clock check (cheap when limit unset; ~1 syscall per
            // WALL_TIME_CHECK_EVERY steps when set). Skipped during the first
            // batch so a 0-step program with a 0-ms limit still produces a
            // deterministic result rather than a flaky time-out.
            if let Some(max_ms) = self.max_wall_ms {
                if self.step_count.is_multiple_of(WALL_TIME_CHECK_EVERY) {
                    if let Some(start) = self.start_time {
                        if start.elapsed().as_millis() as u64 > max_ms {
                            return Err(VmError::WallTimeExceeded(max_ms));
                        }
                    }
                }
            }
            // Budget check for bounded execution
            if let Some(budget) = self.budget {
                if self.step_count - self.budget_start >= budget {
                    return Err(VmError::BudgetExhausted);
                }
            }

            let frame = match self.call_stack.last() {
                Some(f) => f,
                None => {
                    // All frames returned; result is on stack or Unit
                    return Ok(self.stack.pop().unwrap_or(Value::Unit));
                }
            };

            let func_idx = frame.func_idx;
            let ip = frame.ip;

            let func = &self.module.functions[func_idx as usize];
            if ip >= func.code.len() {
                // Implicit return Unit
                let base = frame.stack_base;
                self.call_stack.pop();
                self.stack.truncate(base);
                self.stack.push(Value::Unit);
                continue;
            }

            let op = func.code[ip].clone();

            if self.trace_enabled {
                let fname = &self.module.functions[func_idx as usize].name;
                self.trace.push(format!(
                    "[{fname}:{ip}] {:?}  stack_depth={}",
                    op,
                    self.stack.len()
                ));
            }

            // Advance IP before executing (jumps will override)
            self.call_stack.last_mut().unwrap().ip = ip + 1;

            match op {
                Op::PushConst(idx) => {
                    let val = self
                        .module
                        .constants
                        .get(idx as usize)
                        .ok_or(VmError::InvalidConstant(idx))?
                        .clone();
                    self.push(val)?;
                }
                Op::LoadLocal(idx) => {
                    let val = self.get_local(idx)?.clone();
                    self.push(val)?;
                }
                Op::StoreLocal(idx) => {
                    let val = self.pop()?;
                    self.set_local(idx, val)?;
                }
                Op::LoadGlobal(idx) => {
                    let val = self
                        .globals
                        .get(idx as usize)
                        .ok_or(VmError::InvalidGlobal(idx))?
                        .clone();
                    self.push(val)?;
                }
                Op::StoreGlobal(idx) => {
                    let val = self.pop()?;
                    if (idx as usize) >= self.globals.len() {
                        return Err(VmError::InvalidGlobal(idx));
                    }
                    self.globals[idx as usize] = val;
                }
                Op::Call(target, arity) => {
                    let mut args = Vec::with_capacity(arity as usize);
                    for _ in 0..arity {
                        args.push(self.pop()?);
                    }
                    args.reverse();
                    self.call_function(target, args)?;
                }
                Op::Ret => {
                    let result = self.pop().unwrap_or(Value::Unit);
                    let frame = self.call_stack.pop().unwrap();
                    self.stack.truncate(frame.stack_base);
                    self.push(result)?;
                }
                Op::Jmp(offset) => {
                    self.call_stack.last_mut().unwrap().ip = offset as usize;
                }
                Op::JmpIf(offset) => {
                    let val = self.pop()?;
                    if val.is_truthy() {
                        self.call_stack.last_mut().unwrap().ip = offset as usize;
                    }
                }
                Op::JmpIfNot(offset) => {
                    let val = self.pop()?;
                    if !val.is_truthy() {
                        self.call_stack.last_mut().unwrap().ip = offset as usize;
                    }
                }
                Op::Match(table_idx) => {
                    let val = self.pop()?;
                    let table = self.module.functions[func_idx as usize]
                        .match_tables
                        .get(table_idx as usize)
                        .ok_or(VmError::MatchExhausted)?
                        .clone();

                    let tag = match &val {
                        Value::Enum { variant, .. } => *variant as i32,
                        Value::Bool(true) => 1,
                        Value::Bool(false) => 0,
                        Value::None => -2,
                        Value::Some(_) => -3,
                        Value::Ok(_) => -4,
                        Value::Err(_) => -5,
                        _ => -1,
                    };

                    let mut matched = false;
                    for arm in &table {
                        if arm.tag == tag || arm.tag == -1 {
                            // Push the inner value for destructuring
                            match val.clone() {
                                Value::Enum { payload, .. } => self.push(*payload)?,
                                Value::Some(v) => self.push(*v)?,
                                Value::Ok(v) => self.push(*v)?,
                                Value::Err(v) => self.push(*v)?,
                                other => self.push(other)?,
                            }
                            self.call_stack.last_mut().unwrap().ip = arm.target as usize;
                            matched = true;
                            break;
                        }
                    }
                    if !matched {
                        return Err(VmError::MatchExhausted);
                    }
                }
                Op::MakeRecord(type_id, field_count) => {
                    let mut fields = Vec::with_capacity(field_count as usize);
                    for _ in 0..field_count {
                        fields.push(self.pop()?);
                    }
                    fields.reverse();
                    self.push(Value::Record { type_id, fields })?;
                }
                Op::MakeEnum(type_id, variant) => {
                    let payload = self.pop()?;
                    self.push(Value::Enum {
                        type_id,
                        variant,
                        payload: Box::new(payload),
                    })?;
                }
                Op::GetField(idx) => {
                    let val = self.pop()?;
                    match val {
                        Value::Record { fields, .. } => {
                            let field =
                                fields
                                    .into_iter()
                                    .nth(idx as usize)
                                    .ok_or(VmError::TypeError {
                                        expected: "valid field index",
                                        got: "out of bounds",
                                    })?;
                            self.push(field)?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "Record",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::SpawnActor(func_idx) => {
                    let child_id = self.next_spawn_id;
                    self.next_spawn_id += 1;
                    self.spawn_requests.push(SpawnRequest { func_idx });
                    self.push(Value::ActorId(child_id))?;
                }
                Op::SendMsg => {
                    let payload = self.pop()?;
                    let target = self.pop()?;
                    match target {
                        Value::ActorId(id) => {
                            self.outgoing_messages.push((id, payload));
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "ActorId",
                                got: target.type_name(),
                            })
                        }
                    }
                }
                Op::ReceiveMsg => {
                    if let Some(msg) = self.mailbox.pop_front() {
                        self.push(msg.payload)?;
                    } else if self.in_actor_context {
                        // Actor mode: rewind IP so ReceiveMsg re-executes
                        // when the scheduler delivers a message and
                        // resumes this actor.
                        self.call_stack.last_mut().unwrap().ip = ip;
                        return Err(VmError::MailboxEmpty);
                    } else {
                        // Standalone mode (incl. boruna_run's streaming
                        // progress loop): push Unit and continue. Mirrors
                        // legacy `vm.run()` semantics. Reviewed 0.4-S6 —
                        // the prior signal was `self.budget.is_some()`,
                        // which incorrectly forked behavior whenever a
                        // standalone caller used `execute_bounded` for
                        // anything other than actor scheduling.
                        self.push(Value::Unit)?;
                    }
                }
                Op::Assert(err_const) => {
                    let val = self.pop()?;
                    if !val.is_truthy() {
                        let msg = self
                            .module
                            .constants
                            .get(err_const as usize)
                            .map(|v| format!("{v}"))
                            .unwrap_or_else(|| "assertion failed".into());
                        return Err(VmError::AssertionFailed(msg));
                    }
                }
                Op::CapCall(cap_id, arg_count) => {
                    let cap =
                        Capability::from_id(cap_id).ok_or(VmError::UnknownCapability(cap_id))?;

                    // Check function capabilities
                    let has_cap = self.module.functions[func_idx as usize]
                        .capabilities
                        .contains(&cap);
                    if !has_cap {
                        return Err(VmError::CapabilityDenied(cap));
                    }

                    let mut args = Vec::with_capacity(arg_count as usize);
                    for _ in 0..arg_count {
                        args.push(self.pop()?);
                    }
                    args.reverse();

                    let result = self.gateway.call(&cap, &args, &mut self.event_log)?;
                    self.last_cap_events.push(cap.name());
                    self.push(result)?;
                }
                Op::Add => self.binary_op(|a, b| match (a, b) {
                    (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x + y)),
                    (Value::Float(x), Value::Float(y)) => Ok(Value::Float(x + y)),
                    (Value::Int(x), Value::Float(y)) => Ok(Value::Float(x as f64 + y)),
                    (Value::Float(x), Value::Int(y)) => Ok(Value::Float(x + y as f64)),
                    (a, b) => Err(VmError::TypeError {
                        expected: "numeric",
                        got: if matches!(a, Value::Int(_) | Value::Float(_)) {
                            b.type_name()
                        } else {
                            a.type_name()
                        },
                    }),
                })?,
                Op::Sub => self.binary_op(|a, b| match (a, b) {
                    (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x - y)),
                    (Value::Float(x), Value::Float(y)) => Ok(Value::Float(x - y)),
                    (Value::Int(x), Value::Float(y)) => Ok(Value::Float(x as f64 - y)),
                    (Value::Float(x), Value::Int(y)) => Ok(Value::Float(x - y as f64)),
                    (a, b) => Err(VmError::TypeError {
                        expected: "numeric",
                        got: if matches!(a, Value::Int(_) | Value::Float(_)) {
                            b.type_name()
                        } else {
                            a.type_name()
                        },
                    }),
                })?,
                Op::Mul => self.binary_op(|a, b| match (a, b) {
                    (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x * y)),
                    (Value::Float(x), Value::Float(y)) => Ok(Value::Float(x * y)),
                    (Value::Int(x), Value::Float(y)) => Ok(Value::Float(x as f64 * y)),
                    (Value::Float(x), Value::Int(y)) => Ok(Value::Float(x * y as f64)),
                    (a, b) => Err(VmError::TypeError {
                        expected: "numeric",
                        got: if matches!(a, Value::Int(_) | Value::Float(_)) {
                            b.type_name()
                        } else {
                            a.type_name()
                        },
                    }),
                })?,
                Op::Div => self.binary_op(|a, b| {
                    match (&a, &b) {
                        (_, Value::Int(0)) => return Err(VmError::DivisionByZero),
                        (_, Value::Float(f)) if *f == 0.0 => return Err(VmError::DivisionByZero),
                        _ => {}
                    }
                    match (a, b) {
                        (Value::Int(x), Value::Int(y)) => Ok(Value::Int(x / y)),
                        (Value::Float(x), Value::Float(y)) => Ok(Value::Float(x / y)),
                        (Value::Int(x), Value::Float(y)) => Ok(Value::Float(x as f64 / y)),
                        (Value::Float(x), Value::Int(y)) => Ok(Value::Float(x / y as f64)),
                        (a, b) => Err(VmError::TypeError {
                            expected: "numeric",
                            got: if matches!(a, Value::Int(_) | Value::Float(_)) {
                                b.type_name()
                            } else {
                                a.type_name()
                            },
                        }),
                    }
                })?,
                Op::Mod => self.binary_op(|a, b| match (a, b) {
                    (Value::Int(x), Value::Int(y)) => {
                        if y == 0 {
                            return Err(VmError::DivisionByZero);
                        }
                        Ok(Value::Int(x % y))
                    }
                    (a, b) => Err(VmError::TypeError {
                        expected: "Int",
                        got: if matches!(a, Value::Int(_)) {
                            b.type_name()
                        } else {
                            a.type_name()
                        },
                    }),
                })?,
                Op::Neg => {
                    let val = self.pop()?;
                    match val {
                        Value::Int(n) => self.push(Value::Int(-n))?,
                        Value::Float(n) => self.push(Value::Float(-n))?,
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "numeric",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::Eq => self.binary_op(|a, b| Ok(Value::Bool(a == b)))?,
                Op::Neq => self.binary_op(|a, b| Ok(Value::Bool(a != b)))?,
                Op::Lt => self.compare_op(|ord| ord.is_lt())?,
                Op::Lte => self.compare_op(|ord| ord.is_le())?,
                Op::Gt => self.compare_op(|ord| ord.is_gt())?,
                Op::Gte => self.compare_op(|ord| ord.is_ge())?,
                Op::Not => {
                    let val = self.pop()?;
                    self.push(Value::Bool(!val.is_truthy()))?;
                }
                Op::And => {
                    self.binary_op(|a, b| Ok(Value::Bool(a.is_truthy() && b.is_truthy())))?
                }
                Op::Or => self.binary_op(|a, b| Ok(Value::Bool(a.is_truthy() || b.is_truthy())))?,
                Op::Concat => self.binary_op(|a, b| match (a, b) {
                    (Value::String(x), Value::String(y)) => Ok(Value::String(format!("{x}{y}"))),
                    (a, b) => Err(VmError::TypeError {
                        expected: "String",
                        got: if matches!(a, Value::String(_)) {
                            b.type_name()
                        } else {
                            a.type_name()
                        },
                    }),
                })?,
                Op::Pop => {
                    self.pop()?;
                }
                Op::Dup => {
                    let val = self.stack.last().ok_or(VmError::StackUnderflow)?.clone();
                    self.push(val)?;
                }
                Op::EmitUi => {
                    let tree = self.pop()?;
                    self.event_log.log_ui_emit(&tree);
                    self.ui_output.push(tree);
                }
                Op::MakeList(count) => {
                    let mut items = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        items.push(self.pop()?);
                    }
                    items.reverse();
                    self.push(Value::List(items))?;
                }
                Op::ListLen => {
                    let val = self.pop()?;
                    match val {
                        Value::List(items) => {
                            self.push(Value::Int(items.len() as i64))?;
                        }
                        Value::Record {
                            type_id: 0xFFFF,
                            fields,
                            ..
                        } => {
                            self.push(Value::Int(fields.len() as i64))?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "List",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::ListGet => {
                    let index = self.pop()?;
                    let list = self.pop()?;
                    let idx = match &index {
                        Value::Int(n) => *n,
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "Int",
                                got: index.type_name(),
                            })
                        }
                    };
                    match list {
                        Value::List(items) => {
                            if idx < 0 || idx as usize >= items.len() {
                                return Err(VmError::IndexOutOfBounds {
                                    index: idx,
                                    length: items.len(),
                                });
                            }
                            self.push(items[idx as usize].clone())?;
                        }
                        Value::Record {
                            type_id: 0xFFFF,
                            fields,
                            ..
                        } => {
                            if idx < 0 || idx as usize >= fields.len() {
                                return Err(VmError::IndexOutOfBounds {
                                    index: idx,
                                    length: fields.len(),
                                });
                            }
                            self.push(fields[idx as usize].clone())?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "List",
                                got: list.type_name(),
                            })
                        }
                    }
                }
                Op::ListPush => {
                    let value = self.pop()?;
                    let list = self.pop()?;
                    match list {
                        Value::List(mut items) => {
                            items.push(value);
                            self.push(Value::List(items))?;
                        }
                        Value::Record {
                            type_id: 0xFFFF,
                            mut fields,
                            ..
                        } => {
                            fields.push(value);
                            self.push(Value::List(fields))?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "List",
                                got: list.type_name(),
                            })
                        }
                    }
                }
                Op::ParseInt => {
                    let val = self.pop()?;
                    match val {
                        Value::String(s) => {
                            let n = s.trim().parse::<i64>().unwrap_or(0);
                            self.push(Value::Int(n))?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::TryParseInt => {
                    let val = self.pop()?;
                    match val {
                        Value::String(s) => match s.trim().parse::<i64>() {
                            Result::Ok(n) => self.push(Value::Ok(Box::new(Value::Int(n))))?,
                            Result::Err(_) => self.push(Value::Err(Box::new(Value::String(
                                format!("invalid integer: {s}"),
                            ))))?,
                        },
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::StrContains => {
                    let needle = self.pop()?;
                    let haystack = self.pop()?;
                    match (haystack, needle) {
                        (Value::String(h), Value::String(n)) => {
                            self.push(Value::Bool(h.contains(&*n)))?;
                        }
                        (Value::String(_), b) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: b.type_name(),
                            })
                        }
                        (a, _) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: a.type_name(),
                            })
                        }
                    }
                }
                Op::StrStartsWith => {
                    let prefix = self.pop()?;
                    let string = self.pop()?;
                    match (string, prefix) {
                        (Value::String(s), Value::String(p)) => {
                            self.push(Value::Bool(s.starts_with(&*p)))?;
                        }
                        (Value::String(_), b) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: b.type_name(),
                            })
                        }
                        (a, _) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: a.type_name(),
                            })
                        }
                    }
                }
                Op::IntToString => {
                    let val = self.pop()?;
                    match val {
                        Value::Int(n) => self.push(Value::String(format!("{n}")))?,
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "Int",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::FloatToString => {
                    let val = self.pop()?;
                    match val {
                        Value::Float(f) => self.push(Value::String(format!("{f}")))?,
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "Float",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::StringLen => {
                    let val = self.pop()?;
                    match val {
                        Value::String(s) => self.push(Value::Int(s.len() as i64))?,
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::StringChars => {
                    let val = self.pop()?;
                    match val {
                        Value::String(s) => {
                            let chars: Vec<Value> =
                                s.chars().map(|c| Value::String(c.to_string())).collect();
                            self.push(Value::List(chars))?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::StringContains => {
                    let needle = self.pop()?;
                    let haystack = self.pop()?;
                    match (haystack, needle) {
                        (Value::String(h), Value::String(n)) => {
                            self.push(Value::Bool(h.contains(n.as_str())))?;
                        }
                        (Value::String(_), b) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: b.type_name(),
                            })
                        }
                        (a, _) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: a.type_name(),
                            })
                        }
                    }
                }
                Op::StringStartsWith => {
                    let prefix = self.pop()?;
                    let string = self.pop()?;
                    match (string, prefix) {
                        (Value::String(s), Value::String(p)) => {
                            self.push(Value::Bool(s.starts_with(p.as_str())))?;
                        }
                        (Value::String(_), b) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: b.type_name(),
                            })
                        }
                        (a, _) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: a.type_name(),
                            })
                        }
                    }
                }
                Op::StringEndsWith => {
                    let suffix = self.pop()?;
                    let string = self.pop()?;
                    match (string, suffix) {
                        (Value::String(s), Value::String(sfx)) => {
                            self.push(Value::Bool(s.ends_with(sfx.as_str())))?;
                        }
                        (Value::String(_), b) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: b.type_name(),
                            })
                        }
                        (a, _) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: a.type_name(),
                            })
                        }
                    }
                }
                Op::StringToUpper => {
                    let val = self.pop()?;
                    match val {
                        Value::String(s) => self.push(Value::String(s.to_uppercase()))?,
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::StringToLower => {
                    let val = self.pop()?;
                    match val {
                        Value::String(s) => self.push(Value::String(s.to_lowercase()))?,
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::StringTrim => {
                    let val = self.pop()?;
                    match val {
                        Value::String(s) => self.push(Value::String(s.trim().to_string()))?,
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::StringJoin => {
                    let sep = self.pop()?;
                    let list = self.pop()?;
                    match (list, sep) {
                        (Value::List(items), Value::String(separator)) => {
                            let parts: Result<Vec<String>, VmError> = items
                                .into_iter()
                                .map(|v| match v {
                                    Value::String(s) => Ok(s),
                                    _ => Err(VmError::TypeError {
                                        expected: "String",
                                        got: v.type_name(),
                                    }),
                                })
                                .collect();
                            self.push(Value::String(parts?.join(&separator)))?;
                        }
                        (Value::List(_), b) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: b.type_name(),
                            })
                        }
                        (a, _) => {
                            return Err(VmError::TypeError {
                                expected: "List",
                                got: a.type_name(),
                            })
                        }
                    }
                }
                Op::ListLenBuiltin => {
                    let val = self.pop()?;
                    match val {
                        Value::List(items) => self.push(Value::Int(items.len() as i64))?,
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "List",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::ListIsEmpty => {
                    let val = self.pop()?;
                    match val {
                        Value::List(items) => self.push(Value::Bool(items.is_empty()))?,
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "List",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::ListHead => {
                    let val = self.pop()?;
                    match val {
                        Value::List(items) => {
                            let result = match items.into_iter().next() {
                                Some(v) => Value::Some(Box::new(v)),
                                None => Value::None,
                            };
                            self.push(result)?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "List",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::ListTail => {
                    let val = self.pop()?;
                    match val {
                        Value::List(items) => {
                            let tail = if items.is_empty() {
                                vec![]
                            } else {
                                items[1..].to_vec()
                            };
                            self.push(Value::List(tail))?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "List",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::ListAppend => {
                    let item = self.pop()?;
                    let val = self.pop()?;
                    match val {
                        Value::List(mut items) => {
                            items.push(item);
                            self.push(Value::List(items))?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "List",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::ListConcat => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    match (a, b) {
                        (Value::List(mut la), Value::List(lb)) => {
                            la.extend(lb);
                            self.push(Value::List(la))?;
                        }
                        (Value::List(_), b) => {
                            return Err(VmError::TypeError {
                                expected: "List",
                                got: b.type_name(),
                            })
                        }
                        (a, _) => {
                            return Err(VmError::TypeError {
                                expected: "List",
                                got: a.type_name(),
                            })
                        }
                    }
                }
                Op::ListReverse => {
                    let val = self.pop()?;
                    match val {
                        Value::List(items) => {
                            let reversed: Vec<Value> = items.into_iter().rev().collect();
                            self.push(Value::List(reversed))?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "List",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::StringSplit => {
                    let sep = self.pop()?;
                    let s = self.pop()?;
                    match (s, sep) {
                        (Value::String(s), Value::String(sep)) => {
                            let parts: Vec<Value> = if sep.is_empty() {
                                s.chars().map(|c| Value::String(c.to_string())).collect()
                            } else {
                                s.split(sep.as_str())
                                    .map(|p| Value::String(p.to_string()))
                                    .collect()
                            };
                            self.push(Value::List(parts))?;
                        }
                        (Value::String(_), b) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: b.type_name(),
                            })
                        }
                        (a, _) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: a.type_name(),
                            })
                        }
                    }
                }
                Op::StringReplace => {
                    let to = self.pop()?;
                    let from = self.pop()?;
                    let s = self.pop()?;
                    match (s, from, to) {
                        (Value::String(s), Value::String(from), Value::String(to)) => {
                            self.push(Value::String(s.replace(from.as_str(), to.as_str())))?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: "non-String",
                            })
                        }
                    }
                }
                Op::StringSlice => {
                    let end = self.pop()?;
                    let start = self.pop()?;
                    let s = self.pop()?;
                    match (s, start, end) {
                        (Value::String(s), Value::Int(start), Value::Int(end)) => {
                            let slice = s
                                .get(start as usize..end as usize)
                                .unwrap_or("")
                                .to_string();
                            self.push(Value::String(slice))?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "String, Int, Int",
                                got: "wrong types",
                            })
                        }
                    }
                }
                Op::IntParse => {
                    let val = self.pop()?;
                    match val {
                        Value::String(s) => {
                            let result = match s.parse::<i64>() {
                                std::result::Result::Ok(n) => Value::Some(Box::new(Value::Int(n))),
                                std::result::Result::Err(_) => Value::None,
                            };
                            self.push(result)?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::FloatParse => {
                    let val = self.pop()?;
                    match val {
                        Value::String(s) => {
                            let result = match s.parse::<f64>() {
                                std::result::Result::Ok(f) => {
                                    Value::Some(Box::new(Value::Float(f)))
                                }
                                std::result::Result::Err(_) => Value::None,
                            };
                            self.push(result)?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::BoolToString => {
                    let val = self.pop()?;
                    match val {
                        Value::Bool(b) => {
                            self.push(Value::String(if b {
                                "true".to_string()
                            } else {
                                "false".to_string()
                            }))?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "Bool",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::MapGet => {
                    let key = self.pop()?;
                    let map = self.pop()?;
                    match (map, key) {
                        (Value::Map(m), Value::String(k)) => {
                            let result = match m.get(&k) {
                                Some(v) => Value::Some(Box::new(v.clone())),
                                Option::None => Value::None,
                            };
                            self.push(result)?;
                        }
                        (Value::Map(_), k) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: k.type_name(),
                            })
                        }
                        (m, _) => {
                            return Err(VmError::TypeError {
                                expected: "Map",
                                got: m.type_name(),
                            })
                        }
                    }
                }
                Op::MapSet => {
                    let val = self.pop()?;
                    let key = self.pop()?;
                    let map = self.pop()?;
                    match (map, key) {
                        (Value::Map(mut m), Value::String(k)) => {
                            m.insert(k, val);
                            self.push(Value::Map(m))?;
                        }
                        (Value::Map(_), k) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: k.type_name(),
                            })
                        }
                        (m, _) => {
                            return Err(VmError::TypeError {
                                expected: "Map",
                                got: m.type_name(),
                            })
                        }
                    }
                }
                Op::MapRemove => {
                    let key = self.pop()?;
                    let map = self.pop()?;
                    match (map, key) {
                        (Value::Map(mut m), Value::String(k)) => {
                            m.remove(&k);
                            self.push(Value::Map(m))?;
                        }
                        (Value::Map(_), k) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: k.type_name(),
                            })
                        }
                        (m, _) => {
                            return Err(VmError::TypeError {
                                expected: "Map",
                                got: m.type_name(),
                            })
                        }
                    }
                }
                Op::MapContainsKey => {
                    let key = self.pop()?;
                    let map = self.pop()?;
                    match (map, key) {
                        (Value::Map(m), Value::String(k)) => {
                            self.push(Value::Bool(m.contains_key(&k)))?;
                        }
                        (Value::Map(_), k) => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: k.type_name(),
                            })
                        }
                        (m, _) => {
                            return Err(VmError::TypeError {
                                expected: "Map",
                                got: m.type_name(),
                            })
                        }
                    }
                }
                Op::MapKeys => {
                    let val = self.pop()?;
                    match val {
                        Value::Map(m) => {
                            let keys: Vec<Value> =
                                m.keys().map(|k| Value::String(k.clone())).collect();
                            self.push(Value::List(keys))?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "Map",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::MapValues => {
                    let val = self.pop()?;
                    match val {
                        Value::Map(m) => {
                            let values: Vec<Value> = m.values().cloned().collect();
                            self.push(Value::List(values))?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "Map",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::MapLen => {
                    let val = self.pop()?;
                    match val {
                        Value::Map(m) => {
                            self.push(Value::Int(m.len() as i64))?;
                        }
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "Map",
                                got: val.type_name(),
                            })
                        }
                    }
                }
                Op::Nop => {}
                Op::Halt => {
                    return Ok(self.stack.pop().unwrap_or(Value::Unit));
                }
            }
        }
    }

    fn push(&mut self, val: Value) -> Result<(), VmError> {
        if self.stack.len() >= MAX_STACK {
            return Err(VmError::StackOverflow(MAX_STACK));
        }
        self.stack.push(val);
        Ok(())
    }

    fn pop(&mut self) -> Result<Value, VmError> {
        self.stack.pop().ok_or(VmError::StackUnderflow)
    }

    fn get_local(&self, idx: u32) -> Result<&Value, VmError> {
        let frame = self.call_stack.last().ok_or(VmError::StackUnderflow)?;
        frame
            .locals
            .get(idx as usize)
            .ok_or(VmError::InvalidLocal(idx))
    }

    fn set_local(&mut self, idx: u32, val: Value) -> Result<(), VmError> {
        let frame = self.call_stack.last_mut().ok_or(VmError::StackUnderflow)?;
        if (idx as usize) >= frame.locals.len() {
            return Err(VmError::InvalidLocal(idx));
        }
        frame.locals[idx as usize] = val;
        Ok(())
    }

    fn binary_op(
        &mut self,
        f: impl FnOnce(Value, Value) -> Result<Value, VmError>,
    ) -> Result<(), VmError> {
        let b = self.pop()?;
        let a = self.pop()?;
        let result = f(a, b)?;
        self.push(result)
    }

    fn compare_op(&mut self, f: impl FnOnce(std::cmp::Ordering) -> bool) -> Result<(), VmError> {
        let b = self.pop()?;
        let a = self.pop()?;
        let ord = match (&a, &b) {
            (Value::Int(x), Value::Int(y)) => x.cmp(y),
            (Value::Float(x), Value::Float(y)) => {
                x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal)
            }
            (Value::String(x), Value::String(y)) => x.cmp(y),
            _ => {
                return Err(VmError::TypeError {
                    expected: "comparable",
                    got: a.type_name(),
                })
            }
        };
        self.push(Value::Bool(f(ord)))
    }
}
