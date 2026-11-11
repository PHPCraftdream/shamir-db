//! Runtime transaction-outcome validation for transactional batch responses.

use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

use crate::back::emit::module::{static_address, EmissionPlan, EmitError};
use crate::back::emit::support::db::implementation::emit_context_init;
use crate::back::runtime::{CONTEXT_INPUT_LEN_OFFSET, CONTEXT_INPUT_PTR_OFFSET};

const TAG_BOOL: i32 = 2;
const TAG_RECORD: i32 = 9;
const TAG_STRING: i32 = 6;
const TAG_DYNAMIC_MAP: i32 = 14;

const TRANSACTION_KEY: &[u8] = b"transaction";
const STATUS_KEY: &[u8] = b"status";
const REASON_KEY: &[u8] = b"reason";
const COMMITTED: &[u8] = b"committed";
const ABORTED: &[u8] = b"aborted";
const INDETERMINATE: &[u8] = b"indeterminate";
const SKIP_ABORTED: &[u8] = b"skip_aborted";
const TX_CONFLICT: &[u8] = b"tx_conflict";

const REFERENCE_MESSAGE: &[u8] = b"batch reference did not resolve to the expected scalar";
const CONFLICT_MESSAGE: &[u8] = b"transactional batch aborted by a conflict";
const ABORTED_MESSAGE: &[u8] = b"transactional batch aborted";
const UNKNOWN_MESSAGE: &[u8] = b"transactional batch outcome is unknown";

/// Static keys and constant messages used by the lazy transactional checker.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TxCheckStrings {
    transaction_key: u32,
    status_key: u32,
    reason_key: u32,
    committed: u32,
    aborted: u32,
    indeterminate: u32,
    skip_aborted: u32,
    tx_conflict: u32,
    reference_message: u32,
    conflict_message: u32,
    aborted_message: u32,
    unknown_message: u32,
}

/// The registered `(request, response) -> response` transaction checker.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TxCheckPlan {
    pub(crate) helper_index: u32,
    pub(crate) strings: TxCheckStrings,
}

/// Append transaction-only strings to the static image.
pub(crate) fn append_tx_check_strings(
    image: &mut Vec<u8>,
    limit: u64,
) -> Result<TxCheckStrings, EmitError> {
    Ok(TxCheckStrings {
        transaction_key: append_static_bytes(image, TRANSACTION_KEY, limit)?,
        status_key: append_static_bytes(image, STATUS_KEY, limit)?,
        reason_key: append_static_bytes(image, REASON_KEY, limit)?,
        committed: append_static_bytes(image, COMMITTED, limit)?,
        aborted: append_static_bytes(image, ABORTED, limit)?,
        indeterminate: append_static_bytes(image, INDETERMINATE, limit)?,
        skip_aborted: append_static_bytes(image, SKIP_ABORTED, limit)?,
        tx_conflict: append_static_bytes(image, TX_CONFLICT, limit)?,
        reference_message: append_static_bytes(image, REFERENCE_MESSAGE, limit)?,
        conflict_message: append_static_bytes(image, CONFLICT_MESSAGE, limit)?,
        aborted_message: append_static_bytes(image, ABORTED_MESSAGE, limit)?,
        unknown_message: append_static_bytes(image, UNKNOWN_MESSAGE, limit)?,
    })
}

fn append_static_bytes(image: &mut Vec<u8>, bytes: &[u8], limit: u64) -> Result<u32, EmitError> {
    let offset = u64::try_from(image.len())
        .map_err(|_| EmitError::Internal("static image offset overflow"))?;
    let observed = offset
        .checked_add(bytes.len() as u64)
        .ok_or(EmitError::Internal("static image length overflow"))?;
    if observed > limit {
        return Err(EmitError::LimitExceeded {
            code: "B001",
            limit,
            observed,
        });
    }
    let address = static_address(offset)?;
    image.extend_from_slice(bytes);
    Ok(address)
}

fn mem32(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 2,
        memory_index: 0,
    }
}

fn mem64(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 3,
        memory_index: 0,
    }
}

