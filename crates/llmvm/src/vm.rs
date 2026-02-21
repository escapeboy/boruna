use std::collections::VecDeque;

use boruna_bytecode::{Capability, Module, Op, Value};

use crate::actor::Message;
use crate::capability_gateway::CapabilityGateway;
use crate::error::VmError;
use crate::replay::EventLog;

const MAX_STACK: usize = 4096;
const MAX_CALL_DEPTH: usize = 256;

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
    /// UI tree emitted by EmitUi instructions.
    pub ui_output: Vec<Value>,
    /// Trace log for debugging.
    pub trace: Vec<String>,
    pub trace_enabled: bool,
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
            ui_output: Vec::new(),
            trace: Vec::new(),
            trace_enabled: false,
            actor_id: 0,
            mailbox: VecDeque::new(),
            outgoing_messages: Vec::new(),
            spawn_requests: Vec::new(),
            next_spawn_id: 0,
            budget: None,
            budget_start: 0,
        }
    }

    pub fn set_max_steps(&mut self, max: u64) {
        self.max_steps = max;
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
        self.call_function(entry, vec![])?;
        self.execute()
    }

    /// Run with bounded execution budget. Returns StepResult.
    /// Call `set_entry_function()` first, then call this repeatedly.
    pub fn execute_bounded(&mut self, budget: u64) -> StepResult {
        self.budget = Some(budget);
        self.budget_start = self.step_count;
        let result = self.execute();
        self.budget = None;
        match result {
            Ok(val) => StepResult::Completed(val),
            Err(VmError::BudgetExhausted) => StepResult::Yielded {
                steps_used: self.step_count - self.budget_start,
            },
            Err(VmError::MailboxEmpty) => StepResult::Blocked,
            Err(e) => StepResult::Error(e),
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
                    } else if self.budget.is_some() {
                        // Bounded mode: rewind IP so ReceiveMsg re-executes on resume
                        self.call_stack.last_mut().unwrap().ip = ip;
                        return Err(VmError::MailboxEmpty);
                    } else {
                        // Legacy unbounded mode: push Unit
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
