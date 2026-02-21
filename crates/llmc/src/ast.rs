use serde::{Deserialize, Serialize};

/// A complete program / module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Program {
    pub module_name: Option<String>,
    pub items: Vec<Item>,
}

/// Import definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportDef {
    pub module: String,
    pub items: Vec<String>,
}

/// Top-level items.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Item {
    Function(FnDef),
    TypeDef(TypeDef),
    Import(ImportDef),
    Export(String),
}

/// Function definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FnDef {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub capabilities: Vec<String>,
    pub requires: Vec<Expr>,
    pub ensures: Vec<Expr>,
    pub body: Block,
    pub exported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
}

/// Type expressions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TypeExpr {
    Named(String),
    Option(Box<TypeExpr>),
    Result(Box<TypeExpr>, Box<TypeExpr>),
    List(Box<TypeExpr>),
    Map(Box<TypeExpr>, Box<TypeExpr>),
    Fn(Vec<TypeExpr>, Box<TypeExpr>),
}

/// Type definitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeDef {
    pub name: String,
    pub kind: TypeDefKind,
    pub exported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TypeDefKind {
    Record(Vec<(String, TypeExpr)>),
    Enum(Vec<(String, Option<TypeExpr>)>),
}

/// A block of statements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

/// Statements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Stmt {
    Let {
        name: String,
        mutable: bool,
        ty: Option<TypeExpr>,
        value: Expr,
    },
    Assign {
        target: String,
        value: Expr,
    },
    Expr(Expr),
    Return(Option<Expr>),
    While {
        condition: Expr,
        body: Block,
    },
}

/// Expressions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),
    BoolLit(bool),
    NoneLit,
    Ident(String),
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Call {
        func: Box<Expr>,
        args: Vec<Expr>,
    },
    FieldAccess {
        object: Box<Expr>,
        field: String,
    },
    If {
        condition: Box<Expr>,
        then_block: Block,
        else_block: Option<Block>,
    },
    Match {
        value: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Record {
        type_name: String,
        fields: Vec<(String, Expr)>,
        spread: Option<Box<Expr>>,
    },
    List(Vec<Expr>),
    SomeExpr(Box<Expr>),
    OkExpr(Box<Expr>),
    ErrExpr(Box<Expr>),
    Spawn(Box<Expr>),
    Send {
        target: Box<Expr>,
        message: Box<Expr>,
    },
    Receive,
    Emit(Box<Expr>),
    Block(Block),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Pattern {
    Wildcard,
    Ident(String),
    IntLit(i64),
    StringLit(String),
    BoolLit(bool),
    NonePat,
    SomePat(Box<Pattern>),
    OkPat(Box<Pattern>),
    ErrPat(Box<Pattern>),
    EnumVariant(String, Option<Box<Pattern>>),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
    And,
    Or,
    Concat,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum UnaryOp {
    Neg,
    Not,
}
