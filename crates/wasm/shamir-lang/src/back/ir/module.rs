use crate::api::Span;
use crate::back::ir::ids::BlockId;
use crate::back::ir::ids::InstId;
use crate::back::ir::ops::BlockKind;
use crate::back::ir::ops::InstKind;
use crate::back::ir::ops::Terminator;
use crate::back::ir::ops::ValueKind;

/// One instruction with its source span.
#[derive(Clone, Debug, PartialEq)]
pub struct Inst {
    pub kind: InstKind,
    pub span: Span,
}

/// One basic block. Blocks tile `IrFunction::insts` in order:
/// `block[i + 1].start == block[i].start + block[i].inst_count`.
#[derive(Clone, Debug, PartialEq)]
pub struct IrBlock {
    pub kind: BlockKind,
    pub start: InstId,
    pub inst_count: u32,
    pub terminator: Terminator,
    /// Some only for Loop blocks (and the Plain blocks nested in that region).
    pub region: Option<u32>,
}

/// How the compiled call wrapper projects one entry parameter or result
/// between the wire envelope cells and the wasm value space: raw scalars
/// unpack from their cells, `Cell` passes aggregate/optional cell pointers
/// through unchanged (deep projection is deferred to a later milestone).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamProjection {
    /// Raw i32 scalar; unpacked from an I64 cell with a range check.
    I32,
    /// Raw i64 scalar; unpacked from an I64 (or widened I32) cell.
    I64,
    /// Raw f64 scalar; unpacked from an F64 cell.
    F64,
    /// Raw bool scalar; unpacked from a Bool cell.
    Bool,
    /// Aggregate parameter passed as a cell pointer.
    Cell,
    /// Optional aggregate/wire parameter passed as a cell pointer; may be
    /// omitted in named mode and then reads as the null cell.
    OptionalCell,
}

/// One field of a record type node; fields are sorted by name (the same
/// canonical order the lowering uses for record cells).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordField {
    pub name: String,
    pub ty: u32,
}

/// Structural type of one entry parameter, flattened into `IrModule::type_nodes`
/// (children are node indices; no boxed trees).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeNodeKind {
    I32,
    I64,
    F64,
    Bool,
    String,
    Bytes,
    Null,
    Optional(u32),
    List(u32),
    Record(Vec<RecordField>),
}

/// One interned structural type node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeNode {
    pub kind: TypeNodeKind,
}

/// One lowered function; block 0 is the entry block.
#[derive(Clone, Debug)]
pub struct IrFunction {
    pub params: Vec<ValueKind>,
    pub result: Option<ValueKind>,
    /// Slot space continues after `params`.
    pub locals: Vec<ValueKind>,
    pub blocks: Vec<IrBlock>,
    /// Flat instruction arena addressed by `InstId`.
    pub insts: Vec<Inst>,
    /// Entry wrapper projection per parameter (filled by the lowering
    /// driver; empty while lowering).
    pub param_projections: Vec<ParamProjection>,
    /// Entry wrapper result projection; `None` for `-> null`.
    pub result_projection: Option<ParamProjection>,
    /// Declared parameter names in order (filled by the lowering driver;
    /// empty in hand-built test IR).
    pub param_names: Vec<String>,
    /// Structural type node index per parameter (filled by the lowering
    /// driver; empty in hand-built test IR).
    pub param_type_nodes: Vec<u32>,
}

/// Structural equality shape; children are shape indices into
/// `IrModule::eq_shapes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EqShape {
    I32,
    I64,
    F64,
    Bool,
    String,
    Bytes,
    Null,
    Optional(u32),
    List(u32),
    Record(Vec<u32>),
}

/// The whole lowered module.
#[derive(Clone, Debug)]
pub struct IrModule {
    /// User functions in call order (callees before callers).
    pub functions: Vec<IrFunction>,
    /// Index into `functions`.
    pub entry: u32,
    /// The finished static image; u32 static ids are byte offsets into it.
    pub static_bytes: Vec<u8>,
    /// Shape index space for `StructEq`.
    pub eq_shapes: Vec<EqShape>,
    /// Interned structural type nodes for entry projection.
    pub type_nodes: Vec<TypeNode>,
}

/// Lowering bounds, enforced by `IrBuilder` before growth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IrLimits {
    pub max_ir_nodes: usize,
    pub max_ir_nodes_per_function: usize,
    pub max_single_literal_bytes: usize,
    pub max_total_literal_bytes: usize,
    pub max_static_bytes: usize,
}

impl BlockId {
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

impl InstId {
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

impl IrFunction {
    /// Locates one instruction id by block tiling; `None` when out of range.
    pub(crate) fn block_of(&self, inst_id: u32) -> Option<usize> {
        let mut running = 0u32;
        for (block_index, block) in self.blocks.iter().enumerate() {
            let end = running.checked_add(block.inst_count)?;
            if inst_id >= running && inst_id < end {
                return Some(block_index);
            }
            running = end;
        }
        None
    }
}
