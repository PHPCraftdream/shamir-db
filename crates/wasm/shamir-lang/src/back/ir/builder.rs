use crate::api::Span;
use crate::back::ir::ids::BlockId;
use crate::back::ir::ids::InstId;
use crate::back::ir::module::EqShape;
use crate::back::ir::module::Inst;
use crate::back::ir::module::IrBlock;
use crate::back::ir::module::IrFunction;
use crate::back::ir::module::IrLimits;
use crate::back::ir::module::IrModule;
use crate::back::ir::ops::cell_tag;
use crate::back::ir::ops::BlockKind;
use crate::back::ir::ops::InstKind;
use crate::back::ir::ops::Terminator;
use crate::back::ir::ops::ValueKind;
use crate::front::budget::CompileBudget;
use thiserror::Error;

/// Absolute linear address of the static image (mirrors back::runtime STATIC_START).
pub(crate) const STATIC_BASE: u64 = 1_088;

// Internal-invariant messages (one cause, one message each).
const E_APPEND: &str = "append outside a current block";
const E_NO_FUNCTION: &str = "operation outside a begun function";
const E_NO_BLOCK: &str = "operation without a current block";
const E_BLOCK: &str = "block index overflow";
const E_NODE_ID: &str = "instruction id overflow";
const E_COUNT: &str = "operator count overflow";
const E_LITERAL: &str = "literal size overflow";
const E_IMAGE: &str = "static image offset overflow";
const E_FIELD_COUNT: &str = "record field count overflow";
const E_FIELD_NAME: &str = "record field name overflow";
const E_SHAPE: &str = "shape index overflow";
/// Terminator target of a branch whose target block is not created yet;
/// patched with `patch_pending_branch` before `finish_function`.
const PENDING_BRANCH_TARGET: u32 = u32::MAX;

/// Renders one `EqShape` as a canonical structural key; `#n` child indices
/// mean dedup keys are only stable within one builder run.
fn shape_key(shape: &EqShape) -> String {
    const MAX_DEPTH: u32 = 256;
    const DEEP: &str = "!deep";
    fn render(shape: &EqShape, depth: u32, out: &mut String) {
        if depth > MAX_DEPTH {
            out.push_str(DEEP);
            return;
        }
        match shape {
            EqShape::I32 => out.push_str("I32"),
            EqShape::I64 => out.push_str("I64"),
            EqShape::F64 => out.push_str("F64"),
            EqShape::Bool => out.push_str("Bool"),
            EqShape::String => out.push_str("String"),
            EqShape::Bytes => out.push_str("Bytes"),
            EqShape::Null => out.push_str("Null"),
            EqShape::Optional(child) => {
                out.push_str("Optional[#");
                out.push_str(&child.to_string());
                out.push(']');
            }
            EqShape::List(child) => {
                out.push_str("List[#");
                out.push_str(&child.to_string());
                out.push(']');
            }
            EqShape::Record(children) => {
                out.push_str("Record[");
                for (index, child) in children.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push('#');
                    out.push_str(&child.to_string());
                }
                out.push(']');
            }
        }
    }
    let mut key = String::new();
    render(shape, 0, &mut key);
    key
}
/// Builder failure; `LimitExceeded` always carries the `B001` code.
#[derive(Debug, Error)]
pub enum IrError {
    #[error("limit {code} exceeded: limit {limit}, observed {observed}")]
    LimitExceeded {
        code: &'static str,
        span: Span,
        limit: u64,
        observed: u64,
    },
    #[error("internal invariant: {0}")]
    Internal(&'static str),
}

/// Block under construction; converted to `IrBlock` at `finish_function`.
/// Starts are assigned at `finish_function` (running sum of `inst_count`),
/// so tiling always matches the block append order regardless of the order
/// in which blocks were opened and filled.
struct PendingBlock {
    kind: BlockKind,
    inst_count: u32,
    region: Option<u32>,
    terminator: Option<Terminator>,
}

/// Function under construction.
struct CurrentFunction {
    params: Vec<ValueKind>,
    result: Option<ValueKind>,
    locals: Vec<ValueKind>,
    insts: Vec<Inst>,
    blocks: Vec<PendingBlock>,
    operator_count: u64,
    current: Option<u32>,
}

/// Deterministic typed-IR builder: charges fuel per node, enforces
/// `IrLimits`, and lays out the static image without hash iteration.
pub struct IrBuilder<'b> {
    limits: IrLimits,
    budget: &'b mut CompileBudget,
    functions: Vec<IrFunction>,
    current: Option<CurrentFunction>,
    static_bytes: Vec<u8>,
    eq_shapes: Vec<(String, u32)>,
    shape_values: Vec<EqShape>,
    null_cell: Option<u32>,
    literal_bytes: usize,
    ir_nodes: u64,
}

