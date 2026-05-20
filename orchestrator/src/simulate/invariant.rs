//! Invariant DSL for `boruna simulate --invariant <expr>`.
//!
//! A purpose-built expression language for compliance checks against a
//! [`WorkflowRunResult`]. Grammar (LL(1)-style, simple recursive descent):
//!
//! ```text
//! expr      := or
//! or        := and ( "||" and )*
//! and       := atom ( "&&" atom )*
//! atom      := comparison | "(" expr ")"
//! comparison := lhs op rhs
//! lhs       := "status"
//!            | "total_duration_ms"
//!            | "completed_runs"     ;; reserved — future
//!            | "step." IDENT ".status"
//!            | "step." IDENT ".duration_ms"
//! op        := "==" | "!=" | "<" | "<=" | ">" | ">="
//! rhs       := STRING-LIT | INT-LIT
//! ```
//!
//! Examples:
//!
//! - `status == "completed"` — workflow ran end-to-end successfully
//! - `total_duration_ms < 1000` — finished within budget
//! - `step.approve.status == "completed" && step.notify.status == "completed"`
//! - `status == "completed" || status == "failed"` — terminated either way
//!
//! Not full Boruna `.ax` because:
//! 1. The compiler has no public expression-type-inference API today.
//! 2. The simulator audience is compliance / operations, who want a
//!    domain-restricted vocabulary rather than the full language surface.
//! 3. Adding a real DSL today is ~150 lines; integrating `.ax` would be
//!    significantly more and would interact with the bytecode evolution
//!    landed for the `debug` builtin.
//!
//! When Boruna grows an `.ax` expression-in-context compile API, this
//! module can be deprecated by an `.ax`-driven invariant evaluator (the
//! `WorkflowRunResult` shape stays stable so callers don't break).

use crate::workflow::{StepResult, StepStatus, WorkflowRunResult, WorkflowStatus};

#[derive(Debug, Clone)]
pub struct Invariant {
    source: String,
    expr: Expr,
}

impl std::fmt::Display for Invariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.source)
    }
}

#[derive(Debug, Clone)]
enum Expr {
    Or(Vec<Expr>),
    And(Vec<Expr>),
    Compare { lhs: Lhs, op: CmpOp, rhs: Rhs },
}

#[derive(Debug, Clone)]
enum Lhs {
    Status,
    TotalDurationMs,
    StepStatus(String),
    StepDurationMs(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone)]
enum Rhs {
    Str(String),
    Int(i64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum InvariantParseError {
    Empty,
    UnexpectedToken {
        at: usize,
        found: String,
        expected: &'static str,
    },
    UnknownLhs(String),
    BadStepRef(String),
    BadInteger(String),
    UnterminatedString,
    BadOperator(String),
    TypeMismatch {
        lhs: &'static str,
        rhs: &'static str,
    },
}

impl std::fmt::Display for InvariantParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use InvariantParseError::*;
        match self {
            Empty => write!(f, "empty invariant expression"),
            UnexpectedToken {
                at,
                found,
                expected,
            } => {
                write!(f, "at position {at}: expected {expected}, found `{found}`")
            }
            UnknownLhs(s) => write!(f, "unknown invariant left-hand side `{s}`"),
            BadStepRef(s) => write!(
                f,
                "step reference `{s}` must be `step.<id>.status` or `step.<id>.duration_ms`"
            ),
            BadInteger(s) => write!(f, "cannot parse integer `{s}`"),
            UnterminatedString => write!(f, "unterminated string literal"),
            BadOperator(s) => write!(f, "unknown operator `{s}`"),
            TypeMismatch { lhs, rhs } => {
                write!(f, "type mismatch: left side is {lhs}, right side is {rhs}")
            }
        }
    }
}

impl std::error::Error for InvariantParseError {}

