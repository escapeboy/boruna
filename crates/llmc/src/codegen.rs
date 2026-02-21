use std::collections::HashMap;

use boruna_bytecode::module::{Function, MatchArm as BcMatchArm, Module, TypeDef as BcTypeDef, TypeKind as BcTypeKind};
use boruna_bytecode::opcode::Op;
use boruna_bytecode::value::Value;
use boruna_bytecode::capability::Capability;

use crate::ast::*;
use crate::error::CompileError;

pub fn emit(name: &str, program: &Program) -> Result<Module, CompileError> {
    let mut emitter = Emitter::new(name);
    emitter.emit_program(program)?;
    Ok(emitter.module)
}

struct Emitter {
    module: Module,
    /// Map from function name to function index.
    fn_map: HashMap<String, u32>,
    /// Map from type name to type index.
    type_map: HashMap<String, u32>,
}

struct FnEmitter {
    code: Vec<Op>,
    match_tables: Vec<Vec<BcMatchArm>>,
    locals: HashMap<String, u32>,
    next_local: u32,
    capabilities: Vec<Capability>,
}

impl Emitter {
    fn new(name: &str) -> Self {
        Emitter {
            module: Module::new(name),
            fn_map: HashMap::new(),
            type_map: HashMap::new(),
        }
    }

    fn emit_program(&mut self, program: &Program) -> Result<(), CompileError> {
        // First pass: register types and functions
        for item in &program.items {
            match item {
                Item::TypeDef(t) => {
                    let idx = self.module.types.len() as u32;
                    let kind = match &t.kind {
                        TypeDefKind::Record(fields) => BcTypeKind::Record {
                            fields: fields.iter()
                                .map(|(n, ty)| (n.clone(), type_expr_to_string(ty)))
                                .collect(),
                        },
                        TypeDefKind::Enum(variants) => BcTypeKind::Enum {
                            variants: variants.iter()
                                .map(|(n, ty)| (n.clone(), ty.as_ref().map(type_expr_to_string)))
                                .collect(),
                        },
                    };
                    self.module.types.push(BcTypeDef { name: t.name.clone(), kind });
                    self.type_map.insert(t.name.clone(), idx);
                }
                Item::Function(f) => {
                    let idx = self.fn_map.len() as u32;
                    self.fn_map.insert(f.name.clone(), idx);
                }
                _ => {}
            }
        }

        // Second pass: emit functions
        for item in &program.items {
            if let Item::Function(f) = item {
                self.emit_function(f)?;
            }
        }

        // Set entry to "main" if it exists
        if let Some(&idx) = self.fn_map.get("main") {
            self.module.entry = idx;
        }

        Ok(())
    }

    fn emit_function(&mut self, f: &FnDef) -> Result<(), CompileError> {
        let mut fe = FnEmitter {
            code: Vec::new(),
            match_tables: Vec::new(),
            locals: HashMap::new(),
            next_local: 0,
            capabilities: Vec::new(),
        };

        // Resolve capabilities
        for cap_name in &f.capabilities {
            if let Some(cap) = Capability::from_name(cap_name) {
                fe.capabilities.push(cap);
            }
        }

        // Register params as locals
        for p in &f.params {
            let idx = fe.next_local;
            fe.locals.insert(p.name.clone(), idx);
            fe.next_local += 1;
        }

        // Emit body
        self.emit_block(&f.body, &mut fe)?;

        // Implicit return
        if fe.code.last() != Some(&Op::Ret) {
            fe.code.push(Op::Ret);
        }

        let func = Function {
            name: f.name.clone(),
            arity: f.params.len() as u8,
            locals: fe.next_local as u16,
            code: fe.code,
            capabilities: fe.capabilities,
            match_tables: fe.match_tables,
        };
        self.module.add_function(func);
        Ok(())
    }

