//! Scalar instruction emission under the operand-id contract: operands are
//! always read with `local.get` from the instruction's own value local, and
//! every value-producing instruction stores its result with `local.set` into
//! its dense local (`scratch base + 6 + dense id`). `Drop` is a no-op because
//! values live in locals, and `LocalSet(slot)` copies the immediately
//! preceding instruction's local into its slot (the IR contract).

use wasm_encoder::{BlockType, Function, Ieee64, Instruction};

use crate::back::emit::module::EmitError;
use crate::back::ir::{FloatOp, Inst, InstId, InstKind};

use super::arith::{emit_cast, emit_checked_arith, emit_compare};
use super::control::EmitEnv;
use super::{mem32, mem64, raise_static, static_addr};

/// Emits one instruction. `position` is the instruction's arena index (its
/// `InstId`) and selects its result local.
pub(super) fn emit_inst(
    env: &EmitEnv,
    function: &mut Function,
    position: usize,
    inst: &Inst,
) -> Result<(), EmitError> {
    match &inst.kind {
        InstKind::ConstI32(value) => {
            function.instruction(&Instruction::I32Const(*value));
            store_result(env, function, position)?;
        }
        InstKind::ConstI64(value) => {
            function.instruction(&Instruction::I64Const(*value));
            store_result(env, function, position)?;
        }
        InstKind::ConstF64(bits) => {
            function.instruction(&Instruction::F64Const(Ieee64::from(f64::from_bits(*bits))));
            store_result(env, function, position)?;
        }
        InstKind::StaticCell(offset) => {
            let addr = static_addr(*offset)?;
            function.instruction(&Instruction::I32Const(addr as i32));
            store_result(env, function, position)?;
        }
        InstKind::LocalGet(slot) => {
            function.instruction(&Instruction::LocalGet(*slot));
            store_result(env, function, position)?;
        }
        InstKind::LocalSet(slot) => {
            let position_u32 = u32::try_from(position)
                .map_err(|_| EmitError::Internal("instruction id overflow"))?;
            let previous = InstId(position_u32.checked_sub(1).ok_or(EmitError::Internal(
                "LocalSet without a preceding instruction",
            ))?);
            function.instruction(&Instruction::LocalGet(env.local_of(previous)?));
            function.instruction(&Instruction::LocalSet(*slot));
        }
        InstKind::CheckedArith {
            op,
            width,
            left,
            right,
        } => {
            emit_checked_arith(
                env,
                function,
                *op,
                *width,
                env.local_of(*left)?,
                env.local_of(*right)?,
            )?;
            store_result(env, function, position)?;
        }
        InstKind::FloatArith { op, left, right } => {
            function.instruction(&Instruction::LocalGet(env.local_of(*left)?));
            function.instruction(&Instruction::LocalGet(env.local_of(*right)?));
            match op {
                FloatOp::Add => function.instruction(&Instruction::F64Add),
                FloatOp::Sub => function.instruction(&Instruction::F64Sub),
                FloatOp::Mul => function.instruction(&Instruction::F64Mul),
                FloatOp::Div => function.instruction(&Instruction::F64Div),
            };
            store_result(env, function, position)?;
        }
        InstKind::FloatNeg(value) => {
            function.instruction(&Instruction::LocalGet(env.local_of(*value)?));
            function.instruction(&Instruction::F64Neg);
            store_result(env, function, position)?;
        }
        InstKind::Compare {
            op,
            width,
            left,
            right,
        } => {
            emit_compare(
                function,
                *op,
                *width,
                env.local_of(*left)?,
                env.local_of(*right)?,
            );
            store_result(env, function, position)?;
        }
        InstKind::Cast { op, value } => {
            emit_cast(env, function, *op, env.local_of(*value)?)?;
            store_result(env, function, position)?;
        }
        InstKind::BoolNot(value) => {
            function.instruction(&Instruction::LocalGet(env.local_of(*value)?));
            function.instruction(&Instruction::I32Eqz);
            store_result(env, function, position)?;
        }
        InstKind::SExtend64(value) => {
            function.instruction(&Instruction::LocalGet(env.local_of(*value)?));
            function.instruction(&Instruction::I64ExtendI32S);
            store_result(env, function, position)?;
        }
        InstKind::I32WrapI64(value) => {
            function.instruction(&Instruction::LocalGet(env.local_of(*value)?));
            function.instruction(&Instruction::I32WrapI64);
            store_result(env, function, position)?;
        }
        InstKind::F64FromBits(value) => {
            function.instruction(&Instruction::LocalGet(env.local_of(*value)?));
            function.instruction(&Instruction::F64ReinterpretI64);
            store_result(env, function, position)?;
        }
        InstKind::CellAllocI64(value) => {
            alloc_cell(env, function, 4)?;
            store_payload(env, function, *value, Payload::I64)?;
            finish_cell(env, function, position)?;
        }
        InstKind::CellAllocBool(value) => {
            alloc_cell(env, function, 2)?;
            store_payload(env, function, *value, Payload::Bool)?;
            finish_cell(env, function, position)?;
        }
        InstKind::CellAllocF64(value) => {
            alloc_cell(env, function, 5)?;
            store_payload(env, function, *value, Payload::F64)?;
            finish_cell(env, function, position)?;
        }
        InstKind::RecordCell {
            region,
            descriptor,
            field_count,
        } => emit_record_cell(env, function, position, *region, *descriptor, *field_count)?,
        InstKind::CellAlloc { tag } => {
            alloc_cell(env, function, *tag as i32)?;
            finish_cell(env, function, position)?;
        }
        InstKind::CellsAlloc(value) => {
            function.instruction(&Instruction::LocalGet(env.local_of(*value)?));
            function.instruction(&Instruction::I32Const(4));
            function.instruction(&Instruction::I32Shl);
            function.instruction(&Instruction::Call(env.plan.alloc_index));
            store_result(env, function, position)?;
        }
        InstKind::CellsCopy {
            dest,
            source,
            count,
        } => {
            // Runtime copy helper signature `(dest, source, count) -> ()`; it
            // copies `count * 16` bytes (`emit_copy_cells`).
            function.instruction(&Instruction::LocalGet(env.local_of(*dest)?));
            function.instruction(&Instruction::LocalGet(env.local_of(*source)?));
            function.instruction(&Instruction::LocalGet(env.local_of(*count)?));
            function.instruction(&Instruction::Call(env.plan.copy_cells_index));
        }
        InstKind::DbCall {
            op,
            table,
            arg,
            result,
        } => {
            // helper: `(table cell, argument cell) -> decoded root cell`, then
            // the decode-class projection helper of the result node
            let helper = env
                .plan
                .db_helpers
                .iter()
                .find(|(candidate, _)| candidate == op)
                .map(|(_, index)| *index)
                .ok_or(EmitError::Internal("missing database helper"))?;
            let projection = env
                .plan
                .db_projection_indices
                .iter()
                .find(|(node, _)| node == result)
                .map(|(_, index)| *index)
                .ok_or(EmitError::Internal("missing database projection helper"))?;
            function.instruction(&Instruction::LocalGet(env.local_of(*table)?));
            function.instruction(&Instruction::LocalGet(env.local_of(*arg)?));
            function.instruction(&Instruction::Call(helper));
            function.instruction(&Instruction::Call(projection));
            store_result(env, function, position)?;
        }
        InstKind::BatchAppend {
            batch,
            alias,
            entry,
        } => {
            let helper = env
                .plan
                .batch_append_index
                .ok_or(EmitError::Internal("missing batch append helper"))?;
            function.instruction(&Instruction::LocalGet(env.local_of(*batch)?));
            function.instruction(&Instruction::LocalGet(env.local_of(*alias)?));
            function.instruction(&Instruction::LocalGet(env.local_of(*entry)?));
            function.instruction(&Instruction::Call(helper));
            store_result(env, function, position)?;
        }
        InstKind::DbExecute { request } => {
            let helper = env
                .plan
                .db_execute_index
                .ok_or(EmitError::Internal("missing db_execute helper"))?;
            function.instruction(&Instruction::LocalGet(env.local_of(*request)?));
            function.instruction(&Instruction::Call(helper));
            store_result(env, function, position)?;
        }
        InstKind::ResponseAffected { response, alias } => {
            let helper = env
                .plan
                .response_affected_index
                .ok_or(EmitError::Internal("missing response affected helper"))?;
            function.instruction(&Instruction::LocalGet(env.local_of(*response)?));
            function.instruction(&Instruction::LocalGet(env.local_of(*alias)?));
            function.instruction(&Instruction::Call(helper));
            store_result(env, function, position)?;
        }
        InstKind::ResponseGet {
            response,
            alias,
            result,
        } => {
            let helper = env
                .plan
                .response_get_index
                .ok_or(EmitError::Internal("missing response helper"))?;
            let projection = env
                .plan
                .db_projection_indices
                .iter()
                .find(|(node, _)| node == result)
                .map(|(_, index)| *index)
                .ok_or(EmitError::Internal("missing response projection helper"))?;
            function.instruction(&Instruction::LocalGet(env.local_of(*response)?));
            function.instruction(&Instruction::LocalGet(env.local_of(*alias)?));
            function.instruction(&Instruction::Call(helper));
            function.instruction(&Instruction::Call(projection));
            store_result(env, function, position)?;
        }
        InstKind::CellLoadPayload(value) => {
            function.instruction(&Instruction::LocalGet(env.local_of(*value)?));
            function.instruction(&Instruction::I64Load(mem64(8)));
            store_result(env, function, position)?;
        }
        InstKind::CellStoreAux { cell, value } => {
            function.instruction(&Instruction::LocalGet(env.local_of(*cell)?));
            function.instruction(&Instruction::LocalGet(env.local_of(*value)?));
            function.instruction(&Instruction::I32Store(mem32(4)));
        }
        InstKind::CellStorePayload { cell, value } => {
            function.instruction(&Instruction::LocalGet(env.local_of(*cell)?));
            function.instruction(&Instruction::LocalGet(env.local_of(*value)?));
            function.instruction(&Instruction::I64Store(mem64(8)));
        }
        InstKind::CellCopyTo { dest, source } => emit_cell_copy(env, function, *dest, *source)?,
        // Explicit value id for aggregate bindings: re-yield the cell operand.
        InstKind::CellPass(value) => {
            function.instruction(&Instruction::LocalGet(env.local_of(*value)?));
            store_result(env, function, position)?;
        }
        InstKind::CellIsNull(value) => {
            function.instruction(&Instruction::LocalGet(env.local_of(*value)?));
            function.instruction(&Instruction::I32Load(mem32(0)));
            function.instruction(&Instruction::I32Const(1));
            function.instruction(&Instruction::I32Eq);
            store_result(env, function, position)?;
        }
        InstKind::CellUnwrap(value) => {
            let s0 = env.s0();
            function.instruction(&Instruction::LocalGet(env.local_of(*value)?));
            function.instruction(&Instruction::LocalSet(s0));
            function.instruction(&Instruction::LocalGet(s0));
            function.instruction(&Instruction::I32Load(mem32(0)));
            function.instruction(&Instruction::I32Const(1));
            function.instruction(&Instruction::I32Eq);
            function.instruction(&Instruction::If(BlockType::Empty));
            raise_static(env.plan, function, 2, env.ctx.unwrap_msg_addr, b"unwrap")?;
            function.instruction(&Instruction::End);
            function.instruction(&Instruction::LocalGet(s0));
            store_result(env, function, position)?;
        }
        InstKind::CellElemPtr { base, index } => {
            emit_cell_elem_ptr(env, function, position, *base, *index)?;
        }
        InstKind::CellSlot { base, index } => {
            // unchecked region address (lowering proves the slot in range)
            function.instruction(&Instruction::LocalGet(env.local_of(*base)?));
            function.instruction(&Instruction::LocalGet(env.local_of(*index)?));
            function.instruction(&Instruction::I32Const(4));
            function.instruction(&Instruction::I32Shl);
            function.instruction(&Instruction::I32Add);
            store_result(env, function, position)?;
        }
        InstKind::CellLen(value) => {
            function.instruction(&Instruction::LocalGet(env.local_of(*value)?));
            function.instruction(&Instruction::I32Load(mem32(4)));
            store_result(env, function, position)?;
        }
        InstKind::BytesCompare { left, right } => {
            function.instruction(&Instruction::LocalGet(env.local_of(*left)?));
            function.instruction(&Instruction::LocalGet(env.local_of(*right)?));
            function.instruction(&Instruction::Call(env.plan.eq_base));
            store_result(env, function, position)?;
        }
        InstKind::StructEq { shape, left, right } => {
            function.instruction(&Instruction::LocalGet(env.local_of(*left)?));
            function.instruction(&Instruction::LocalGet(env.local_of(*right)?));
            let offset = 1u32
                .checked_add(*shape)
                .ok_or(EmitError::Internal("shape index overflow"))?;
            let index = env
                .plan
                .eq_base
                .checked_add(offset)
                .ok_or(EmitError::Internal("function index overflow"))?;
            function.instruction(&Instruction::Call(index));
            store_result(env, function, position)?;
        }
        InstKind::Call { callee, args } => {
            for arg in args {
                function.instruction(&Instruction::LocalGet(env.local_of(*arg)?));
            }
            let index = env
                .plan
                .users_base
                .checked_add(*callee)
                .ok_or(EmitError::Internal("function index overflow"))?;
            function.instruction(&Instruction::Call(index));
            store_result(env, function, position)?;
        }
        // Values live in locals: dropping one is free.
        InstKind::Drop(_) => {}
    }
    Ok(())
}