impl<'b> IrBuilder<'b> {
    pub fn new(limits: IrLimits, budget: &'b mut CompileBudget) -> Self {
        Self {
            limits,
            budget,
            functions: Vec::new(),
            current: None,
            static_bytes: Vec::new(),
            eq_shapes: Vec::new(),
            shape_values: Vec::new(),
            null_cell: None,
            literal_bytes: 0,
            ir_nodes: 0,
        }
    }

    pub fn begin_function(
        &mut self,
        params: Vec<ValueKind>,
        result: Option<ValueKind>,
        locals: Vec<ValueKind>,
    ) -> Result<(), IrError> {
        if self.current.is_some() {
            return Err(IrError::Internal(
                "begin_function inside unfinished function",
            ));
        }
        self.current = Some(CurrentFunction {
            params,
            result,
            locals,
            insts: Vec::new(),
            blocks: Vec::new(),
            operator_count: 0,
            current: None,
        });
        Ok(())
    }

    /// Appends one instruction to the current block, charging 1 fuel and
    /// counting 1 IR node and 1 operator.
    pub fn append(&mut self, kind: InstKind, span: Span) -> Result<InstId, IrError> {
        let limit = self.limits.max_ir_nodes as u64;
        let next_nodes = self
            .ir_nodes
            .checked_add(1)
            .ok_or(IrError::Internal(E_COUNT))?;
        if next_nodes > limit {
            return Err(IrError::LimitExceeded {
                code: "B001",
                span,
                limit,
                observed: next_nodes,
            });
        }
        let operator_limit = self.limits.max_operators_per_function as u64;
        let operator_count = self.operator_count()?;
        if operator_count + 1 > operator_limit {
            return Err(IrError::LimitExceeded {
                code: "B001",
                span,
                limit: operator_limit,
                observed: operator_count + 1,
            });
        }
        self.charge_fuel(span)?;
        let function = self.current.as_mut().ok_or(IrError::Internal(E_APPEND))?;
        let id = u32::try_from(function.insts.len()).map_err(|_| IrError::Internal(E_NODE_ID))?;
        function.insts.push(Inst { kind, span });
        let block = function.current.ok_or(IrError::Internal(E_APPEND))?;
        let pending = function
            .blocks
            .get_mut(block as usize)
            .ok_or(IrError::Internal(E_BLOCK))?;
        pending.inst_count = pending
            .inst_count
            .checked_add(1)
            .ok_or(IrError::Internal(E_BLOCK))?;
        function.operator_count = function
            .operator_count
            .checked_add(1)
            .ok_or(IrError::Internal(E_COUNT))?;
        self.ir_nodes = self
            .ir_nodes
            .checked_add(1)
            .ok_or(IrError::Internal(E_COUNT))?;
        Ok(InstId(id))
    }

    pub fn open_block(&mut self, region: Option<u32>) -> Result<BlockId, IrError> {
        self.open_block_of_kind(BlockKind::Plain, region)
    }

    pub fn open_loop(&mut self, region: u32) -> Result<BlockId, IrError> {
        self.open_block_of_kind(BlockKind::Loop, Some(region))
    }

    /// Switches the append target to an UNterminated block.
    pub fn set_current(&mut self, block: BlockId) -> Result<(), IrError> {
        let function = self
            .current
            .as_mut()
            .ok_or(IrError::Internal(E_NO_FUNCTION))?;
        let pending = function
            .blocks
            .get_mut(block.index())
            .ok_or(IrError::Internal(E_BLOCK))?;
        if pending.terminator.is_some() {
            return Err(IrError::Internal("set_current onto terminated block"));
        }
        function.current = Some(block.0);
        Ok(())
    }

    pub fn current_block(&self) -> Option<BlockId> {
        self.current.as_ref().and_then(|f| f.current).map(BlockId)
    }

    pub fn terminate_return(&mut self, value: Option<InstId>) -> Result<(), IrError> {
        self.terminate(Terminator::Return(value))
    }

