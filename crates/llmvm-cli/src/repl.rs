//! Interactive REPL for `.ax` modules.
//!
//! Strategy: each REPL input is wrapped in a synthetic `fn __repl_eval() -> T {
//! <expr> }`, appended to the loaded module source, and compiled fresh per
//! input. The `Vm` is re-constructed per input — this is strictly more
//! correct than mutating a persistent `Vm` (which owns its `Module` by value)
//! and avoids any risk of state contamination across inputs.
//!
//! The synthetic wrapper declares its return type as `Int` because Boruna's
//! current typechecker is permissive about return-type unification — the
//! actual `Value` returned by the VM may be any variant, and we report its
//! true variant via `Value::Display` and `Value::type_name()` post-hoc.
//! Documented as a known v1 limitation in `docs/design-boruna-repl.md`;
//! a proper expression-type inference API in the compiler would replace this.
//!
//! No line-editing dep — uses plain `std::io::BufRead`. Sufficient for
//! piped agent-driven use (Boruna's primary REPL audience). History and
//! Ctrl-A line nav can land in a follow-up.
//!
//! Bytecode 1.1 surface is exercised through the REPL: `__builtin_debug(v)`
//! prints to stderr and returns its argument unchanged, so users can sprinkle
//! traces in expressions without restructuring.

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use boruna_bytecode::{Module, Value};
use boruna_compiler::compile;
use boruna_vm::{capability_gateway::Policy, vm::Vm, CapabilityGateway};

/// Return type declared on the synthetic wrapper function. Boruna's
/// typechecker is permissive about return-type unification today, so a single
/// declared type accepts any expression. The true runtime type is recovered
/// via `Value::type_name()`.
pub const SYNTHETIC_RETURN_TYPE: &str = "Int";

/// Name used for the synthetic function that wraps each REPL expression.
/// Reserved — user `.ax` code MUST NOT define a function with this name.
pub const REPL_FN_NAME: &str = "__repl_eval";

/// Outcome of dispatching a meta-command.
#[derive(Debug)]
enum MetaResult {
    /// Continue REPL loop; show `msg` to user.
    Ok(String),
    /// Quit cleanly.
    Quit,
    /// Continue loop; report error to user.
    Err(String),
}

/// Persistent REPL state across input lines.
pub struct ReplSession {
    /// Accumulated source from loaded files (concatenated).
    /// New inputs are appended via the synthetic wrapper at run time.
    module_source: String,
    /// Path of the most-recently-loaded file, for `:reload`.
    loaded_from: Option<PathBuf>,
    /// Capability policy applied to every input.
    policy: Policy,
}

impl ReplSession {
    /// New empty session (no module loaded, supplied policy).
    pub fn new(policy: Policy) -> Self {
        Self {
            module_source: String::new(),
            loaded_from: None,
            policy,
        }
    }

    /// Load (or reload) a `.ax` file. Replaces the current module source.
    pub fn load(&mut self, path: &Path) -> Result<(), String> {
        let src =
            fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        self.module_source = src;
        self.loaded_from = Some(path.to_path_buf());
        Ok(())
    }

    /// Drop loaded source. Used by `:reset`.
    pub fn reset(&mut self) {
        self.module_source.clear();
        self.loaded_from = None;
    }

    /// Currently-loaded file path, if any.
    pub fn loaded_from(&self) -> Option<&Path> {
        self.loaded_from.as_deref()
    }

    /// Compile and run a single expression. Returns the resulting `Value`.
    pub fn run_expr(&self, expr: &str) -> Result<Value, String> {
        let mut module = self.compile_with(expr, SYNTHETIC_RETURN_TYPE)?;
        let entry_idx = module
            .functions
            .iter()
            .position(|f| f.name == REPL_FN_NAME)
            .ok_or_else(|| {
                "internal: __repl_eval function disappeared after compile".to_string()
            })?;
        module.entry = entry_idx as u32;
        let gateway = CapabilityGateway::new(self.policy.clone());
        let mut vm = Vm::new(module, gateway);
        vm.run().map_err(|e| format!("runtime error: {e}"))
    }

