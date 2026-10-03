use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::error::CompileError;
use crate::suggest;
use boruna_bytecode::Capability;

/// Build a `did you mean: '...'?` suffix from in-scope locals and
/// known function names, or empty string when no unique candidate
/// exists. (post1-T-1.5)
fn ident_suggestion_suffix(
    typo: &str,
    locals: &HashSet<String>,
    functions: &HashMap<String, usize>,
) -> String {
    let candidates = locals
        .iter()
        .map(String::as_str)
        .chain(functions.keys().map(String::as_str));
    match suggest::suggestion_from(typo, candidates) {
        Some(name) => format!("\n  did you mean: '{name}'?"),
        Option::None => String::new(),
    }
}

/// Type checking pass.
/// For MVP, this does basic validation: name resolution, basic type consistency.
pub fn check(program: &Program) -> Result<(), CompileError> {
    let mut checker = TypeChecker::new();
    checker.check_program(program)
}

struct TypeChecker {
    /// Known type names.
    types: HashSet<String>,
    /// Known function names and their arities.
    functions: HashMap<String, usize>,
}

/// Built-ins that perform a side effect through the capability gateway:
/// `(name, capability, arity)`.
///
/// - `net_fetch(url) -> String`: HTTP GET, returns the response body.
/// - `net_request(url, method, body) -> String`: any method; empty `body` sends none.
/// - `llm_call(prompt, model) -> String`: `model` is `"provider/model"` (e.g.
///   `"openai/gpt-4o-mini"`), routed by the host's LLM handler; returns the reply text.
pub const CAPABILITY_BUILTINS: &[(&str, Capability, usize)] = &[
    ("net_fetch", Capability::NetFetch, 1),
    ("net_request", Capability::NetFetch, 3),
    ("llm_call", Capability::LlmCall, 2),
];

/// A function that calls a capability built-in must declare that capability
/// (`!{net.fetch}`, `!{llm.call}`), so the effect is visible in its signature.
fn check_capability_builtins(f: &FnDef, user_fns: &HashSet<&str>) -> Result<(), CompileError> {
    let declared: Vec<Capability> = f
        .capabilities
        .iter()
        .filter_map(|c| Capability::from_name(c))
        .collect();
    let mut used = Vec::new();
    collect_builtin_calls_block(&f.body, &mut used);
    for name in used.into_iter().filter(|n| !user_fns.contains(n)) {
        let (_, cap, _) = CAPABILITY_BUILTINS
            .iter()
            .find(|(n, _, _)| *n == name)
            .expect("collected only known built-ins");
        if !declared.contains(cap) {
            return Err(CompileError::Type(format!(
                "capability not declared: function '{}' calls {name}, which needs {cap}; \
                 declare it: fn {}(...) -> ... !{{{}}}",
                f.name,
                f.name,
                cap.name(),
                cap = cap.name(),
            )));
        }
    }
    Ok(())
}

fn collect_builtin_calls_block<'a>(block: &'a Block, out: &mut Vec<&'a str>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
                collect_builtin_calls_expr(value, out)
            }
            Stmt::Expr(e) | Stmt::Return(Some(e)) => collect_builtin_calls_expr(e, out),
            Stmt::Return(None) => {}
            Stmt::While { condition, body } => {
                collect_builtin_calls_expr(condition, out);
                collect_builtin_calls_block(body, out);
            }
            Stmt::For { iter, body, .. } => {
                collect_builtin_calls_expr(iter, out);
                collect_builtin_calls_block(body, out);
            }
        }
    }
}

