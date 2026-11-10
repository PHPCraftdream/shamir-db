use crate::back::ir::ids::BlockId;
use crate::back::ir::ids::InstId;
use crate::back::ir::module::IrFunction;

/// Wasm-level value kinds; bool and every aggregate/optional value is an i32
/// (bool 0/1, aggregates are 16-byte cell pointers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    I32,
    I64,
    F64,
}

/// Integer arithmetic; the runtime applies the overflow, divide-by-zero and
/// MIN/-1 rules of the design and raises error kind 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

/// f64 arithmetic with IEEE semantics (NaN rules of the design).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// The six numeric casts with their checked ranges enforced by the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastOp {
    ToI64FromI32,
    ToF64FromI32,
    ToF64FromI64,
    ToI32FromI64,
    ToI32FromF64,
    ToI64FromF64,
}

/// Runtime cell tag numbers (mirror `back::runtime::cell::CellTag` values).
pub mod cell_tag {
    pub const NULL: u32 = 1;
    pub const BOOL: u32 = 2;
    pub const I64: u32 = 4;
    pub const STRING: u32 = 6;
    pub const BYTES: u32 = 7;
    pub const LIST: u32 = 8;
    pub const RECORD: u32 = 9;
    pub const DYNAMIC_MAP: u32 = 14;
}

/// Direct database host operation of a stage-2 intrinsic call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DbOp {
    /// `db_get(table, key)`: optional record (missing row is null).
    Get,
    /// `db_insert(table, document)`: the stored record.
    Insert,
    /// `db_query(table, filter)`: list of records (null filter means all).
    Query,
}

/// One typed IR instruction. Operand `InstId`s must live in the same block.
#[derive(Clone, Debug, PartialEq)]
pub enum InstKind {
    ConstI32(i32),
    ConstI64(i64),
    /// Raw IEEE bit pattern of an f64 constant.
    ConstF64(u64),
    /// Static-image offset of a prebuilt 16-byte cell -> I32 address.
    StaticCell(u32),
    LocalGet(u32),
    /// Pops the operand value into the slot.
    LocalSet(u32),
    /// Integer arithmetic; `width` is I32 or I64 only.
    CheckedArith {
        op: ArithOp,
        width: ValueKind,
        left: InstId,
        right: InstId,
    },
    FloatArith {
        op: FloatOp,
        left: InstId,
        right: InstId,
    },
    FloatNeg(InstId),
    /// Comparison; result is I32 0/1. `width` may be F64.
    Compare {
        op: CmpOp,
        width: ValueKind,
        left: InstId,
        right: InstId,
    },
    Cast {
        op: CastOp,
        value: InstId,
    },
    BoolNot(InstId),
    /// I32 -> I64 sign-extend.
    SExtend64(InstId),
    /// I64 -> I32 wrap; used only where values are compiler-guaranteed to fit.
    I32WrapI64(InstId),
    /// I64 raw bits -> F64.
    F64FromBits(InstId),
    /// Operand I64; allocates a zeroed cell with tag I64 and payload = operand
    /// -> I32 cell pointer.
    CellAllocI64(InstId),
    /// Operand I32 (0/1); cell tag BOOL, payload = operand zero-extended.
    CellAllocBool(InstId),
    /// Operand F64; cell tag F64, payload = operand bits.
    CellAllocF64(InstId),
    /// Allocates a cell with tag RECORD, aux = field_count and payload =
    /// `(descriptor_offset as u64) << 32 | region_offset`; -> I32 cell pointer.
    RecordCell {
        region: InstId,
        descriptor: u32,
        field_count: u32,
    },
    /// Allocate one zeroed 16-byte cell with the tag -> I32.
    CellAlloc {
        tag: u32,
    },
    /// Operand i32 count -> allocate `count * 16` zeroed bytes -> I32.
    CellsAlloc(InstId),
    /// -> I64
    CellLoadPayload(InstId),
    /// `value` is I32.
    CellStoreAux {
        cell: InstId,
        value: InstId,
    },
    /// `value` is I64.
    CellStorePayload {
        cell: InstId,
        value: InstId,
    },
    /// Copy tag/aux/payload, `dest <- source`.
    CellCopyTo {
        dest: InstId,
        source: InstId,
    },
    /// Copies `count` 16-byte cells from the `source` region to the `dest`
    /// region through the M1 runtime copy helper; void.
    CellsCopy {
        dest: InstId,
        source: InstId,
        count: InstId,
    },
    /// -> I32 0/1 (tag == NULL).
    CellIsNull(InstId),
    /// Explicit value id for aggregate bindings: re-yields `value` (I32 cell
    /// id) after intervening value-less stores.
    CellPass(InstId),
    /// -> I32 (same pointer); runtime raises kind 2 when null.
    CellUnwrap(InstId),
    /// Bounds-checked against aux; runtime raises kind 2; -> I32.
    CellElemPtr {
        base: InstId,
        index: InstId,
    },
    /// UNCHECKED `base + index * 16`; lowering-only, compiler-proven in range.
    CellSlot {
        base: InstId,
        index: InstId,
    },
    /// -> I32 (aux of string/bytes/list cell).
    CellLen(InstId),
    /// -> I32 (-1/0/1) over string/bytes payloads, generated helper.
    BytesCompare {
        left: InstId,
        right: InstId,
    },
    /// -> I32 0/1, generated equality helper for `eq_shapes[shape]`.
    StructEq {
        shape: u32,
        left: InstId,
        right: InstId,
    },
    /// Stage-2 direct database call: encodes the `arg` cell, calls the host
    /// import for `op` with the `table` string cell, decodes the answer and
    /// projects it through `IrModule::type_nodes[result]` with decode-class
    /// (kind 3) failures -> I32 cell pointer of the typed result.
    DbCall {
        op: DbOp,
        table: InstId,
        arg: InstId,
        result: u32,
    },
    /// Appends one `(alias, entry)` operation to an immutable batch plan and
    /// returns the new plan (kind-2 failure on an invalid or duplicate alias
    /// or a 51st operation) -> I32 cell pointer.
    BatchAppend {
        batch: InstId,
        alias: InstId,
        entry: InstId,
    },
    ResponseAffected {
        response: InstId,
        alias: InstId,
    },
    /// Encodes the request plan, calls the host `db_execute` import and
    /// returns an opaque response cell holding the raw answer bytes -> I32.
    DbExecute {
        request: InstId,
    },
    /// Looks `alias` up in a response and projects its `records` list through
    /// `IrModule::type_nodes[result]` with decode-class (kind 3) failures; a
    /// missing alias reads as null -> I32 cell pointer.
    ResponseGet {
        response: InstId,
        alias: InstId,
        result: u32,
    },
    /// `callee` indexes `IrModule::functions`; args in order.
    Call {
        callee: u32,
        args: Vec<InstId>,
    },
    Drop(InstId),
}

