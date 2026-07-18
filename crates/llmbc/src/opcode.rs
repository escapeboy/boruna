use serde::{Deserialize, Serialize};

/// Which contract clause an [`Op::Assert`] guards. Recorded verbatim
/// (`requires`/`ensures`) into the evidence trail's `ContractCheck`
/// events, so an auditor can tell preconditions from postconditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContractKind {
    Requires,
    Ensures,
}

impl ContractKind {
    /// Stable lowercase label used in the audit trail.
    pub fn as_str(&self) -> &'static str {
        match self {
            ContractKind::Requires => "requires",
            ContractKind::Ensures => "ensures",
        }
    }
}

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

    /// Indirect call: the callee `Value::FnRef(func_idx)` is on top of the
    /// stack, with the N args below it. Pops the FnRef and the N args, calls
    /// the referenced function, pushes the return value. Used for higher-order
    /// / computed call targets (a function passed as a value).
    CallIndirect(u8),

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
    ///
    /// Carries contract provenance for the evidence trail: `kind`
    /// distinguishes a precondition (`requires`) from a postcondition
    /// (`ensures`), and `index` is the 0-based clause position within
    /// that list. The VM records a `ContractCheck` event on every
    /// evaluation (pass or fail) so a sealed run proves each contract
    /// held. Emitted only by codegen for `requires`/`ensures` clauses.
    Assert {
        msg: u32,
        kind: ContractKind,
        index: u32,
    },

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

    /// Pop an Int, push its decimal String representation.
    IntToString,

    /// Pop a Float, push its decimal String representation.
    FloatToString,

    /// Pop a String, push its byte length as Int.
    StringLen,

    /// Pop a String, push a List of single-character Strings.
    StringChars,

    /// Pop two strings (haystack, needle), push Bool.
    StringContains,

    /// Pop two strings (string, prefix), push Bool.
    StringStartsWith,

    /// Pop two strings (string, suffix), push Bool.
    StringEndsWith,

    /// Pop a String, push its uppercase version as String.
    StringToUpper,

    /// Pop a String, push its lowercase version as String.
    StringToLower,

    /// Pop a String, push its trimmed version as String.
    StringTrim,

    /// Pop a List<String> and a separator String, push joined String.
    StringJoin,

    /// Pop a List, push its length as Int.
    ListLenBuiltin,

    /// Pop a List, push Bool (true if empty).
    ListIsEmpty,

    /// Pop a List, push Option<T> (first element or None).
    ListHead,

    /// Pop a List, push List (all elements except first).
    ListTail,

    /// Pop a List and a value, push new List with value appended.
    ListAppend,

    /// Pop two Lists, push their concatenation.
    ListConcat,

    /// Pop a List, push reversed List.
    ListReverse,

    // String built-ins (0x9A+)
    /// Pop two strings (string, sep), push List<String>.
    StringSplit,

    /// Pop three strings (string, from, to), push String.
    StringReplace,

    /// Pop a String and two Ints (start, end), push String slice.
    StringSlice,

    /// Pop a String, push Option<Int>.
    IntParse,

    /// Pop a String, push Option<Float>.
    FloatParse,

    /// Pop a Bool, push String ("true" or "false").
    BoolToString,

    // Map built-ins (0xA0+)
    /// Pop a Map and a String key, push Option<Value>.
    MapGet,

    /// Pop a Map, a String key, and a Value; push new Map with key set.
    MapSet,

    /// Pop a Map and a String key; push new Map with key removed.
    MapRemove,

    /// Pop a Map and a String key; push Bool.
    MapContainsKey,

    /// Pop a Map; push List<String> of keys.
    MapKeys,

    /// Pop a Map; push List of values.
    MapValues,

    /// Pop a Map; push Int length.
    MapLen,

    /// Pop one value; print it to stderr in `Value::Display` form followed
    /// by a newline; push the value back unchanged. Used by the `debug(v)`
    /// builtin. Operational-only — emits no audit-log event, no capability
    /// gate, no effect on replay verification.
    Debug,

    /// Pop a value, then pop a message String; print `<msg> <value>\n` to
    /// stderr; push the value back unchanged. Used by the `debug_msg(msg, v)`
    /// builtin. Same operational-only semantics as [`Op::Debug`].
    DebugMsg,

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
            Op::CallIndirect(_) => 0x14,
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
            Op::Assert { .. } => 0x12,
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
            Op::IntToString => 0x88,
            Op::FloatToString => 0x89,
            Op::StringLen => 0x8A,
            Op::StringChars => 0x8B,
            Op::StringContains => 0x8C,
            Op::StringStartsWith => 0x8D,
            Op::StringEndsWith => 0x8E,
            Op::StringToUpper => 0x8F,
            Op::StringToLower => 0x90,
            Op::StringTrim => 0x91,
            Op::StringJoin => 0x92,
            Op::ListLenBuiltin => 0x93,
            Op::ListIsEmpty => 0x94,
            Op::ListHead => 0x95,
            Op::ListTail => 0x96,
            Op::ListAppend => 0x97,
            Op::ListConcat => 0x98,
            Op::ListReverse => 0x99,
            Op::StringSplit => 0x9A,
            Op::StringReplace => 0x9B,
            Op::StringSlice => 0x9C,
            Op::IntParse => 0x9D,
            Op::FloatParse => 0x9E,
            Op::BoolToString => 0x9F,
            Op::MapGet => 0xA0,
            Op::MapSet => 0xA1,
            Op::MapRemove => 0xA2,
            Op::MapContainsKey => 0xA3,
            Op::MapKeys => 0xA4,
            Op::MapValues => 0xA5,
            Op::MapLen => 0xA6,
            Op::Debug => 0xA7,
            Op::DebugMsg => 0xA8,
            Op::Nop => 0xFE,
            Op::Halt => 0xFF,
        }
    }
}
