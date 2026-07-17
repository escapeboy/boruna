//! `boruna fmt` — canonical pretty-printer for `.ax` source.
//!
//! The formatter parses the source via [`boruna_compiler`], walks the AST,
//! and emits a canonically formatted string with 4-space indentation,
//! trailing commas on multi-line lists/records, and one blank line between
//! top-level declarations.
//!
//! `//` line comments are preserved. The formatter uses `lex_full` trivia
//! to attach comments to their following token, then re-inserts them into
//! the AST-formatted output at the correct indentation level.
//!
//! See `docs/design-boruna-fmt.md` for the full design.
//!
//! # Public API
//!
//! - [`format_source`] — return the canonically formatted string (comments preserved).
//! - [`check_source`] — return `true` if the source is already
//!   canonically formatted (i.e. equal to `format_source(src)`),
//!   `false` otherwise. Used by `boruna fmt --check`.
//!
//! Both return [`FormatError::ParseFailed`] if the input does not parse.

use std::collections::HashMap;
use std::fmt;

use boruna_compiler::ast::{
    BinOp, Block, Expr, FnDef, Item, MatchArm, Param, Pattern, Program, Stmt, TypeDef, TypeDefKind,
    TypeExpr, UnaryOp,
};
use boruna_compiler::error::CompileError;
use boruna_compiler::lexer::{self, Trivia};
use boruna_compiler::parser;

/// Canonical indent width.
const INDENT: &str = "    ";

/// Errors returned by the formatter.
#[derive(Debug, Clone)]
pub enum FormatError {
    /// The input source did not lex/parse.
    ///
    /// `col` is `None` when the underlying error is a parse error
    /// (the parser tracks line only).
    ParseFailed {
        line: usize,
        col: Option<usize>,
        message: String,
    },
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::ParseFailed { line, col, message } => match col {
                Some(c) => write!(f, "parse failed at line {line}, col {c}: {message}"),
                None => write!(f, "parse failed at line {line}: {message}"),
            },
        }
    }
}

impl std::error::Error for FormatError {}

impl From<CompileError> for FormatError {
    fn from(err: CompileError) -> Self {
        match err {
            CompileError::Lexer { line, col, msg } => FormatError::ParseFailed {
                line,
                col: Some(col),
                message: msg,
            },
            CompileError::Parse { line, msg } => FormatError::ParseFailed {
                line,
                col: None,
                message: msg,
            },
            // typeck/codegen errors don't appear here (we don't run them),
            // but if they did we'd surface them as parse-stage failures.
            other => FormatError::ParseFailed {
                line: 0,
                col: None,
                message: other.to_string(),
            },
        }
    }
}

/// Format the given `.ax` source. Returns the canonically formatted
/// string (always ending with a single trailing newline). `//` line
/// comments are preserved and re-inserted at the correct indentation.
pub fn format_source(src: &str) -> Result<String, FormatError> {
    format_source_v2(src)
}