    fn emit_block(&mut self, block: &Block, fe: &mut FnEmitter) -> Result<(), CompileError> {
        for (i, stmt) in block.stmts.iter().enumerate() {
            self.emit_stmt(stmt, fe)?;
            // Pop unused expression results (except the last statement)
            if i < block.stmts.len() - 1 {
                if let Stmt::Expr(_) = stmt {
                    fe.code.push(Op::Pop);
                }
            }
        }
        Ok(())
    }

    fn emit_stmt(&mut self, stmt: &Stmt, fe: &mut FnEmitter) -> Result<(), CompileError> {
        match stmt {
            Stmt::Let { name, value, .. } => {
                self.emit_expr(value, fe)?;
                let idx = fe.next_local;
                fe.locals.insert(name.clone(), idx);
                fe.next_local += 1;
                fe.code.push(Op::StoreLocal(idx));
            }
            Stmt::Assign { target, value } => {
                self.emit_expr(value, fe)?;
                if let Some(&idx) = fe.locals.get(target) {
                    fe.code.push(Op::StoreLocal(idx));
                } else {
                    return Err(CompileError::Codegen(format!("undefined variable: {target}")));
                }
            }
            Stmt::Expr(e) => {
                self.emit_expr(e, fe)?;
            }
            Stmt::Return(Some(e)) => {
                self.emit_expr(e, fe)?;
                fe.code.push(Op::Ret);
            }
            Stmt::Return(None) => {
                let idx = self.module.add_const(Value::Unit);
                fe.code.push(Op::PushConst(idx));
                fe.code.push(Op::Ret);
            }
            Stmt::While { condition, body } => {
                let loop_start = fe.code.len() as u32;
                self.emit_expr(condition, fe)?;
                let exit_jmp = fe.code.len();
                fe.code.push(Op::JmpIfNot(0)); // placeholder
                self.emit_block(body, fe)?;
                fe.code.push(Op::Jmp(loop_start));
                let exit_target = fe.code.len() as u32;
                fe.code[exit_jmp] = Op::JmpIfNot(exit_target);
            }
        }
        Ok(())
    }

