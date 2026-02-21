use thiserror::Error;

#[derive(Debug, Error)]
pub enum FrameworkError {
    #[error("validation: {0}")]
    Validation(String),

    #[error("missing required function: {0}")]
    MissingFunction(String),

    #[error("function `{name}` must not have capability annotations")]
    PurityViolation { name: String },

    #[error("function `{name}` has wrong arity: expected {expected}, got {got}")]
    WrongArity {
        name: String,
        expected: usize,
        got: usize,
    },

    #[error("missing type definition: {0}")]
    MissingType(String),

    #[error("effect error: {0}")]
    Effect(String),

    #[error("policy violation: {0}")]
    PolicyViolation(String),

    #[error("state error: {0}")]
    State(String),

    #[error("compile error: {0}")]
    Compile(#[from] boruna_compiler::CompileError),

    #[error("runtime error: {0}")]
    Runtime(#[from] boruna_vm::VmError),

    #[error("max cycles exceeded: {0}")]
    MaxCyclesExceeded(u64),
}
