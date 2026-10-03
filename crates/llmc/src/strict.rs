//! Language 2.0 checks that reject a program: type mismatches the checker can name with
//! confidence (E009) and reassignment of a binding that is not `mut` (E010).
//!
//! Both were warnings in language 1.x. The type part is deliberately narrow: it only reasons
//! about types it can name (literals, annotated bindings, record and enum constructors, user
//! function signatures) and reports a mismatch only when both sides resolve to different
//! concrete named types. Generics, built-ins and operators stay untyped, so a correct program
//! is not rejected for something the checker cannot see.
//!
//! `boruna_compiler::compile` rejects the first issue. `boruna lang check` reports every issue
//! with a location and, for E010 on a `let`, an automatic fix.

use std::collections::{HashMap, HashSet};

use crate::ast::*;

/// How the reassigned name was bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    Let,
    Mut,
    Param,
    ForVar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Issue {
    /// `let name: declared = <value of type actual>`
    LetAnnotation {
        func: String,
        name: String,
        declared: String,
        actual: String,
    },
    /// `callee(.., <arg `index` (1-based) of type actual>, ..)` where the parameter is `expected`.
    CallArgument {
        func: String,
        callee: String,
        /// Which call to `callee` in `func` this is, counting from 1 in source order.
        occurrence: usize,
        index: usize,
        expected: String,
        actual: String,
    },
    /// `name = <value of type actual>` where `name` has type `declared`.
    Assign {
        func: String,
        name: String,
        /// Which assignment to `name` in `func` this is, counting from 1 in source order.
        occurrence: usize,
        declared: String,
        actual: String,
    },
    /// `while <condition of type actual>`, which is not `Bool`.
    WhileCondition { func: String, actual: String },
    /// `name = ...` where `name` was bound by `binding` (never `Mut`).
    AssignImmutable {
        func: String,
        name: String,
        binding: Binding,
    },
}

impl Issue {
    /// Diagnostic code: `E009` for type mismatches, `E010` for reassigning a non-`mut` binding.
    pub fn code(&self) -> &'static str {
        match self {
            Issue::AssignImmutable { .. } => "E010",
            _ => "E009",
        }
    }

    pub fn func(&self) -> &str {
        match self {
            Issue::LetAnnotation { func, .. }
            | Issue::CallArgument { func, .. }
            | Issue::Assign { func, .. }
            | Issue::WhileCondition { func, .. }
            | Issue::AssignImmutable { func, .. } => func,
        }
    }

    /// The message without the function name.
    pub fn message(&self) -> String {
        match self {
            Issue::LetAnnotation {
                name,
                declared,
                actual,
                ..
            } => format!(
                "type mismatch: '{name}' is annotated '{declared}' but its initializer has type \
                 '{actual}'"
            ),
            Issue::CallArgument {
                callee,
                index,
                expected,
                actual,
                ..
            } => format!(
                "type mismatch: call to '{callee}' argument {index} expects '{expected}' but got \
                 '{actual}'"
            ),
            Issue::Assign {
                name,
                declared,
                actual,
                ..
            } => format!(
                "type mismatch: '{name}' has type '{declared}' but is assigned a value of type \
                 '{actual}'"
            ),
            Issue::WhileCondition { actual, .. } => {
                format!("type mismatch: a while condition must be Bool, got '{actual}'")
            }
            Issue::AssignImmutable { name, binding, .. } => match binding {
                Binding::Param => format!(
                    "cannot reassign parameter '{name}'; copy it into a new binding first, e.g. \
                     `let mut {name}_acc = {name}`"
                ),
                Binding::ForVar => format!(
                    "cannot reassign loop variable '{name}'; copy it into a new binding first, \
                     e.g. `let mut {name}_acc = {name}`"
                ),
                _ => format!(
                    "cannot reassign '{name}': it was declared without `mut`; declare it with \
                     `let mut {name}`"
                ),
            },
        }
    }
}