    /// `:type` meta — runs the expression and reports the runtime
    /// `Value::type_name()`. Since Boruna's typechecker is permissive about
    /// return-type unification, post-hoc inspection of the produced `Value`
    /// is the only honest source of truth available without a public
    /// expression-type-inference API.
    pub fn type_of(&self, expr: &str) -> Result<&'static str, String> {
        let v = self.run_expr(expr)?;
        Ok(v.type_name())
    }

    /// Names of functions defined in the loaded module. Empty when no
    /// module has been loaded.
    pub fn env(&self) -> Vec<String> {
        let synthetic = format!("{}\n\nfn __probe() -> Int {{ 0 }}\n", self.module_source);
        match compile("__repl_probe__", &synthetic) {
            Ok(module) => module
                .functions
                .iter()
                .map(|f| f.name.clone())
                .filter(|n| n != "__probe" && n != REPL_FN_NAME)
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn compile_with(&self, expr: &str, ret_ty: &str) -> Result<Module, String> {
        let synthetic = format!(
            "{}\n\nfn {REPL_FN_NAME}() -> {ret_ty} {{ {expr} }}\n",
            self.module_source
        );
        compile("__repl__", &synthetic).map_err(|e| format!("{e}"))
    }
}

/// Drive the REPL loop reading from `reader` and writing to `writer`.
///
/// Separated from CLI argument handling so integration tests can drive it
/// with `Cursor`/`Vec` pairs and inspect outputs deterministically.
pub fn run_loop<R: BufRead, W: Write>(
    session: &mut ReplSession,
    reader: &mut R,
    writer: &mut W,
    interactive: bool,
) -> io::Result<()> {
    if interactive {
        write_banner(writer)?;
    }
    let mut line = String::new();
    loop {
        if interactive {
            write!(writer, ">>> ")?;
            writer.flush()?;
        }
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            // EOF
            if interactive {
                writeln!(writer)?;
            }
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(cmd) = trimmed.strip_prefix(':') {
            match dispatch_meta(cmd, session) {
                MetaResult::Ok(msg) => writeln!(writer, "{msg}")?,
                MetaResult::Quit => return Ok(()),
                MetaResult::Err(e) => writeln!(writer, "error: {e}")?,
            }
        } else {
            match session.run_expr(trimmed) {
                Ok(value) => writeln!(writer, "{value}")?,
                Err(e) => writeln!(writer, "error: {e}")?,
            }
        }
    }
}

fn dispatch_meta(cmd: &str, session: &mut ReplSession) -> MetaResult {
    let mut parts = cmd.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("");
    let arg = parts.next().map(str::trim).unwrap_or("");
    match name {
        "quit" | "q" | "exit" => MetaResult::Quit,
        "help" | "h" | "?" => MetaResult::Ok(help_text()),
        "reset" => {
            session.reset();
            MetaResult::Ok("session reset (module unloaded, policy preserved)".into())
        }
        "reload" => match session.loaded_from() {
            None => MetaResult::Err("no file loaded; use `:load <file>` first".into()),
            Some(p) => {
                let path = p.to_path_buf();
                match session.load(&path) {
                    Ok(()) => MetaResult::Ok(format!("reloaded {}", path.display())),
                    Err(e) => MetaResult::Err(e),
                }
            }
        },
        "load" => {
            if arg.is_empty() {
                return MetaResult::Err(":load requires a path".into());
            }
            let path = PathBuf::from(arg);
            match session.load(&path) {
                Ok(()) => MetaResult::Ok(format!("loaded {}", path.display())),
                Err(e) => MetaResult::Err(e),
            }
        }
        "type" | "t" => {
            if arg.is_empty() {
                return MetaResult::Err(":type requires an expression".into());
            }
            match session.type_of(arg) {
                Ok(ty) => MetaResult::Ok(format!("{arg} : {ty}")),
                Err(e) => MetaResult::Err(e),
            }
        }
        "env" => {
            let names = session.env();
            if names.is_empty() {
                MetaResult::Ok(
                    "(no functions in scope; use `:load <file>` to load a module)".into(),
                )
            } else {
                MetaResult::Ok(names.join("\n"))
            }
        }
        other => MetaResult::Err(format!("unknown meta-command `:{other}` (try `:help`)")),
    }
}

fn write_banner<W: Write>(w: &mut W) -> io::Result<()> {
    writeln!(
        w,
        "boruna repl {}  (`:help` for meta-commands, `:quit` to exit)",
        env!("CARGO_PKG_VERSION")
    )
}

fn help_text() -> String {
    [
        "REPL meta-commands:",
        "  :load <file>   Load a .ax file as the current module",
        "  :reload        Reload the most-recently-loaded file",
        "  :reset         Drop the loaded module",
        "  :type <expr>   Show the inferred return-type label for <expr>",
        "  :env           List functions defined in the loaded module",
        "  :help          Show this help",
        "  :quit          Exit the REPL",
        "",
        "Expression input is wrapped in a synthetic `fn __repl_eval() -> Int { <expr> }`",
        "and run with the current capability policy. The Boruna typechecker is",
        "permissive about return-type unification, so the returned value's actual",
        "type is reported post-hoc via `:type` (which runs the expression).",
        "",
        "Side-effect-free debugging: __builtin_debug(v) prints to stderr and returns v.",
    ]
    .join("\n")
}

/// CLI entry point. Reads from stdin, writes to stdout.
pub fn run(initial_file: Option<PathBuf>, policy: Policy) -> Result<(), String> {
    let mut session = ReplSession::new(policy);
    if let Some(path) = initial_file {
        session.load(&path)?;
    }
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    let interactive = atty_stdin();
    run_loop(&mut session, &mut reader, &mut writer, interactive)
        .map_err(|e| format!("io error: {e}"))
}

/// TTY detection using std's `IsTerminal` (stable since Rust 1.70).
fn atty_stdin() -> bool {
    use std::io::IsTerminal;
    io::stdin().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn cursor(input: &str) -> Cursor<Vec<u8>> {
        Cursor::new(input.as_bytes().to_vec())
    }

    fn run_with_input(input: &str, policy: Policy) -> String {
        let mut session = ReplSession::new(policy);
        let mut reader = cursor(input);
        let mut writer = Vec::new();
        run_loop(&mut session, &mut reader, &mut writer, false)
            .expect("run_loop should not error in tests");
        String::from_utf8(writer).expect("UTF-8 output")
    }

    #[test]
    fn simple_int_expression() {
        let out = run_with_input("1 + 2\n:quit\n", Policy::allow_all());
        assert!(out.contains('3'), "expected `3` in output, got: {out:?}");
    }

    #[test]
    fn string_expression_via_candidate_cascade() {
        let out = run_with_input("\"hi\"\n:quit\n", Policy::allow_all());
        assert!(out.contains("\"hi\""), "expected `\"hi\"`, got: {out:?}");
    }

    #[test]
    fn bool_expression() {
        let out = run_with_input("true\n:quit\n", Policy::allow_all());
        assert!(out.contains("true"), "expected `true`, got: {out:?}");
    }

    #[test]
    fn empty_lines_are_ignored() {
        let out = run_with_input("\n\n\n42\n:quit\n", Policy::allow_all());
        let lines: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines, vec!["42"]);
    }

    #[test]
    fn quit_meta_command_exits() {
        let out = run_with_input(":quit\n", Policy::allow_all());
        assert_eq!(out, "");
    }

    #[test]
    fn quit_shorthand_works() {
        let out = run_with_input(":q\n", Policy::allow_all());
        assert_eq!(out, "");
    }

    #[test]
    fn eof_exits_cleanly() {
        let out = run_with_input("99\n", Policy::allow_all());
        assert!(out.contains("99"));
    }

    #[test]
    fn unknown_meta_command_reports_error() {
        let out = run_with_input(":nonsense\n:quit\n", Policy::allow_all());
        assert!(out.contains("unknown meta-command"), "got: {out:?}");
    }

    #[test]
    fn help_meta_command_lists_known_commands() {
        let out = run_with_input(":help\n:quit\n", Policy::allow_all());
        assert!(out.contains(":load"));
        assert!(out.contains(":type"));
        assert!(out.contains(":quit"));
    }

    #[test]
    fn type_meta_returns_int_for_int_expression() {
        let out = run_with_input(":type 1 + 2\n:quit\n", Policy::allow_all());
        assert!(out.contains(": Int"), "got: {out:?}");
    }

    #[test]
    fn type_meta_returns_bool_for_bool_expression() {
        let out = run_with_input(":type true\n:quit\n", Policy::allow_all());
        assert!(out.contains(": Bool"), "got: {out:?}");
    }

    #[test]
    fn type_meta_returns_string_for_string_expression() {
        let out = run_with_input(":type \"hi\"\n:quit\n", Policy::allow_all());
        assert!(out.contains(": String"), "got: {out:?}");
    }

    #[test]
    fn type_meta_returns_list_for_list_expression() {
        // Demonstrates the runtime-inspection approach picks up types
        // beyond the synthetic-wrapper's declared `Int` return.
        let out = run_with_input(":type [1, 2, 3]\n:quit\n", Policy::allow_all());
        assert!(out.contains(": List"), "got: {out:?}");
    }

    #[test]
    fn type_meta_no_arg_reports_error() {
        let out = run_with_input(":type\n:quit\n", Policy::allow_all());
        assert!(out.contains("error:"), "got: {out:?}");
    }

    #[test]
    fn invalid_expression_reports_error_without_crashing() {
        let out = run_with_input("this is not valid ax\n42\n:quit\n", Policy::allow_all());
        assert!(out.contains("error:"), "expected error line, got: {out:?}");
        assert!(out.contains("42"), "REPL must recover and accept new input");
    }

    #[test]
    fn reset_meta_clears_session() {
        let out = run_with_input(":reset\n:quit\n", Policy::allow_all());
        assert!(out.contains("session reset"));
    }

    #[test]
    fn env_meta_with_empty_session_is_explicit() {
        let out = run_with_input(":env\n:quit\n", Policy::allow_all());
        assert!(out.contains("no functions in scope"), "got: {out:?}");
    }

    #[test]
    fn debug_builtin_is_callable_in_repl() {
        // Bytecode 1.1 surface: __builtin_debug(v) returns v.
        let out = run_with_input("__builtin_debug(5)\n:quit\n", Policy::allow_all());
        assert!(
            out.contains('5'),
            "stdout should carry returned 5, got: {out:?}"
        );
    }

    #[test]
    fn deny_all_policy_still_works_for_pure_expressions() {
        let out = run_with_input("7 * 6\n:quit\n", Policy::deny_all());
        assert!(out.contains("42"), "got: {out:?}");
    }

    #[test]
    fn load_with_missing_file_reports_error() {
        let out = run_with_input(":load /nonexistent/path.ax\n:quit\n", Policy::allow_all());
        assert!(out.contains("error:"));
        assert!(out.contains("cannot read"));
    }

    #[test]
    fn load_then_invoke_loaded_function() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("greet.ax");
        std::fs::write(
            &path,
            "fn greeting() -> Int { 42 }\nfn main() -> Int { greeting() }\n",
        )
        .unwrap();
        let cmd = format!(":load {}\ngreeting()\n:quit\n", path.display());
        let out = run_with_input(&cmd, Policy::allow_all());
        assert!(
            out.contains("loaded"),
            "expected `loaded` confirmation, got: {out:?}"
        );
        assert!(
            out.contains("42"),
            "expected greeting() result `42`, got: {out:?}"
        );
    }

    #[test]
    fn reload_without_prior_load_reports_error() {
        let out = run_with_input(":reload\n:quit\n", Policy::allow_all());
        assert!(out.contains("no file loaded"), "got: {out:?}");
    }

    #[test]
    fn loaded_module_env_lists_functions() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("m.ax");
        std::fs::write(
            &path,
            "fn double(n: Int) -> Int { n + n }\nfn main() -> Int { 0 }\n",
        )
        .unwrap();
        let cmd = format!(":load {}\n:env\ndouble(7)\n:quit\n", path.display());
        let out = run_with_input(&cmd, Policy::allow_all());
        assert!(
            out.contains("double"),
            "env should list double, got: {out:?}"
        );
        assert!(out.contains("14"), "double(7) should be 14, got: {out:?}");
    }

    #[test]
    fn banner_is_skipped_in_non_interactive_mode() {
        let out = run_with_input(":quit\n", Policy::allow_all());
        assert!(!out.contains("boruna repl"));
    }
}