    pub fn terminate_trap_cell(&mut self, message: InstId) -> Result<(), IrError> {
        self.terminate(Terminator::TrapCell(message))
    }

    pub fn terminate_branch(&mut self, target: BlockId) -> Result<(), IrError> {
        self.terminate(Terminator::Branch(target))
    }

    /// Terminates the current block with a branch whose target is not known
    /// yet; `patch_pending_branch` names it before the function finishes.
    pub fn terminate_pending_branch(&mut self) -> Result<(), IrError> {
        self.terminate(Terminator::Branch(BlockId(PENDING_BRANCH_TARGET)))
    }

    /// Names the target of one pending branch terminator.
    pub fn patch_pending_branch(&mut self, block: BlockId, target: BlockId) -> Result<(), IrError> {
        let function = self
            .current
            .as_mut()
            .ok_or(IrError::Internal(E_NO_FUNCTION))?;
        let pending = function
            .blocks
            .get_mut(block.index())
            .ok_or(IrError::Internal(E_BLOCK))?;
        match &pending.terminator {
            Some(Terminator::Branch(target)) if target.0 == PENDING_BRANCH_TARGET => {}
            _ => return Err(IrError::Internal("patch on a non-pending branch")),
        }
        pending.terminator = Some(Terminator::Branch(target));
        Ok(())
    }

    pub fn terminate_branch_if(
        &mut self,
        cond: InstId,
        then: BlockId,
        else_: BlockId,
    ) -> Result<(), IrError> {
        self.terminate(Terminator::BranchIf { cond, then, else_ })
    }

    pub fn terminate_unreachable(&mut self) -> Result<(), IrError> {
        self.terminate(Terminator::Unreachable)
    }

    /// Closes the current function; requires no open current block and at
    /// least one (terminated) block.
    pub fn finish_function(&mut self) -> Result<(), IrError> {
        let function = self
            .current
            .take()
            .ok_or(IrError::Internal(E_NO_FUNCTION))?;
        if function.current.is_some() {
            return Err(IrError::Internal("finish_function with open current block"));
        }
        if function.blocks.is_empty() {
            return Err(IrError::Internal("finish_function without blocks"));
        }
        let mut start = 0u32;
        let mut blocks = Vec::with_capacity(function.blocks.len());
        for pending in function.blocks {
            let terminator = match pending.terminator {
                Some(Terminator::Branch(target)) if target.0 == PENDING_BRANCH_TARGET => {
                    return Err(IrError::Internal(
                        "finish_function with an unresolved pending branch",
                    ));
                }
                Some(terminator) => terminator,
                None => return Err(IrError::Internal("finish_function with unterminated block")),
            };
            blocks.push(IrBlock {
                kind: pending.kind,
                start: InstId(start),
                inst_count: pending.inst_count,
                terminator,
                region: pending.region,
            });
            start = start
                .checked_add(pending.inst_count)
                .ok_or(IrError::Internal(E_NODE_ID))?;
        }
        self.functions.push(IrFunction {
            params: function.params,
            result: function.result,
            locals: function.locals,
            blocks,
            insts: function.insts,
            param_projections: Vec::new(),
            result_projection: None,
            param_names: Vec::new(),
        });
        Ok(())
    }

    /// Appends raw bytes then a STRING cell (aux = len, payload = byte
    /// offset); returns the CELL offset in the static image.
    pub fn string_literal(&mut self, bytes: Vec<u8>) -> Result<u32, IrError> {
        self.bytes_with_tag(bytes, cell_tag::STRING)
    }

    /// Same layout as `string_literal` with the BYTES tag.
    pub fn bytes_literal(&mut self, bytes: Vec<u8>) -> Result<u32, IrError> {
        self.bytes_with_tag(bytes, cell_tag::BYTES)
    }

    /// The single cached NULL cell; one instance per module.
    pub fn null_cell(&mut self) -> Result<u32, IrError> {
        if let Some(offset) = self.null_cell {
            return Ok(offset);
        }
        let offset = self.append_cell(cell_tag::NULL, 0, 0)?;
        self.null_cell = Some(offset);
        Ok(offset)
    }

    /// Raw message bytes; only `max_static_bytes` applies.
    pub fn message_bytes(&mut self, bytes: &[u8]) -> Result<u32, IrError> {
        self.reserve_image(bytes.len())?;
        let offset = self.image_len_u32()?;
        self.static_bytes.extend_from_slice(bytes);
        Ok(offset)
    }

