pub mod ast;
pub mod codegen;
pub mod error;
pub mod lexer;
pub mod parser;
pub mod strict;
pub mod suggest;
#[cfg(test)]
mod tests;
pub mod typeck;

pub use error::CompileError;

use boruna_bytecode::Module;

/// Version of the `.ax` language this compiler implements.
///
/// The authoritative specification for this version is
/// `docs/spec/ax-language-1.0.md`. The string is `<major>.<minor>` decimal.
/// Within a major line the language is additive-only: a program that compiles against `2.x`
/// compiles against `2.y` for `y >= x`. 2.0 rejects what 1.x only warned about (E009, E010).
pub const LANGUAGE_VERSION: &str = "2.0";

/// Returns the `.ax` language version this compiler implements.
///
/// See `docs/spec/ax-language-1.0.md` for the formal specification.
pub fn language_version() -> &'static str {
    LANGUAGE_VERSION
}

/// Compile source code to a bytecode module.
pub fn compile(name: &str, source: &str) -> Result<Module, CompileError> {
    let tokens = lexer::lex(source)?;
    let program = parser::parse(tokens)?;
    typeck::check(&program)?;
    check_strict(&program)?;
    codegen::emit(name, &program)
}

/// Reject the first language 2.0 strictness issue (E009 type mismatch, E010 reassignment of a
/// binding that is not `mut`). `boruna lang check` lists all of them with locations.
fn check_strict(program: &ast::Program) -> Result<(), CompileError> {
    let issues = strict::check(program);
    match issues.first() {
        None => Ok(()),
        Some(first) => {
            let more = match issues.len() - 1 {
                0 => String::new(),
                n => format!(" (and {n} more; run `boruna lang check` to list them)"),
            };
            Err(CompileError::Type(format!(
                "in function '{}': {}{more}",
                first.func(),
                first.message()
            )))
        }
    }
}

#[cfg(test)]
mod version_tests {
    use super::{language_version, LANGUAGE_VERSION};

    #[test]
    fn language_version_is_two_zero() {
        assert_eq!(LANGUAGE_VERSION, "2.0");
        assert_eq!(language_version(), "2.0");
    }
}