/// Block terminator; every block ends in exactly one.
#[derive(Clone, Debug, PartialEq)]
pub enum Terminator {
    Return(Option<InstId>),
    /// User throw: kind 1, message is a string cell.
    TrapCell(InstId),
    Branch(BlockId),
    BranchIf {
        cond: InstId,
        then: BlockId,
        else_: BlockId,
    },
    Unreachable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Plain,
    /// Loop blocks carry their region id in `IrBlock::region` (Some).
    Loop,
}

/// Value kind produced by one instruction, or `None` when it produces no
/// value. `function` supplies the slot space for `LocalGet`; `functions`
/// supplies callee signatures for `Call`.
pub(crate) fn inst_result_kind(
    kind: &InstKind,
    function: &IrFunction,
    functions: &[IrFunction],
) -> Option<ValueKind> {
    match kind {
        InstKind::ConstI32(_)
        | InstKind::StaticCell(_)
        | InstKind::CellAlloc { .. }
        | InstKind::CellsAlloc(_)
        | InstKind::CellIsNull(_)
        | InstKind::CellPass(_)
        | InstKind::CellUnwrap(_)
        | InstKind::CellElemPtr { .. }
        | InstKind::CellSlot { .. }
        | InstKind::CellLen(_)
        | InstKind::BytesCompare { .. }
        | InstKind::StructEq { .. }
        | InstKind::BoolNot(_)
        | InstKind::I32WrapI64(_)
        | InstKind::CellAllocI64(_)
        | InstKind::CellAllocBool(_)
        | InstKind::CellAllocF64(_)
        | InstKind::RecordCell { .. }
        | InstKind::DbCall { .. }
        | InstKind::BatchAppend { .. }
        | InstKind::ResponseAffected { .. }
        | InstKind::DbExecute { .. }
        | InstKind::ResponseGet { .. }
        | InstKind::Compare { .. } => Some(ValueKind::I32),
        // SExtend64 emits I64ExtendI32S, so it produces an I64.
        InstKind::ConstI64(_) | InstKind::CellLoadPayload(_) | InstKind::SExtend64(_) => {
            Some(ValueKind::I64)
        }
        InstKind::ConstF64(_) | InstKind::FloatNeg(_) | InstKind::F64FromBits(_) => {
            Some(ValueKind::F64)
        }
        InstKind::LocalGet(slot) => local_slot_kind(*slot, function),
        InstKind::CheckedArith { width, .. } => Some(*width),
        InstKind::FloatArith { .. } => Some(ValueKind::F64),
        InstKind::Cast { op, .. } => Some(cast_result(*op)),
        InstKind::Call { callee, .. } => functions.get(*callee as usize).and_then(|f| f.result),
        InstKind::LocalSet(_)
        | InstKind::CellStoreAux { .. }
        | InstKind::CellStorePayload { .. }
        | InstKind::CellCopyTo { .. }
        | InstKind::CellsCopy { .. }
        | InstKind::Drop(_) => None,
    }
}