    fn emit_expr(&mut self, expr: &Expr, fe: &mut FnEmitter) -> Result<(), CompileError> {
        match expr {
            Expr::IntLit(n) => {
                let idx = self.module.add_const(Value::Int(*n));
                fe.code.push(Op::PushConst(idx));
            }
            Expr::FloatLit(n) => {
                let idx = self.module.add_const(Value::Float(*n));
                fe.code.push(Op::PushConst(idx));
            }
            Expr::StringLit(s) => {
                let idx = self.module.add_const(Value::String(s.clone()));
                fe.code.push(Op::PushConst(idx));
            }
            Expr::BoolLit(b) => {
                let idx = self.module.add_const(Value::Bool(*b));
                fe.code.push(Op::PushConst(idx));
            }
            Expr::NoneLit => {
                let idx = self.module.add_const(Value::None);
                fe.code.push(Op::PushConst(idx));
            }
            Expr::Ident(name) => {
                if let Some(&idx) = fe.locals.get(name) {
                    fe.code.push(Op::LoadLocal(idx));
                } else if let Some(&func_idx) = self.fn_map.get(name) {
                    let idx = self.module.add_const(Value::FnRef(func_idx));
                    fe.code.push(Op::PushConst(idx));
                } else {
                    return Err(CompileError::Codegen(format!("undefined: {name}")));
                }
            }
            Expr::Binary { op, left, right } => {
                self.emit_expr(left, fe)?;
                self.emit_expr(right, fe)?;
                fe.code.push(match op {
                    BinOp::Add => Op::Add,
                    BinOp::Sub => Op::Sub,
                    BinOp::Mul => Op::Mul,
                    BinOp::Div => Op::Div,
                    BinOp::Mod => Op::Mod,
                    BinOp::Eq => Op::Eq,
                    BinOp::Neq => Op::Neq,
                    BinOp::Lt => Op::Lt,
                    BinOp::Lte => Op::Lte,
                    BinOp::Gt => Op::Gt,
                    BinOp::Gte => Op::Gte,
                    BinOp::And => Op::And,
                    BinOp::Or => Op::Or,
                    BinOp::Concat => Op::Concat,
                });
            }
            Expr::Unary { op, expr } => {
                self.emit_expr(expr, fe)?;
                fe.code.push(match op {
                    UnaryOp::Neg => Op::Neg,
                    UnaryOp::Not => Op::Not,
                });
            }
            Expr::Call { func, args } => {
                // If func is a direct function name, check builtins first
                if let Expr::Ident(name) = func.as_ref() {
                    // Builtin functions → emit opcodes directly
                    match name.as_str() {
                        "list_len" if args.len() == 1 => {
                            self.emit_expr(&args[0], fe)?;
                            fe.code.push(Op::ListLen);
                            return Ok(());
                        }
                        "list_get" if args.len() == 2 => {
                            self.emit_expr(&args[0], fe)?;
                            self.emit_expr(&args[1], fe)?;
                            fe.code.push(Op::ListGet);
                            return Ok(());
                        }
                        "list_push" if args.len() == 2 => {
                            self.emit_expr(&args[0], fe)?;
                            self.emit_expr(&args[1], fe)?;
                            fe.code.push(Op::ListPush);
                            return Ok(());
                        }
                        "parse_int" if args.len() == 1 => {
                            self.emit_expr(&args[0], fe)?;
                            fe.code.push(Op::ParseInt);
                            return Ok(());
                        }
                        "try_parse_int" if args.len() == 1 => {
                            self.emit_expr(&args[0], fe)?;
                            fe.code.push(Op::TryParseInt);
                            return Ok(());
                        }
                        "str_contains" if args.len() == 2 => {
                            self.emit_expr(&args[0], fe)?;
                            self.emit_expr(&args[1], fe)?;
                            fe.code.push(Op::StrContains);
                            return Ok(());
                        }
                        "str_starts_with" if args.len() == 2 => {
                            self.emit_expr(&args[0], fe)?;
                            self.emit_expr(&args[1], fe)?;
                            fe.code.push(Op::StrStartsWith);
                            return Ok(());
                        }
                        _ => {}
                    }
                    // User-defined function call
                    if let Some(&func_idx) = self.fn_map.get(name) {
                        for arg in args {
                            self.emit_expr(arg, fe)?;
                        }
                        fe.code.push(Op::Call(func_idx, args.len() as u8));
                        return Ok(());
                    }
                }
                // Otherwise: push args, push func, call indirect (not supported yet; fallback)
                for arg in args {
                    self.emit_expr(arg, fe)?;
                }
                self.emit_expr(func, fe)?;
                // For now, just emit a direct call (requires func to be a function reference)
                fe.code.push(Op::Call(0, args.len() as u8));
            }
            Expr::FieldAccess { object, field } => {
                self.emit_expr(object, fe)?;
                // Resolve field index from type info (simplified: use positional)
                // For MVP, parse field as index
                let field_idx = self.resolve_field(field);
                fe.code.push(Op::GetField(field_idx));
            }
            Expr::If { condition, then_block, else_block } => {
                self.emit_expr(condition, fe)?;
                let else_jmp = fe.code.len();
                fe.code.push(Op::JmpIfNot(0)); // placeholder

                self.emit_block(then_block, fe)?;

                if let Some(eb) = else_block {
                    let end_jmp = fe.code.len();
                    fe.code.push(Op::Jmp(0)); // placeholder
                    let else_target = fe.code.len() as u32;
                    fe.code[else_jmp] = Op::JmpIfNot(else_target);
                    self.emit_block(eb, fe)?;
                    let end_target = fe.code.len() as u32;
                    fe.code[end_jmp] = Op::Jmp(end_target);
                } else {
                    let else_target = fe.code.len() as u32;
                    fe.code[else_jmp] = Op::JmpIfNot(else_target);
                }
            }
            Expr::Match { value, arms } => {
                let has_string_patterns = arms.iter().any(|a| matches!(&a.pattern, Pattern::StringLit(_)));

                if has_string_patterns {
                    // String match: compile as if-else chain with Eq comparisons.
                    // Store scrutinee in a temp local.
                    self.emit_expr(value, fe)?;
                    let scrutinee_local = fe.next_local;
                    fe.next_local += 1;
                    fe.code.push(Op::StoreLocal(scrutinee_local));

                    let mut end_jmps = Vec::new();

                    for (i, arm) in arms.iter().enumerate() {
                        let is_last = i == arms.len() - 1;
                        match &arm.pattern {
                            Pattern::StringLit(s) => {
                                // LoadLocal(scrutinee) → PushConst(s) → Eq → JmpIfNot(next)
                                fe.code.push(Op::LoadLocal(scrutinee_local));
                                let const_idx = self.module.add_const(Value::String(s.clone()));
                                fe.code.push(Op::PushConst(const_idx));
                                fe.code.push(Op::Eq);
                                let skip_jmp = fe.code.len();
                                fe.code.push(Op::JmpIfNot(0)); // placeholder

                                self.emit_expr(&arm.body, fe)?;

                                if !is_last {
                                    let end_jmp = fe.code.len();
                                    fe.code.push(Op::Jmp(0)); // placeholder
                                    end_jmps.push(end_jmp);
                                }

                                let next = fe.code.len() as u32;
                                fe.code[skip_jmp] = Op::JmpIfNot(next);
                            }
                            Pattern::IntLit(n) => {
                                fe.code.push(Op::LoadLocal(scrutinee_local));
                                let const_idx = self.module.add_const(Value::Int(*n));
                                fe.code.push(Op::PushConst(const_idx));
                                fe.code.push(Op::Eq);
                                let skip_jmp = fe.code.len();
                                fe.code.push(Op::JmpIfNot(0));

                                self.emit_expr(&arm.body, fe)?;

                                if !is_last {
                                    let end_jmp = fe.code.len();
                                    fe.code.push(Op::Jmp(0));
                                    end_jmps.push(end_jmp);
                                }

                                let next = fe.code.len() as u32;
                                fe.code[skip_jmp] = Op::JmpIfNot(next);
                            }
                            Pattern::Wildcard => {
                                // Catch-all: just emit the body
                                self.emit_expr(&arm.body, fe)?;
                            }
                            Pattern::Ident(name) => {
                                // Bind scrutinee to a local
                                fe.code.push(Op::LoadLocal(scrutinee_local));
                                let idx = fe.next_local;
                                fe.locals.insert(name.clone(), idx);
                                fe.next_local += 1;
                                fe.code.push(Op::StoreLocal(idx));
                                self.emit_expr(&arm.body, fe)?;
                            }
                            _ => {
                                // Other patterns in string match context: treat as wildcard
                                self.emit_expr(&arm.body, fe)?;
                            }
                        }
                    }

                    let end = fe.code.len() as u32;
                    for jmp_idx in end_jmps {
                        fe.code[jmp_idx] = Op::Jmp(end);
                    }
                } else {
                    // Standard match (non-string): use Op::Match table
                    self.emit_expr(value, fe)?;

                    let table_idx = fe.match_tables.len() as u32;
                    let mut bc_arms = Vec::new();
                    let mut arm_starts = Vec::new();
                    let mut end_jmps = Vec::new();

                    fe.code.push(Op::Match(table_idx));

                    for (i, arm) in arms.iter().enumerate() {
                        let arm_start = fe.code.len() as u32;
                        arm_starts.push(arm_start);

                        if let Pattern::Ident(name) = &arm.pattern {
                            let idx = fe.next_local;
                            fe.locals.insert(name.clone(), idx);
                            fe.next_local += 1;
                            fe.code.push(Op::StoreLocal(idx));
                        } else if has_binding(&arm.pattern) {
                            store_pattern_binding(arm, fe);
                        } else {
                            fe.code.push(Op::Pop);
                        }

                        self.emit_expr(&arm.body, fe)?;

                        if i < arms.len() - 1 {
                            let jmp_idx = fe.code.len();
                            fe.code.push(Op::Jmp(0));
                            end_jmps.push(jmp_idx);
                        }
                    }

                    let end = fe.code.len() as u32;
                    for jmp_idx in end_jmps {
                        fe.code[jmp_idx] = Op::Jmp(end);
                    }

                    for (i, arm) in arms.iter().enumerate() {
                        let tag = pattern_to_tag(&arm.pattern);
                        bc_arms.push(BcMatchArm {
                            tag,
                            target: arm_starts[i],
                        });
                    }
                    fe.match_tables.push(bc_arms);
                }
            }
            Expr::Record { type_name, fields, spread } => {
                let type_id = self.type_map.get(type_name).copied().unwrap_or(0);
                if let Some(base_expr) = spread {
                    // Record spread: State { ..base, field_a: val }
                    // 1) Evaluate base into a temp local
                    self.emit_expr(base_expr, fe)?;
                    let base_local = fe.next_local;
                    fe.next_local += 1;
                    fe.code.push(Op::StoreLocal(base_local));

                    // 2) Get the full field list for this type
                    let type_fields = self.get_type_fields(type_name);
                    let total_fields = type_fields.len();

                    // 3) Build override set
                    let overrides: std::collections::HashMap<&str, &Expr> =
                        fields.iter().map(|(n, e)| (n.as_str(), e)).collect();

                    // 4) For each field in type order: emit override or copy from base
                    for (i, field_name) in type_fields.iter().enumerate() {
                        if let Some(val_expr) = overrides.get(field_name.as_str()) {
                            self.emit_expr(val_expr, fe)?;
                        } else {
                            fe.code.push(Op::LoadLocal(base_local));
                            fe.code.push(Op::GetField(i as u8));
                        }
                    }

                    fe.code.push(Op::MakeRecord(type_id, total_fields as u8));
                } else {
                    // Standard record literal (no spread)
                    for (_, val) in fields {
                        self.emit_expr(val, fe)?;
                    }
                    fe.code.push(Op::MakeRecord(type_id, fields.len() as u8));
                }
            }
            Expr::List(items) => {
                for item in items {
                    self.emit_expr(item, fe)?;
                }
                fe.code.push(Op::MakeList(items.len() as u8));
            }
            Expr::SomeExpr(inner) => {
                self.emit_expr(inner, fe)?;
                // Wrap in Some: use constant Some marker
                let idx = self.module.add_const(Value::Bool(true)); // marker
                fe.code.push(Op::PushConst(idx));
                fe.code.push(Op::Pop); // discard marker
                // The value is already on stack; wrap it
                // Use a dedicated approach: push the value, then make it Some
                // Since our Value type has Some variant, we emit a special pattern:
                // Actually, let's just emit it as-is and use MakeEnum with a special type
                fe.code.push(Op::MakeEnum(0xFFFE, 1)); // variant 1 = Some
            }
            Expr::OkExpr(inner) => {
                self.emit_expr(inner, fe)?;
                fe.code.push(Op::MakeEnum(0xFFFD, 0)); // variant 0 = Ok
            }
            Expr::ErrExpr(inner) => {
                self.emit_expr(inner, fe)?;
                fe.code.push(Op::MakeEnum(0xFFFD, 1)); // variant 1 = Err
            }
            Expr::Spawn(func_expr) => {
                if let Expr::Ident(name) = func_expr.as_ref() {
                    if let Some(&func_idx) = self.fn_map.get(name) {
                        fe.code.push(Op::SpawnActor(func_idx));
                    } else {
                        return Err(CompileError::Codegen(format!("unknown function: {name}")));
                    }
                } else {
                    return Err(CompileError::Codegen("spawn requires a function name".into()));
                }
            }
            Expr::Send { target, message } => {
                self.emit_expr(target, fe)?;
                self.emit_expr(message, fe)?;
                fe.code.push(Op::SendMsg);
            }
            Expr::Receive => {
                fe.code.push(Op::ReceiveMsg);
            }
            Expr::Emit(tree) => {
                self.emit_expr(tree, fe)?;
                fe.code.push(Op::Dup);     // keep value for the expression result
                fe.code.push(Op::EmitUi);  // pops the duplicate
            }
            Expr::Block(block) => {
                self.emit_block(block, fe)?;
            }
        }
        Ok(())
    }

