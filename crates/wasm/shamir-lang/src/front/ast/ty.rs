use crate::api::Span;

use super::{ExprId, TypeFieldId, TypeId};

/// One primitive or opaque grammar type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimitiveType {
    I32,
    I64,
    F64,
    Bool,
    String,
    Bytes,
    Null,
    Validation,
    Batch,
    Request,
    Response,
}

/// Semantic structure of one flat type node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeKind {
    Primitive(PrimitiveType),
    Optional(TypeId),
    List(TypeId),
    Record(Vec<TypeFieldId>),
}

/// Record member declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeField {
    pub name: String,
    pub ty: TypeId,
    pub span: Span,
    pub depth: u32,
}

/// Type node; descendants are arena IDs, never boxed trees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Type {
    pub kind: TypeKind,
    pub span: Span,
    pub depth: u32,
}

/// Record literal member, kept in the expression arena's flat node.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueField {
    pub name: String,
    pub value: ExprId,
    pub span: Span,
    pub depth: u32,
}