fn collect_builtin_calls_expr<'a>(expr: &'a Expr, out: &mut Vec<&'a str>) {
    match expr {
        Expr::Call { func, args } => {
            if let Expr::Ident(name) = func.as_ref() {
                if CAPABILITY_BUILTINS.iter().any(|(n, _, _)| n == name) {
                    out.push(name);
                }
            }
            collect_builtin_calls_expr(func, out);
            for a in args {
                collect_builtin_calls_expr(a, out);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_builtin_calls_expr(left, out);
            collect_builtin_calls_expr(right, out);
        }
        Expr::Unary { expr, .. }
        | Expr::SomeExpr(expr)
        | Expr::OkExpr(expr)
        | Expr::ErrExpr(expr)
        | Expr::Spawn(expr)
        | Expr::Emit(expr) => collect_builtin_calls_expr(expr, out),
        Expr::FieldAccess { object, .. } => collect_builtin_calls_expr(object, out),
        Expr::If {
            condition,
            then_block,
            else_block,
        } => {
            collect_builtin_calls_expr(condition, out);
            collect_builtin_calls_block(then_block, out);
            if let Some(b) = else_block {
                collect_builtin_calls_block(b, out);
            }
        }
        Expr::Match { value, arms } => {
            collect_builtin_calls_expr(value, out);
            for arm in arms {
                collect_builtin_calls_expr(&arm.body, out);
            }
        }
        Expr::Record { fields, spread, .. } => {
            for (_, e) in fields {
                collect_builtin_calls_expr(e, out);
            }
            if let Some(s) = spread {
                collect_builtin_calls_expr(s, out);
            }
        }
        Expr::EnumVariant { payload, .. } => {
            if let Some(p) = payload {
                collect_builtin_calls_expr(p, out);
            }
        }
        Expr::List(items) => {
            for e in items {
                collect_builtin_calls_expr(e, out);
            }
        }
        Expr::Send { target, message } => {
            collect_builtin_calls_expr(target, out);
            collect_builtin_calls_expr(message, out);
        }
        Expr::Block(b) => collect_builtin_calls_block(b, out),
        Expr::IntLit(_)
        | Expr::FloatLit(_)
        | Expr::StringLit(_)
        | Expr::BoolLit(_)
        | Expr::NoneLit
        | Expr::Ident(_)
        | Expr::Receive => {}
    }
}

impl TypeChecker {
    fn new() -> Self {
        let mut types = HashSet::new();
        // Built-in types
        for t in &["Int", "Float", "String", "Bool", "Unit"] {
            types.insert(t.to_string());
        }

        let mut functions = HashMap::new();
        // Built-in functions (compiled to opcodes, not user-defined)
        functions.insert("list_len".to_string(), 1);
        functions.insert("list_get".to_string(), 2);
        functions.insert("list_push".to_string(), 2);
        functions.insert("parse_int".to_string(), 1);
        functions.insert("try_parse_int".to_string(), 1);
        functions.insert("str_contains".to_string(), 2);
        functions.insert("str_starts_with".to_string(), 2);
        functions.insert("__builtin_int_to_string".to_string(), 1);
        functions.insert("__builtin_float_to_string".to_string(), 1);
        functions.insert("__builtin_string_len".to_string(), 1);
        functions.insert("__builtin_string_chars".to_string(), 1);
        functions.insert("__builtin_string_contains".to_string(), 2);
        functions.insert("__builtin_string_starts_with".to_string(), 2);
        functions.insert("__builtin_string_ends_with".to_string(), 2);
        functions.insert("__builtin_string_to_upper".to_string(), 1);
        functions.insert("__builtin_string_to_lower".to_string(), 1);
        functions.insert("__builtin_string_trim".to_string(), 1);
        functions.insert("__builtin_string_join".to_string(), 2);
        functions.insert("__builtin_list_len".to_string(), 1);
        functions.insert("__builtin_list_is_empty".to_string(), 1);
        functions.insert("__builtin_list_head".to_string(), 1);
        functions.insert("__builtin_list_tail".to_string(), 1);
        functions.insert("__builtin_list_append".to_string(), 2);
        functions.insert("__builtin_list_concat".to_string(), 2);
        functions.insert("__builtin_list_reverse".to_string(), 1);
        functions.insert("__builtin_string_split".to_string(), 2);
        functions.insert("__builtin_string_replace".to_string(), 3);
        functions.insert("__builtin_string_slice".to_string(), 3);
        functions.insert("__builtin_int_parse".to_string(), 1);
        functions.insert("__builtin_float_parse".to_string(), 1);
        functions.insert("__builtin_bool_to_string".to_string(), 1);
        functions.insert("__builtin_map_get".to_string(), 2);
        functions.insert("__builtin_map_set".to_string(), 3);
        functions.insert("__builtin_map_remove".to_string(), 2);
        functions.insert("__builtin_map_contains_key".to_string(), 2);
        functions.insert("__builtin_map_keys".to_string(), 1);
        functions.insert("__builtin_map_values".to_string(), 1);
        functions.insert("__builtin_map_len".to_string(), 1);
        // bytecode 1.1 — print-and-passthrough debug helpers.
        // See docs/spec/bytecode-1.0.md §4 (Debug, DebugMsg).
        functions.insert("__builtin_debug".to_string(), 1);
        functions.insert("__builtin_debug_msg".to_string(), 2);
        // guard-and-seal: `__builtin_guard(value, passed, label)` runs a
        // deterministic boolean check on a value, fails closed on false,
        // and seals the verdict into the evidence trail. Returns `value`
        // unchanged on pass. Compiles to `Op::GuardSeal`.
        functions.insert("__builtin_guard".to_string(), 3);
        // 0.3-S14: read a workflow step's resolved input value at
        // runtime. Compiles to `Op::CapCall(StepInput, 1)` which
        // dispatches through the gateway's StepInputHandler. Returns
        // the JSON-encoded upstream output as a String. Steps that
        // need typed access parse the JSON.
        functions.insert("step_input".to_string(), 1);
        // Capability built-ins (language 1.2). Each compiles to `Op::CapCall` and goes through
        // the capability gateway, so the policy decides and the call is recorded. The calling
        // function must declare the capability (see `check_capability_builtins`).
        for (name, _cap, arity) in CAPABILITY_BUILTINS {
            functions.insert(name.to_string(), *arity);
        }

        TypeChecker { types, functions }
    }

