pub mod lexer;
pub mod ast;
pub mod parser;
pub mod typeck;
pub mod codegen;
pub mod error;
#[cfg(test)]
mod tests;

pub use error::CompileError;

use boruna_bytecode::Module;

/// Compile source code to a bytecode module.
pub fn compile(name: &str, source: &str) -> Result<Module, CompileError> {
    let tokens = lexer::lex(source)?;
    let program = parser::parse(tokens)?;
    typeck::check(&program)?;
    codegen::emit(name, &program)
}
