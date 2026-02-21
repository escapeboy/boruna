pub mod ast;
pub mod codegen;
pub mod error;
pub mod lexer;
pub mod parser;
#[cfg(test)]
mod tests;
pub mod typeck;

pub use error::CompileError;

use boruna_bytecode::Module;

/// Compile source code to a bytecode module.
pub fn compile(name: &str, source: &str) -> Result<Module, CompileError> {
    let tokens = lexer::lex(source)?;
    let program = parser::parse(tokens)?;
    typeck::check(&program)?;
    codegen::emit(name, &program)
}
