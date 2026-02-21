use crate::ast::*;
use crate::error::CompileError;
use crate::lexer::{Token, TokenKind};

pub fn parse(tokens: Vec<Token>) -> Result<Program, CompileError> {
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&TokenKind> {
        self.skip_newlines_peek()
    }

    fn skip_newlines_peek(&self) -> Option<&TokenKind> {
        let mut i = self.pos;
        while i < self.tokens.len() {
            if self.tokens[i].kind != TokenKind::Newline {
                return Some(&self.tokens[i].kind);
            }
            i += 1;
        }
        None
    }

    fn current_line(&self) -> usize {
        if self.pos < self.tokens.len() {
            self.tokens[self.pos].line
        } else {
            self.tokens.last().map(|t| t.line).unwrap_or(1)
        }
    }

    fn skip_newlines(&mut self) {
        while self.pos < self.tokens.len() && self.tokens[self.pos].kind == TokenKind::Newline {
            self.pos += 1;
        }
    }

    fn advance(&mut self) -> Option<TokenKind> {
        self.skip_newlines();
        if self.pos < self.tokens.len() {
            let tok = self.tokens[self.pos].kind.clone();
            self.pos += 1;
            Some(tok)
        } else {
            None
        }
    }

    fn expect(&mut self, expected: &TokenKind) -> Result<TokenKind, CompileError> {
        self.skip_newlines();
        if self.pos >= self.tokens.len() {
            return Err(self.error(format!("expected {:?}, got EOF", expected)));
        }
        let tok = self.tokens[self.pos].kind.clone();
        if std::mem::discriminant(&tok) == std::mem::discriminant(expected) {
            self.pos += 1;
            Ok(tok)
        } else {
            Err(self.error(format!("expected {:?}, got {:?}", expected, tok)))
        }
    }

    fn expect_ident(&mut self) -> Result<String, CompileError> {
        self.skip_newlines();
        if self.pos >= self.tokens.len() {
            return Err(self.error("expected identifier, got EOF".into()));
        }
        match &self.tokens[self.pos].kind {
            TokenKind::Ident(s) => {
                let s = s.clone();
                self.pos += 1;
                Ok(s)
            }
            other => Err(self.error(format!("expected identifier, got {:?}", other))),
        }
    }

    fn check(&self, kind: &TokenKind) -> bool {
        match self.peek() {
            Some(k) => std::mem::discriminant(k) == std::mem::discriminant(kind),
            None => false,
        }
    }

    fn error(&self, msg: String) -> CompileError {
        CompileError::Parse { line: self.current_line(), msg }
    }

    fn parse_program(&mut self) -> Result<Program, CompileError> {
        self.skip_newlines();
        let module_name = if self.check(&TokenKind::ModuleKw) {
            self.advance();
            let name = self.expect_ident()?;
            Some(name)
        } else {
            None
        };

        let mut items = Vec::new();
        while self.peek().is_some() {
            self.skip_newlines();
            if self.peek().is_none() { break; }
            items.push(self.parse_item()?);
        }

        Ok(Program { module_name, items })
    }

    fn parse_item(&mut self) -> Result<Item, CompileError> {
        let exported = if self.check(&TokenKind::Export) {
            self.advance();
            true
        } else {
            false
        };

        match self.peek() {
            Some(TokenKind::Fn) => {
                let mut fndef = self.parse_fn_def()?;
                fndef.exported = exported;
                Ok(Item::Function(fndef))
            }
            Some(TokenKind::Type) | Some(TokenKind::Enum) => {
                let mut typedef = self.parse_type_def()?;
                typedef.exported = exported;
                Ok(Item::TypeDef(typedef))
            }
            Some(TokenKind::Import) => {
                self.advance();
                let name = self.expect_ident()?;
                Ok(Item::Import(ImportDef { module: name, items: vec![] }))
            }
            other => Err(self.error(format!("expected item, got {:?}", other))),
        }
    }

    fn parse_fn_def(&mut self) -> Result<FnDef, CompileError> {
        self.expect(&TokenKind::Fn)?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LParen)?;

        let mut params = Vec::new();
        while !self.check(&TokenKind::RParen) {
            if !params.is_empty() {
                self.expect(&TokenKind::Comma)?;
            }
            let pname = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let ty = self.parse_type_expr()?;
            params.push(Param { name: pname, ty });
        }
        self.expect(&TokenKind::RParen)?;

        let return_type = if self.check(&TokenKind::Arrow) {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };

        // Parse capability annotations: !{cap1, cap2}
        let capabilities = if self.check(&TokenKind::Bang) {
            self.advance();
            self.expect(&TokenKind::LBrace)?;
            let mut caps = Vec::new();
            while !self.check(&TokenKind::RBrace) {
                if !caps.is_empty() {
                    self.expect(&TokenKind::Comma)?;
                }
                let cap = self.expect_ident()?;
                // Allow dotted names like "fs.read"
                if self.check(&TokenKind::Dot) {
                    self.advance();
                    let sub = self.expect_ident()?;
                    caps.push(format!("{cap}.{sub}"));
                } else {
                    caps.push(cap);
                }
            }
            self.expect(&TokenKind::RBrace)?;
            caps
        } else {
            Vec::new()
        };

        // Parse requires/ensures
        let mut requires = Vec::new();
        let mut ensures = Vec::new();
        while self.check(&TokenKind::Requires) || self.check(&TokenKind::Ensures) {
            if self.check(&TokenKind::Requires) {
                self.advance();
                requires.push(self.parse_expr()?);
            } else {
                self.advance();
                ensures.push(self.parse_expr()?);
            }
        }

        let body = self.parse_block()?;

        Ok(FnDef {
            name,
            params,
            return_type,
            capabilities,
            requires,
            ensures,
            body,
            exported: false,
        })
    }

    fn parse_type_def(&mut self) -> Result<TypeDef, CompileError> {
        match self.peek() {
            Some(TokenKind::Type) => {
                self.advance();
                let name = self.expect_ident()?;
                self.expect(&TokenKind::LBrace)?;
                let mut fields = Vec::new();
                while !self.check(&TokenKind::RBrace) {
                    if !fields.is_empty() {
                        self.expect(&TokenKind::Comma)?;
                        // Allow trailing comma
                        if self.check(&TokenKind::RBrace) { break; }
                    }
                    let fname = self.expect_ident()?;
                    self.expect(&TokenKind::Colon)?;
                    let ftype = self.parse_type_expr()?;
                    fields.push((fname, ftype));
                }
                self.expect(&TokenKind::RBrace)?;
                Ok(TypeDef {
                    name,
                    kind: TypeDefKind::Record(fields),
                    exported: false,
                })
            }
            Some(TokenKind::Enum) => {
                self.advance();
                let name = self.expect_ident()?;
                self.expect(&TokenKind::LBrace)?;
                let mut variants = Vec::new();
                while !self.check(&TokenKind::RBrace) {
                    if !variants.is_empty() {
                        self.expect(&TokenKind::Comma)?;
                        if self.check(&TokenKind::RBrace) { break; }
                    }
                    let vname = self.expect_ident()?;
                    let payload = if self.check(&TokenKind::LParen) {
                        self.advance();
                        let ty = self.parse_type_expr()?;
                        self.expect(&TokenKind::RParen)?;
                        Some(ty)
                    } else {
                        None
                    };
                    variants.push((vname, payload));
                }
                self.expect(&TokenKind::RBrace)?;
                Ok(TypeDef {
                    name,
                    kind: TypeDefKind::Enum(variants),
                    exported: false,
                })
            }
            _ => Err(self.error("expected type or enum".into())),
        }
    }

    fn parse_type_expr(&mut self) -> Result<TypeExpr, CompileError> {
        let name = self.expect_ident()?;
        match name.as_str() {
            "Option" => {
                self.expect(&TokenKind::Lt)?;
                let inner = self.parse_type_expr()?;
                self.expect(&TokenKind::Gt)?;
                Ok(TypeExpr::Option(Box::new(inner)))
            }
            "Result" => {
                self.expect(&TokenKind::Lt)?;
                let ok = self.parse_type_expr()?;
                self.expect(&TokenKind::Comma)?;
                let err = self.parse_type_expr()?;
                self.expect(&TokenKind::Gt)?;
                Ok(TypeExpr::Result(Box::new(ok), Box::new(err)))
            }
            "List" => {
                self.expect(&TokenKind::Lt)?;
                let inner = self.parse_type_expr()?;
                self.expect(&TokenKind::Gt)?;
                Ok(TypeExpr::List(Box::new(inner)))
            }
            _ => Ok(TypeExpr::Named(name)),
        }
    }

    fn parse_block(&mut self) -> Result<Block, CompileError> {
        self.expect(&TokenKind::LBrace)?;
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            self.skip_newlines();
            if self.check(&TokenKind::RBrace) { break; }
            stmts.push(self.parse_stmt()?);
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Block { stmts })
    }

    fn parse_stmt(&mut self) -> Result<Stmt, CompileError> {
        match self.peek() {
            Some(TokenKind::Let) => {
                self.advance();
                let mutable = if self.check(&TokenKind::Mut) {
                    self.advance();
                    true
                } else {
                    false
                };
                let name = self.expect_ident()?;
                let ty = if self.check(&TokenKind::Colon) {
                    self.advance();
                    Some(self.parse_type_expr()?)
                } else {
                    None
                };
                self.expect(&TokenKind::Eq)?;
                let value = self.parse_expr()?;
                Ok(Stmt::Let { name, mutable, ty, value })
            }
            Some(TokenKind::Return) => {
                self.advance();
                let value = if self.check(&TokenKind::RBrace) || self.check(&TokenKind::Newline) || self.peek().is_none() {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                Ok(Stmt::Return(value))
            }
            Some(TokenKind::While) => {
                self.advance();
                let condition = self.parse_expr()?;
                let body = self.parse_block()?;
                Ok(Stmt::While { condition, body })
            }
            _ => {
                let expr = self.parse_expr()?;
                // Check for assignment
                if self.check(&TokenKind::Eq) {
                    self.advance();
                    if let Expr::Ident(name) = expr {
                        let value = self.parse_expr()?;
                        Ok(Stmt::Assign { target: name, value })
                    } else {
                        Err(self.error("invalid assignment target".into()))
                    }
                } else {
                    Ok(Stmt::Expr(expr))
                }
            }
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, CompileError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.parse_and()?;
        while self.check(&TokenKind::OrOr) {
            self.advance();
            let right = self.parse_and()?;
            left = Expr::Binary {
                op: BinOp::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.parse_equality()?;
        while self.check(&TokenKind::AndAnd) {
            self.advance();
            let right = self.parse_equality()?;
            left = Expr::Binary {
                op: BinOp::And,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.parse_comparison()?;
        loop {
            let op = match self.peek() {
                Some(TokenKind::EqEq) => BinOp::Eq,
                Some(TokenKind::BangEq) => BinOp::Neq,
                _ => break,
            };
            self.advance();
            let right = self.parse_comparison()?;
            left = Expr::Binary { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.parse_concat()?;
        loop {
            let op = match self.peek() {
                Some(TokenKind::Lt) => BinOp::Lt,
                Some(TokenKind::LtEq) => BinOp::Lte,
                Some(TokenKind::Gt) => BinOp::Gt,
                Some(TokenKind::GtEq) => BinOp::Gte,
                _ => break,
            };
            self.advance();
            let right = self.parse_concat()?;
            left = Expr::Binary { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_concat(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.parse_additive()?;
        while self.check(&TokenKind::PlusPlus) {
            self.advance();
            let right = self.parse_additive()?;
            left = Expr::Binary {
                op: BinOp::Concat,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Some(TokenKind::Plus) => BinOp::Add,
                Some(TokenKind::Minus) => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplicative()?;
            left = Expr::Binary { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Some(TokenKind::Star) => BinOp::Mul,
                Some(TokenKind::Slash) => BinOp::Div,
                Some(TokenKind::Percent) => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary()?;
            left = Expr::Binary { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, CompileError> {
        match self.peek() {
            Some(TokenKind::Minus) => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary { op: UnaryOp::Neg, expr: Box::new(expr) })
            }
            Some(TokenKind::Bang) => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary { op: UnaryOp::Not, expr: Box::new(expr) })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, CompileError> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.check(&TokenKind::LParen) {
                self.advance();
                let mut args = Vec::new();
                while !self.check(&TokenKind::RParen) {
                    if !args.is_empty() {
                        self.expect(&TokenKind::Comma)?;
                    }
                    args.push(self.parse_expr()?);
                }
                self.expect(&TokenKind::RParen)?;
                expr = Expr::Call { func: Box::new(expr), args };
            } else if self.check(&TokenKind::Dot) {
                self.advance();
                let field = self.expect_ident()?;
                expr = Expr::FieldAccess { object: Box::new(expr), field };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, CompileError> {
        match self.peek().cloned() {
            Some(TokenKind::IntLit(_)) => {
                if let Some(TokenKind::IntLit(n)) = self.advance() {
                    Ok(Expr::IntLit(n))
                } else { unreachable!() }
            }
            Some(TokenKind::FloatLit(_)) => {
                if let Some(TokenKind::FloatLit(n)) = self.advance() {
                    Ok(Expr::FloatLit(n))
                } else { unreachable!() }
            }
            Some(TokenKind::StringLit(_)) => {
                if let Some(TokenKind::StringLit(s)) = self.advance() {
                    Ok(Expr::StringLit(s))
                } else { unreachable!() }
            }
            Some(TokenKind::True) => { self.advance(); Ok(Expr::BoolLit(true)) }
            Some(TokenKind::False) => { self.advance(); Ok(Expr::BoolLit(false)) }
            Some(TokenKind::None) => { self.advance(); Ok(Expr::NoneLit) }
            Some(TokenKind::Some) => {
                self.advance();
                self.expect(&TokenKind::LParen)?;
                let inner = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(Expr::SomeExpr(Box::new(inner)))
            }
            Some(TokenKind::Ok) => {
                self.advance();
                self.expect(&TokenKind::LParen)?;
                let inner = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(Expr::OkExpr(Box::new(inner)))
            }
            Some(TokenKind::ErrKw) => {
                self.advance();
                self.expect(&TokenKind::LParen)?;
                let inner = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(Expr::ErrExpr(Box::new(inner)))
            }
            Some(TokenKind::If) => self.parse_if(),
            Some(TokenKind::Match) => self.parse_match(),
            Some(TokenKind::Spawn) => {
                self.advance();
                let expr = self.parse_primary()?;
                Ok(Expr::Spawn(Box::new(expr)))
            }
            Some(TokenKind::Send) => {
                self.advance();
                let target = self.parse_primary()?;
                let message = self.parse_primary()?;
                Ok(Expr::Send {
                    target: Box::new(target),
                    message: Box::new(message),
                })
            }
            Some(TokenKind::Receive) => {
                self.advance();
                Ok(Expr::Receive)
            }
            Some(TokenKind::Emit) => {
                self.advance();
                let expr = self.parse_primary()?;
                Ok(Expr::Emit(Box::new(expr)))
            }
            Some(TokenKind::LParen) => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(expr)
            }
            Some(TokenKind::LBracket) => {
                self.advance();
                let mut items = Vec::new();
                while !self.check(&TokenKind::RBracket) {
                    if !items.is_empty() {
                        self.expect(&TokenKind::Comma)?;
                        // Allow trailing comma
                        if self.check(&TokenKind::RBracket) { break; }
                    }
                    items.push(self.parse_expr()?);
                }
                self.expect(&TokenKind::RBracket)?;
                Ok(Expr::List(items))
            }
            Some(TokenKind::LBrace) => {
                let block = self.parse_block()?;
                Ok(Expr::Block(block))
            }
            Some(TokenKind::Ident(name)) => {
                self.advance();
                // Check for record literal: TypeName { field: value, ... }
                if name.chars().next().map_or(false, |c| c.is_uppercase())
                    && self.check(&TokenKind::LBrace)
                {
                    self.advance(); // {
                    let mut fields = Vec::new();
                    let mut spread = None;
                    // Check for spread: ..expr as first entry
                    if self.check(&TokenKind::DotDot) {
                        self.advance(); // ..
                        spread = Some(Box::new(self.parse_expr()?));
                        if self.check(&TokenKind::Comma) {
                            self.advance();
                        }
                    }
                    while !self.check(&TokenKind::RBrace) {
                        if !fields.is_empty() {
                            self.expect(&TokenKind::Comma)?;
                            if self.check(&TokenKind::RBrace) { break; }
                        }
                        let fname = self.expect_ident()?;
                        self.expect(&TokenKind::Colon)?;
                        let fval = self.parse_expr()?;
                        fields.push((fname, fval));
                    }
                    self.expect(&TokenKind::RBrace)?;
                    Ok(Expr::Record { type_name: name, fields, spread })
                } else {
                    Ok(Expr::Ident(name))
                }
            }
            other => Err(self.error(format!("expected expression, got {:?}", other))),
        }
    }

    fn parse_if(&mut self) -> Result<Expr, CompileError> {
        self.expect(&TokenKind::If)?;
        let condition = self.parse_expr()?;
        let then_block = self.parse_block()?;
        let else_block = if self.check(&TokenKind::Else) {
            self.advance();
            Some(self.parse_block()?)
        } else {
            None
        };
        Ok(Expr::If {
            condition: Box::new(condition),
            then_block,
            else_block,
        })
    }

    fn parse_match(&mut self) -> Result<Expr, CompileError> {
        self.expect(&TokenKind::Match)?;
        let value = self.parse_expr()?;
        self.expect(&TokenKind::LBrace)?;
        let mut arms = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            self.skip_newlines();
            if self.check(&TokenKind::RBrace) { break; }
            let pattern = self.parse_pattern()?;
            self.expect(&TokenKind::FatArrow)?;
            let body = self.parse_expr()?;
            arms.push(MatchArm { pattern, body });
            // Optional comma between arms
            if self.check(&TokenKind::Comma) {
                self.advance();
            }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Expr::Match { value: Box::new(value), arms })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, CompileError> {
        match self.peek().cloned() {
            Some(TokenKind::Underscore) => { self.advance(); Ok(Pattern::Wildcard) }
            Some(TokenKind::True) => { self.advance(); Ok(Pattern::BoolLit(true)) }
            Some(TokenKind::False) => { self.advance(); Ok(Pattern::BoolLit(false)) }
            Some(TokenKind::None) => { self.advance(); Ok(Pattern::NonePat) }
            Some(TokenKind::Some) => {
                self.advance();
                self.expect(&TokenKind::LParen)?;
                let inner = self.parse_pattern()?;
                self.expect(&TokenKind::RParen)?;
                Ok(Pattern::SomePat(Box::new(inner)))
            }
            Some(TokenKind::Ok) => {
                self.advance();
                self.expect(&TokenKind::LParen)?;
                let inner = self.parse_pattern()?;
                self.expect(&TokenKind::RParen)?;
                Ok(Pattern::OkPat(Box::new(inner)))
            }
            Some(TokenKind::ErrKw) => {
                self.advance();
                self.expect(&TokenKind::LParen)?;
                let inner = self.parse_pattern()?;
                self.expect(&TokenKind::RParen)?;
                Ok(Pattern::ErrPat(Box::new(inner)))
            }
            Some(TokenKind::IntLit(_)) => {
                if let Some(TokenKind::IntLit(n)) = self.advance() {
                    Ok(Pattern::IntLit(n))
                } else { unreachable!() }
            }
            Some(TokenKind::StringLit(_)) => {
                if let Some(TokenKind::StringLit(s)) = self.advance() {
                    Ok(Pattern::StringLit(s))
                } else { unreachable!() }
            }
            Some(TokenKind::Ident(name)) if name.chars().next().map_or(false, |c| c.is_uppercase()) => {
                self.advance();
                let payload = if self.check(&TokenKind::LParen) {
                    self.advance();
                    let inner = self.parse_pattern()?;
                    self.expect(&TokenKind::RParen)?;
                    Some(Box::new(inner))
                } else {
                    None
                };
                Ok(Pattern::EnumVariant(name, payload))
            }
            Some(TokenKind::Ident(_)) => {
                if let Some(TokenKind::Ident(name)) = self.advance() {
                    Ok(Pattern::Ident(name))
                } else { unreachable!() }
            }
            other => Err(self.error(format!("expected pattern, got {:?}", other))),
        }
    }
}
