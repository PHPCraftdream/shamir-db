mod builder;
mod ids;
mod module;
mod ops;
mod verifier;

#[cfg(test)]
mod tests;

pub(crate) use builder::{IrBuilder, IrError};
pub(crate) use ids::{BlockId, InstId};
pub(crate) use module::{
    EqShape, Inst, IrFunction, IrLimits, IrModule, ParamProjection, RecordField, TypeNode,
    TypeNodeKind,
};
pub(crate) use ops::{
    cell_tag, inst_result_kind, ArithOp, BlockKind, CastOp, CmpOp, DbOp, FloatOp, InstKind,
    Terminator, ValueKind,
};
pub(crate) use verifier::verify;