    fn check_program(&mut self, program: &Program) -> Result<(), CompileError> {
        // First pass: collect type and function names
        for item in &program.items {
            match item {
                Item::Function(f) => {
                    self.functions.insert(f.name.clone(), f.params.len());
                }
                Item::TypeDef(t) => {
                    self.types.insert(t.name.clone());
                }
                _ => {}
            }
        }

        // A function the program defines (itself or through an imported library, e.g.
        // std-llm's own `llm_call(req, tag) -> Effect`) takes precedence over a built-in
        // of the same name, so adding built-ins never breaks existing programs.
        let user_fns: HashSet<&str> = program
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Function(f) => Some(f.name.as_str()),
                _ => None,
            })
            .collect();

        // Second pass: validate
        for item in &program.items {
            match item {
                Item::Function(f) => {
                    check_capability_builtins(f, &user_fns)?;
                    self.check_fn(f)?
                }
                Item::TypeDef(t) => self.check_type_def(t)?,
                _ => {}
            }
        }

        Ok(())
    }

    fn check_fn(&self, f: &FnDef) -> Result<(), CompileError> {
        let mut locals: HashSet<String> = HashSet::new();
        for p in &f.params {
            locals.insert(p.name.clone());
        }
        self.check_block(&f.body, &mut locals)
    }

    fn check_type_def(&self, _t: &TypeDef) -> Result<(), CompileError> {
        // MVP: no deep type checking on type definitions
        Ok(())
    }

    fn check_block(&self, block: &Block, locals: &mut HashSet<String>) -> Result<(), CompileError> {
        for stmt in &block.stmts {
            self.check_stmt(stmt, locals)?;
        }
        Ok(())
    }

    fn check_stmt(&self, stmt: &Stmt, locals: &mut HashSet<String>) -> Result<(), CompileError> {
        match stmt {
            Stmt::Let { name, value, .. } => {
                self.check_expr(value, locals)?;
                locals.insert(name.clone());
            }
            Stmt::Assign { target, value } => {
                if !locals.contains(target) && !self.functions.contains_key(target) {
                    let suffix = ident_suggestion_suffix(target, locals, &self.functions);
                    return Err(CompileError::Type(format!(
                        "undefined variable: {target}{suffix}"
                    )));
                }
                self.check_expr(value, locals)?;
            }
            Stmt::Expr(e) => self.check_expr(e, locals)?,
            Stmt::Return(Some(e)) => self.check_expr(e, locals)?,
            Stmt::Return(None) => {}
            Stmt::While { condition, body } => {
                self.check_expr(condition, locals)?;
                let mut inner = locals.clone();
                self.check_block(body, &mut inner)?;
            }
            Stmt::For { var, iter, body } => {
                self.check_expr(iter, locals)?;
                let mut inner = locals.clone();
                inner.insert(var.clone());
                self.check_block(body, &mut inner)?;
            }
        }
        Ok(())
    }

    fn check_expr(&self, expr: &Expr, locals: &HashSet<String>) -> Result<(), CompileError> {
        match expr {
            Expr::Ident(name) if !locals.contains(name) && !self.functions.contains_key(name) => {
                let suffix = ident_suggestion_suffix(name, locals, &self.functions);
                return Err(CompileError::Type(format!(
                    "undefined variable: {name}{suffix}"
                )));
            }
            Expr::Ident(_) => {}
            Expr::Binary { left, right, .. } => {
                self.check_expr(left, locals)?;
                self.check_expr(right, locals)?;
            }
            Expr::Unary { expr, .. } => self.check_expr(expr, locals)?,
            Expr::Call { func, args } => {
                self.check_expr(func, locals)?;
                for arg in args {
                    self.check_expr(arg, locals)?;
                }
                // Arity check for direct calls to a named function. Skipped when
                // the callee name is a local binding (a first-class function
                // value / higher-order parameter), whose arity isn't known here.
                if let Expr::Ident(name) = func.as_ref() {
                    if !locals.contains(name) {
                        if let Some(&arity) = self.functions.get(name) {
                            if args.len() != arity {
                                return Err(CompileError::Type(format!(
                                    "function '{name}' expects {arity} argument{}, got {}",
                                    if arity == 1 { "" } else { "s" },
                                    args.len()
                                )));
                            }
                        }
                    }
                }
            }
            Expr::FieldAccess { object, .. } => self.check_expr(object, locals)?,
            Expr::If {
                condition,
                then_block,
                else_block,
            } => {
                self.check_expr(condition, locals)?;
                let mut inner = locals.clone();
                self.check_block(then_block, &mut inner)?;
                if let Some(eb) = else_block {
                    let mut inner = locals.clone();
                    self.check_block(eb, &mut inner)?;
                }
            }
            Expr::Match { value, arms } => {
                self.check_expr(value, locals)?;
                for arm in arms {
                    let mut inner = locals.clone();
                    self.collect_pattern_bindings(&arm.pattern, &mut inner);
                    self.check_expr(&arm.body, &inner)?;
                }
            }
            Expr::Record { fields, spread, .. } => {
                if let Some(base) = spread {
                    self.check_expr(base, locals)?;
                }
                for (_, val) in fields {
                    self.check_expr(val, locals)?;
                }
            }
            Expr::List(items) => {
                for item in items {
                    self.check_expr(item, locals)?;
                }
            }
            Expr::SomeExpr(e)
            | Expr::OkExpr(e)
            | Expr::ErrExpr(e)
            | Expr::Spawn(e)
            | Expr::Emit(e) => {
                self.check_expr(e, locals)?;
            }
            Expr::Send { target, message } => {
                self.check_expr(target, locals)?;
                self.check_expr(message, locals)?;
            }
            Expr::Block(b) => {
                let mut inner = locals.clone();
                self.check_block(b, &mut inner)?;
            }
            Expr::EnumVariant {
                payload: Some(p), ..
            } => self.check_expr(p, locals)?,
            _ => {}
        }
        Ok(())
    }

    #[allow(clippy::only_used_in_recursion)]
    fn collect_pattern_bindings(&self, pattern: &Pattern, locals: &mut HashSet<String>) {
        match pattern {
            Pattern::Ident(name) => {
                locals.insert(name.clone());
            }
            Pattern::SomePat(inner)
            | Pattern::OkPat(inner)
            | Pattern::ErrPat(inner)
            | Pattern::EnumVariant(_, Some(inner)) => {
                self.collect_pattern_bindings(inner, locals);
            }
            _ => {}
        }
    }
}
