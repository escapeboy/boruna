use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("lexer error at line {line}, col {col}: {msg}")]
    Lexer { line: usize, col: usize, msg: String },

    #[error("parse error at line {line}: {msg}")]
    Parse { line: usize, msg: String },

    #[error("type error: {0}")]
    Type(String),

    #[error("codegen error: {0}")]
    Codegen(String),
}
