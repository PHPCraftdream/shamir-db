use crate::api::Span;

use super::{ParameterId, StatementId, TypeId};

/// Source-level marked entry kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Scalar,
    Validator,
    Procedure,
}

/// Function parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parameter {
    pub name: String,
    pub ty: TypeId,
    pub span: Span,
    pub depth: u32,
}

/// Function declaration; body points into the shared statement arena.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub entry_kind: Option<EntryKind>,
    pub parameters: Vec<ParameterId>,
    pub return_type: TypeId,
    pub body: StatementId,
    pub span: Span,
    pub depth: u32,
}
