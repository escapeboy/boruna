use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::error::CompileError;

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

        // Second pass: validate
        for item in &program.items {
            match item {
                Item::Function(f) => self.check_fn(f)?,
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
                    return Err(CompileError::Type(format!("undefined variable: {target}")));
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
        }
        Ok(())
    }

    fn check_expr(&self, expr: &Expr, locals: &HashSet<String>) -> Result<(), CompileError> {
        match expr {
            Expr::Ident(name) => {
                if !locals.contains(name) && !self.functions.contains_key(name) {
                    return Err(CompileError::Type(format!("undefined variable: {name}")));
                }
            }
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