/// Every issue in the program, function by function, in source order. E010 is reported once
/// per variable per function.
pub fn check(program: &Program) -> Vec<Issue> {
    let mut sigs: FnSigs = HashMap::new();
    for item in &program.items {
        if let Item::Function(f) = item {
            let params = f.params.iter().map(|p| named_type(&p.ty)).collect();
            let ret = f.return_type.as_ref().and_then(named_type);
            sigs.insert(f.name.as_str(), (params, ret));
        }
    }
    let mut issues = Vec::new();
    for item in &program.items {
        if let Item::Function(f) = item {
            let mut types = TypePass {
                func: &f.name,
                sigs: &sigs,
                issues: &mut issues,
                calls: HashMap::new(),
                assigns: HashMap::new(),
            };
            let mut env: Env = HashMap::new();
            for p in &f.params {
                env.insert(p.name.clone(), local_of(Some(&p.ty)));
            }
            types.block(&f.body, &mut env);

            let mut scope: HashMap<String, Binding> = HashMap::new();
            for p in &f.params {
                scope.insert(p.name.clone(), Binding::Param);
            }
            let mut hits = Vec::new();
            mut_block(&f.body, &mut scope, &mut hits);
            let mut reported = HashSet::new();
            for (name, binding) in hits {
                if reported.insert(name.clone()) {
                    issues.push(Issue::AssignImmutable {
                        func: f.name.clone(),
                        name,
                        binding,
                    });
                }
            }
        }
    }
    issues
}

/// A user function's signature: per-parameter concrete type (None when the parameter type is
/// not a plain named type) and the concrete return type.
/// What the pass knows about a local name in scope.
#[derive(Clone)]
enum Local {
    /// A concrete named type (`Int`, a record, an enum).
    Typed(String),
    /// Declared with a `Fn` type: a call `name(..)` calls this value, not a top-level function
    /// of the same name (the code generator does the same).
    Function,
    /// Any other local; a call `name(..)` still goes to the top-level function.
    Unknown,
}

impl Local {
    fn ty(&self) -> Option<String> {
        match self {
            Local::Typed(t) => Some(t.clone()),
            _ => None,
        }
    }
}

fn local_of(ty: Option<&TypeExpr>) -> Local {
    match ty {
        Some(TypeExpr::Named(n)) => Local::Typed(n.clone()),
        Some(TypeExpr::Fn(..)) => Local::Function,
        _ => Local::Unknown,
    }
}

type Env = HashMap<String, Local>;

type FnSigs<'a> = HashMap<&'a str, (Vec<Option<String>>, Option<String>)>;

/// The concrete name of a type expression, or `None` for generic constructors
/// (`Option`/`Result`/`List`/`Map`/`Fn`), which this pass treats as untyped.
fn named_type(ty: &TypeExpr) -> Option<String> {
    match ty {
        TypeExpr::Named(n) => Some(n.clone()),
        _ => None,
    }
}

struct TypePass<'a, 'b> {
    func: &'a str,
    sigs: &'a FnSigs<'b>,
    issues: &'a mut Vec<Issue>,
    /// Calls per callee and assignments per name seen so far, to tell repeated ones apart.
    calls: HashMap<String, usize>,
    assigns: HashMap<String, usize>,
}

fn bump(counts: &mut HashMap<String, usize>, key: &str) -> usize {
    let n = counts.entry(key.to_string()).or_insert(0);
    *n += 1;
    *n
}

impl TypePass<'_, '_> {
    fn block(&mut self, block: &Block, env: &mut Env) {
        for stmt in &block.stmts {
            self.stmt(stmt, env);
        }
    }

    fn stmt(&mut self, stmt: &Stmt, env: &mut Env) {
        match stmt {
            Stmt::Let {
                name, ty, value, ..
            } => {
                self.expr(value, env);
                let inferred = self.infer(value, env);
                if let Some(TypeExpr::Named(declared)) = ty {
                    if let Some(actual) = inferred {
                        if &actual != declared {
                            self.issues.push(Issue::LetAnnotation {
                                func: self.func.to_string(),
                                name: name.clone(),
                                declared: declared.clone(),
                                actual,
                            });
                        }
                    }
                    env.insert(name.clone(), Local::Typed(declared.clone()));
                } else {
                    // Unknown type is recorded too, so a shadowed name loses its old type.
                    let local = match (ty, inferred) {
                        (Some(t), _) => local_of(Some(t)),
                        (None, Some(t)) => Local::Typed(t),
                        (None, None) => Local::Unknown,
                    };
                    env.insert(name.clone(), local);
                }
            }
            Stmt::Assign { target, value } => {
                self.expr(value, env);
                let occurrence = bump(&mut self.assigns, target);
                if let (Some(declared), Some(actual)) =
                    (env.get(target).and_then(Local::ty), self.infer(value, env))
                {
                    if actual != declared {
                        self.issues.push(Issue::Assign {
                            func: self.func.to_string(),
                            name: target.clone(),
                            occurrence,
                            declared,
                            actual,
                        });
                    }
                }
            }
            Stmt::Expr(e) | Stmt::Return(Some(e)) => self.expr(e, env),
            Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
            Stmt::While { condition, body } => {
                self.expr(condition, env);
                if let Some(actual) = self.infer(condition, env) {
                    if actual != "Bool" {
                        self.issues.push(Issue::WhileCondition {
                            func: self.func.to_string(),
                            actual,
                        });
                    }
                }
                self.block(body, &mut env.clone());
            }
            Stmt::For { var, iter, body } => {
                self.expr(iter, env);
                let mut inner = env.clone();
                inner.insert(var.clone(), Local::Unknown);
                self.block(body, &mut inner);
            }
        }
    }

