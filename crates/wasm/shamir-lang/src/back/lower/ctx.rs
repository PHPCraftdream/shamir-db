use std::collections::BTreeMap;

use crate::api::{CompileLimits, Span};
use crate::back::batch::BatchStatics;
use crate::back::ir::{BlockId, IrBuilder, IrError, ParamProjection, TypeNode};
use crate::front::{ExprId, Module, StatementId};
use crate::sem::{TypeData, TypeId, TypedModule};

/// One enclosing loop: its region id shared by every block opened inside the
/// loop body. Break/continue targets are patched after the loop blocks are
/// created, so they are tracked in `pending_exits`/`pending_continues`.
#[derive(Clone, Copy)]
pub(crate) struct LoopFrame {
    pub(crate) region: u32,
}

/// Per-lowering context: the builder, the call-order function map, the
/// record-descriptor cache, the region counter and the current function's
/// slot tables.
pub(crate) struct LowerCtx<'a, 'b> {
    pub(crate) module: &'a Module,
    pub(crate) typed: &'a TypedModule,
    pub(crate) limits: &'a CompileLimits,
    pub(crate) builder: IrBuilder<'b>,
    /// Front `FunctionId` -> IR function index (call order).
    pub(crate) function_map: BTreeMap<u32, u32>,
    /// Semantic record shape index -> static descriptor offset.
    pub(crate) record_descriptors: BTreeMap<u32, u32>,
    pub(crate) region_counter: u32,
    pub(crate) loop_stack: Vec<LoopFrame>,
    /// Blocks terminated with a pending branch to the innermost loop's exit
    /// (`break`), patched once the exit block exists.
    pub(crate) pending_exits: Vec<BlockId>,
    /// Blocks terminated with a pending branch to the innermost loop's
    /// continue target (`continue`, and the loop-body fallthrough), patched
    /// once that block exists.
    pub(crate) pending_continues: Vec<BlockId>,
    pub(crate) param_types: Vec<TypeId>,
    /// Typed local types in slot order (slot `param_count + index`).
    pub(crate) local_types: Vec<TypeId>,
    pub(crate) local_mutable: Vec<bool>,
    /// `Some` for value-returning functions, `None` for `-> null`.
    pub(crate) result_ty: Option<TypeId>,
    /// Entry wrapper projections per finished function, in call order.
    pub(crate) projections: Vec<(Vec<ParamProjection>, Option<ParamProjection>)>,
    /// Cached absolute validation-record descriptor address, baked at most once.
    pub(crate) validation_descriptor: Option<u32>,
    /// Cached absolute field-error descriptor address, baked at most once.
    pub(crate) field_error_descriptor: Option<u32>,
    /// Cached absolute code-error descriptor address, baked at most once.
    pub(crate) code_error_descriptor: Option<u32>,
    /// Cached absolute error-cap list-cell address, baked at most once.
    pub(crate) error_cap_cell: Option<u32>,
    /// Structural type nodes interned while lowering (database result types);
    /// the driver continues interning the entry parameters into this list.
    pub(crate) type_nodes: Vec<TypeNode>,
    /// Static DTO templates of the typed builder intrinsics, baked on first use.
    pub(crate) batch_statics: Option<BatchStatics>,
}

impl<'a, 'b> LowerCtx<'a, 'b> {
    pub(crate) fn expr_span(&self, id: ExprId) -> Span {
        self.module
            .expression(id)
            .map(|expr| expr.span)
            .unwrap_or_else(Span::source_start)
    }

    pub(crate) fn stmt_span(&self, id: StatementId) -> Span {
        self.module
            .statement(id)
            .map(|stmt| stmt.span)
            .unwrap_or_else(Span::source_start)
    }

    /// The interned static type of one expression; M3 fills every entry of a
    /// clean module, so a miss is an internal invariant.
    pub(crate) fn expr_type(&self, id: ExprId) -> Result<TypeId, IrError> {
        self.typed
            .expr_types
            .get(id.0 as usize)
            .copied()
            .flatten()
            .ok_or(IrError::Internal("lower: missing expression type"))
    }

    pub(crate) fn type_data(&self, id: TypeId) -> Result<TypeData, IrError> {
        self.typed
            .types
            .data(id)
            .ok_or(IrError::Internal("lower: missing interned type"))
    }

    /// The declared type of one slot; parameters first, then locals.
    pub(crate) fn slot_type(&self, slot: u32) -> Result<TypeId, IrError> {
        let params = u32::try_from(self.param_types.len())
            .map_err(|_| IrError::Internal("lower: parameter count overflow"))?;
        if slot < params {
            return self
                .param_types
                .get(slot as usize)
                .copied()
                .ok_or(IrError::Internal("lower: missing parameter type"));
        }
        let index = slot
            .checked_sub(params)
            .ok_or(IrError::Internal("lower: slot underflow"))?;
        self.local_types
            .get(index as usize)
            .copied()
            .ok_or(IrError::Internal("lower: missing local type"))
    }

    /// True iff `slot` is a declared mutable local (never a parameter).
    pub(crate) fn is_mutable_local(&self, slot: u32) -> Result<bool, IrError> {
        let params = u32::try_from(self.param_types.len())
            .map_err(|_| IrError::Internal("lower: parameter count overflow"))?;
        let index = slot.checked_sub(params).ok_or(IrError::Internal(
            "lower: local slot below the parameter range",
        ))?;
        Ok(self
            .local_mutable
            .get(index as usize)
            .copied()
            .unwrap_or(false))
    }

    /// The binding slot of one `let`/`for` statement.
    pub(crate) fn binding_slot(&self, id: StatementId) -> Result<u32, IrError> {
        self.typed
            .binding_slots
            .get(id.0 as usize)
            .copied()
            .flatten()
            .ok_or(IrError::Internal("lower: missing binding slot"))
    }

    /// The region id for a plain block opened right now: the outermost
    /// enclosing loop's region while inside a loop, `None` otherwise.
    /// Sharing one id keeps each region a contiguous, loop-led block range
    /// even for nested loops (verifier rule 5).
    pub(crate) fn block_region(&self) -> Option<u32> {
        self.loop_stack.first().map(|frame| frame.region)
    }

    /// The region id for a loop header: the outermost enclosing loop's
    /// region, or a freshly allocated one for a new outermost loop.
    pub(crate) fn loop_region(&mut self) -> Result<u32, IrError> {
        match self.loop_stack.first() {
            Some(frame) => Ok(frame.region),
            None => self.new_region(),
        }
    }

    fn new_region(&mut self) -> Result<u32, IrError> {
        let id = self.region_counter;
        let next = id
            .checked_add(1)
            .ok_or(IrError::Internal("lower: region id overflow"))?;
        self.region_counter = next;
        Ok(id)
    }
}
