use std::collections::{HashMap, HashSet};

use boruna_compiler::ast::*;

use super::suggest;
use super::*;

/// AST analyzer that produces additional diagnostics beyond what the compiler checks.
pub struct Analyzer<'a> {
    file: &'a str,
    source: &'a str,
    program: &'a Program,
    /// Type definitions: name -> TypeDefKind
    types: HashMap<String, &'a TypeDefKind>,
    /// Function definitions: name -> &FnDef
    functions: HashMap<String, &'a FnDef>,
}

impl<'a> Analyzer<'a> {
    pub fn new(file: &'a str, source: &'a str, program: &'a Program) -> Self {
        let mut types = HashMap::new();
        let mut functions = HashMap::new();

        for item in &program.items {
            match item {
                Item::TypeDef(t) => {
                    types.insert(t.name.clone(), &t.kind);
                }
                Item::Function(f) => {
                    functions.insert(f.name.clone(), f);
                }
                _ => {}
            }
        }

        Analyzer {
            file,
            source,
            program,
            types,
            functions,
        }
    }

    /// Run all analysis passes and return diagnostics.
    pub fn analyze(&self) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        self.check_match_exhaustiveness(&mut diags);
        self.check_record_fields(&mut diags);
        self.check_capability_purity(&mut diags);
        self.check_type_consistency(&mut diags);
        self.check_mutability(&mut diags);
        diags
    }

    /// Check match expressions for missing enum variants.
    fn check_match_exhaustiveness(&self, diags: &mut Vec<Diagnostic>) {
        for item in &self.program.items {
            if let Item::Function(f) = item {
                self.check_match_in_fn(f, diags);
            }
        }
    }

    fn check_match_in_fn(&self, f: &FnDef, diags: &mut Vec<Diagnostic>) {
        // Build a map of parameter name -> type name (if annotated)
        let mut param_types: HashMap<&str, &str> = HashMap::new();
        for p in &f.params {
            if let TypeExpr::Named(ref type_name) = p.ty {
                param_types.insert(&p.name, type_name);
            }
        }

        self.check_match_in_block(&f.body, &param_types, diags);
    }

    fn check_match_in_block(
        &self,
        block: &Block,
        param_types: &HashMap<&str, &str>,
        diags: &mut Vec<Diagnostic>,
    ) {
        for stmt in &block.stmts {
            self.check_match_in_stmt(stmt, param_types, diags);
        }
    }

    fn check_match_in_stmt(
        &self,
        stmt: &Stmt,
        param_types: &HashMap<&str, &str>,
        diags: &mut Vec<Diagnostic>,
    ) {
        match stmt {
            Stmt::Let { value, .. } => self.check_match_in_expr(value, param_types, diags),
            Stmt::Assign { value, .. } => self.check_match_in_expr(value, param_types, diags),
            Stmt::Expr(e) => self.check_match_in_expr(e, param_types, diags),
            Stmt::Return(Some(e)) => self.check_match_in_expr(e, param_types, diags),
            Stmt::Return(None) => {}
            Stmt::While { condition, body } => {
                self.check_match_in_expr(condition, param_types, diags);
                self.check_match_in_block(body, param_types, diags);
            }
            Stmt::For { iter, body, .. } => {
                self.check_match_in_expr(iter, param_types, diags);
                self.check_match_in_block(body, param_types, diags);
            }
        }
    }

    fn check_match_in_expr(
        &self,
        expr: &Expr,
        param_types: &HashMap<&str, &str>,
        diags: &mut Vec<Diagnostic>,
    ) {
        match expr {
            Expr::Match { value, arms } => {
                // Check if scrutinee is an identifier with a known enum type
                if let Expr::Ident(ref name) = **value {
                    if let Some(&type_name) = param_types.get(name.as_str()) {
                        if let Some(TypeDefKind::Enum(variants)) = self.types.get(type_name) {
                            self.check_arms_cover_variants(name, type_name, variants, arms, diags);
                        }
                    }
                }

                // Recurse into arm bodies
                for arm in arms {
                    self.check_match_in_expr(&arm.body, param_types, diags);
                }
            }
            Expr::Binary { left, right, .. } => {
                self.check_match_in_expr(left, param_types, diags);
                self.check_match_in_expr(right, param_types, diags);
            }
            Expr::Unary { expr, .. } => self.check_match_in_expr(expr, param_types, diags),
            Expr::Call { func, args } => {
                self.check_match_in_expr(func, param_types, diags);
                for arg in args {
                    self.check_match_in_expr(arg, param_types, diags);
                }
            }
            Expr::FieldAccess { object, .. } => {
                self.check_match_in_expr(object, param_types, diags)
            }
            Expr::If {
                condition,
                then_block,
                else_block,
            } => {
                self.check_match_in_expr(condition, param_types, diags);
                self.check_match_in_block(then_block, param_types, diags);
                if let Some(eb) = else_block {
                    self.check_match_in_block(eb, param_types, diags);
                }
            }
            Expr::Record { fields, spread, .. } => {
                for (_, val) in fields {
                    self.check_match_in_expr(val, param_types, diags);
                }
                if let Some(base) = spread {
                    self.check_match_in_expr(base, param_types, diags);
                }
            }
            Expr::List(items) => {
                for item in items {
                    self.check_match_in_expr(item, param_types, diags);
                }
            }
            Expr::SomeExpr(e)
            | Expr::OkExpr(e)
            | Expr::ErrExpr(e)
            | Expr::Spawn(e)
            | Expr::Emit(e) => {
                self.check_match_in_expr(e, param_types, diags);
            }
            Expr::Send { target, message } => {
                self.check_match_in_expr(target, param_types, diags);
                self.check_match_in_expr(message, param_types, diags);
            }
            Expr::Block(b) => self.check_match_in_block(b, param_types, diags),
            Expr::EnumVariant {
                payload: Some(e), ..
            } => self.check_match_in_expr(e, param_types, diags),
            _ => {}
        }
    }

    fn check_arms_cover_variants(
        &self,
        scrutinee_name: &str,
        type_name: &str,
        variants: &[(String, Option<TypeExpr>)],
        arms: &[MatchArm],
        diags: &mut Vec<Diagnostic>,
    ) {
        // Check if there's a wildcard or catch-all pattern
        let has_wildcard = arms
            .iter()
            .any(|a| matches!(a.pattern, Pattern::Wildcard | Pattern::Ident(_)));
        if has_wildcard {
            return; // Wildcard covers everything
        }

        let variant_names: HashSet<&str> = variants.iter().map(|(n, _)| n.as_str()).collect();
        let covered: HashSet<&str> = arms
            .iter()
            .filter_map(|a| {
                if let Pattern::EnumVariant(ref name, _) = a.pattern {
                    Some(name.as_str())
                } else {
                    None
                }
            })
            .collect();

        let missing: Vec<&str> = variant_names
            .difference(&covered)
            .copied()
            .collect::<Vec<_>>();

        if !missing.is_empty() {
            let mut missing_sorted = missing;
            missing_sorted.sort();

            let match_line = find_match_line(self.source, scrutinee_name);
            let match_end_line =
                match_line.and_then(|start| find_match_end_line(self.source, start));

            let mut diag = Diagnostic::error(
                E005_NON_EXHAUSTIVE_MATCH,
                format!(
                    "non-exhaustive match on '{}' of type '{}': missing variants: {}",
                    scrutinee_name,
                    type_name,
                    missing_sorted.join(", "),
                ),
            );

            if let Some(line) = match_line {
                diag = diag.at(self.file, line, None);
            }

            // Generate suggested patch
            if let (Some(start), Some(end)) = (match_line, match_end_line) {
                let patch = suggest::suggest_missing_match_arms(
                    self.file,
                    self.source,
                    start,
                    end,
                    &missing_sorted,
                    variants,
                );
                if let Some(p) = patch {
                    diag = diag.with_suggestion(p);
                }
            }

            diags.push(diag);
        }
    }

    /// Check record literals for unknown field names.
    fn check_record_fields(&self, diags: &mut Vec<Diagnostic>) {
        for item in &self.program.items {
            if let Item::Function(f) = item {
                self.check_fields_in_block(&f.body, diags);
            }
        }
    }

    fn check_fields_in_block(&self, block: &Block, diags: &mut Vec<Diagnostic>) {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
                    self.check_fields_in_expr(value, diags);
                }
                Stmt::Expr(e) | Stmt::Return(Some(e)) => {
                    self.check_fields_in_expr(e, diags);
                }
                Stmt::While { condition, body } => {
                    self.check_fields_in_expr(condition, diags);
                    self.check_fields_in_block(body, diags);
                }
                _ => {}
            }
        }
    }

    fn check_fields_in_expr(&self, expr: &Expr, diags: &mut Vec<Diagnostic>) {
        match expr {
            Expr::Record {
                type_name,
                fields,
                spread,
            } => {
                if let Some(TypeDefKind::Record(type_fields)) = self.types.get(type_name.as_str()) {
                    let known_fields: HashSet<&str> =
                        type_fields.iter().map(|(n, _)| n.as_str()).collect();

                    for (field_name, _) in fields {
                        if !known_fields.contains(field_name.as_str()) {
                            let line = find_field_line(self.source, field_name);
                            let mut diag = Diagnostic::error(
                                E006_UNKNOWN_FIELD,
                                format!(
                                    "unknown field '{}' in record type '{}'",
                                    field_name, type_name,
                                ),
                            );
                            if let Some(l) = line {
                                diag = diag.at(self.file, l, None);
                            }

                            // Suggest closest field name
                            let closest = suggest::find_closest_name(
                                field_name,
                                &known_fields.iter().copied().collect::<Vec<_>>(),
                            );
                            if let Some(suggestion) = closest {
                                let patch = suggest::suggest_rename_field(
                                    self.file,
                                    self.source,
                                    field_name,
                                    suggestion,
                                    line,
                                );
                                if let Some(p) = patch {
                                    diag = diag.with_suggestion(p);
                                }
                            }

                            diag = diag.with_related(RelatedInfo {
                                message: format!(
                                    "type '{}' has fields: {}",
                                    type_name,
                                    type_fields
                                        .iter()
                                        .map(|(n, _)| n.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", "),
                                ),
                                location: None,
                            });

                            diags.push(diag);
                        }
                    }
                }

                // Recurse
                for (_, val) in fields {
                    self.check_fields_in_expr(val, diags);
                }
                if let Some(base) = spread {
                    self.check_fields_in_expr(base, diags);
                }
            }
            Expr::Binary { left, right, .. } => {
                self.check_fields_in_expr(left, diags);
                self.check_fields_in_expr(right, diags);
            }
            Expr::Unary { expr, .. } => self.check_fields_in_expr(expr, diags),
            Expr::Call { func, args } => {
                self.check_fields_in_expr(func, diags);
                for arg in args {
                    self.check_fields_in_expr(arg, diags);
                }
            }
            Expr::FieldAccess { object, .. } => self.check_fields_in_expr(object, diags),
            Expr::If {
                condition,
                then_block,
                else_block,
            } => {
                self.check_fields_in_expr(condition, diags);
                self.check_fields_in_block(then_block, diags);
                if let Some(eb) = else_block {
                    self.check_fields_in_block(eb, diags);
                }
            }
            Expr::Match { value, arms } => {
                self.check_fields_in_expr(value, diags);
                for arm in arms {
                    self.check_fields_in_expr(&arm.body, diags);
                }
            }
            Expr::List(items) => {
                for item in items {
                    self.check_fields_in_expr(item, diags);
                }
            }
            Expr::SomeExpr(e)
            | Expr::OkExpr(e)
            | Expr::ErrExpr(e)
            | Expr::Spawn(e)
            | Expr::Emit(e) => self.check_fields_in_expr(e, diags),
            Expr::Send { target, message } => {
                self.check_fields_in_expr(target, diags);
                self.check_fields_in_expr(message, diags);
            }
            Expr::Block(b) => self.check_fields_in_block(b, diags),
            Expr::EnumVariant {
                payload: Some(e), ..
            } => self.check_fields_in_expr(e, diags),
            _ => {}
        }
    }

    /// Check that update() and view() don't declare capabilities (framework purity rule).
    fn check_capability_purity(&self, diags: &mut Vec<Diagnostic>) {
        // Only check if this looks like a framework app (has init/update/view)
        let has_init = self.functions.contains_key("init");
        let has_update = self.functions.contains_key("update");
        let has_view = self.functions.contains_key("view");

        if !has_init || !has_update || !has_view {
            return; // Not a framework app
        }

        for name in &["update", "view"] {
            if let Some(f) = self.functions.get(*name) {
                if !f.capabilities.is_empty() {
                    let line = find_fn_def_line(self.source, name);
                    let mut diag = Diagnostic::error(
                        E007_CAPABILITY_VIOLATION,
                        format!(
                            "capability violation: function '{}' must be pure but is annotated with !{{{}}} — remove the capability annotation",
                            name,
                            f.capabilities.join(", "),
                        ),
                    );
                    if let Some(l) = line {
                        diag = diag.at(self.file, l, None);
                    }

                    // Suggest removing the capability annotation
                    if let Some(l) = line {
                        let patch = suggest::suggest_remove_capabilities(
                            self.file,
                            self.source,
                            l,
                            &f.capabilities,
                        );
                        if let Some(p) = patch {
                            diag = diag.with_suggestion(p);
                        }
                    }

                    diags.push(diag);
                }
            }
        }
    }

    /// Warn-only type-consistency pass (first step toward strict static typing).
    /// Uses a conservative, inference-free local type environment: it only
    /// reasons about types it can name with confidence (literals, annotated
    /// bindings, record/enum constructors, and user-function return types), and
    /// only reports a mismatch when BOTH sides resolve to concrete named types
    /// that differ. Everything uncertain (generics, builtins, binary ops) is
    /// left untyped, so false positives are rare. Emitted at `Warning` severity
    /// — never blocks compilation.
    fn check_type_consistency(&self, diags: &mut Vec<Diagnostic>) {
        let mut fn_sigs: FnSigs = HashMap::new();
        for item in &self.program.items {
            if let Item::Function(f) = item {
                let params = f.params.iter().map(|p| named_type(&p.ty)).collect();
                let ret = f.return_type.as_ref().and_then(named_type);
                fn_sigs.insert(f.name.as_str(), (params, ret));
            }
        }
        for item in &self.program.items {
            if let Item::Function(f) = item {
                let mut env: HashMap<String, String> = HashMap::new();
                for p in &f.params {
                    if let Some(t) = named_type(&p.ty) {
                        env.insert(p.name.clone(), t);
                    }
                }
                self.check_types_in_block(&f.body, &mut env, &fn_sigs, diags);
            }
        }
    }

    /// Warn when a binding declared without `mut`, a parameter or a `for` variable is
    /// reassigned. The compiler accepts this today (the `mut` flag is parsed but not
    /// enforced), so it is a warning; language version 2.0 turns it into an error.
    /// One diagnostic per variable per function.
    fn check_mutability(&self, diags: &mut Vec<Diagnostic>) {
        for item in &self.program.items {
            if let Item::Function(f) = item {
                let mut scope: HashMap<String, Binding> = HashMap::new();
                for p in &f.params {
                    scope.insert(p.name.clone(), Binding::Param);
                }
                let mut hits = Vec::new();
                walk_block_for_mut(&f.body, &mut scope, &mut hits);
                let mut reported = HashSet::new();
                for (name, kind) in hits {
                    if reported.insert(name.clone()) {
                        diags.push(self.assign_immutable_diag(f, &name, kind));
                    }
                }
            }
        }
    }

    fn assign_immutable_diag(&self, f: &FnDef, name: &str, kind: Binding) -> Diagnostic {
        let (start, end) = fn_line_range(self.source, &f.name);
        let lines: Vec<&str> = self.source.lines().collect();
        match kind {
            Binding::Let => {
                let mut diag = Diagnostic::warning(
                    E010_ASSIGN_IMMUTABLE,
                    format!(
                        "'{name}' is reassigned but was declared without `mut`; declare it \
                         with `let mut {name}` (accepted today, an error in language version 2.0)"
                    ),
                );
                let candidates: Vec<usize> = (start..end)
                    .filter(|&i| is_immutable_let_of(lines[i], name))
                    .collect();
                if let [i] = candidates[..] {
                    diag = diag.at(self.file, i + 1, None);
                    diag.suggested_patches.push(SuggestedPatch {
                        id: format!("{E010_ASSIGN_IMMUTABLE}-add-mut-{name}"),
                        description: format!("declare '{name}' with `let mut`"),
                        confidence: Confidence::High,
                        rationale: "the binding is reassigned later in the same function".into(),
                        edits: vec![TextEdit {
                            file: self.file.to_string(),
                            start_line: i + 1,
                            old_text: lines[i].to_string(),
                            new_text: lines[i].replacen("let ", "let mut ", 1),
                        }],
                    });
                } else if start < end {
                    // Shadowed or not found: point at the function, offer no automatic edit.
                    diag = diag.at(self.file, start + 1, None);
                }
                diag
            }
            Binding::Param | Binding::ForVar => {
                let what = if kind == Binding::Param {
                    "parameter"
                } else {
                    "loop variable"
                };
                let mut diag = Diagnostic::warning(
                    E010_ASSIGN_IMMUTABLE,
                    format!(
                        "{what} '{name}' is reassigned; copy it into a new binding first, e.g. \
                         `let mut {name}_acc = {name}` (accepted today, an error in language version 2.0)"
                    ),
                );
                if start < end {
                    diag = diag.at(self.file, start + 1, None);
                }
                diag
            }
            Binding::Mut => unreachable!("mutable bindings are never reported"),
        }
    }

    fn check_types_in_block(
        &self,
        block: &Block,
        env: &mut HashMap<String, String>,
        fn_sigs: &FnSigs,
        diags: &mut Vec<Diagnostic>,
    ) {
        for stmt in &block.stmts {
            self.check_types_in_stmt(stmt, env, fn_sigs, diags);
        }
    }

    fn check_types_in_stmt(
        &self,
        stmt: &Stmt,
        env: &mut HashMap<String, String>,
        fn_sigs: &FnSigs,
        diags: &mut Vec<Diagnostic>,
    ) {
        match stmt {
            Stmt::Let {
                name, ty, value, ..
            } => {
                self.check_types_in_expr(value, env, fn_sigs, diags);
                let inferred = self.infer_expr_type(value, env, fn_sigs);
                if let Some(TypeExpr::Named(declared)) = ty {
                    if let Some(actual) = &inferred {
                        if actual != declared {
                            let mut diag = Diagnostic::warning(
                                E009_TYPE_ERROR,
                                format!(
                                    "type mismatch: '{name}' is annotated '{declared}' but its \
                                     initializer has type '{actual}'"
                                ),
                            );
                            if let Some(l) = find_let_line(self.source, name) {
                                diag = diag.at(self.file, l, None);
                            }
                            diags.push(diag);
                        }
                    }
                    env.insert(name.clone(), declared.clone());
                } else if let Some(actual) = inferred {
                    env.insert(name.clone(), actual);
                }
            }
            Stmt::Assign { value, .. } => self.check_types_in_expr(value, env, fn_sigs, diags),
            Stmt::Expr(e) | Stmt::Return(Some(e)) => {
                self.check_types_in_expr(e, env, fn_sigs, diags)
            }
            Stmt::Return(None) => {}
            Stmt::While { condition, body } => {
                self.check_types_in_expr(condition, env, fn_sigs, diags);
                let mut inner = env.clone();
                self.check_types_in_block(body, &mut inner, fn_sigs, diags);
            }
            Stmt::For { iter, body, .. } => {
                self.check_types_in_expr(iter, env, fn_sigs, diags);
                let mut inner = env.clone();
                self.check_types_in_block(body, &mut inner, fn_sigs, diags);
            }
        }
    }

    fn check_types_in_expr(
        &self,
        expr: &Expr,
        env: &HashMap<String, String>,
        fn_sigs: &FnSigs,
        diags: &mut Vec<Diagnostic>,
    ) {
        // Direct call to a named user function: check each argument's concrete
        // type against the declared parameter type. Skip when the callee name is
        // a local binding (a first-class function value passed as a parameter).
        if let Expr::Call { func, args } = expr {
            if let Expr::Ident(fname) = func.as_ref() {
                if !env.contains_key(fname) {
                    if let Some((param_types, _)) = fn_sigs.get(fname.as_str()) {
                        for (i, arg) in args.iter().enumerate() {
                            if let Some(Some(pt)) = param_types.get(i) {
                                if let Some(at) = self.infer_expr_type(arg, env, fn_sigs) {
                                    if &at != pt {
                                        let mut diag = Diagnostic::warning(
                                            E009_TYPE_ERROR,
                                            format!(
                                                "type mismatch: call to '{fname}' argument {} \
                                                 expects '{pt}' but got '{at}'",
                                                i + 1
                                            ),
                                        );
                                        if let Some(l) =
                                            find_line_containing(self.source, &format!("{fname}("))
                                        {
                                            diag = diag.at(self.file, l, None);
                                        }
                                        diags.push(diag);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Recurse into sub-expressions so calls/lets nested anywhere are covered.
        match expr {
            Expr::Binary { left, right, .. } => {
                self.check_types_in_expr(left, env, fn_sigs, diags);
                self.check_types_in_expr(right, env, fn_sigs, diags);
            }
            Expr::Unary { expr, .. } => self.check_types_in_expr(expr, env, fn_sigs, diags),
            Expr::Call { func, args } => {
                self.check_types_in_expr(func, env, fn_sigs, diags);
                for a in args {
                    self.check_types_in_expr(a, env, fn_sigs, diags);
                }
            }
            Expr::FieldAccess { object, .. } => {
                self.check_types_in_expr(object, env, fn_sigs, diags)
            }
            Expr::If {
                condition,
                then_block,
                else_block,
            } => {
                self.check_types_in_expr(condition, env, fn_sigs, diags);
                let mut inner = env.clone();
                self.check_types_in_block(then_block, &mut inner, fn_sigs, diags);
                if let Some(eb) = else_block {
                    let mut inner = env.clone();
                    self.check_types_in_block(eb, &mut inner, fn_sigs, diags);
                }
            }
            Expr::Match { value, arms } => {
                self.check_types_in_expr(value, env, fn_sigs, diags);
                for arm in arms {
                    self.check_types_in_expr(&arm.body, env, fn_sigs, diags);
                }
            }
            Expr::Record { fields, spread, .. } => {
                for (_, v) in fields {
                    self.check_types_in_expr(v, env, fn_sigs, diags);
                }
                if let Some(s) = spread {
                    self.check_types_in_expr(s, env, fn_sigs, diags);
                }
            }
            Expr::List(items) => {
                for it in items {
                    self.check_types_in_expr(it, env, fn_sigs, diags);
                }
            }
            Expr::SomeExpr(e)
            | Expr::OkExpr(e)
            | Expr::ErrExpr(e)
            | Expr::Spawn(e)
            | Expr::Emit(e) => self.check_types_in_expr(e, env, fn_sigs, diags),
            Expr::EnumVariant {
                payload: Some(p), ..
            } => self.check_types_in_expr(p, env, fn_sigs, diags),
            Expr::Send { target, message } => {
                self.check_types_in_expr(target, env, fn_sigs, diags);
                self.check_types_in_expr(message, env, fn_sigs, diags);
            }
            Expr::Block(b) => {
                let mut inner = env.clone();
                self.check_types_in_block(b, &mut inner, fn_sigs, diags);
            }
            _ => {}
        }
    }

    /// Best-effort concrete type of an expression, or `None` when it cannot be
    /// named with confidence. Deliberately narrow to keep false positives low.
    fn infer_expr_type(
        &self,
        expr: &Expr,
        env: &HashMap<String, String>,
        fn_sigs: &FnSigs,
    ) -> Option<String> {
        match expr {
            Expr::IntLit(_) => Some("Int".to_string()),
            Expr::FloatLit(_) => Some("Float".to_string()),
            Expr::StringLit(_) => Some("String".to_string()),
            Expr::BoolLit(_) => Some("Bool".to_string()),
            Expr::Ident(name) => env.get(name).cloned(),
            Expr::Record { type_name, .. } => Some(type_name.clone()),
            Expr::EnumVariant { enum_name, .. } => Some(enum_name.clone()),
            Expr::Call { func, .. } => {
                if let Expr::Ident(fname) = func.as_ref() {
                    if !env.contains_key(fname) {
                        return fn_sigs.get(fname.as_str()).and_then(|(_, ret)| ret.clone());
                    }
                }
                None
            }
            _ => None,
        }
    }
}

/// A user function's signature: per-parameter concrete type (None when the
/// parameter type isn't a plain named type) and the concrete return type.
type FnSigs<'a> = HashMap<&'a str, (Vec<Option<String>>, Option<String>)>;

/// The concrete name of a type expression, or `None` for generic constructors
/// (`Option`/`Result`/`List`/`Map`/`Fn`) which this pass treats as untyped.
fn named_type(ty: &TypeExpr) -> Option<String> {
    match ty {
        TypeExpr::Named(n) => Some(n.clone()),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    Let,
    Mut,
    Param,
    ForVar,
}

fn walk_block_for_mut(
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
                walk_expr_for_mut(value, scope, hits);
                let kind = if *mutable { Binding::Mut } else { Binding::Let };
                scope.insert(name.clone(), kind);
            }
            Stmt::Assign { target, value } => {
                walk_expr_for_mut(value, scope, hits);
                if let Some(&kind) = scope.get(target) {
                    if kind != Binding::Mut {
                        hits.push((target.clone(), kind));
                    }
                }
            }
            Stmt::Expr(e) | Stmt::Return(Some(e)) => walk_expr_for_mut(e, scope, hits),
            Stmt::Return(None) => {}
            Stmt::While { condition, body } => {
                walk_expr_for_mut(condition, scope, hits);
                walk_block_for_mut(body, &mut scope.clone(), hits);
            }
            Stmt::For { var, iter, body } => {
                walk_expr_for_mut(iter, scope, hits);
                let mut inner = scope.clone();
                inner.insert(var.clone(), Binding::ForVar);
                walk_block_for_mut(body, &mut inner, hits);
            }
        }
    }
}

fn walk_expr_for_mut(
    expr: &Expr,
    scope: &HashMap<String, Binding>,
    hits: &mut Vec<(String, Binding)>,
) {
    match expr {
        Expr::If {
            condition,
            then_block,
            else_block,
        } => {
            walk_expr_for_mut(condition, scope, hits);
            walk_block_for_mut(then_block, &mut scope.clone(), hits);
            if let Some(b) = else_block {
                walk_block_for_mut(b, &mut scope.clone(), hits);
            }
        }
        Expr::Match { value, arms } => {
            walk_expr_for_mut(value, scope, hits);
            for arm in arms {
                // A name bound by the pattern shadows the outer binding; leave it out
                // rather than guess how an assignment to it behaves.
                let mut inner = scope.clone();
                for n in pattern_names(&arm.pattern) {
                    inner.remove(&n);
                }
                walk_expr_for_mut(&arm.body, &inner, hits);
            }
        }
        Expr::Block(b) => walk_block_for_mut(b, &mut scope.clone(), hits),
        Expr::Binary { left, right, .. } => {
            walk_expr_for_mut(left, scope, hits);
            walk_expr_for_mut(right, scope, hits);
        }
        Expr::Unary { expr, .. }
        | Expr::SomeExpr(expr)
        | Expr::OkExpr(expr)
        | Expr::ErrExpr(expr)
        | Expr::Spawn(expr)
        | Expr::Emit(expr) => walk_expr_for_mut(expr, scope, hits),
        Expr::Call { func, args } => {
            walk_expr_for_mut(func, scope, hits);
            for a in args {
                walk_expr_for_mut(a, scope, hits);
            }
        }
        Expr::FieldAccess { object, .. } => walk_expr_for_mut(object, scope, hits),
        Expr::Record { fields, spread, .. } => {
            for (_, e) in fields {
                walk_expr_for_mut(e, scope, hits);
            }
            if let Some(s) = spread {
                walk_expr_for_mut(s, scope, hits);
            }
        }
        Expr::EnumVariant { payload, .. } => {
            if let Some(p) = payload {
                walk_expr_for_mut(p, scope, hits);
            }
        }
        Expr::List(items) => {
            for e in items {
                walk_expr_for_mut(e, scope, hits);
            }
        }
        Expr::Send { target, message } => {
            walk_expr_for_mut(target, scope, hits);
            walk_expr_for_mut(message, scope, hits);
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

fn pattern_names(p: &Pattern) -> Vec<String> {
    match p {
        Pattern::Ident(n) => vec![n.clone()],
        Pattern::SomePat(inner) | Pattern::OkPat(inner) | Pattern::ErrPat(inner) => {
            pattern_names(inner)
        }
        Pattern::EnumVariant(_, Some(inner)) => pattern_names(inner),
        _ => Vec::new(),
    }
}

/// 0-indexed `[start, end)` line range of the function `name`: from its `fn` line to the next
/// top-level item. `(0, 0)` when not found.
fn fn_line_range(source: &str, name: &str) -> (usize, usize) {
    let lines: Vec<&str> = source.lines().collect();
    let is_fn_line = |l: &str| {
        let t = l.trim_start();
        let t = t.strip_prefix("export ").unwrap_or(t);
        t.strip_prefix("fn ")
            .map(|rest| {
                rest.strip_prefix(name)
                    .is_some_and(|r| r.trim_start().starts_with('('))
            })
            .unwrap_or(false)
    };
    let Some(start) = lines.iter().position(|l| is_fn_line(l)) else {
        return (0, 0);
    };
    let is_top_level_item = |l: &str| {
        ["fn ", "export ", "type ", "enum ", "import ", "module "]
            .iter()
            .any(|k| l.starts_with(k))
    };
    let end = (start + 1..lines.len())
        .find(|&i| is_top_level_item(lines[i]))
        .unwrap_or(lines.len());
    (start, end)
}

/// `let NAME` (without `mut`) at the start of the line, NAME followed by `:`, `=` or a space.
fn is_immutable_let_of(line: &str, name: &str) -> bool {
    line.trim_start()
        .strip_prefix("let ")
        .and_then(|r| r.strip_prefix(name))
        .and_then(|r| r.chars().next())
        .is_some_and(|c| c == ':' || c == '=' || c == ' ')
}

/// Line (1-indexed) of the `let` binding for `name`, best-effort.
fn find_let_line(source: &str, name: &str) -> Option<usize> {
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if (trimmed.starts_with("let ") || trimmed.starts_with("let mut "))
            && trimmed.contains(name)
        {
            return Some(i + 1);
        }
    }
    None
}

/// First line (1-indexed) containing `needle`, best-effort.
fn find_line_containing(source: &str, needle: &str) -> Option<usize> {
    source
        .lines()
        .position(|line| line.contains(needle))
        .map(|i| i + 1)
}

/// Find the line number where `match <name>` occurs.
fn find_match_line(source: &str, var_name: &str) -> Option<usize> {
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("match ") && trimmed.contains(var_name) {
            return Some(i + 1);
        }
    }
    None
}

/// Find the closing brace of a match expression starting at `start_line` (1-indexed).
fn find_match_end_line(source: &str, start_line: usize) -> Option<usize> {
    let lines: Vec<&str> = source.lines().collect();
    if start_line == 0 || start_line > lines.len() {
        return None;
    }

    let mut depth = 0i32;
    for (i, line) in lines.iter().enumerate().skip(start_line - 1) {
        for ch in line.chars() {
            if ch == '{' {
                depth += 1;
            }
            if ch == '}' {
                depth -= 1;
            }
        }
        if depth <= 0 {
            return Some(i + 1); // 1-indexed
        }
    }
    None
}

/// Find the line number where a function is defined.
fn find_fn_def_line(source: &str, fn_name: &str) -> Option<usize> {
    let pattern = format!("fn {fn_name}");
    for (i, line) in source.lines().enumerate() {
        if line.contains(&pattern) {
            return Some(i + 1);
        }
    }
    None
}

/// Find the line number where a field name appears in a record literal.
fn find_field_line(source: &str, field_name: &str) -> Option<usize> {
    let pattern = format!("{field_name}:");
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with(&pattern)
            || trimmed.contains(&format!(" {pattern}"))
            || trimmed.contains(&format!(",{pattern}"))
        {
            return Some(i + 1);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_match_line() {
        let source = "fn update(state: State, msg: Msg) -> State {\n    match msg {\n        Add => state\n    }\n}\n";
        assert_eq!(find_match_line(source, "msg"), Some(2));
    }

    #[test]
    fn test_find_match_end_line() {
        let source = "fn update(state: State, msg: Msg) -> State {\n    match msg {\n        Add => state\n    }\n}\n";
        assert_eq!(find_match_end_line(source, 2), Some(4));
    }

    #[test]
    fn test_find_fn_def_line() {
        let source = "fn init() -> State { State { count: 0 } }\nfn update(s: State, m: Msg) -> State { s }\n";
        assert_eq!(find_fn_def_line(source, "update"), Some(2));
    }

    #[test]
    fn test_non_exhaustive_match() {
        let source = "\
enum Action { Add, Remove, Clear }
type State { count: Int }

fn update(state: State, action: Action) -> State {
    match action {
        Add => State { count: state.count + 1 }
    }
}

fn init() -> State { State { count: 0 } }
fn view(state: State) -> String { \"ok\" }
";
        let tokens = boruna_compiler::lexer::lex(source).unwrap();
        let program = boruna_compiler::parser::parse(tokens).unwrap();
        let analyzer = Analyzer::new("test.ax", source, &program);
        let diags = analyzer.analyze();
        let match_diag = diags.iter().find(|d| d.id == E005_NON_EXHAUSTIVE_MATCH);
        assert!(
            match_diag.is_some(),
            "expected non-exhaustive match diagnostic"
        );
        let d = match_diag.unwrap();
        assert!(d.message.contains("Clear"));
        assert!(d.message.contains("Remove"));
    }

    fn type_warnings(source: &str) -> Vec<Diagnostic> {
        let tokens = boruna_compiler::lexer::lex(source).unwrap();
        let program = boruna_compiler::parser::parse(tokens).unwrap();
        let analyzer = Analyzer::new("test.ax", source, &program);
        analyzer
            .analyze()
            .into_iter()
            .filter(|d| d.id == E009_TYPE_ERROR && d.severity == Severity::Warning)
            .collect()
    }

    #[test]
    fn test_type_mismatch_let_annotation_warns() {
        let warns = type_warnings("fn main() -> Int {\n    let x: Int = \"hello\"\n    0\n}\n");
        assert_eq!(warns.len(), 1, "expected one type-mismatch warning");
        assert!(warns[0].message.contains("Int") && warns[0].message.contains("String"));
    }

    #[test]
    fn test_type_mismatch_call_argument_warns() {
        let warns = type_warnings(
            "fn add(a: Int, b: Int) -> Int { a + b }\nfn main() -> Int { add(\"x\", 2) }\n",
        );
        assert!(
            warns.iter().any(|d| d.message.contains("add")
                && d.message.contains("expects 'Int'")
                && d.message.contains("String")),
            "expected a call-argument type warning, got: {warns:?}"
        );
    }

    #[test]
    fn test_type_consistency_no_false_positive() {
        // A well-typed program must produce no type-mismatch warnings.
        let warns = type_warnings(
            "fn add(a: Int, b: Int) -> Int { a + b }\n\
             fn main() -> Int {\n    let x: Int = 5\n    add(x, 2)\n}\n",
        );
        assert!(warns.is_empty(), "unexpected type warnings: {warns:?}");
    }

    #[test]
    fn test_exhaustive_match_with_wildcard() {
        let source = "\
enum Action { Add, Remove }
type State { count: Int }

fn update(state: State, action: Action) -> State {
    match action {
        Add => State { count: state.count + 1 }
        _ => state
    }
}

fn init() -> State { State { count: 0 } }
fn view(state: State) -> String { \"ok\" }
";
        let tokens = boruna_compiler::lexer::lex(source).unwrap();
        let program = boruna_compiler::parser::parse(tokens).unwrap();
        let analyzer = Analyzer::new("test.ax", source, &program);
        let diags = analyzer.analyze();
        let match_diag = diags.iter().find(|d| d.id == E005_NON_EXHAUSTIVE_MATCH);
        assert!(match_diag.is_none(), "wildcard should cover all variants");
    }

    #[test]
    fn test_unknown_record_field() {
        let source = "\
type State { count: Int, name: String }

fn init() -> State {
    State { countt: 0, name: \"test\" }
}
";
        let tokens = boruna_compiler::lexer::lex(source).unwrap();
        let program = boruna_compiler::parser::parse(tokens).unwrap();
        let analyzer = Analyzer::new("test.ax", source, &program);
        let diags = analyzer.analyze();
        let field_diag = diags.iter().find(|d| d.id == E006_UNKNOWN_FIELD);
        assert!(field_diag.is_some(), "expected unknown field diagnostic");
        assert!(field_diag.unwrap().message.contains("countt"));
    }

    #[test]
    fn test_capability_violation() {
        let source = "\
type State { count: Int }
type Msg { tag: String }

fn init() -> State { State { count: 0 } }
fn update(state: State, msg: Msg) -> State !{fs.read} { state }
fn view(state: State) -> String { \"ok\" }
";
        let tokens = boruna_compiler::lexer::lex(source).unwrap();
        let program = boruna_compiler::parser::parse(tokens).unwrap();
        let analyzer = Analyzer::new("test.ax", source, &program);
        let diags = analyzer.analyze();
        let cap_diag = diags.iter().find(|d| d.id == E007_CAPABILITY_VIOLATION);
        assert!(
            cap_diag.is_some(),
            "expected capability violation diagnostic"
        );
        assert!(cap_diag.unwrap().message.contains("update"));
    }
}
