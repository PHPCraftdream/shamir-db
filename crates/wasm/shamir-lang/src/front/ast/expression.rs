use crate::api::Span;

use super::{ExprId, ValueFieldId};

/// Unary source operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Negate,
    Positive,
}

/// Binary source operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Multiply,
    Divide,
    Remainder,
    Add,
    Subtract,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    And,
    Or,
}

/// Expression node structure. All child references are flat arena IDs.
#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Identifier(String),
    Integer {
        magnitude: u64,
        suffix: Option<crate::front::IntegerSuffix>,
        radix: crate::front::IntegerRadix,
        negative_min_candidate: bool,
    },
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Boolean(bool),
    Null,
    Group(ExprId),
    List(Vec<ExprId>),
    Record(Vec<ValueFieldId>),
    Unary {
        op: UnaryOp,
        operand: ExprId,
    },
    Binary {
        op: BinaryOp,
        left: ExprId,
        right: ExprId,
    },
    Conditional {
        condition: ExprId,
        then_value: ExprId,
        else_value: ExprId,
    },
    Call {
        target: ExprId,
        arguments: Vec<ExprId>,
    },
    Field {
        target: ExprId,
        name: String,
    },
    Index {
        target: ExprId,
        index: ExprId,
    },
}

/// Expression node with exact source range and logical constructed depth.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
    pub depth: u32,
}
