use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::capability::Capability;
use crate::opcode::Op;
use crate::value::Value;

/// Magic bytes for .boruna_bytecode files: "LLMB"
pub const MAGIC: [u8; 4] = [0x4C, 0x4C, 0x4D, 0x42];
pub const VERSION: u16 = 1;

#[derive(Debug, Error)]
pub enum BytecodeError {
    #[error("invalid magic bytes")]
    InvalidMagic,
    #[error("unsupported version: {0}")]
    UnsupportedVersion(u16),
    #[error("invalid bytecode: {0}")]
    InvalidBytecode(String),
    #[error("serialization error: {0}")]
    Serialization(String),
}

/// A pattern arm for match expressions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchArm {
    /// What to match: variant index, literal, or wildcard (-1)
    pub tag: i32,
    /// Jump offset if matched
    pub target: u32,
}

/// Type metadata for records and enums.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeDef {
    pub name: String,
    pub kind: TypeKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TypeKind {
    Record {
        fields: Vec<(String, String)>,
    },
    Enum {
        variants: Vec<(String, Option<String>)>,
    },
}

/// A function in the bytecode module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Function {
    pub name: String,
    pub arity: u8,
    pub locals: u16,
    pub code: Vec<Op>,
    /// Capabilities this function is allowed to use.
    pub capabilities: Vec<Capability>,
    /// Match tables referenced by Match instructions.
    pub match_tables: Vec<Vec<MatchArm>>,
}

/// A compiled bytecode module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Module {
    pub name: String,
    pub version: u16,
    pub constants: Vec<Value>,
    pub globals: Vec<String>,
    pub types: Vec<TypeDef>,
    pub functions: Vec<Function>,
    pub entry: u32,
}

impl Module {
    pub fn new(name: impl Into<String>) -> Self {
        Module {
            name: name.into(),
            version: VERSION,
            constants: Vec::new(),
            globals: Vec::new(),
            types: Vec::new(),
            functions: Vec::new(),
            entry: 0,
        }
    }

    /// Serialize to JSON (portable text format).
    pub fn to_json(&self) -> Result<String, BytecodeError> {
        serde_json::to_string_pretty(self).map_err(|e| BytecodeError::Serialization(e.to_string()))
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> Result<Self, BytecodeError> {
        serde_json::from_str(json).map_err(|e| BytecodeError::Serialization(e.to_string()))
    }

    /// Serialize to binary .boruna_bytecode format.
    pub fn to_bytes(&self) -> Result<Vec<u8>, BytecodeError> {
        let mut buf = Vec::new();
        // Magic
        buf.extend_from_slice(&MAGIC);
        // Version
        buf.extend_from_slice(&self.version.to_le_bytes());
        // JSON payload (simple binary format: magic + version + length + json)
        let json =
            serde_json::to_vec(self).map_err(|e| BytecodeError::Serialization(e.to_string()))?;
        let len = json.len() as u32;
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(&json);
        Ok(buf)
    }

    /// Deserialize from binary .boruna_bytecode format.
    pub fn from_bytes(data: &[u8]) -> Result<Self, BytecodeError> {
        if data.len() < 10 {
            return Err(BytecodeError::InvalidBytecode("too short".into()));
        }
        if data[0..4] != MAGIC {
            return Err(BytecodeError::InvalidMagic);
        }
        let version = u16::from_le_bytes([data[4], data[5]]);
        if version != VERSION {
            return Err(BytecodeError::UnsupportedVersion(version));
        }
        let len = u32::from_le_bytes([data[6], data[7], data[8], data[9]]) as usize;
        if data.len() < 10 + len {
            return Err(BytecodeError::InvalidBytecode("truncated payload".into()));
        }
        let module: Module = serde_json::from_slice(&data[10..10 + len])
            .map_err(|e| BytecodeError::Serialization(e.to_string()))?;
        Ok(module)
    }

    /// Add a constant and return its index.
    pub fn add_const(&mut self, value: Value) -> u32 {
        let idx = self.constants.len() as u32;
        self.constants.push(value);
        idx
    }

    /// Add a function and return its index.
    pub fn add_function(&mut self, func: Function) -> u32 {
        let idx = self.functions.len() as u32;
        self.functions.push(func);
        idx
    }
}