/// Stores the result of a value-producing instruction into its dense local
/// (no-op for non-value instructions).
fn store_result(env: &EmitEnv, function: &mut Function, position: usize) -> Result<(), EmitError> {
    if let Some(local) = env.result_local(position) {
        env.note_local(local);
        function.instruction(&Instruction::LocalSet(local));
    }
    Ok(())
}

enum Payload {
    I64,
    /// i32 0/1 zero-extended into the 8-byte payload.
    Bool,
    /// Raw f64 bits (the operand already carries the bit pattern).
    F64,
}

/// `alloc(16)` into s0, then the tag store (finding 4: address before payload).
fn alloc_cell(env: &EmitEnv, function: &mut Function, tag: i32) -> Result<(), EmitError> {
    let s0 = env.s0();
    function.instruction(&Instruction::I32Const(16));
    function.instruction(&Instruction::Call(env.plan.alloc_index));
    function.instruction(&Instruction::LocalSet(s0));
    function.instruction(&Instruction::LocalGet(s0));
    function.instruction(&Instruction::I32Const(tag));
    function.instruction(&Instruction::I32Store(mem32(0)));
    Ok(())
}

/// Payload store into the freshly allocated cell held in s0.
fn store_payload(
    env: &EmitEnv,
    function: &mut Function,
    value: InstId,
    payload: Payload,
) -> Result<(), EmitError> {
    let s0 = env.s0();
    function.instruction(&Instruction::LocalGet(s0));
    function.instruction(&Instruction::LocalGet(env.local_of(value)?));
    match payload {
        Payload::I64 => {
            function.instruction(&Instruction::I64Store(mem64(8)));
        }
        Payload::Bool => {
            function.instruction(&Instruction::I64ExtendI32U);
            function.instruction(&Instruction::I64Store(mem64(8)));
        }
        Payload::F64 => {
            function.instruction(&Instruction::I64ReinterpretF64);
            function.instruction(&Instruction::I64Store(mem64(8)));
        }
    }
    Ok(())
}

