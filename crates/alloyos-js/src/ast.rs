use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// Variable declaration binding kind (`let`, `const`, `var`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    Let,
    Const,
    Var,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,    // !
    Neg,    // -
    Pos,    // +
    Typeof, // typeof
}

/// Increment / Decrement operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOp {
    Inc, // ++
    Dec, // --
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,         // +
    Sub,         // -
    Mul,         // *
    Div,         // /
    Rem,         // %
    EqEq,        // ==
    NotEq,       // !=
    StrictEq,    // ===
    StrictNotEq, // !==
    Lt,          // <
    Le,          // <=
    Gt,          // >
    Ge,          // >=
    Instanceof,  // instanceof
    And,         // &&
    Or,          // ||
}

/// Assignment operators (`=`, `+=`, `-=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    AddAssign,
    SubAssign,
}

/// Left-hand side target of an assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignTarget {
    Ident(String),
    Member { object: Box<Expr>, property: String },
    Index { object: Box<Expr>, index: Box<Expr> },
}

/// Arrow function body kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrowBody {
    Expr(Box<Expr>),
    Block(Vec<Stmt>),
}

/// JavaScript AST Expression node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Undefined,
    Null,
    Bool(bool),
    Number(i64),
    String(String),
    Ident(String),
    ArrayLit(Vec<Expr>),
    ObjectLit(Vec<(String, Expr)>),
    FunctionExpr {
        name: Option<String>,
        params: Vec<String>,
        body: Vec<Stmt>,
    },
    ArrowFunction {
        params: Vec<String>,
        body: ArrowBody,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Update {
        op: UpdateOp,
        target: AssignTarget,
        prefix: bool,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Assign {
        op: AssignOp,
        target: AssignTarget,
        value: Box<Expr>,
    },
    Member {
        object: Box<Expr>,
        property: String,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    New {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
}

/// JavaScript AST Statement node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    VarDecl {
        kind: VarKind,
        name: String,
        init: Option<Expr>,
    },
    ExprStmt(Expr),
    Block(Vec<Stmt>),
    If {
        cond: Expr,
        then_branch: Vec<Stmt>,
        else_branch: Option<Vec<Stmt>>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        update: Option<Expr>,
        body: Vec<Stmt>,
    },
    FunctionDecl {
        name: String,
        params: Vec<String>,
        body: Vec<Stmt>,
    },
    Return(Option<Expr>),
    Throw(Expr),
    Try {
        body: Vec<Stmt>,
        catch_param: Option<String>,
        catch_body: Option<Vec<Stmt>>,
        finally_body: Option<Vec<Stmt>>,
    },
}
