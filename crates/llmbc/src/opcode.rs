use serde::{Deserialize, Serialize};

/// Bytecode instructions for the Boruna VM.
/// Stack-based: operands are pushed/popped from the operand stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    /// Push a constant from the constant pool onto the stack.
    PushConst(u32),

    /// Load a local variable onto the stack.
    LoadLocal(u32),

    /// Pop the top of stack into a local variable.
    StoreLocal(u32),

    /// Load a global variable onto the stack.
    LoadGlobal(u32),

    /// Pop the top of stack into a global variable.
    StoreGlobal(u32),

    /// Pop N args, call function by index. Pushes return value.
    Call(u32, u8),

    /// Return from the current function. Pops return value from stack.
    Ret,

    /// Unconditional jump to instruction offset.
    Jmp(u32),

    /// Pop top of stack; jump if truthy.
    JmpIf(u32),

    /// Pop top of stack; jump if falsy.
    JmpIfNot(u32),

    /// Pop value, match against N patterns. Each pattern is (tag, jump_offset).
    /// Pattern data follows in the constant pool.
    Match(u32),

    /// Pop N values, create a record with type_id.
    MakeRecord(u32, u8),

    /// Pop value, wrap in enum variant (type_id, variant_index).
    MakeEnum(u32, u8),

    /// Access field at index from record on top of stack.
    GetField(u8),

    /// Spawn a new actor from function index.
    SpawnActor(u32),

    /// Pop value, send message to actor (actor_id on stack below value).
    SendMsg,

    /// Block until a message arrives. Pushes received value.
    ReceiveMsg,

    /// Pop value, assert it is truthy or abort with error const index.
    Assert(u32),

    /// Capability call: cap_id, arg_count. Args on stack.
    CapCall(u32, u8),

    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Neg,

    // Comparison
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,

    // Logical
    Not,
    And,
    Or,

    // String
    Concat,

    /// Pop and discard top of stack.
    Pop,

    /// Duplicate top of stack.
    Dup,

    /// Emit a UI tree (top of stack is the UI descriptor).
    EmitUi,

    // List operations
    /// Pop N values, create a list. Operand is element count.
    MakeList(u8),

    /// Pop a list, push its length as Int.
    ListLen,

    /// Pop list and index (Int), push element. Traps on out-of-bounds.
    ListGet,

    /// Pop list and value, push new list with value appended.
    ListPush,

    // String builtins
    /// Pop a string, push parsed Int (0 on failure).
    ParseInt,

    /// Pop two strings (haystack, needle), push Bool.
    StrContains,

    /// Pop two strings (string, prefix), push Bool.
    StrStartsWith,

    /// Pop a string, push Result: Ok(Int) on success, Err(String) on failure.
    TryParseInt,

    /// No operation.
    Nop,

    /// Halt execution.
    Halt,
}

impl Op {
    /// Encode to bytes (for binary format).
    pub fn to_byte_tag(&self) -> u8 {
        match self {
            Op::PushConst(_) => 0x01,
            Op::LoadLocal(_) => 0x02,
            Op::StoreLocal(_) => 0x03,
            Op::LoadGlobal(_) => 0x04,
            Op::StoreGlobal(_) => 0x05,
            Op::Call(_, _) => 0x06,
            Op::Ret => 0x07,
            Op::Jmp(_) => 0x08,
            Op::JmpIf(_) => 0x09,
            Op::JmpIfNot(_) => 0x0A,
            Op::Match(_) => 0x0B,
            Op::MakeRecord(_, _) => 0x0C,
            Op::MakeEnum(_, _) => 0x0D,
            Op::GetField(_) => 0x0E,
            Op::SpawnActor(_) => 0x0F,
            Op::SendMsg => 0x10,
            Op::ReceiveMsg => 0x11,
            Op::Assert(_) => 0x12,
            Op::CapCall(_, _) => 0x13,
            Op::Add => 0x20,
            Op::Sub => 0x21,
            Op::Mul => 0x22,
            Op::Div => 0x23,
            Op::Mod => 0x24,
            Op::Neg => 0x25,
            Op::Eq => 0x30,
            Op::Neq => 0x31,
            Op::Lt => 0x32,
            Op::Lte => 0x33,
            Op::Gt => 0x34,
            Op::Gte => 0x35,
            Op::Not => 0x40,
            Op::And => 0x41,
            Op::Or => 0x42,
            Op::Concat => 0x50,
            Op::Pop => 0x60,
            Op::Dup => 0x61,
            Op::EmitUi => 0x70,
            Op::MakeList(_) => 0x80,
            Op::ListLen => 0x81,
            Op::ListGet => 0x82,
            Op::ListPush => 0x83,
            Op::ParseInt => 0x84,
            Op::StrContains => 0x85,
            Op::StrStartsWith => 0x86,
            Op::TryParseInt => 0x87,
            Op::Nop => 0xFE,
            Op::Halt => 0xFF,
        }
    }
}