impl Invariant {
    pub fn parse(src: &str) -> Result<Self, InvariantParseError> {
        let trimmed = src.trim();
        if trimmed.is_empty() {
            return Err(InvariantParseError::Empty);
        }
        let mut parser = Parser::new(trimmed);
        let expr = parser.parse_or()?;
        parser.consume_eof()?;
        Ok(Self {
            source: src.to_string(),
            expr,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn check(&self, result: &WorkflowRunResult) -> bool {
        evaluate(&self.expr, result)
    }
}

fn evaluate(expr: &Expr, result: &WorkflowRunResult) -> bool {
    match expr {
        Expr::Or(parts) => parts.iter().any(|p| evaluate(p, result)),
        Expr::And(parts) => parts.iter().all(|p| evaluate(p, result)),
        Expr::Compare { lhs, op, rhs } => compare(lhs, *op, rhs, result),
    }
}

fn compare(lhs: &Lhs, op: CmpOp, rhs: &Rhs, result: &WorkflowRunResult) -> bool {
    match (lhs, rhs) {
        (Lhs::Status, Rhs::Str(s)) => string_cmp(workflow_status_label(&result.status), s, op),
        (Lhs::TotalDurationMs, Rhs::Int(n)) => int_cmp(result.total_duration_ms as i64, *n, op),
        (Lhs::StepStatus(step_id), Rhs::Str(s)) => {
            let actual = result
                .step_results
                .get(step_id)
                .map(|sr| step_status_label(sr))
                .unwrap_or("missing");
            string_cmp(actual, s, op)
        }
        (Lhs::StepDurationMs(step_id), Rhs::Int(n)) => {
            let d = result
                .step_results
                .get(step_id)
                .map(step_duration_ms)
                .unwrap_or(-1);
            int_cmp(d, *n, op)
        }
        _ => false, // type mismatch silently false at run-time
                    // (parse phase rejects mismatch — this is defense-in-depth)
    }
}

fn string_cmp(actual: &str, expected: &str, op: CmpOp) -> bool {
    match op {
        CmpOp::Eq => actual == expected,
        CmpOp::Ne => actual != expected,
        // Ordering comparisons on strings are lexicographic but rarely
        // useful for the invariant audience; accepted for grammar
        // uniformity.
        CmpOp::Lt => actual < expected,
        CmpOp::Le => actual <= expected,
        CmpOp::Gt => actual > expected,
        CmpOp::Ge => actual >= expected,
    }
}

fn int_cmp(actual: i64, expected: i64, op: CmpOp) -> bool {
    match op {
        CmpOp::Eq => actual == expected,
        CmpOp::Ne => actual != expected,
        CmpOp::Lt => actual < expected,
        CmpOp::Le => actual <= expected,
        CmpOp::Gt => actual > expected,
        CmpOp::Ge => actual >= expected,
    }
}

fn workflow_status_label(s: &WorkflowStatus) -> &'static str {
    match s {
        WorkflowStatus::Running => "running",
        WorkflowStatus::Completed => "completed",
        WorkflowStatus::Failed => "failed",
        WorkflowStatus::Paused => "paused",
    }
}

fn step_status_label(s: &StepResult) -> &'static str {
    match s.status {
        StepStatus::Pending => "pending",
        StepStatus::Running => "running",
        StepStatus::Completed => "completed",
        StepStatus::Failed => "failed",
        StepStatus::Skipped => "skipped",
        StepStatus::AwaitingApproval => "awaiting_approval",
        StepStatus::AwaitingExternalEvent => "awaiting_external_event",
    }
}

fn step_duration_ms(s: &StepResult) -> i64 {
    s.duration_ms as i64
}

// ─── Parser ─────────────────────────────────────────────────────────────

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek_char() {
            if c.is_whitespace() {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn rest(&self) -> &str {
        &self.src[self.pos..]
    }

    fn consume_eof(&mut self) -> Result<(), InvariantParseError> {
        self.skip_whitespace();
        if self.pos < self.src.len() {
            return Err(InvariantParseError::UnexpectedToken {
                at: self.pos,
                found: self.rest().chars().take(16).collect(),
                expected: "end of input",
            });
        }
        Ok(())
    }

    fn parse_or(&mut self) -> Result<Expr, InvariantParseError> {
        let first = self.parse_and()?;
        let mut parts = vec![first];
        loop {
            self.skip_whitespace();
            if self.rest().starts_with("||") {
                self.pos += 2;
                let next = self.parse_and()?;
                parts.push(next);
            } else {
                break;
            }
        }
        Ok(if parts.len() == 1 {
            parts.into_iter().next().unwrap()
        } else {
            Expr::Or(parts)
        })
    }

    fn parse_and(&mut self) -> Result<Expr, InvariantParseError> {
        let first = self.parse_atom()?;
        let mut parts = vec![first];
        loop {
            self.skip_whitespace();
            if self.rest().starts_with("&&") {
                self.pos += 2;
                let next = self.parse_atom()?;
                parts.push(next);
            } else {
                break;
            }
        }
        Ok(if parts.len() == 1 {
            parts.into_iter().next().unwrap()
        } else {
            Expr::And(parts)
        })
    }

    fn parse_atom(&mut self) -> Result<Expr, InvariantParseError> {
        self.skip_whitespace();
        if self.peek_char() == Some('(') {
            self.pos += 1;
            let inner = self.parse_or()?;
            self.skip_whitespace();
            if self.peek_char() != Some(')') {
                return Err(InvariantParseError::UnexpectedToken {
                    at: self.pos,
                    found: self.peek_char().map(|c| c.to_string()).unwrap_or_default(),
                    expected: "closing `)`",
                });
            }
            self.pos += 1;
            return Ok(inner);
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Expr, InvariantParseError> {
        let lhs = self.parse_lhs()?;
        self.skip_whitespace();
        let op = self.parse_op()?;
        self.skip_whitespace();
        let rhs = self.parse_rhs()?;

        // Static type check — string LHS only accepts string RHS, etc.
        let lhs_kind = lhs_type_kind(&lhs);
        let rhs_kind = rhs_type_kind(&rhs);
        if lhs_kind != rhs_kind {
            return Err(InvariantParseError::TypeMismatch {
                lhs: lhs_kind,
                rhs: rhs_kind,
            });
        }

        Ok(Expr::Compare { lhs, op, rhs })
    }

    fn parse_lhs(&mut self) -> Result<Lhs, InvariantParseError> {
        self.skip_whitespace();
        let token = self.consume_ident_path();
        if token.is_empty() {
            return Err(InvariantParseError::UnexpectedToken {
                at: self.pos,
                found: self.peek_char().map(|c| c.to_string()).unwrap_or_default(),
                expected: "left-hand-side identifier",
            });
        }
        match token.as_str() {
            "status" => Ok(Lhs::Status),
            "total_duration_ms" => Ok(Lhs::TotalDurationMs),
            other if other.starts_with("step.") => {
                let parts: Vec<&str> = other.split('.').collect();
                if parts.len() != 3 {
                    return Err(InvariantParseError::BadStepRef(token));
                }
                let step_id = parts[1].to_string();
                match parts[2] {
                    "status" => Ok(Lhs::StepStatus(step_id)),
                    "duration_ms" => Ok(Lhs::StepDurationMs(step_id)),
                    _ => Err(InvariantParseError::BadStepRef(token)),
                }
            }
            _ => Err(InvariantParseError::UnknownLhs(token)),
        }
    }

    /// Consume `[A-Za-z0-9_.]+`.
    fn consume_ident_path(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        self.src[start..self.pos].to_string()
    }

    fn parse_op(&mut self) -> Result<CmpOp, InvariantParseError> {
        // Try two-char operators first.
        for (sym, op) in [
            ("==", CmpOp::Eq),
            ("!=", CmpOp::Ne),
            ("<=", CmpOp::Le),
            (">=", CmpOp::Ge),
        ] {
            if self.rest().starts_with(sym) {
                self.pos += sym.len();
                return Ok(op);
            }
        }
        for (sym, op) in [("<", CmpOp::Lt), (">", CmpOp::Gt)] {
            if self.rest().starts_with(sym) {
                self.pos += sym.len();
                return Ok(op);
            }
        }
        Err(InvariantParseError::BadOperator(
            self.rest().chars().take(4).collect(),
        ))
    }

    fn parse_rhs(&mut self) -> Result<Rhs, InvariantParseError> {
        self.skip_whitespace();
        if self.peek_char() == Some('"') {
            self.parse_string_literal().map(Rhs::Str)
        } else {
            self.parse_int_literal().map(Rhs::Int)
        }
    }

    fn parse_string_literal(&mut self) -> Result<String, InvariantParseError> {
        // Opening quote already at pos
        self.pos += 1;
        let mut s = String::new();
        loop {
            match self.peek_char() {
                Some('"') => {
                    self.pos += 1;
                    return Ok(s);
                }
                Some('\\') => {
                    self.pos += 1;
                    if let Some(esc) = self.peek_char() {
                        s.push(esc);
                        self.pos += esc.len_utf8();
                    }
                }
                Some(c) => {
                    s.push(c);
                    self.pos += c.len_utf8();
                }
                None => return Err(InvariantParseError::UnterminatedString),
            }
        }
    }

    fn parse_int_literal(&mut self) -> Result<i64, InvariantParseError> {
        let start = self.pos;
        if self.peek_char() == Some('-') {
            self.pos += 1;
        }
        while let Some(c) = self.peek_char() {
            if c.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        let token = &self.src[start..self.pos];
        token
            .parse::<i64>()
            .map_err(|_| InvariantParseError::BadInteger(token.to_string()))
    }
}

fn lhs_type_kind(l: &Lhs) -> &'static str {
    match l {
        Lhs::Status | Lhs::StepStatus(_) => "string",
        Lhs::TotalDurationMs | Lhs::StepDurationMs(_) => "int",
    }
}

fn rhs_type_kind(r: &Rhs) -> &'static str {
    match r {
        Rhs::Str(_) => "string",
        Rhs::Int(_) => "int",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::definition::{StepResult, StepStatus, WorkflowStatus};
    use crate::workflow::WorkflowRunResult;
    use std::collections::BTreeMap;

    fn completed_step(id: &str, duration_ms: u64) -> StepResult {
        StepResult {
            step_id: id.into(),
            status: StepStatus::Completed,
            output_hash: Some("h".into()),
            duration_ms,
            capabilities_used: vec![],
            error: None,
            attempt_count: 1,
        }
    }

    fn completed_result() -> WorkflowRunResult {
        let mut sr = BTreeMap::new();
        sr.insert("fetch".to_string(), completed_step("fetch", 50));
        sr.insert("analyze".to_string(), completed_step("analyze", 200));
        WorkflowRunResult {
            run_id: "test".into(),
            workflow_name: "fixture".into(),
            status: WorkflowStatus::Completed,
            step_results: sr,
            total_duration_ms: 250,
        }
    }

    #[test]
    fn parses_status_equals_completed() {
        let inv = Invariant::parse("status == \"completed\"").unwrap();
        assert!(inv.check(&completed_result()));
    }

    #[test]
    fn parses_status_not_equals() {
        let inv = Invariant::parse("status != \"failed\"").unwrap();
        assert!(inv.check(&completed_result()));
    }

    #[test]
    fn parses_total_duration_int_compare() {
        let inv = Invariant::parse("total_duration_ms < 1000").unwrap();
        assert!(inv.check(&completed_result()));
        let inv2 = Invariant::parse("total_duration_ms > 1000").unwrap();
        assert!(!inv2.check(&completed_result()));
    }

    #[test]
    fn parses_step_status() {
        let inv = Invariant::parse("step.fetch.status == \"completed\"").unwrap();
        assert!(inv.check(&completed_result()));
    }

    #[test]
    fn parses_step_duration() {
        let inv = Invariant::parse("step.fetch.duration_ms == 50").unwrap();
        assert!(inv.check(&completed_result()));
    }

    #[test]
    fn parses_missing_step_returns_missing_status() {
        let inv = Invariant::parse("step.nonexistent.status == \"missing\"").unwrap();
        assert!(inv.check(&completed_result()));
    }

    #[test]
    fn parses_and_combinator() {
        let inv = Invariant::parse("status == \"completed\" && step.fetch.status == \"completed\"")
            .unwrap();
        assert!(inv.check(&completed_result()));
    }

    #[test]
    fn parses_or_combinator() {
        let inv = Invariant::parse("status == \"failed\" || status == \"completed\"").unwrap();
        assert!(inv.check(&completed_result()));
    }

    #[test]
    fn parses_parentheses_grouping() {
        let inv = Invariant::parse(
            "(status == \"completed\" || status == \"failed\") && total_duration_ms < 9999",
        )
        .unwrap();
        assert!(inv.check(&completed_result()));
    }

    #[test]
    fn rejects_empty_input() {
        assert_eq!(
            Invariant::parse("").unwrap_err(),
            InvariantParseError::Empty
        );
        assert_eq!(
            Invariant::parse("   ").unwrap_err(),
            InvariantParseError::Empty
        );
    }

    #[test]
    fn rejects_unknown_lhs() {
        let e = Invariant::parse("frobnicate == \"x\"").unwrap_err();
        match e {
            InvariantParseError::UnknownLhs(s) => assert_eq!(s, "frobnicate"),
            other => panic!("expected UnknownLhs, got {other:?}"),
        }
    }

    #[test]
    fn rejects_bad_step_ref() {
        let e = Invariant::parse("step.fetch == \"completed\"").unwrap_err();
        // Two-component step.<id> with no trailing field is invalid.
        // This actually parses as "step.fetch" UnknownLhs since it doesn't
        // match the 3-component form. Either error_kind is acceptable.
        assert!(matches!(
            e,
            InvariantParseError::UnknownLhs(_) | InvariantParseError::BadStepRef(_)
        ));
    }

    #[test]
    fn rejects_type_mismatch_string_lhs_int_rhs() {
        let e = Invariant::parse("status == 5").unwrap_err();
        assert!(matches!(e, InvariantParseError::TypeMismatch { .. }));
    }

    #[test]
    fn rejects_type_mismatch_int_lhs_string_rhs() {
        let e = Invariant::parse("total_duration_ms == \"slow\"").unwrap_err();
        assert!(matches!(e, InvariantParseError::TypeMismatch { .. }));
    }

    #[test]
    fn rejects_unterminated_string() {
        let e = Invariant::parse("status == \"completed").unwrap_err();
        assert_eq!(e, InvariantParseError::UnterminatedString);
    }

    #[test]
    fn rejects_garbage_after_expression() {
        let e = Invariant::parse("status == \"completed\" garbage").unwrap_err();
        assert!(matches!(e, InvariantParseError::UnexpectedToken { .. }));
    }

    #[test]
    fn invariant_preserves_source() {
        let src = "status == \"completed\"";
        let inv = Invariant::parse(src).unwrap();
        assert_eq!(inv.source(), src);
    }

    #[test]
    fn all_six_comparison_ops_parse() {
        for op in &["==", "!=", "<", "<=", ">", ">="] {
            let src = format!("total_duration_ms {op} 100");
            Invariant::parse(&src).unwrap_or_else(|e| panic!("`{src}` failed: {e}"));
        }
    }
}
