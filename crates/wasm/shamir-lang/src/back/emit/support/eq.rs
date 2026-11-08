//! Generated equality helpers: one bytes-compare wrapper plus one structural
//! equality helper per `EqShape`. Helpers may call each other (wasm recursion
//! depth is bounded by the runtime container depth), but generation is
//! iterative over shape indices and each helper only references shapes with
//! smaller indices (lowering builds bottom-up).

use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

use crate::back::emit::module::{EmissionPlan, EmitError};
use crate::back::ir::EqShape;

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

/// `(a: i32, b: i32) -> i32` in {-1, 0, 1}: compare lengths and the common
/// prefix via the runtime compare (already a signed byte difference over the
/// minimum length), then fall back to the length difference (lengths are
/// capped at 1 MiB, so the subtraction cannot overflow). Branch-free: both
/// selections are `select`, so the function is one straight-line block whose
/// final `End` closes the body with exactly one i32 on the stack.
pub(crate) fn emit_bytes_compare(plan: &EmissionPlan) -> Function {
    let mut function = Function::new([(6, ValType::I32)]);
    // la/lb/pa/pb/m/c = locals 2..8
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalSet(2)); // la
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalSet(3)); // lb
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(4)); // pa
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(5)); // pb
                                                     // m = la < lb ? la : lb
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::Select);
    function.instruction(&Instruction::LocalSet(6));
    // c = compare(pa, m, pb, m); the runtime compare is a signed byte
    // difference, so `c != 0` is already the ordered result
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::LocalGet(6));
    function.instruction(&Instruction::LocalGet(5));
    function.instruction(&Instruction::LocalGet(6));
    function.instruction(&Instruction::Call(plan.compare_index));
    function.instruction(&Instruction::LocalSet(7));
    // result = c != 0 ? c : la - lb; select pops (v1, v2, cond) and yields
    // v1 when cond is true, so push c first (one i32 left for the final `End`)
    function.instruction(&Instruction::LocalGet(7));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::LocalGet(7));
    function.instruction(&Instruction::LocalGet(7));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::Select);
    function.instruction(&Instruction::End);
    function
}

/// `(a: i32, b: i32) -> i32` in {0, 1}: structural equality over one cell.
/// Locals 2..12: ta, tb, la, lb, pa, pb, i, ea, eb, r.
pub(crate) fn emit_eq_helper(
    plan: &EmissionPlan,
    shapes: &[EqShape],
    shape_index: usize,
) -> Result<Function, EmitError> {
    let shape = shapes
        .get(shape_index)
        .ok_or(EmitError::Internal("shape index out of range"))?;
    let eq_index = |child: u32| -> Result<u32, EmitError> {
        if child as usize >= shape_index {
            return Err(EmitError::Internal("eq child shape not yet emitted"));
        }
        plan.eq_base
            .checked_add(
                1u32.checked_add(child)
                    .ok_or(EmitError::Internal("shape overflow"))?,
            )
            .ok_or(EmitError::Internal("function index overflow"))
    };
    let mut function = Function::new([(10, ValType::I32)]);
    match shape {
        EqShape::I32 | EqShape::I64 | EqShape::Bool => {
            payload_eq(&mut function);
        }
        EqShape::F64 => {
            // reinterpret the raw bits and compare as floats (NaN != NaN)
            function.instruction(&Instruction::LocalGet(0));
            function.instruction(&Instruction::I64Load(mem64(8)));
            function.instruction(&Instruction::F64ReinterpretI64);
            function.instruction(&Instruction::LocalGet(1));
            function.instruction(&Instruction::I64Load(mem64(8)));
            function.instruction(&Instruction::F64ReinterpretI64);
            function.instruction(&Instruction::F64Eq);
        }
        EqShape::Null => {
            function.instruction(&Instruction::I32Const(1));
        }
        EqShape::String | EqShape::Bytes => {
            blob_eq(&mut function, plan.compare_index);
        }
        EqShape::Optional(child) => {
            let child_index = eq_index(*child)?;
            optional_eq(&mut function, child_index);
        }
        EqShape::List(child) => {
            let child_index = eq_index(*child)?;
            list_eq(&mut function, child_index)?;
        }
        EqShape::Record(children) => {
            // pa/pb = payload low words of both cells
            function.instruction(&Instruction::LocalGet(0));
            function.instruction(&Instruction::I64Load(mem64(8)));
            function.instruction(&Instruction::I32WrapI64);
            function.instruction(&Instruction::LocalSet(6));
            function.instruction(&Instruction::LocalGet(1));
            function.instruction(&Instruction::I64Load(mem64(8)));
            function.instruction(&Instruction::I32WrapI64);
            function.instruction(&Instruction::LocalSet(7));
            if children.is_empty() {
                function.instruction(&Instruction::I32Const(1));
            }
            for (field, child) in children.iter().enumerate() {
                let child_index = eq_index(*child)?;
                let offset = i32::try_from(
                    field
                        .checked_mul(16)
                        .ok_or(EmitError::Internal("record field offset overflow"))?,
                )
                .map_err(|_| EmitError::Internal("record field offset overflow"))?;
                // ra = pa + field * 16, rb = pb + field * 16, then the child
                function.instruction(&Instruction::LocalGet(6));
                function.instruction(&Instruction::I32Const(offset));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::LocalGet(7));
                function.instruction(&Instruction::I32Const(offset));
                function.instruction(&Instruction::I32Add);
                function.instruction(&Instruction::Call(child_index));
                if field == 0 {
                    function.instruction(&Instruction::LocalSet(11));
                } else {
                    function.instruction(&Instruction::LocalSet(9));
                    // r &= next
                    function.instruction(&Instruction::LocalGet(11));
                    function.instruction(&Instruction::LocalGet(9));
                    function.instruction(&Instruction::I32And);
                    function.instruction(&Instruction::LocalSet(11));
                }
            }
            function.instruction(&Instruction::LocalGet(11));
        }
    }
    function.instruction(&Instruction::End);
    Ok(function)
}

