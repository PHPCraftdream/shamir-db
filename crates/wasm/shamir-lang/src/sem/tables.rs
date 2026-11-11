use crate::api::Span;
use crate::front::{EntryKind, FunctionId, StatementId};

use crate::sem::effects::{BuiltinId, EffectSet};
use crate::sem::types::{TypeData, TypeId, TypeInterner};

/// Call target resolution; parameter and local slots share one space,
/// parameters first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedTarget {
    /// Parameter slot index in the combined slot space.
    Parameter(u32),
    /// Local slot index in the combined slot space.
    Local(u32),
    Function(FunctionId),
    Builtin(BuiltinId),
    /// Resolution failure; suppresses follow-on diagnostics.
    Error,
}

/// Resolved callee of a call expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Callee {
    Function(FunctionId),
    Builtin(BuiltinId),
    /// Resolution failure; suppresses follow-on diagnostics.
    Error,
}

/// Resolution result for one call expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedCall {
    /// The resolved callee.
    pub callee: Callee,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReferenceKind {
    String,
    Bool,
    Bytes,
    F64,
    I32,
    I64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReferenceMarker {
    pub(crate) alias: String,
    pub(crate) path: String,
    pub(crate) kind: ReferenceKind,
}

/// Typed module consumed by the lowering milestone.
#[derive(Debug)]
pub struct TypedModule {
    /// Interned semantic types for the whole module.
    pub types: TypeInterner,
    /// One typed function per front function, in front order.
    pub functions: Vec<TypedFunction>,
    /// Topological call order; callees before callers.
    pub call_order: Vec<FunctionId>,
    /// The module entry function.
    pub entry: FunctionId,
    /// Source-level entry kind of the entry function.
    pub entry_kind: EntryKind,
    /// Expression types, parallel to the front expression arena; `None`
    /// where the checker produced no entry (e.g. unreachable arena nodes).
    pub expr_types: Vec<Option<TypeId>>,
    /// Name resolutions, parallel to the front expression arena; `None`
    /// where the checker produced no entry.
    pub resolved: Vec<Option<ResolvedTarget>>,
    /// Call resolutions, parallel to the front expression arena; `None`
    /// where the checker produced no entry.
    pub calls: Vec<Option<ResolvedCall>>,
    /// Checked stored-call metadata indexed by expression id.
    pub(crate) checked_calls: Vec<Option<CheckedCall>>,
    /// Scalar reference marker keyed by intrinsic call expression id.
    pub(crate) reference_markers: Vec<Option<ReferenceMarker>>,
    /// Scalar reference markers keyed by batch sink call expression id.
    pub(crate) reference_sinks: Vec<Vec<ReferenceMarker>>,
    /// Binding slot of each `let`/`for` statement, parallel to the front
    /// statement arena; `None` where the statement binds no slot.
    pub binding_slots: Vec<Option<u32>>,
    /// Source span of the whole module.
    pub span: Span,
}

#[derive(Clone, Debug)]
pub(crate) struct CheckedCall {
    /// Arbitrary compile-time target name.
    pub(crate) name: String,
    /// Canonical expected callable signature bytes.
    pub(crate) signature: Vec<u8>,
}

/// Fully typed function.
#[derive(Debug)]
pub struct TypedFunction {
    /// Arena address of the front function.
    pub id: FunctionId,
    /// Source name.
    pub name: String,
    /// Source-level entry kind, if marked.
    pub kind: Option<EntryKind>,
    /// Parameters in declaration order.
    pub parameters: Vec<TypedParameter>,
    /// Locals in slot order.
    pub locals: Vec<TypedLocal>,
    /// Resolved return type.
    pub return_type: TypeId,
    /// Transitive effects of the body (own effects plus reachable callees).
    pub effects: EffectSet,
    /// Body statement in the front statement arena.
    pub body: StatementId,
    /// Source span of the declaration.
    pub span: Span,
}

/// Typed function parameter.
#[derive(Debug)]
pub struct TypedParameter {
    /// Slot index in the combined slot space.
    pub slot: u32,
    /// Source name.
    pub name: String,
    /// Resolved type.
    pub ty: TypeId,
    /// Source span of the declaration.
    pub span: Span,
}

/// Typed local declaration.
#[derive(Debug)]
pub struct TypedLocal {
    /// Slot index in the combined slot space.
    pub slot: u32,
    /// Source name.
    pub name: String,
    /// Resolved type.
    pub ty: TypeId,
    /// True iff the local is mutable.
    pub mutable: bool,
    /// Source span of the declaration.
    pub span: Span,
}

impl TypedModule {
    /// Looks up one typed function by its front arena address.
    pub fn function(&self, id: FunctionId) -> Option<&TypedFunction> {
        self.functions.get(id.0 as usize)
    }

    /// Copies out one interned type via the module's interner.
    pub fn type_data(&self, id: TypeId) -> Option<TypeData> {
        self.types.data(id)
    }
}