    fn expr(&mut self, expr: &Expr, env: &Env) {
        // Direct call to a named user function: check each argument's concrete type against the
        // declared parameter type. Skip when the callee name is a local binding (a function
        // value passed as a parameter).
        if let Expr::Call { func, args } = expr {
            if let Expr::Ident(fname) = func.as_ref() {
                let occurrence = bump(&mut self.calls, fname);
                if !matches!(env.get(fname), Some(Local::Function)) {
                    if let Some((param_types, _)) = self.sigs.get(fname.as_str()) {
                        for (i, arg) in args.iter().enumerate() {
                            if let (Some(Some(pt)), Some(at)) =
                                (param_types.get(i), self.infer(arg, env))
                            {
                                if &at != pt {
                                    self.issues.push(Issue::CallArgument {
                                        func: self.func.to_string(),
                                        callee: fname.clone(),
                                        occurrence,
                                        index: i + 1,
                                        expected: pt.clone(),
                                        actual: at,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        match expr {
            Expr::Binary { left, right, .. } => {
                self.expr(left, env);
                self.expr(right, env);
            }
            Expr::Unary { expr, .. } => self.expr(expr, env),
            Expr::Call { func, args } => {
                self.expr(func, env);
                for a in args {
                    self.expr(a, env);
                }
            }
            Expr::FieldAccess { object, .. } => self.expr(object, env),
            Expr::If {
                condition,
                then_block,
                else_block,
            } => {
                self.expr(condition, env);
                self.block(then_block, &mut env.clone());
                if let Some(eb) = else_block {
                    self.block(eb, &mut env.clone());
                }
            }
            Expr::Match { value, arms } => {
                self.expr(value, env);
                for arm in arms {
                    // A name bound by the pattern shadows the outer one with an unknown type.
                    let mut inner = env.clone();
                    for n in pattern_names(&arm.pattern) {
                        inner.insert(n, Local::Unknown);
                    }
                    self.expr(&arm.body, &inner);
                }
            }
            Expr::Record { fields, spread, .. } => {
                for (_, v) in fields {
                    self.expr(v, env);
                }
                if let Some(s) = spread {
                    self.expr(s, env);
                }
            }
            Expr::List(items) => {
                for it in items {
                    self.expr(it, env);
                }
            }
            Expr::SomeExpr(e)
            | Expr::OkExpr(e)
            | Expr::ErrExpr(e)
            | Expr::Spawn(e)
            | Expr::Emit(e) => self.expr(e, env),
            Expr::EnumVariant {
                payload: Some(p), ..
            } => self.expr(p, env),
            Expr::Send { target, message } => {
                self.expr(target, env);
                self.expr(message, env);
            }
            Expr::Block(b) => self.block(b, &mut env.clone()),
            _ => {}
        }
    }

    /// Best-effort concrete type of an expression, or `None` when it cannot be named with
    /// confidence.
    fn infer(&self, expr: &Expr, env: &Env) -> Option<String> {
        match expr {
            Expr::IntLit(_) => Some("Int".to_string()),
            Expr::FloatLit(_) => Some("Float".to_string()),
            Expr::StringLit(_) => Some("String".to_string()),
            Expr::BoolLit(_) => Some("Bool".to_string()),
            Expr::Ident(name) => env.get(name).and_then(Local::ty),
            Expr::Record { type_name, .. } => Some(type_name.clone()),
            Expr::EnumVariant { enum_name, .. } => Some(enum_name.clone()),
            Expr::Call { func, .. } => match func.as_ref() {
                Expr::Ident(fname) if !matches!(env.get(fname), Some(Local::Function)) => self
                    .sigs
                    .get(fname.as_str())
                    .and_then(|(_, ret)| ret.clone()),
                _ => None,
            },
            _ => None,
        }
    }
}

fn mut_block(
    block: &Block,
    scope: &mut HashMap<String, Binding>,
    hits: &mut Vec<(String, Binding)>,
) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let {
                name,
                mutable,
                value,
                ..
            } => {
                mut_expr(value, scope, hits);
                let kind = if *mutable { Binding::Mut } else { Binding::Let };
                scope.insert(name.clone(), kind);
            }
            Stmt::Assign { target, value } => {
                mut_expr(value, scope, hits);
                if let Some(&kind) = scope.get(target) {
                    if kind != Binding::Mut {
                        hits.push((target.clone(), kind));
                    }
                }
            }
            Stmt::Expr(e) | Stmt::Return(Some(e)) => mut_expr(e, scope, hits),
            Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
            Stmt::While { condition, body } => {
                mut_expr(condition, scope, hits);
                mut_block(body, &mut scope.clone(), hits);
            }
            Stmt::For { var, iter, body } => {
                mut_expr(iter, scope, hits);
                let mut inner = scope.clone();
                inner.insert(var.clone(), Binding::ForVar);
                mut_block(body, &mut inner, hits);
            }
        }
    }
}

fn mut_expr(expr: &Expr, scope: &HashMap<String, Binding>, hits: &mut Vec<(String, Binding)>) {
    match expr {
        Expr::If {
            condition,
            then_block,
            else_block,
        } => {
            mut_expr(condition, scope, hits);
            mut_block(then_block, &mut scope.clone(), hits);
            if let Some(b) = else_block {
                mut_block(b, &mut scope.clone(), hits);
            }
        }
        Expr::Match { value, arms } => {
            mut_expr(value, scope, hits);
            for arm in arms {
                // A name bound by the pattern shadows the outer binding; leave it out rather
                // than guess how an assignment to it behaves.
                let mut inner = scope.clone();
                for n in pattern_names(&arm.pattern) {
                    inner.remove(&n);
                }
                mut_expr(&arm.body, &inner, hits);
            }
        }
        Expr::Block(b) => mut_block(b, &mut scope.clone(), hits),
        Expr::Binary { left, right, .. } => {
            mut_expr(left, scope, hits);
            mut_expr(right, scope, hits);
        }
        Expr::Unary { expr, .. }
        | Expr::SomeExpr(expr)
        | Expr::OkExpr(expr)
        | Expr::ErrExpr(expr)
        | Expr::Spawn(expr)
        | Expr::Emit(expr) => mut_expr(expr, scope, hits),
        Expr::Call { func, args } => {
            mut_expr(func, scope, hits);
            for a in args {
                mut_expr(a, scope, hits);
            }
        }
        Expr::FieldAccess { object, .. } => mut_expr(object, scope, hits),
        Expr::Record { fields, spread, .. } => {
            for (_, e) in fields {
                mut_expr(e, scope, hits);
            }
            if let Some(s) = spread {
                mut_expr(s, scope, hits);
            }
        }
        Expr::EnumVariant { payload, .. } => {
            if let Some(p) = payload {
                mut_expr(p, scope, hits);
            }
        }
        Expr::List(items) => {
            for e in items {
                mut_expr(e, scope, hits);
            }
        }
        Expr::Send { target, message } => {
            mut_expr(target, scope, hits);
            mut_expr(message, scope, hits);
        }
        Expr::IntLit(_)
        | Expr::FloatLit(_)
        | Expr::StringLit(_)
        | Expr::BoolLit(_)
        | Expr::NoneLit
        | Expr::Ident(_)
        | Expr::Receive => {}
    }
}

pub fn pattern_names(p: &Pattern) -> Vec<String> {
    match p {
        Pattern::Ident(n) => vec![n.clone()],
        Pattern::SomePat(inner) | Pattern::OkPat(inner) | Pattern::ErrPat(inner) => {
            pattern_names(inner)
        }
        Pattern::EnumVariant(_, Some(inner)) => pattern_names(inner),
        _ => Vec::new(),
    }
}
