pub mod ast;
pub mod codegen;
pub mod error;
pub mod lexer;
pub mod parser;
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
/// Within a `1.x` line, the language is additive-only — any program that
/// compiles against `1.x` continues to compile against `1.y` for `y >= x`.
pub const LANGUAGE_VERSION: &str = "1.0";

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
    codegen::emit(name, &program)
}

#[cfg(test)]
mod version_tests {
    use super::{language_version, LANGUAGE_VERSION};

    #[test]
    fn language_version_is_one_zero() {
        assert_eq!(LANGUAGE_VERSION, "1.0");
        assert_eq!(language_version(), "1.0");
    }
}
