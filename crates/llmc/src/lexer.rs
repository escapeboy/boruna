use crate::error::CompileError;
use logos::Logos;

/// A piece of source text that is not semantically meaningful to the compiler
/// but should be preserved for formatting tools.
#[derive(Debug, Clone, PartialEq)]
pub enum Trivia {
    /// A `// ...` line comment including the `//` prefix but not the trailing newline.
    LineComment(String),
}

/// The full output of lexing: a token stream plus any trivia that appears
/// after the last real token (trailing comments at end of file).
pub struct LexOutput {
    pub tokens: Vec<Token>,
    /// Trivia that appeared after the final token (e.g. a comment on the last line).
    pub trailing_trivia: Vec<Trivia>,
}

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t]+")]
pub enum TokenKind {
    /// Captured line comment — stripped from the token stream and attached as trivia.
    #[regex(r"//[^\n]*", |lex| lex.slice().to_string())]
    LineComment(String),
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
    #[token("intent")]
    Intent,
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
    /// Comments that appeared immediately before this token (on preceding lines
    /// or on the same line before other content). Empty for most tokens.
    pub leading_trivia: Vec<Trivia>,
}

/// Lex `source` and return the full output including trailing trivia.
pub fn lex_full(source: &str) -> Result<LexOutput, CompileError> {
    let mut tokens = Vec::new();
    let mut line = 1usize;
    let mut line_start = 0usize;
    let mut trivia_buf: Vec<Trivia> = Vec::new();

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
            Ok(kind) => match kind {
                TokenKind::LineComment(text) => {
                    trivia_buf.push(Trivia::LineComment(text));
                }
                TokenKind::Newline => {
                    line += 1;
                    line_start = span.end;
                    // Skip consecutive newlines but emit one.
                    // Trivia is NOT attached to Newline tokens; it carries forward
                    // to the next real token so that `// comment\nlet` attaches the
                    // comment to `let`, not to the intermediate newline.
                    if tokens
                        .last()
                        .is_none_or(|t: &Token| t.kind != TokenKind::Newline)
                    {
                        tokens.push(Token {
                            kind: TokenKind::Newline,
                            line,
                            col,
                            leading_trivia: Vec::new(),
                        });
                    }
                }
                other => {
                    tokens.push(Token {
                        kind: other,
                        line,
                        col,
                        leading_trivia: std::mem::take(&mut trivia_buf),
                    });
                }
            },
            Err(_) => {
                return Err(CompileError::Lexer {
                    line,
                    col,
                    msg: format!("unexpected character: {:?}", &source[span.start..span.end]),
                });
            }
        }
    }

    Ok(LexOutput {
        tokens,
        trailing_trivia: trivia_buf,
    })
}

/// Lex `source` and return only the token stream (trivia discarded from the return value
/// but still attached as `leading_trivia` on individual tokens).
pub fn lex(source: &str) -> Result<Vec<Token>, CompileError> {
    lex_full(source).map(|o| o.tokens)
}
