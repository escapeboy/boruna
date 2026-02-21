use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Runtime values in the Boruna VM.
/// All values are immutable once created.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    /// Unit / void
    Unit,
    /// Boolean
    Bool(bool),
    /// 64-bit signed integer
    Int(i64),
    /// 64-bit float
    Float(f64),
    /// UTF-8 string
    String(String),
    /// Option<T> — None
    None,
    /// Option<T> — Some(value)
    Some(Box<Value>),
    /// Result<T, E> — Ok(value)
    Ok(Box<Value>),
    /// Result<T, E> — Err(value)
    Err(Box<Value>),
    /// Record { type_id, fields }
    Record {
        type_id: u32,
        fields: Vec<Value>,
    },
    /// Enum variant { type_id, variant, payload }
    Enum {
        type_id: u32,
        variant: u8,
        payload: Box<Value>,
    },
    /// List of values
    List(Vec<Value>),
    /// Map (ordered for determinism)
    Map(BTreeMap<String, Value>),
    /// Actor ID reference
    ActorId(u64),
    /// Function reference (for higher-order functions)
    FnRef(u32),
}

impl Value {
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Unit => false,
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            Value::String(s) => !s.is_empty(),
            Value::None => false,
            Value::Some(_) => true,
            Value::Ok(_) => true,
            Value::Err(_) => false,
            Value::Record { .. } => true,
            Value::Enum { .. } => true,
            Value::List(l) => !l.is_empty(),
            Value::Map(m) => !m.is_empty(),
            Value::ActorId(_) => true,
            Value::FnRef(_) => true,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "Unit",
            Value::Bool(_) => "Bool",
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::String(_) => "String",
            Value::None => "None",
            Value::Some(_) => "Some",
            Value::Ok(_) => "Ok",
            Value::Err(_) => "Err",
            Value::Record { .. } => "Record",
            Value::Enum { .. } => "Enum",
            Value::List(_) => "List",
            Value::Map(_) => "Map",
            Value::ActorId(_) => "ActorId",
            Value::FnRef(_) => "FnRef",
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Unit => write!(f, "()"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(n) => write!(f, "{n}"),
            Value::Float(n) => write!(f, "{n}"),
            Value::String(s) => write!(f, "\"{s}\""),
            Value::None => write!(f, "None"),
            Value::Some(v) => write!(f, "Some({v})"),
            Value::Ok(v) => write!(f, "Ok({v})"),
            Value::Err(v) => write!(f, "Err({v})"),
            Value::Record { type_id, fields } => {
                write!(f, "Record#{type_id}{{")?;
                for (i, field) in fields.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{field}")?;
                }
                write!(f, "}}")
            }
            Value::Enum { type_id, variant, payload } => {
                write!(f, "Enum#{type_id}::{variant}({payload})")
            }
            Value::List(items) => {
                write!(f, "[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{item}")?;
                }
                write!(f, "]")
            }
            Value::Map(entries) => {
                write!(f, "{{")?;
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, "}}")
            }
            Value::ActorId(id) => write!(f, "Actor#{id}"),
            Value::FnRef(id) => write!(f, "Fn#{id}"),
        }
    }
}
