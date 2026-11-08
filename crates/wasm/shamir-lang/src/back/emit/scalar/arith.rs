use wasm_encoder::{BlockType, Function, Ieee64, Instruction};

use crate::back::emit::module::EmitError;
use crate::back::ir::{ArithOp, CastOp, CmpOp, ValueKind};

use super::control::EmitEnv;
use super::raise_static;

pub(crate) fn i32_scratch(env: &EmitEnv, which: usize) -> u32 {
    let index = env.base + which as u32;
    env.note_local(index);
    index
}

pub(crate) fn i64_scratch(env: &EmitEnv, which: usize) -> u32 {
    let index = env.base + 2 + which as u32;
    env.note_local(index);
    index
}

pub(super) fn f64_scratch(env: &EmitEnv) -> u32 {
    let index = env.base + 5;
    env.note_local(index);
    index
}

/// Checked add/sub over the operand locals (finding 5): the wrapping result
/// survives; overflow is detected by a sign-bit identity (add:
/// `t = (a^s) & ~(a^b)`; sub: `t = (a^b) & (a^s)`), raising kind 2 "ovf".
/// Leaves the wrapping result on the stack.
fn emit_wrapping_arith(
    env: &EmitEnv,
    function: &mut Function,
    is_add: bool,
    wide: bool,
    left: u32,
    right: u32,
) -> Result<(), EmitError> {
    let (a, b, s) = if wide {
        (
            i64_scratch(env, 0),
            i64_scratch(env, 1),
            i64_scratch(env, 2),
        )
    } else {
        (
            i32_scratch(env, 0),
            i32_scratch(env, 1),
            i32_scratch(env, 1),
        )
    };
    function.instruction(&Instruction::LocalGet(left));
    function.instruction(&Instruction::LocalSet(a));
    function.instruction(&Instruction::LocalGet(right));
    function.instruction(&Instruction::LocalSet(b));
    // d = a ^ b
    function.instruction(&Instruction::LocalGet(a));
    function.instruction(&Instruction::LocalGet(b));
    if wide {
        function.instruction(&Instruction::I64Xor);
    } else {
        function.instruction(&Instruction::I32Xor);
    }
    // s = a + b / a - b (wrapping)
    function.instruction(&Instruction::LocalGet(a));
    function.instruction(&Instruction::LocalGet(b));
    if wide {
        if is_add {
            function.instruction(&Instruction::I64Add);
        } else {
            function.instruction(&Instruction::I64Sub);
        }
    } else if is_add {
        function.instruction(&Instruction::I32Add);
    } else {
        function.instruction(&Instruction::I32Sub);
    }
    function.instruction(&Instruction::LocalSet(s)); // s saved; [d]
                                                     // e = s ^ a
    function.instruction(&Instruction::LocalGet(s));
    function.instruction(&Instruction::LocalGet(a));
    if wide {
        function.instruction(&Instruction::I64Xor);
    } else {
        function.instruction(&Instruction::I32Xor);
    }
    if is_add {
        // t = e & ~d (d on the stack below e)
        function.instruction(&Instruction::LocalSet(a)); // a reused for e; [d]
        if wide {
            function.instruction(&Instruction::I64Const(-1));
            function.instruction(&Instruction::I64Xor);
            function.instruction(&Instruction::LocalGet(a));
            function.instruction(&Instruction::I64And);
        } else {
            function.instruction(&Instruction::I32Const(-1));
            function.instruction(&Instruction::I32Xor);
            function.instruction(&Instruction::LocalGet(a));
            function.instruction(&Instruction::I32And);
        }
    } else {
        // t = d & e
        if wide {
            function.instruction(&Instruction::I64And);
        } else {
            function.instruction(&Instruction::I32And);
        }
    }
    // t < 0 -> overflow
    if wide {
        function.instruction(&Instruction::I64Const(0));
        function.instruction(&Instruction::I64LtS);
    } else {
        function.instruction(&Instruction::I32Const(0));
        function.instruction(&Instruction::I32LtS);
    }
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_static(env.plan, function, 2, env.ctx.overflow_msg_addr, b"ovf")?;
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(s));
    Ok(())
}

