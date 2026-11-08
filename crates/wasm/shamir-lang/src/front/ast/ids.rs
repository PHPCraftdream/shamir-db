/// Stable arena address for an expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExprId(pub u32);

/// Stable arena address for a function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FunctionId(pub u32);

/// Stable arena address for a function parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ParameterId(pub u32);

/// Stable arena address for a statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StatementId(pub u32);

/// Stable arena address for a type node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeId(pub u32);

/// Stable arena address for a record type field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeFieldId(pub u32);

/// Stable arena address for a record literal field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ValueFieldId(pub u32);
