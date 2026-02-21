use boruna_bytecode::Capability;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum VmError {
    #[error("stack underflow")]
    StackUnderflow,

    #[error("stack overflow (max {0})")]
    StackOverflow(usize),

    #[error("invalid instruction pointer: {0}")]
    InvalidIp(usize),

    #[error("invalid function index: {0}")]
    InvalidFunction(u32),

    #[error("invalid constant index: {0}")]
    InvalidConstant(u32),

    #[error("invalid local index: {0}")]
    InvalidLocal(u32),

    #[error("invalid global index: {0}")]
    InvalidGlobal(u32),

    #[error("type error: expected {expected}, got {got}")]
    TypeError {
        expected: &'static str,
        got: &'static str,
    },

    #[error("division by zero")]
    DivisionByZero,

    #[error("capability denied: {0}")]
    CapabilityDenied(Capability),

    #[error("capability budget exceeded: {0}")]
    CapabilityBudgetExceeded(Capability),

    #[error("unknown capability id: {0}")]
    UnknownCapability(u32),

    #[error("assertion failed: {0}")]
    AssertionFailed(String),

    #[error("list index out of bounds: index {index}, length {length}")]
    IndexOutOfBounds { index: i64, length: usize },

    #[error("no match found for value")]
    MatchExhausted,

    #[error("actor not found: {0}")]
    ActorNotFound(u64),

    #[error("actor mailbox empty (blocking not supported in this mode)")]
    MailboxEmpty,

    #[error("max execution steps exceeded ({0})")]
    ExecutionLimitExceeded(u64),

    #[error("halt")]
    Halt,

    #[error("execution budget exhausted")]
    BudgetExhausted,

    #[error("deadlock: all actors blocked with no pending messages")]
    Deadlock,

    #[error("max scheduler rounds exceeded ({0})")]
    MaxRoundsExceeded(u64),

    #[error("bytecode error: {0}")]
    Bytecode(#[from] boruna_bytecode::BytecodeError),
}