/// Emits one checked integer operation from the operand locals; leaves the
/// result on the stack (the caller stores it into the result local).
pub(crate) fn emit_checked_arith(
    env: &EmitEnv,
    function: &mut Function,
    op: ArithOp,
    width: ValueKind,
    left: u32,
    right: u32,
) -> Result<(), EmitError> {
    let wide = width == ValueKind::I64;
    match op {
        ArithOp::Add | ArithOp::Sub => {
            emit_wrapping_arith(env, function, op == ArithOp::Add, wide, left, right)
        }
        ArithOp::Mul => {
            if wide {
                emit_i64_mul(env, function, left, right)
            } else {
                // p = (i64)a * b, range-checked, wrapped back (finding 5)
                function.instruction(&Instruction::LocalGet(left));
                function.instruction(&Instruction::I64ExtendI32S);
                function.instruction(&Instruction::LocalGet(right));
                function.instruction(&Instruction::I64ExtendI32S);
                function.instruction(&Instruction::I64Mul);
                function.instruction(&Instruction::LocalTee(i64_scratch(env, 0)));
                function.instruction(&Instruction::LocalGet(i64_scratch(env, 0)));
                function.instruction(&Instruction::I64Const(i64::from(i32::MIN)));
                function.instruction(&Instruction::I64LtS);
                function.instruction(&Instruction::LocalGet(i64_scratch(env, 0)));
                function.instruction(&Instruction::I64Const(i64::from(i32::MAX)));
                function.instruction(&Instruction::I64GtS);
                function.instruction(&Instruction::I32Or);
                function.instruction(&Instruction::If(BlockType::Empty));
                raise_static(env.plan, function, 2, env.ctx.overflow_msg_addr, b"ovf")?;
                function.instruction(&Instruction::End);
                function.instruction(&Instruction::I32WrapI64);
                Ok(())
            }
        }
        ArithOp::Div | ArithOp::Rem => emit_div_rem(env, function, op, wide, left, right),
    }
}

/// Div/Rem: rhs == 0 raises "div0", MIN/-1 raises "ovf", then the wasm
/// signed operation.
fn emit_div_rem(
    env: &EmitEnv,
    function: &mut Function,
    op: ArithOp,
    wide: bool,
    left: u32,
    right: u32,
) -> Result<(), EmitError> {
    let (a, b) = if wide {
        (i64_scratch(env, 0), i64_scratch(env, 1))
    } else {
        (i32_scratch(env, 0), i32_scratch(env, 1))
    };
    function.instruction(&Instruction::LocalGet(left));
    function.instruction(&Instruction::LocalSet(a));
    function.instruction(&Instruction::LocalGet(right));
    function.instruction(&Instruction::LocalSet(b));
    // rhs == 0 -> div0
    function.instruction(&Instruction::LocalGet(b));
    if wide {
        function.instruction(&Instruction::I64Eqz);
    } else {
        function.instruction(&Instruction::I32Eqz);
    }
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_static(env.plan, function, 2, env.ctx.div0_msg_addr, b"div0")?;
    function.instruction(&Instruction::End);
    // MIN / -1 -> ovf
    function.instruction(&Instruction::LocalGet(a));
    if wide {
        function.instruction(&Instruction::I64Const(i64::MIN));
        function.instruction(&Instruction::I64Eq);
    } else {
        function.instruction(&Instruction::I32Const(i32::MIN));
        function.instruction(&Instruction::I32Eq);
    }
    function.instruction(&Instruction::LocalGet(b));
    if wide {
        function.instruction(&Instruction::I64Const(-1));
        function.instruction(&Instruction::I64Eq);
    } else {
        function.instruction(&Instruction::I32Const(-1));
        function.instruction(&Instruction::I32Eq);
    }
    function.instruction(&Instruction::I32And);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_static(env.plan, function, 2, env.ctx.overflow_msg_addr, b"ovf")?;
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(a));
    function.instruction(&Instruction::LocalGet(b));
    match (wide, op) {
        (false, ArithOp::Div) => {
            function.instruction(&Instruction::I32DivS);
        }
        (false, _) => {
            function.instruction(&Instruction::I32RemS);
        }
        (true, ArithOp::Div) => {
            function.instruction(&Instruction::I64DivS);
        }
        (true, _) => {
            function.instruction(&Instruction::I64RemS);
        }
    }
    Ok(())
}