    /// Image layout: `u32` count, then per field `u32 name_len` +
    /// `u32 name_offset` of a raw bytes item appended first; returns the
    /// descriptor offset.
    pub fn record_descriptor(&mut self, names: &[String]) -> Result<u32, IrError> {
        let mut offsets = Vec::with_capacity(names.len());
        for name in names {
            let offset = self.message_bytes(name.as_bytes())?;
            offsets.push(offset);
        }
        let count = u32::try_from(names.len()).map_err(|_| IrError::Internal(E_FIELD_COUNT))?;
        let mut descriptor = Vec::with_capacity(4 + names.len() * 8);
        descriptor.extend_from_slice(&count.to_le_bytes());
        for (index, name) in names.iter().enumerate() {
            let len = u32::try_from(name.len()).map_err(|_| IrError::Internal(E_FIELD_NAME))?;
            descriptor.extend_from_slice(&len.to_le_bytes());
            let offset = *offsets.get(index).ok_or(IrError::Internal(E_FIELD_NAME))?;
            // Descriptor name offsets are ABSOLUTE (STATIC_BASE + image offset).
            let absolute = u32::try_from(STATIC_BASE)
                .ok()
                .and_then(|base| base.checked_add(offset))
                .ok_or(IrError::Internal(E_IMAGE))?;
            descriptor.extend_from_slice(&absolute.to_le_bytes());
        }
        self.reserve_image(descriptor.len())?;
        let offset = self.image_len_u32()?;
        self.static_bytes.extend_from_slice(&descriptor);
        // Returned descriptor offsets are ABSOLUTE (STATIC_BASE + image offset).
        u32::try_from(STATIC_BASE)
            .ok()
            .and_then(|base| base.checked_add(offset))
            .ok_or(IrError::Internal(E_IMAGE))
    }

    /// Interns one equality shape (dedup by canonical structural key) and
    /// returns its index. Linear scan keeps the builder deterministic.
    pub fn eq_shape(&mut self, shape: EqShape) -> Result<u32, IrError> {
        let key = shape_key(&shape);
        if let Some(found) = self.eq_shapes.iter().find(|(existing, _)| *existing == key) {
            return Ok(found.1);
        }
        let index =
            u32::try_from(self.shape_values.len()).map_err(|_| IrError::Internal(E_SHAPE))?;
        self.eq_shapes.push((key, index));
        self.shape_values.push(shape);
        Ok(index)
    }

    /// Declares one extra local on the function being built (lowering temps);
    /// returns its slot index (params first, then locals).
    pub fn declare_local(&mut self, kind: ValueKind) -> Result<u32, IrError> {
        let function = self
            .current
            .as_mut()
            .ok_or(IrError::Internal("declare_local outside a function"))?;
        function.locals.push(kind);
        let locals =
            u32::try_from(function.locals.len()).map_err(|_| IrError::Internal(E_COUNT))?;
        let params =
            u32::try_from(function.params.len()).map_err(|_| IrError::Internal(E_COUNT))?;
        let last = locals.checked_sub(1).ok_or(IrError::Internal(E_COUNT))?;
        params.checked_add(last).ok_or(IrError::Internal(E_COUNT))
    }

    /// Assembles the module; requires all begun functions finished.
    pub fn finish(self) -> Result<IrModule, IrError> {
        if self.current.is_some() {
            return Err(IrError::Internal("finish with unfinished function"));
        }
        let shape_values = self.shape_values;
        Ok(IrModule {
            functions: self.functions,
            entry: 0,
            static_bytes: self.static_bytes,
            eq_shapes: shape_values,
        })
    }

    fn open_block_of_kind(
        &mut self,
        kind: BlockKind,
        region: Option<u32>,
    ) -> Result<BlockId, IrError> {
        let function = self
            .current
            .as_mut()
            .ok_or(IrError::Internal(E_NO_FUNCTION))?;
        let index = u32::try_from(function.blocks.len()).map_err(|_| IrError::Internal(E_BLOCK))?;
        function.blocks.push(PendingBlock {
            kind,
            inst_count: 0,
            region,
            terminator: None,
        });
        function.current = Some(index);
        Ok(BlockId(index))
    }