    fn get_type_fields(&self, type_name: &str) -> Vec<String> {
        for typedef in &self.module.types {
            if typedef.name == type_name {
                if let BcTypeKind::Record { fields } = &typedef.kind {
                    return fields.iter().map(|(name, _)| name.clone()).collect();
                }
            }
        }
        Vec::new()
    }

    fn resolve_field(&self, field: &str) -> u8 {
        // For MVP, fields are resolved positionally by iterating type definitions.
        // If we can parse the field as a number, use that directly.
        // Otherwise, search type definitions.
        if let Ok(idx) = field.parse::<u8>() {
            return idx;
        }

        // Search all record types for this field name
        for typedef in &self.module.types {
            if let BcTypeKind::Record { fields } = &typedef.kind {
                for (i, (name, _)) in fields.iter().enumerate() {
                    if name == field {
                        return i as u8;
                    }
                }
            }
        }

        0 // fallback
    }
}

fn pattern_to_tag(pattern: &Pattern) -> i32 {
    match pattern {
        Pattern::Wildcard | Pattern::Ident(_) => -1,
        Pattern::BoolLit(true) => 1,
        Pattern::BoolLit(false) => 0,
        Pattern::NonePat => -2,
        Pattern::SomePat(_) => -3,
        Pattern::OkPat(_) => -4,
        Pattern::ErrPat(_) => -5,
        Pattern::IntLit(n) => *n as i32,
        Pattern::EnumVariant(_, _) => -1, // simplified
        Pattern::StringLit(_) => -1,
    }
}