/// Cell payloads are equal: two I64Loads and I64Eq -> 0/1.
fn payload_eq(function: &mut Function) {
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I64Eq);
}

/// String/bytes equality: aux (length) equality first, then the runtime
/// compare over the full length.
fn blob_eq(function: &mut Function, compare_index: u32) {
    // la/lb/pa/pb = locals 4..8
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalSet(4));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalSet(5));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(6));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(7));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::LocalGet(5));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::Return);
    function.instruction(&Instruction::Else);
    function.instruction(&Instruction::LocalGet(6));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::LocalGet(7));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::Call(compare_index));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::Return);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::I32Const(0));
}

/// Optional equality: both null -> 1, exactly one null -> 0, else compare
/// the optional cells themselves as the child.
fn optional_eq(function: &mut Function, child_index: u32) {
    // ta/tb = locals 2/3
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::LocalSet(2));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::LocalSet(3));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::Return);
    function.instruction(&Instruction::Else);
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::Return);
    function.instruction(&Instruction::Else);
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::Call(child_index));
    function.instruction(&Instruction::Return);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::I32Const(0));
}

/// List equality: equal lengths, then a byte-offset element loop where every
/// element must compare equal.
fn list_eq(function: &mut Function, child_index: u32) -> Result<(), EmitError> {
    // la/lb/pa/pb/i/ea/eb/r = locals 4..11
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalSet(4));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalSet(5));
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(6));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(7));
    // unequal lengths are unequal without touching any elements
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::LocalGet(5));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::Return);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::LocalSet(11)); // r = all-equal flag
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(8)); // i = 0
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    // i >= la -> done
    function.instruction(&Instruction::LocalGet(8));
    function.instruction(&Instruction::LocalGet(4));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1)); // exit $done
                                                 // ea = pa + i * 16, eb = pb + i * 16
    function.instruction(&Instruction::LocalGet(6));
    function.instruction(&Instruction::LocalGet(8));
    function.instruction(&Instruction::I32Const(4));
    function.instruction(&Instruction::I32Shl);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(9));
    function.instruction(&Instruction::LocalGet(7));
    function.instruction(&Instruction::LocalGet(8));
    function.instruction(&Instruction::I32Const(4));
    function.instruction(&Instruction::I32Shl);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(10));
    // r = child(ea, eb); r == 0 -> leave the loop with r = 0
    function.instruction(&Instruction::LocalGet(9));
    function.instruction(&Instruction::LocalGet(10));
    function.instruction(&Instruction::Call(child_index));
    function.instruction(&Instruction::LocalSet(11));
    function.instruction(&Instruction::LocalGet(11));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::BrIf(1)); // exit $done with r = 0
    function.instruction(&Instruction::LocalGet(8));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(8));
    function.instruction(&Instruction::Br(0));
    function.instruction(&Instruction::End); // $step
    function.instruction(&Instruction::End); // $done
    function.instruction(&Instruction::LocalGet(11));
    Ok(())
}