/// I64 multiplication: p = a * b (wrapping); rhs == 0 gives zero; otherwise
/// MIN/-1 raises "ovf" and the quotient must round-trip to the lhs.
fn emit_i64_mul(
    env: &EmitEnv,
    function: &mut Function,
    left: u32,
    right: u32,
) -> Result<(), EmitError> {
    let a = i64_scratch(env, 0);
    let b = i64_scratch(env, 1);
    let p = i64_scratch(env, 2);
    function.instruction(&Instruction::LocalGet(left));
    function.instruction(&Instruction::LocalSet(a));
    function.instruction(&Instruction::LocalGet(right));
    function.instruction(&Instruction::LocalSet(b));
    function.instruction(&Instruction::LocalGet(a));
    function.instruction(&Instruction::LocalGet(b));
    function.instruction(&Instruction::I64Mul);
    function.instruction(&Instruction::LocalSet(p));
    function.instruction(&Instruction::Block(BlockType::Empty));
    // rhs == 0: skip the checks; p is already the correct zero
    function.instruction(&Instruction::LocalGet(b));
    function.instruction(&Instruction::I64Eqz);
    function.instruction(&Instruction::BrIf(0));
    function.instruction(&Instruction::LocalGet(a));
    function.instruction(&Instruction::I64Const(i64::MIN));
    function.instruction(&Instruction::I64Eq);
    function.instruction(&Instruction::LocalGet(b));
    function.instruction(&Instruction::I64Const(-1));
    function.instruction(&Instruction::I64Eq);
    function.instruction(&Instruction::I32And);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_static(env.plan, function, 2, env.ctx.overflow_msg_addr, b"ovf")?;
    function.instruction(&Instruction::End);
    // q = p / b must round-trip (b != 0 and not the MIN/-1 pair: safe)
    function.instruction(&Instruction::LocalGet(p));
    function.instruction(&Instruction::LocalGet(b));
    function.instruction(&Instruction::I64DivS);
    function.instruction(&Instruction::LocalGet(a));
    function.instruction(&Instruction::I64Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_static(env.plan, function, 2, env.ctx.overflow_msg_addr, b"ovf")?;
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(p));
    Ok(())
}

/// Comparison over the operand locals; result is i32 0/1 on the stack.
pub(crate) fn emit_compare(
    function: &mut Function,
    op: CmpOp,
    width: ValueKind,
    left: u32,
    right: u32,
) {
    function.instruction(&Instruction::LocalGet(left));
    function.instruction(&Instruction::LocalGet(right));
    let instruction = match (width, op) {
        (ValueKind::I32, CmpOp::Eq) => Instruction::I32Eq,
        (ValueKind::I32, CmpOp::Ne) => Instruction::I32Ne,
        (ValueKind::I32, CmpOp::Lt) => Instruction::I32LtS,
        (ValueKind::I32, CmpOp::Le) => Instruction::I32LeS,
        (ValueKind::I32, CmpOp::Gt) => Instruction::I32GtS,
        (ValueKind::I32, CmpOp::Ge) => Instruction::I32GeS,
        (ValueKind::I64, CmpOp::Eq) => Instruction::I64Eq,
        (ValueKind::I64, CmpOp::Ne) => Instruction::I64Ne,
        (ValueKind::I64, CmpOp::Lt) => Instruction::I64LtS,
        (ValueKind::I64, CmpOp::Le) => Instruction::I64LeS,
        (ValueKind::I64, CmpOp::Gt) => Instruction::I64GtS,
        (ValueKind::I64, CmpOp::Ge) => Instruction::I64GeS,
        (ValueKind::F64, CmpOp::Eq) => Instruction::F64Eq,
        (ValueKind::F64, CmpOp::Ne) => Instruction::F64Ne,
        (ValueKind::F64, CmpOp::Lt) => Instruction::F64Lt,
        (ValueKind::F64, CmpOp::Le) => Instruction::F64Le,
        (ValueKind::F64, CmpOp::Gt) => Instruction::F64Gt,
        (ValueKind::F64, CmpOp::Ge) => Instruction::F64Ge,
    };
    function.instruction(&instruction);
}

