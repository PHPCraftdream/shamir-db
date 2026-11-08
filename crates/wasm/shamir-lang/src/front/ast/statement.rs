use crate::api::Span;

use super::{ExprId, StatementId, TypeId};

/// Loop source form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForKind {
    List,
    Range,
}

/// Statement node structure with all edges represented by arena IDs.
#[derive(Clone, Debug, PartialEq)]
pub enum StatementKind {
    Block(Vec<StatementId>),
    Let {
        mutable: bool,
        name: String,
        ty: Option<TypeId>,
        value: ExprId,
    },
    Assign {
        target: ExprId,
        value: ExprId,
    },
    If {
        condition: ExprId,
        then_block: StatementId,
        else_branch: Option<StatementId>,
    },
    While {
        condition: ExprId,
        body: StatementId,
    },
    For {
        kind: ForKind,
        name: String,
        start_or_list: ExprId,
        end: Option<ExprId>,
        body: StatementId,
    },
    Return(Option<ExprId>),
    Break,
    Continue,
    Throw(ExprId),
    Expression(ExprId),
}

/// Statement node with exact source range and logical constructed depth.
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
    pub depth: u32,
}