    fn terminate(&mut self, terminator: Terminator) -> Result<(), IrError> {
        let function = self
            .current
            .as_mut()
            .ok_or(IrError::Internal(E_NO_FUNCTION))?;
        let block = function.current.ok_or(IrError::Internal(E_NO_BLOCK))?;
        let pending = function
            .blocks
            .get_mut(block as usize)
            .ok_or(IrError::Internal(E_BLOCK))?;
        if pending.terminator.is_some() {
            return Err(IrError::Internal("terminate on terminated block"));
        }
        pending.terminator = Some(terminator);
        function.current = None;
        Ok(())
    }

    fn operator_count(&self) -> Result<u64, IrError> {
        Ok(self.current.as_ref().map_or(0, |f| f.operator_count))
    }

    fn charge_fuel(&mut self, span: Span) -> Result<(), IrError> {
        self.budget
            .charge(1, span)
            .map_err(|_| IrError::LimitExceeded {
                code: "B001",
                span,
                limit: self.budget.limit(),
                observed: self.budget.spent(),
            })
    }

    fn bytes_with_tag(&mut self, bytes: Vec<u8>, tag: u32) -> Result<u32, IrError> {
        let limit = self.limits.max_single_literal_bytes as u64;
        if bytes.len() as u64 > limit {
            return Err(IrError::LimitExceeded {
                code: "B001",
                span: Span::source_start(),
                limit,
                observed: bytes.len() as u64,
            });
        }
        let total_limit = self.limits.max_total_literal_bytes as u64;
        let literal_total = self.literal_bytes as u64;
        let next_literal = literal_total
            .checked_add(bytes.len() as u64)
            .ok_or(IrError::Internal(E_LITERAL))?;
        if next_literal > total_limit {
            return Err(IrError::LimitExceeded {
                code: "B001",
                span: Span::source_start(),
                limit: total_limit,
                observed: next_literal,
            });
        }
        let len = u32::try_from(bytes.len()).map_err(|_| IrError::Internal(E_LITERAL))?;
        self.reserve_image(
            bytes
                .len()
                .checked_add(16)
                .ok_or(IrError::Internal(E_LITERAL))?,
        )?;
        let data_offset = self.image_len_u32()?;
        self.static_bytes.extend_from_slice(&bytes);
        // Cell payloads hold ABSOLUTE linear addresses (STATIC_BASE + image offset).
        let payload = STATIC_BASE
            .checked_add(u64::from(data_offset))
            .ok_or(IrError::Internal(E_IMAGE))?;
        let cell_offset = self.append_cell(tag, len, payload)?;
        self.literal_bytes = self
            .literal_bytes
            .checked_add(bytes.len())
            .ok_or(IrError::Internal(E_LITERAL))?;
        Ok(cell_offset)
    }

    /// Appends one 16-byte cell (tag, aux, payload, little-endian) and
    /// returns its ABSOLUTE linear address (STATIC_BASE + image offset).
    fn append_cell(&mut self, tag: u32, aux: u32, payload: u64) -> Result<u32, IrError> {
        self.reserve_image(16)?;
        let offset = self.image_len_u32()?;
        self.static_bytes.extend_from_slice(&tag.to_le_bytes());
        self.static_bytes.extend_from_slice(&aux.to_le_bytes());
        self.static_bytes.extend_from_slice(&payload.to_le_bytes());
        // Returned offsets are ABSOLUTE so consumers treat them as addresses.
        u32::try_from(STATIC_BASE)
            .ok()
            .and_then(|base| base.checked_add(offset))
            .ok_or(IrError::Internal(E_IMAGE))
    }

    /// Verifies the image can grow by `bytes` without exceeding
    /// `max_static_bytes` or the u32 offset space.
    fn reserve_image(&mut self, bytes: usize) -> Result<(), IrError> {
        let limit = self.limits.max_static_bytes as u64;
        let observed = (self.static_bytes.len() as u64)
            .checked_add(bytes as u64)
            .ok_or(IrError::Internal(E_IMAGE))?;
        if observed > limit || observed > u32::MAX as u64 {
            return Err(IrError::LimitExceeded {
                code: "B001",
                span: Span::source_start(),
                limit,
                observed,
            });
        }
        Ok(())
    }

    fn image_len_u32(&self) -> Result<u32, IrError> {
        u32::try_from(self.static_bytes.len()).map_err(|_| IrError::Internal(E_IMAGE))
    }
}