/// Comment-preserving formatter (the canonical implementation).
///
/// Steps:
/// 1. Run the AST-based printer (v1) to get a comment-stripped, canonically
///    formatted string.
/// 2. Run `lex_full` on the original source to collect `leading_trivia`
///    (comments) attached to each real token.
/// 3. Re-insert comments into the formatted output immediately before the
///    line that starts with the token they were attached to, using an
///    occurrence counter to disambiguate identical token texts.
/// 4. Append any trailing trivia (comments after the last token).
pub fn format_source_v2(src: &str) -> Result<String, FormatError> {
    // Step 1: AST-based format (strips comments).
    let tokens_for_parse = lexer::lex(src)?;
    let program = parser::parse(tokens_for_parse)?;
    let mut printer = Printer::new();
    printer.print_program(&program);
    let formatted = printer.finish();

    // Step 2: Collect trivia from the original source.
    let lex_out = lexer::lex_full(src)?;

    // Build an ordered list of (token_text, occurrence_wanted, comments).
    // `occurrence_wanted` is the Nth time this token_text appears in the
    // formatted output (0-based), so we can place the comment before the
    // correct line when the same keyword appears multiple times.
    let mut comment_anchors: Vec<(String, usize, Vec<String>)> = Vec::new();
    {
        let mut occurrence_counter: HashMap<String, usize> = HashMap::new();
        for tok in &lex_out.tokens {
            if !tok.leading_trivia.is_empty() {
                let text = token_kind_text(&tok.kind);
                if text.is_empty() {
                    continue;
                }
                let comments: Vec<String> = tok
                    .leading_trivia
                    .iter()
                    .map(|t| match t {
                        Trivia::LineComment(s) => s.clone(),
                    })
                    .collect();
                let n = *occurrence_counter.get(&text).unwrap_or(&0);
                comment_anchors.push((text.clone(), n, comments));
            }
            // Count every non-newline token so occurrence_counter tracks
            // how many times we've seen each token text in source order.
            let text = token_kind_text(&tok.kind);
            if !text.is_empty() {
                *occurrence_counter.entry(text).or_insert(0) += 1;
            }
        }
    }

    // Fast path: no comments at all.
    let trailing: Vec<String> = lex_out
        .trailing_trivia
        .iter()
        .map(|t| match t {
            Trivia::LineComment(s) => s.clone(),
        })
        .collect();

    if comment_anchors.is_empty() && trailing.is_empty() {
        return Ok(formatted);
    }

    // Step 3: Walk the formatted output line by line.
    // For each anchor (tok_text, occurrence_wanted, comments) we find the
    // `occurrence_wanted`-th line whose trimmed content starts with tok_text
    // as a whole token, then prepend the comment lines (at the same indent).
    //
    // We process anchors in order and consume them greedily as we scan lines.
    let lines: Vec<&str> = formatted.lines().collect();
    // seen_count[tok_text] = lines matching tok_text that we have already passed.
    let mut seen_count: HashMap<String, usize> = HashMap::new();
    // Index into comment_anchors of the next anchor to place.
    let mut anchor_idx = 0;

    let mut result_lines: Vec<String> = Vec::with_capacity(lines.len() + comment_anchors.len() * 2);

    for line in &lines {
        let trimmed = line.trim_start();

        // Before emitting this line, check whether any pending anchors target it.
        // An anchor (tok_text, n, comments) targets this line when:
        //   trimmed starts with tok_text as a whole token
        //   AND seen_count[tok_text] == n  (this is the n-th such line, 0-based)
        while anchor_idx < comment_anchors.len() {
            let (ref tok_text, occurrence_wanted, ref comments) = comment_anchors[anchor_idx];
            let current = *seen_count.get(tok_text).unwrap_or(&0);
            if line_starts_with_whole_token(trimmed, tok_text) && current == occurrence_wanted {
                let indent = &line[..line.len() - trimmed.len()];
                for comment in comments {
                    result_lines.push(format!("{indent}{comment}"));
                }
                anchor_idx += 1;
                // Don't increment seen_count here; we do it after the loop
                // to keep the invariant stable across multiple anchors on the
                // same line (multiple consecutive comments before one token).
            } else {
                break;
            }
        }

        // Track that we've now passed this line's leading token.
        if !trimmed.is_empty() {
            let leading = extract_leading_token(trimmed);
            *seen_count.entry(leading).or_insert(0) += 1;
        }

        result_lines.push(line.to_string());
    }

    // Step 4: append trailing trivia.
    for comment in &trailing {
        result_lines.push(comment.clone());
    }

    let mut out = result_lines.join("\n");
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// Return true if `src` is already canonically formatted, i.e.
/// `format_source(src) == src`. Returns the same parse error as
/// [`format_source`] when the source does not parse.
pub fn check_source(src: &str) -> Result<bool, FormatError> {
    let formatted = format_source(src)?;
    Ok(formatted == src)
}

// ---------------------------------------------------------------------------
// Trivia helpers
// ---------------------------------------------------------------------------

/// Return the canonical text representation of a token kind for matching
/// against formatted-output lines.  Returns an empty string for token kinds
/// that can't reliably anchor a comment (e.g. newlines, operators).
fn token_kind_text(kind: &lexer::TokenKind) -> String {
    use lexer::TokenKind::*;
    match kind {
        // Keywords
        Fn => "fn".into(),
        Let => "let".into(),
        Mut => "mut".into(),
        If => "if".into(),
        Else => "else".into(),
        Match => "match".into(),
        Return => "return".into(),
        Type => "type".into(),
        Enum => "enum".into(),
        ModuleKw => "module".into(),
        Import => "import".into(),
        Export => "export".into(),
        True => "true".into(),
        False => "false".into(),
        None => "None".into(),
        Some => "Some".into(),
        Ok => "Ok".into(),
        ErrKw => "Err".into(),
        Requires => "requires".into(),
        Ensures => "ensures".into(),
        Spawn => "spawn".into(),
        Send => "send".into(),
        Receive => "receive".into(),
        Emit => "emit".into(),
        While => "while".into(),
        For => "for".into(),
        In => "in".into(),
        // Identifiers and literals — use their text directly.
        Ident(s) => s.clone(),
        IntLit(n) => n.to_string(),
        FloatLit(f) => format!("{f}"),
        StringLit(s) => format!("\"{s}\""),
        // Everything else (operators, delimiters, newlines) is not useful as
        // an anchor because it appears too frequently or mid-line.
        _ => String::new(),
    }
}

/// True if `trimmed` (a line with leading whitespace already stripped) starts
/// with `tok` as a whole token, i.e. not as a prefix of a longer identifier.
fn line_starts_with_whole_token(trimmed: &str, tok: &str) -> bool {
    if !trimmed.starts_with(tok) {
        return false;
    }
    // The character after `tok` must be non-alphanumeric / non-underscore,
    // or there is no character after (end of string / whitespace).
    match trimmed[tok.len()..].chars().next() {
        None => true,
        Some(c) => !c.is_alphanumeric() && c != '_',
    }
}

/// Extract the leading token text from an already-trimmed line, for occurrence
/// counting.  Returns the first word (identifier/keyword) or first character.
fn extract_leading_token(trimmed: &str) -> String {
    let end = trimmed
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(trimmed.len());
    if end == 0 {
        trimmed
            .chars()
            .next()
            .map(|c| c.to_string())
            .unwrap_or_default()
    } else {
        trimmed[..end].to_string()
    }
}

// ---------------------------------------------------------------------------
// Printer
// ---------------------------------------------------------------------------

struct Printer {
    out: String,
    indent: usize,
    /// True if the last character pushed was a newline. Used to decide
    /// whether to insert indentation before the next chunk.
    at_line_start: bool,
}

impl Printer {
    fn new() -> Self {
        Printer {
            out: String::new(),
            indent: 0,
            at_line_start: true,
        }
    }

    fn finish(mut self) -> String {
        // Ensure exactly one trailing newline.
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.out
    }

    fn write(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if self.at_line_start {
            for _ in 0..self.indent {
                self.out.push_str(INDENT);
            }
            self.at_line_start = false;
        }
        self.out.push_str(s);
    }

    fn newline(&mut self) {
        self.out.push('\n');
        self.at_line_start = true;
    }

    fn blank_line(&mut self) {
        if !self.at_line_start {
            self.newline();
        }
        self.out.push('\n');
        self.at_line_start = true;
    }

    fn with_indent(&mut self, f: impl FnOnce(&mut Self)) {
        self.indent += 1;
        f(self);
        self.indent -= 1;
    }

    // -----------------------------------------------------------------
    // Program / items
    // -----------------------------------------------------------------

    fn print_program(&mut self, p: &Program) {
        if let Some(name) = &p.module_name {
            self.write("module ");
            self.write(name);
            self.newline();
            if !p.items.is_empty() {
                self.newline();
            }
        }
        for (i, item) in p.items.iter().enumerate() {
            if i > 0 {
                self.blank_line();
            }
            self.print_item(item);
        }
    }

    fn print_item(&mut self, item: &Item) {
        match item {
            Item::Function(f) => self.print_fn(f),
            Item::TypeDef(t) => self.print_type_def(t),
            Item::Import(imp) => {
                self.write("import ");
                self.write(&imp.module);
                self.newline();
            }
            Item::Export(name) => {
                self.write("export ");
                self.write(name);
                self.newline();
            }
        }
    }

    fn print_fn(&mut self, f: &FnDef) {
        if f.exported {
            self.write("export ");
        }
        self.write("fn ");
        self.write(&f.name);
        self.write("(");
        for (i, param) in f.params.iter().enumerate() {
            if i > 0 {
                self.write(", ");
            }
            self.print_param(param);
        }
        self.write(")");
        if let Some(ret) = &f.return_type {
            self.write(" -> ");
            self.print_type(ret);
        }
        if !f.capabilities.is_empty() {
            self.write(" !{");
            for (i, cap) in f.capabilities.iter().enumerate() {
                if i > 0 {
                    self.write(", ");
                }
                self.write(cap);
            }
            self.write("}");
        }
        for r in &f.requires {
            self.newline();
            self.write("requires ");
            self.print_expr(r);
        }
        for e in &f.ensures {
            self.newline();
            self.write("ensures ");
            self.print_expr(e);
        }
        self.write(" ");
        self.print_block(&f.body);
        self.newline();
    }

    fn print_param(&mut self, p: &Param) {
        self.write(&p.name);
        self.write(": ");
        self.print_type(&p.ty);
    }

    fn print_type_def(&mut self, t: &TypeDef) {
        if t.exported {
            self.write("export ");
        }
        match &t.kind {
            TypeDefKind::Record(fields) => {
                self.write("type ");
                self.write(&t.name);
                self.write(" {");
                self.newline();
                self.with_indent(|p| {
                    for (name, ty) in fields {
                        p.write(name);
                        p.write(": ");
                        p.print_type(ty);
                        p.write(",");
                        p.newline();
                    }
                });
                self.write("}");
                self.newline();
            }
            TypeDefKind::Enum(variants) => {
                self.write("enum ");
                self.write(&t.name);
                self.write(" {");
                self.newline();
                self.with_indent(|p| {
                    for (name, payload) in variants {
                        p.write(name);
                        if let Some(ty) = payload {
                            p.write("(");
                            p.print_type(ty);
                            p.write(")");
                        }
                        p.write(",");
                        p.newline();
                    }
                });
                self.write("}");
                self.newline();
            }
        }
    }

    fn print_type(&mut self, t: &TypeExpr) {
        match t {
            TypeExpr::Named(name) => self.write(name),
            TypeExpr::Option(inner) => {
                self.write("Option<");
                self.print_type(inner);
                self.write(">");
            }
            TypeExpr::Result(ok, err) => {
                self.write("Result<");
                self.print_type(ok);
                self.write(", ");
                self.print_type(err);
                self.write(">");
            }
            TypeExpr::List(inner) => {
                self.write("List<");
                self.print_type(inner);
                self.write(">");
            }
            TypeExpr::Map(k, v) => {
                self.write("Map<");
                self.print_type(k);
                self.write(", ");
                self.print_type(v);
                self.write(">");
            }
            TypeExpr::Fn(args, ret) => {
                self.write("Fn(");
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.print_type(a);
                }
                self.write(") -> ");
                self.print_type(ret);
            }
        }
    }

    // -----------------------------------------------------------------
    // Statements / blocks
    // -----------------------------------------------------------------

    fn print_block(&mut self, b: &Block) {
        self.write("{");
        self.newline();
        self.with_indent(|p| {
            for stmt in &b.stmts {
                p.print_stmt(stmt);
                p.newline();
            }
        });
        self.write("}");
    }

    fn print_stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let {
                name,
                mutable,
                ty,
                value,
            } => {
                self.write("let ");
                if *mutable {
                    self.write("mut ");
                }
                self.write(name);
                if let Some(t) = ty {
                    self.write(": ");
                    self.print_type(t);
                }
                self.write(" = ");
                self.print_expr(value);
            }
            Stmt::Assign { target, value } => {
                self.write(target);
                self.write(" = ");
                self.print_expr(value);
            }
            Stmt::Expr(e) => self.print_expr(e),
            Stmt::Return(opt) => {
                self.write("return");
                if let Some(e) = opt {
                    self.write(" ");
                    self.print_expr(e);
                }
            }
            Stmt::While { condition, body } => {
                self.write("while ");
                self.print_expr(condition);
                self.write(" ");
                self.print_block(body);
            }
            Stmt::For { var, iter, body } => {
                self.write("for ");
                self.write(var);
                self.write(" in ");
                self.print_expr(iter);
                self.write(" ");
                self.print_block(body);
            }
        }
    }

    // -----------------------------------------------------------------
    // Expressions
    // -----------------------------------------------------------------

    fn print_expr(&mut self, e: &Expr) {
        self.print_expr_prec(e, 0);
    }

    /// Precedence-aware printer for binary expressions. We only emit
    /// parentheses where necessary based on precedence + associativity.
    fn print_expr_prec(&mut self, e: &Expr, parent_prec: u8) {
        match e {
            Expr::IntLit(n) => self.write(&n.to_string()),
            Expr::FloatLit(n) => {
                let s = format!("{n}");
                // ensure we always render `1.0` not `1`
                if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                    self.write(&format!("{s}.0"));
                } else {
                    self.write(&s);
                }
            }
            Expr::StringLit(s) => {
                self.write("\"");
                self.write(&escape_string(s));
                self.write("\"");
            }
            Expr::BoolLit(b) => self.write(if *b { "true" } else { "false" }),
            Expr::NoneLit => self.write("None"),
            Expr::Ident(name) => self.write(name),
            Expr::Binary { op, left, right } => {
                let prec = bin_prec(*op);
                let needs_parens = prec < parent_prec;
                if needs_parens {
                    self.write("(");
                }
                self.print_expr_prec(left, prec);
                self.write(" ");
                self.write(bin_op_str(*op));
                self.write(" ");
                // right side: use prec+1 for left-assoc operators
                self.print_expr_prec(right, prec + 1);
                if needs_parens {
                    self.write(")");
                }
            }
            Expr::Unary { op, expr } => {
                self.write(unary_op_str(*op));
                self.print_expr_prec(expr, u8::MAX);
            }
            Expr::Call { func, args } => {
                self.print_expr_prec(func, u8::MAX);
                self.write("(");
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.print_expr(a);
                }
                self.write(")");
            }
            Expr::FieldAccess { object, field } => {
                self.print_expr_prec(object, u8::MAX);
                self.write(".");
                self.write(field);
            }
            Expr::If {
                condition,
                then_block,
                else_block,
            } => {
                self.write("if ");
                self.print_expr(condition);
                self.write(" ");
                self.print_block(then_block);
                if let Some(eb) = else_block {
                    self.write(" else ");
                    self.print_block(eb);
                }
            }
            Expr::Match { value, arms } => {
                self.write("match ");
                self.print_expr(value);
                self.write(" {");
                self.newline();
                self.with_indent(|p| {
                    for arm in arms {
                        p.print_match_arm(arm);
                        p.newline();
                    }
                });
                self.write("}");
            }
            Expr::Record {
                type_name,
                fields,
                spread,
            } => {
                self.write(type_name);
                self.write(" {");
                let mut first = true;
                if let Some(sp) = spread {
                    self.write(" ..");
                    self.print_expr(sp);
                    first = false;
                }
                for (name, val) in fields {
                    if !first {
                        self.write(",");
                    } else {
                        first = false;
                    }
                    self.write(" ");
                    self.write(name);
                    self.write(": ");
                    self.print_expr(val);
                }
                if !fields.is_empty() || spread.is_some() {
                    self.write(" ");
                }
                self.write("}");
            }
            Expr::EnumVariant {
                enum_name,
                variant,
                payload,
            } => {
                self.write(enum_name);
                self.write("::");
                self.write(variant);
                if let Some(p) = payload {
                    self.write("(");
                    self.print_expr(p);
                    self.write(")");
                }
            }
            Expr::List(items) => {
                self.write("[");
                for (i, it) in items.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.print_expr(it);
                }
                self.write("]");
            }
            Expr::SomeExpr(inner) => {
                self.write("Some(");
                self.print_expr(inner);
                self.write(")");
            }
            Expr::OkExpr(inner) => {
                self.write("Ok(");
                self.print_expr(inner);
                self.write(")");
            }
            Expr::ErrExpr(inner) => {
                self.write("Err(");
                self.print_expr(inner);
                self.write(")");
            }
            Expr::Spawn(inner) => {
                self.write("spawn ");
                self.print_expr_prec(inner, u8::MAX);
            }
            Expr::Send { target, message } => {
                self.write("send ");
                self.print_expr_prec(target, u8::MAX);
                self.write(" ");
                self.print_expr_prec(message, u8::MAX);
            }
            Expr::Receive => self.write("receive"),
            Expr::Emit(inner) => {
                self.write("emit ");
                self.print_expr_prec(inner, u8::MAX);
            }
            Expr::Block(b) => self.print_block(b),
        }
    }

    fn print_match_arm(&mut self, arm: &MatchArm) {
        self.print_pattern(&arm.pattern);
        self.write(" => ");
        self.print_expr(&arm.body);
        self.write(",");
    }

    fn print_pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Wildcard => self.write("_"),
            Pattern::Ident(name) => self.write(name),
            Pattern::IntLit(n) => self.write(&n.to_string()),
            Pattern::StringLit(s) => {
                self.write("\"");
                self.write(&escape_string(s));
                self.write("\"");
            }
            Pattern::BoolLit(b) => self.write(if *b { "true" } else { "false" }),
            Pattern::NonePat => self.write("None"),
            Pattern::SomePat(inner) => {
                self.write("Some(");
                self.print_pattern(inner);
                self.write(")");
            }
            Pattern::OkPat(inner) => {
                self.write("Ok(");
                self.print_pattern(inner);
                self.write(")");
            }
            Pattern::ErrPat(inner) => {
                self.write("Err(");
                self.print_pattern(inner);
                self.write(")");
            }
            Pattern::EnumVariant(name, payload) => {
                self.write(name);
                if let Some(inner) = payload {
                    self.write("(");
                    self.print_pattern(inner);
                    self.write(")");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Operator helpers
// ---------------------------------------------------------------------------

fn bin_op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::Neq => "!=",
        BinOp::Lt => "<",
        BinOp::Lte => "<=",
        BinOp::Gt => ">",
        BinOp::Gte => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        BinOp::Concat => "++",
    }
}