/// Pushes the cell pointer from s0 and stores it into the result local.
fn finish_cell(env: &EmitEnv, function: &mut Function, position: usize) -> Result<(), EmitError> {
    function.instruction(&Instruction::LocalGet(env.s0()));
    store_result(env, function, position)
}

/// Record cell: tag 9, aux = field count, payload =
/// `(descriptor << 32) | region` (finding 4).
fn emit_record_cell(
    env: &EmitEnv,
    function: &mut Function,
    position: usize,
    region: InstId,
    descriptor: u32,
    field_count: u32,
) -> Result<(), EmitError> {
    let s0 = env.s0();
    alloc_cell(env, function, 9)?;
    function.instruction(&Instruction::LocalGet(s0));
    let count =
        i32::try_from(field_count).map_err(|_| EmitError::Internal("field count overflow"))?;
    function.instruction(&Instruction::I32Const(count));
    function.instruction(&Instruction::I32Store(mem32(4)));
    function.instruction(&Instruction::LocalGet(s0));
    function.instruction(&Instruction::LocalGet(env.local_of(region)?));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::I64Const((i64::from(descriptor as i32)) << 32));
    function.instruction(&Instruction::I64Or);
    function.instruction(&Instruction::I64Store(mem64(8)));
    function.instruction(&Instruction::LocalGet(s0));
    store_result(env, function, position)
}