/// Emits `(request cell, response cell) -> response cell`. The request's
/// `transactional` boolean is inspected before response decoding, so a
/// non-transactional request follows its former path without requiring a
/// `transaction` response member.
pub(crate) fn emit_response_tx_check_helper(
    plan: &EmissionPlan,
    tx_plan: &TxCheckPlan,
    find_index: u32,
) -> Result<Function, EmitError> {
    // params: 0 request, 1 response; locals: request region, transaction
    // flag cell, map lookup result, transaction map, status cell, reason cell.
    let request_region = 2;
    let flag_cell = 3;
    let transaction = 4;
    let status = 5;
    let reason = 6;
    let mut function = Function::new([(5, ValType::I32)]);
    let strings = tx_plan.strings;

    // Non-record input cannot represent this request template; leave it to
    // the normal downstream behavior instead of misclassifying it as tx.
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::I32Const(TAG_RECORD));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::Return);
    function.instruction(&Instruction::End);

    // BatchRequest field 1 is `transactional`; only BOOL true opts in.
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(request_region));
    function.instruction(&Instruction::LocalGet(request_region));
    function.instruction(&Instruction::I32Const(16));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(flag_cell));
    function.instruction(&Instruction::LocalGet(flag_cell));
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::I32Const(TAG_BOOL));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::LocalGet(flag_cell));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I64Eqz);
    function.instruction(&Instruction::I32Or);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::Return);
    function.instruction(&Instruction::End);

    // Decode the host response and require its transaction object.
    let bases = plan.wrapper_bases();
    let ctx = bases.context;
    emit_context_init(&mut function, &bases, bases.output_cap);
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::I32Store(mem32(CONTEXT_INPUT_PTR_OFFSET)));
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::I32Store(mem32(CONTEXT_INPUT_LEN_OFFSET)));
    function.instruction(&Instruction::I32Const(ctx as i32));
    function.instruction(&Instruction::Call(plan.decode_index));
    require_map(&mut function, plan, bases.scratch, false);
    find_value(
        &mut function,
        find_index,
        bases.scratch,
        strings.transaction_key,
        TRANSACTION_KEY.len(),
        false,
    );
    function.instruction(&Instruction::LocalTee(transaction));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_decode_error(&mut function, plan);
    function.instruction(&Instruction::End);
    require_map(&mut function, plan, transaction, true);

    // `status` is mandatory and must be a string.
    find_value(
        &mut function,
        find_index,
        transaction,
        strings.status_key,
        STATUS_KEY.len(),
        true,
    );
    function.instruction(&Instruction::LocalTee(status));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_decode_error(&mut function, plan);
    function.instruction(&Instruction::End);
    require_string(&mut function, plan, status);

    // The engine omits `reason` for committed and indeterminate outcomes. A
    // present reason must still be a string; aborted outcomes require one.
    find_value(
        &mut function,
        find_index,
        transaction,
        strings.reason_key,
        REASON_KEY.len(),
        true,
    );
    function.instruction(&Instruction::LocalSet(reason));
    function.instruction(&Instruction::LocalGet(reason));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Else);
    require_string(&mut function, plan, reason);
    function.instruction(&Instruction::End);

    // A committed transaction succeeds even when TransactionInfo.error is
    // `wal_durability`; status is the outcome authority.
    if_string_equals(
        &mut function,
        plan,
        status,
        strings.committed,
        COMMITTED.len(),
    );
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::Return);
    function.instruction(&Instruction::End);

    if_string_equals(
        &mut function,
        plan,
        status,
        strings.indeterminate,
        INDETERMINATE.len(),
    );
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_kind2(
        &mut function,
        plan,
        strings.unknown_message,
        UNKNOWN_MESSAGE.len(),
    );
    function.instruction(&Instruction::End);

    // The only remaining recognized status is aborted. Missing reason is a
    // malformed transaction object (kind 3), never a generic abort.
    if_string_equals(&mut function, plan, status, strings.aborted, ABORTED.len());
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(reason));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_decode_error(&mut function, plan);
    function.instruction(&Instruction::End);

    if_string_equals(
        &mut function,
        plan,
        reason,
        strings.skip_aborted,
        SKIP_ABORTED.len(),
    );
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_kind2(
        &mut function,
        plan,
        strings.reference_message,
        REFERENCE_MESSAGE.len(),
    );
    function.instruction(&Instruction::End);

    if_string_equals(
        &mut function,
        plan,
        reason,
        strings.tx_conflict,
        TX_CONFLICT.len(),
    );
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_kind2(
        &mut function,
        plan,
        strings.conflict_message,
        CONFLICT_MESSAGE.len(),
    );
    function.instruction(&Instruction::End);
    raise_kind2(
        &mut function,
        plan,
        strings.aborted_message,
        ABORTED_MESSAGE.len(),
    );
    function.instruction(&Instruction::End);

    // Unknown status is malformed metadata. No engine-provided reason is ever
    // formatted into a guest error message.
    raise_decode_error(&mut function, plan);
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::End);
    Ok(function)
}

fn find_value(
    function: &mut Function,
    find_index: u32,
    map: u32,
    key: u32,
    len: usize,
    is_local: bool,
) {
    if is_local {
        function.instruction(&Instruction::LocalGet(map));
    } else {
        function.instruction(&Instruction::I32Const(map as i32));
    }
    function.instruction(&Instruction::I32Const(key as i32));
    function.instruction(&Instruction::I32Const(len as i32));
    function.instruction(&Instruction::Call(find_index));
}

fn require_map(function: &mut Function, plan: &EmissionPlan, cell: u32, is_local: bool) {
    if is_local {
        function.instruction(&Instruction::LocalGet(cell));
    } else {
        function.instruction(&Instruction::I32Const(cell as i32));
    }
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::I32Const(TAG_DYNAMIC_MAP));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_decode_error(function, plan);
    function.instruction(&Instruction::End);
}

fn require_string(function: &mut Function, plan: &EmissionPlan, cell: u32) {
    function.instruction(&Instruction::LocalGet(cell));
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::I32Const(TAG_STRING));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_decode_error(function, plan);
    function.instruction(&Instruction::End);
}

fn if_string_equals(
    function: &mut Function,
    plan: &EmissionPlan,
    cell: u32,
    expected: u32,
    expected_len: usize,
) {
    function.instruction(&Instruction::LocalGet(cell));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalGet(cell));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::I32Const(expected as i32));
    function.instruction(&Instruction::I32Const(expected_len as i32));
    function.instruction(&Instruction::Call(plan.compare_index));
    function.instruction(&Instruction::I32Eqz);
}

fn raise_decode_error(function: &mut Function, plan: &EmissionPlan) {
    function.instruction(&Instruction::I32Const(3));
    function.instruction(&Instruction::I32Const(plan.msg_arg as i32));
    function.instruction(&Instruction::I32Const(3));
    function.instruction(&Instruction::Call(plan.raise_index));
    function.instruction(&Instruction::Unreachable);
}

fn raise_kind2(function: &mut Function, plan: &EmissionPlan, message: u32, len: usize) {
    function.instruction(&Instruction::I32Const(2));
    function.instruction(&Instruction::I32Const(message as i32));
    function.instruction(&Instruction::I32Const(len as i32));
    function.instruction(&Instruction::Call(plan.raise_index));
    function.instruction(&Instruction::Unreachable);
}
