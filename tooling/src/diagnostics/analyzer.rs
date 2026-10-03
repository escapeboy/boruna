use std::collections::{HashMap, HashSet};

use boruna_compiler::ast::*;
use boruna_compiler::strict::{Binding, Issue};

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
        self.check_strict(&mut diags);
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
            Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
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

    /// Language 2.0 strictness (E009 type mismatches, E010 reassigning a binding that is not
    /// `mut`). The compiler rejects these; this pass reports every one with a location and,
    /// where safe, an automatic fix. The rules live in `boruna_compiler::strict`.
    fn check_strict(&self, diags: &mut Vec<Diagnostic>) {
        for issue in boruna_compiler::strict::check(self.program) {
            let diag = match &issue {
                Issue::AssignImmutable { name, binding, .. } => {
                    match self.functions.get(issue.func()) {
                        Some(f) => self.assign_immutable_diag(f, name, *binding, &issue),
                        None => Diagnostic::error(E010_ASSIGN_IMMUTABLE, issue.message()),
                    }
                }
                _ => {
                    let func = issue.func();
                    let line = match &issue {
                        Issue::LetAnnotation { name, .. } => {
                            self.line_in_fn(func, |l| let_of(l).is_some_and(|n| n == name))
                        }
                        Issue::CallArgument {
                            callee, occurrence, ..
                        } => self.nth_in_fn(func, *occurrence, |l| calls(l, callee)),
                        Issue::Assign {
                            name, occurrence, ..
                        } => self.nth_in_fn(func, *occurrence, |l| assigns(l, name)),
                        Issue::WhileCondition { .. } => {
                            self.line_in_fn(func, |l| l.trim_start().starts_with("while "))
                        }
                        Issue::AssignImmutable { .. } => None,
                    };
                    let mut diag = Diagnostic::error(E009_TYPE_ERROR, issue.message());
                    if let Some(l) = line {
                        diag = diag.at(self.file, l, None);
                    }
                    diag
                }
            };
            diags.push(diag);
        }
    }

    fn assign_immutable_diag(
        &self,
        f: &FnDef,
        name: &str,
        kind: Binding,
        issue: &Issue,
    ) -> Diagnostic {
        let (start, end) = fn_line_range(self.source, &f.name);
        let lines: Vec<&str> = self.source.lines().collect();
        let mut diag = Diagnostic::error(E010_ASSIGN_IMMUTABLE, issue.message());
        if kind == Binding::Let {
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
                return diag;
            }
        }
        // A parameter, a loop variable, or a `let` that is shadowed or not found: point at the
        // first reassignment and offer no automatic edit.
        if let Some(l) = self.nth_in_fn(&f.name, 1, |l| assigns(l, name)) {
            diag = diag.at(self.file, l, None);
        } else if start < end {
            diag = diag.at(self.file, start + 1, None);
        }
        diag
    }

    /// 1-indexed line of the first line in the body of function `func` that matches `pred`.
    /// Searching only inside the function keeps a finding from pointing at another function or
    /// at the declaration of the called function.
    fn nth_in_fn(&self, func: &str, n: usize, count: impl Fn(&str) -> usize) -> Option<usize> {
        let (start, end) = fn_line_range(self.source, func);
        let mut seen = 0;
        for (i, l) in self.source.lines().enumerate().take(end).skip(start + 1) {
            seen += count(l);
            if seen >= n {
                return Some(i + 1);
            }
        }
        None
    }

    /// 1-indexed line of the first line in the body of function `func` that matches `pred`.
    fn line_in_fn(&self, func: &str, pred: impl Fn(&str) -> bool) -> Option<usize> {
        let (start, end) = fn_line_range(self.source, func);
        self.source
            .lines()
            .enumerate()
            .take(end)
            .skip(start + 1)
            .find(|(_, l)| pred(l))
            .map(|(i, _)| i + 1)
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

/// The name a `let` line binds (`let x`, `let mut x`), if the line is a `let`.
fn let_of(line: &str) -> Option<&str> {
    let t = line.trim_start().strip_prefix("let ")?;
    let t = t.strip_prefix("mut ").unwrap_or(t).trim_start();
    let end = t
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(t.len());
    Some(&t[..end])
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Byte `i` of `line` starts a whole word (not the tail of `my_name`).
fn word_start(line: &str, i: usize) -> bool {
    line[..i]
        .chars()
        .next_back()
        .is_none_or(|c| !is_ident_char(c))
}

/// How many assignments `NAME = ...` the line holds (not `==`, not a `let`).
fn assigns(line: &str, name: &str) -> usize {
    line.match_indices(name)
        .filter(|&(i, _)| {
            let after = &line[i + name.len()..];
            let rest = after.trim_start();
            word_start(line, i)
                && !after.starts_with(is_ident_char)
                && rest.starts_with('=')
                && !rest.starts_with("==")
                && !line[..i].trim_end().ends_with("let")
                && !line[..i].trim_end().ends_with("mut")
        })
        .count()
}

/// How many calls `callee(` the line holds, as a whole word and not its `fn` declaration.
fn calls(line: &str, callee: &str) -> usize {
    let needle = format!("{callee}(");
    line.match_indices(&needle)
        .filter(|&(i, _)| word_start(line, i) && !line[..i].trim_end().ends_with("fn"))
        .count()
}

/// `let NAME` (without `mut`) at the start of the line, NAME followed by `:`, `=` or a space.
fn is_immutable_let_of(line: &str, name: &str) -> bool {
    line.trim_start()
        .strip_prefix("let ")
        .and_then(|r| r.strip_prefix(name))
        .and_then(|r| r.chars().next())
        .is_some_and(|c| c == ':' || c == '=' || c == ' ')
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
            .filter(|d| d.id == E009_TYPE_ERROR && d.severity == Severity::Error)
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
    fn test_assignment_changing_type_warns() {
        let warns =
            type_warnings("fn main() -> Int {\n    let mut x: Int = 1\n    x = \"a\"\n    0\n}\n");
        assert_eq!(warns.len(), 1, "{warns:?}");
        assert!(warns[0]
            .message
            .contains("assigned a value of type 'String'"));
    }

    #[test]
    fn test_non_bool_while_condition_warns() {
        let warns = type_warnings("fn main() -> Int {\n    while 1 {\n    }\n    0\n}\n");
        assert_eq!(warns.len(), 1, "{warns:?}");
        assert!(warns[0].message.contains("while condition must be Bool"));
    }

    #[test]
    fn test_same_type_assignment_and_comparison_while_do_not_warn() {
        let warns = type_warnings(
            "fn main() -> Int {\n    let mut i: Int = 0\n    while i < 3 {\n        i = i + 1\n    }\n    i = 5\n    i\n}\n",
        );
        assert!(warns.is_empty(), "{warns:?}");
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