fn unary_op_str(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "!",
    }
}

/// Precedence levels matching the parser. Higher = tighter binding.
fn bin_prec(op: BinOp) -> u8 {
    match op {
        BinOp::Or => 1,
        BinOp::And => 2,
        BinOp::Eq | BinOp::Neq => 3,
        BinOp::Lt | BinOp::Lte | BinOp::Gt | BinOp::Gte => 4,
        BinOp::Concat => 5,
        BinOp::Add | BinOp::Sub => 6,
        BinOp::Mul | BinOp::Div | BinOp::Mod => 7,
    }
}

fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_hello_world() {
        let src = "fn main() -> Int {\nlet x: Int = 40\nlet y: Int = 2\nx + y\n}\n";
        let out = format_source(src).unwrap();
        let expected =
            "fn main() -> Int {\n    let x: Int = 40\n    let y: Int = 2\n    x + y\n}\n";
        assert_eq!(out, expected);
    }

    #[test]
    fn idempotent_on_simple_program() {
        let src = "fn add(a: Int, b: Int) -> Int { a + b }\nfn main() -> Int { add(1, 2) }\n";
        let once = format_source(src).unwrap();
        let twice = format_source(&once).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn parse_failure_returns_error() {
        let bad = "fn main( -> Int { 0 }";
        let res = format_source(bad);
        assert!(matches!(res, Err(FormatError::ParseFailed { .. })));
    }

    #[test]
    fn check_returns_false_for_unformatted() {
        // single-line, missing indent — parses fine but is not canonical
        let src = "fn main() -> Int {\nlet x: Int = 1\nx\n}\n";
        assert!(!check_source(src).unwrap());
    }

    #[test]
    fn check_returns_true_for_canonical() {
        let src = format_source("fn main() -> Int { 0 }").unwrap();
        assert!(check_source(&src).unwrap());
    }

    #[test]
    fn comments_before_fn_preserved() {
        let src = "// top-level comment\nfn main() -> Int {\n0\n}\n";
        let out = format_source(src).unwrap();
        assert!(
            out.contains("// top-level comment"),
            "comment missing: {out}"
        );
        assert!(out.contains("fn main()"), "fn missing: {out}");
        // Comment must appear before fn in the output.
        let comment_pos = out.find("// top-level comment").unwrap();
        let fn_pos = out.find("fn main()").unwrap();
        assert!(comment_pos < fn_pos, "comment must precede fn: {out}");
    }

    #[test]
    fn comments_before_let_preserved() {
        let src = "fn main() -> Int {\n// before let\nlet x: Int = 1\nx\n}\n";
        let out = format_source(src).unwrap();
        assert!(out.contains("// before let"), "comment missing: {out}");
        // Comment should appear before `let x` and be indented.
        let comment_pos = out.find("// before let").unwrap();
        let let_pos = out.find("let x").unwrap();
        assert!(comment_pos < let_pos, "comment must precede let: {out}");
        // Comment line should be indented (4 spaces).
        let comment_line = out.lines().find(|l| l.contains("// before let")).unwrap();
        assert!(
            comment_line.starts_with("    "),
            "comment should be indented: {comment_line:?}"
        );
    }

    #[test]
    fn comment_at_eof_preserved() {
        let src = "fn main() -> Int {\n0\n}\n// trailing comment\n";
        let out = format_source(src).unwrap();
        assert!(
            out.contains("// trailing comment"),
            "trailing comment missing: {out}"
        );
    }

    #[test]
    fn idempotent_with_comments() {
        let src =
            "// header comment\nfn add(a: Int, b: Int) -> Int {\n// return the sum\na + b\n}\n";
        let once = format_source(src).unwrap();
        let twice = format_source(&once).unwrap();
        assert_eq!(once, twice, "formatting should be idempotent");
    }
}