fn has_binding(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Ident(_) => true,
        Pattern::SomePat(inner) | Pattern::OkPat(inner) | Pattern::ErrPat(inner)
        | Pattern::EnumVariant(_, Some(inner)) => has_binding(inner),
        _ => false,
    }
}

fn store_pattern_binding(arm: &MatchArm, fe: &mut FnEmitter) {
    fn store_inner(pattern: &Pattern, fe: &mut FnEmitter) {
        match pattern {
            Pattern::Ident(name) => {
                let idx = fe.next_local;
                fe.locals.insert(name.clone(), idx);
                fe.next_local += 1;
                fe.code.push(Op::StoreLocal(idx));
            }
            Pattern::SomePat(inner) | Pattern::OkPat(inner) | Pattern::ErrPat(inner)
            | Pattern::EnumVariant(_, Some(inner)) => {
                store_inner(inner, fe);
            }
            _ => {
                fe.code.push(Op::Pop);
            }
        }
    }
    store_inner(&arm.pattern, fe);
}

fn type_expr_to_string(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(n) => n.clone(),
        TypeExpr::Option(inner) => format!("Option<{}>", type_expr_to_string(inner)),
        TypeExpr::Result(ok, err) => format!("Result<{}, {}>", type_expr_to_string(ok), type_expr_to_string(err)),
        TypeExpr::List(inner) => format!("List<{}>", type_expr_to_string(inner)),
        TypeExpr::Map(k, v) => format!("Map<{}, {}>", type_expr_to_string(k), type_expr_to_string(v)),
        TypeExpr::Fn(params, ret) => {
            let params_str: Vec<_> = params.iter().map(type_expr_to_string).collect();
            format!("Fn({}) -> {}", params_str.join(", "), type_expr_to_string(ret))
        }
    }
}