/// Copy tag/aux/payload of `source` into `dest`; the destination address is
/// reloaded for every store (finding 4).
fn emit_cell_copy(
    env: &EmitEnv,
    function: &mut Function,
    dest: InstId,
    source: InstId,
) -> Result<(), EmitError> {
    let dest_local = env.local_of(dest)?;
    let source_local = env.local_of(source)?;
    // payload(8, i64), aux(4, i32), tag(0, i32)
    function.instruction(&Instruction::LocalGet(dest_local));
    function.instruction(&Instruction::LocalGet(source_local));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I64Store(mem64(8)));
    function.instruction(&Instruction::LocalGet(dest_local));
    function.instruction(&Instruction::LocalGet(source_local));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::I32Store(mem32(4)));
    function.instruction(&Instruction::LocalGet(dest_local));
    function.instruction(&Instruction::LocalGet(source_local));
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::I32Store(mem32(0)));
    Ok(())
}

/// Element address: the bound test compares the index against the list's aux
/// (`index >= aux` -> kind-2 "bound" raise, finding 10), then the payload-low
/// region plus `index * 16`.
fn emit_cell_elem_ptr(
    env: &EmitEnv,
    function: &mut Function,
    position: usize,
    base: InstId,
    index: InstId,
) -> Result<(), EmitError> {
    let s0 = env.s0();
    let s1 = env.s1();
    function.instruction(&Instruction::LocalGet(env.local_of(base)?));
    function.instruction(&Instruction::LocalSet(s0));
    function.instruction(&Instruction::LocalGet(env.local_of(index)?));
    function.instruction(&Instruction::LocalSet(s1));
    function.instruction(&Instruction::LocalGet(s1));
    function.instruction(&Instruction::LocalGet(s0));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_static(env.plan, function, 2, env.ctx.bound_msg_addr, b"bound")?;
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(s0));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalGet(s1));
    function.instruction(&Instruction::I32Const(4));
    function.instruction(&Instruction::I32Shl);
    function.instruction(&Instruction::I32Add);
    store_result(env, function, position)
}