/// Range-checked truncation via the f64 scratch temp; NaN, non-integral and
/// out-of-range inputs raise kind 2 "range".
fn emit_float_trunc(
    env: &EmitEnv,
    function: &mut Function,
    to_i64: bool,
    value: u32,
) -> Result<(), EmitError> {
    let scratch = f64_scratch(env);
    let (lo, hi) = if to_i64 {
        (-9223372036854775808.0f64, 9223372036854775808.0f64)
    } else {
        (-2147483648.0f64, 2147483648.0f64)
    };
    function.instruction(&Instruction::LocalGet(value));
    function.instruction(&Instruction::LocalSet(scratch));
    // NaN: x != x
    function.instruction(&Instruction::LocalGet(scratch));
    function.instruction(&Instruction::LocalGet(scratch));
    function.instruction(&Instruction::F64Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_static(env.plan, function, 2, env.ctx.range_msg_addr, b"range")?;
    function.instruction(&Instruction::End);
    // non-integral: trunc(x) != x
    function.instruction(&Instruction::LocalGet(scratch));
    function.instruction(&Instruction::F64Trunc);
    function.instruction(&Instruction::LocalGet(scratch));
    function.instruction(&Instruction::F64Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_static(env.plan, function, 2, env.ctx.range_msg_addr, b"range")?;
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(scratch));
    function.instruction(&Instruction::F64Trunc);
    function.instruction(&Instruction::LocalSet(scratch));
    function.instruction(&Instruction::LocalGet(scratch));
    function.instruction(&Instruction::F64Const(Ieee64::from(lo)));
    function.instruction(&Instruction::F64Ge);
    function.instruction(&Instruction::LocalGet(scratch));
    function.instruction(&Instruction::F64Const(Ieee64::from(hi)));
    function.instruction(&Instruction::F64Lt);
    function.instruction(&Instruction::I32And);
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise_static(env.plan, function, 2, env.ctx.range_msg_addr, b"range")?;
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(scratch));
    if to_i64 {
        function.instruction(&Instruction::I64TruncF64S);
    } else {
        function.instruction(&Instruction::I32TruncF64S);
    }
    Ok(())
}

/// Emits one numeric cast from the operand local; the result is left on the
/// stack.
pub(crate) fn emit_cast(
    env: &EmitEnv,
    function: &mut Function,
    op: CastOp,
    value: u32,
) -> Result<(), EmitError> {
    match op {
        CastOp::ToI64FromI32 => {
            function.instruction(&Instruction::LocalGet(value));
            function.instruction(&Instruction::I64ExtendI32S);
        }
        CastOp::ToF64FromI32 => {
            function.instruction(&Instruction::LocalGet(value));
            function.instruction(&Instruction::F64ConvertI32S);
        }
        CastOp::ToF64FromI64 => {
            function.instruction(&Instruction::LocalGet(value));
            function.instruction(&Instruction::F64ConvertI64S);
        }
        CastOp::ToI32FromI64 => {
            function.instruction(&Instruction::LocalGet(value));
            function.instruction(&Instruction::LocalTee(i64_scratch(env, 0)));
            function.instruction(&Instruction::LocalGet(i64_scratch(env, 0)));
            function.instruction(&Instruction::I64Const(i64::from(i32::MIN)));
            function.instruction(&Instruction::I64LtS);
            function.instruction(&Instruction::LocalGet(i64_scratch(env, 0)));
            function.instruction(&Instruction::I64Const(i64::from(i32::MAX)));
            function.instruction(&Instruction::I64GtS);
            function.instruction(&Instruction::I32Or);
            function.instruction(&Instruction::If(BlockType::Empty));
            raise_static(env.plan, function, 2, env.ctx.range_msg_addr, b"range")?;
            function.instruction(&Instruction::End);
            function.instruction(&Instruction::I32WrapI64);
        }
        CastOp::ToI32FromF64 => emit_float_trunc(env, function, false, value)?,
        CastOp::ToI64FromF64 => emit_float_trunc(env, function, true, value)?,
    }
    Ok(())
}