/// Slot space: parameters first, then locals continuing after the last param.
pub(crate) fn local_slot_kind(slot: u32, function: &IrFunction) -> Option<ValueKind> {
    let params = u32::try_from(function.params.len()).ok()?;
    if slot < params {
        function.params.get(slot as usize).copied()
    } else {
        let local_index = slot.checked_sub(params)?;
        function.locals.get(local_index as usize).copied()
    }
}

/// Appends every operand `InstId` of one instruction to `out`.
pub(crate) fn inst_operands(kind: &InstKind, out: &mut Vec<InstId>) {
    match kind {
        InstKind::CheckedArith { left, right, .. }
        | InstKind::FloatArith { left, right, .. }
        | InstKind::Compare { left, right, .. } => {
            out.push(*left);
            out.push(*right);
        }
        InstKind::FloatNeg(value)
        | InstKind::Cast { value, .. }
        | InstKind::BoolNot(value)
        | InstKind::SExtend64(value)
        | InstKind::I32WrapI64(value)
        | InstKind::F64FromBits(value)
        | InstKind::CellAllocI64(value)
        | InstKind::CellAllocBool(value)
        | InstKind::CellAllocF64(value)
        | InstKind::CellsAlloc(value)
        | InstKind::CellLoadPayload(value)
        | InstKind::CellIsNull(value)
        | InstKind::CellPass(value)
        | InstKind::CellUnwrap(value)
        | InstKind::CellLen(value)
        | InstKind::Drop(value) => out.push(*value),
        InstKind::CellStoreAux { cell, value } | InstKind::CellStorePayload { cell, value } => {
            out.push(*cell);
            out.push(*value);
        }
        InstKind::CellCopyTo { dest, source } => {
            out.push(*dest);
            out.push(*source);
        }
        InstKind::CellsCopy {
            dest,
            source,
            count,
        } => {
            out.push(*dest);
            out.push(*source);
            out.push(*count);
        }
        InstKind::CellElemPtr { base, index } | InstKind::CellSlot { base, index } => {
            out.push(*base);
            out.push(*index);
        }
        InstKind::BytesCompare { left, right } | InstKind::StructEq { left, right, .. } => {
            out.push(*left);
            out.push(*right);
        }
        InstKind::RecordCell { region, .. } => out.push(*region),
        InstKind::DbCall { table, arg, .. } => {
            out.push(*table);
            out.push(*arg);
        }
        InstKind::BatchAppend {
            batch,
            alias,
            entry,
        } => {
            out.push(*batch);
            out.push(*alias);
            out.push(*entry);
        }
        InstKind::DbExecute { request } => out.push(*request),
        InstKind::ResponseAffected { response, alias } => {
            out.push(*response);
            out.push(*alias);
        }
        InstKind::ResponseGet {
            response, alias, ..
        } => {
            out.push(*response);
            out.push(*alias);
        }
        InstKind::Call { args, .. } => out.extend_from_slice(args),
        // LocalSet pops implicitly; its value check uses the preceding inst.
        InstKind::ConstI32(_)
        | InstKind::ConstI64(_)
        | InstKind::ConstF64(_)
        | InstKind::StaticCell(_)
        | InstKind::LocalGet(_)
        | InstKind::LocalSet(_)
        | InstKind::CellAlloc { .. } => {}
    }
}

/// Appends every branch target of one terminator to `out`.
pub(crate) fn terminator_targets(terminator: &Terminator, out: &mut Vec<u32>) {
    match terminator {
        Terminator::Branch(target) => out.push(target.0),
        Terminator::BranchIf { then, else_, .. } => {
            out.push(then.0);
            out.push(else_.0);
        }
        Terminator::Return(_) | Terminator::TrapCell(_) | Terminator::Unreachable => {}
    }
}

fn cast_result(op: CastOp) -> ValueKind {
    match op {
        CastOp::ToI64FromI32 | CastOp::ToI64FromF64 => ValueKind::I64,
        CastOp::ToF64FromI32 | CastOp::ToF64FromI64 => ValueKind::F64,
        CastOp::ToI32FromI64 | CastOp::ToI32FromF64 => ValueKind::I32,
    }
}
