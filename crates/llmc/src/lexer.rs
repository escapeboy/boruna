use crate::error::CompileError;
use logos::Logos;

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t]+")]
#[logos(skip r"//[^\n]*")]
pub enum TokenKind {
    // Keywords
    #[token("fn")]
    Fn,
    #[token("let")]
    Let,
    #[token("mut")]
    Mut,
    #[token("if")]
    If,
    #[token("else")]
    Else,
    #[token("match")]
    Match,
    #[token("return")]
    Return,
    #[token("type")]
    Type,
    #[token("enum")]
    Enum,
    #[token("module")]
    ModuleKw,
    #[token("import")]
    Import,
    #[token("export")]
    Export,
    #[token("true")]
    True,
    #[token("false")]
    False,
    #[token("None")]
    None,
    #[token("Some")]
    Some,
    #[token("Ok")]
    Ok,
    #[token("Err")]
    ErrKw,
    #[token("requires")]
    Requires,
    #[token("ensures")]
    Ensures,
    #[token("spawn")]
    Spawn,
    #[token("send")]
    Send,
    #[token("receive")]
    Receive,
    #[token("emit")]
    Emit,
    #[token("while")]
    While,
    #[token("for")]
    For,
    #[token("in")]
    In,

    // Literals
    #[regex(r"[0-9]+", |lex| lex.slice().parse::<i64>().ok())]
    IntLit(i64),
    #[regex(r"[0-9]+\.[0-9]+", |lex| lex.slice().parse::<f64>().ok())]
    FloatLit(f64),
    #[regex(r#""([^"\\]|\\.)*""#, |lex| {
        let s = lex.slice();
        Some(s[1..s.len()-1].to_string())
    })]
    StringLit(String),

    // Identifiers
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_string(), priority = 1)]
    Ident(String),

    // Operators
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("==")]
    EqEq,
    #[token("!=")]
    BangEq,
    #[token("<")]
    Lt,
    #[token("<=")]
    LtEq,
    #[token(">")]
    Gt,
    #[token(">=")]
    GtEq,
    #[token("&&")]
    AndAnd,
    #[token("||")]
    OrOr,
    #[token("!")]
    Bang,
    #[token("=")]
    Eq,
    #[token("->")]
    Arrow,
    #[token("=>")]
    FatArrow,
    #[token("..")]
    DotDot,
    #[token("++")]
    PlusPlus,

    // Delimiters
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,

    // Punctuation
    #[token(",")]
    Comma,
    #[token(":")]
    Colon,
    #[token(";")]
    Semi,
    #[token(".")]
    Dot,
    #[token("_")]
    Underscore,

    // Newlines (significant for statement separation)
    #[token("\n")]
    Newline,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub col: usize,
}

pub fn lex(source: &str) -> Result<Vec<Token>, CompileError> {
    let mut tokens = Vec::new();
    let mut line = 1usize;
    let mut line_start = 0usize;

    let mut lexer = TokenKind::lexer(source);

    while let Some(result) = lexer.next() {
        let span = lexer.span();

        // Track line/col from source
        let text_before = &source[line_start..span.start];
        for ch in text_before.chars() {
            if ch == '\n' {
                line += 1;
                line_start = span.start;
            }
        }
        let col = span.start - line_start + 1;

        match result {
            Ok(kind) => {
                if kind == TokenKind::Newline {
                    line += 1;
                    line_start = span.end;
                    // Skip consecutive newlines but emit one
                    if tokens
                        .last()
                        .is_none_or(|t: &Token| t.kind != TokenKind::Newline)
                    {
                        tokens.push(Token {
                            kind: TokenKind::Newline,
                            line,
                            col,
                        });
                    }
                } else {
                    tokens.push(Token { kind, line, col });
                }
            }
            Err(_) => {
                return Err(CompileError::Lexer {
                    line,
                    col,
                    msg: format!("unexpected character: {:?}", &source[span.start..span.end]),
                });
            }
        }
    }

    Ok(tokens)
}
