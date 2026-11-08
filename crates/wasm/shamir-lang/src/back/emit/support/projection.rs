//! Typed entry-parameter projection helpers (finding 15) plus the static
//! message/descriptor baking shared by the wrapper path. Split out of
//! `emit/module.rs` so the assembler stays under its size budget.
//!
//! One Wasm helper is generated per DISTINCT projected entry-parameter type:
//! signature `(i32 cell) -> i32`; it validates the decoded cell against the
//! static type node and returns the projected cell (usually the same pointer;
//! lists and records are rebuilt).
//!
//! GENERATION RECURSION: `emit_node` is a recursive descent over the type
//! node tree, depth-capped at `PROJECTION_MAX_DEPTH` (the design bounds wire
//! parameter depth at 30/31 containers, and parse already caps type depth
//! below this). The EMITTED code is always iterative `block { loop { ... } }`
//! Wasm; runtime Wasm never recurses through generation-time machinery.

use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

use crate::api::CompileLimits;
use crate::back::ir::{IrFunction, IrModule, ParamProjection, RecordField, TypeNode, TypeNodeKind};

use crate::back::emit::module::{static_address, EmitError, MAX_UNROLLED_NAME_BYTES};

/// Design: wire parameter depth <= 30/31 containers; one spare frame.
pub(crate) const PROJECTION_MAX_DEPTH: u32 = 31;

/// Addresses (inside the static image) of the wrapper raise messages
/// appended in a fixed second block after the scalar messages.
pub(crate) struct WrapperMessages {
    /// Unknown or out-of-range envelope key.
    pub arg: u32,
    /// Positional and named carriers combined (numeric keys with named keys,
    /// or `args` with named keys).
    pub mixed: u32,
    /// Argument count does not match the entry arity.
    pub arity: u32,
    /// i32 parameter value out of range.
    pub i32: u32,
    /// Missing required record field.
    pub field: u32,
}

/// Static addresses baked into the wrapper: scalar raise messages, wrapper
/// raise messages, and entry parameter-name (address, length) pairs.
pub(crate) struct BakedMessages {
    pub scalar: [u32; 5],
    pub wrapper: WrapperMessages,
    pub names: Vec<(u32, u32)>,
}

/// Fixed wrapper raise messages, appended in a fixed second block after the
/// scalar messages (`deep` is reserved for deep projection diagnostics and
/// `field` reports missing required record fields).
pub(crate) const WRAPPER_MESSAGES: [&[u8]; 6] =
    [b"arg", b"mixed", b"arity", b"i32", b"deep", b"field"];

/// Stable cell tags the projection helpers read (mirrors `runtime::CellTag`).
const TAG_NULL: i32 = 1;
const TAG_BOOL: i32 = 2;
const TAG_I64: i32 = 4;
const TAG_F64: i32 = 5;
const TAG_STRING: i32 = 6;
const TAG_BYTES: i32 = 7;
const TAG_LIST: i32 = 8;
const TAG_RECORD: i32 = 9;
/// The decoder represents wire maps as dynamic map cells.
const TAG_DYNMAP: i32 = 14;

fn mem32(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 2,
        memory_index: 0,
    }
}

/// 8-bit accesses must declare byte alignment.
fn byte_mem(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 0,
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

/// Appends one fixed message block to a copy of the static image (fixed
/// order at the end) and returns the image with their absolute addresses.
pub(crate) fn append_message_block(
    image: &mut Vec<u8>,
    messages: &[&[u8]],
    limit: u64,
) -> Result<Vec<u32>, EmitError> {
    let mut addrs = Vec::with_capacity(messages.len());
    for message in messages {
        let addr = append_bytes(image, message, limit)?;
        addrs.push(addr);
    }
    Ok(addrs)
}

/// Appends raw bytes to the static image under `limit`; returns the bytes'
/// ABSOLUTE address (`STATIC_START + offset`).
fn append_bytes(image: &mut Vec<u8>, bytes: &[u8], limit: u64) -> Result<u32, EmitError> {
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
    let addr = static_address(offset)?;
    image.extend_from_slice(bytes);
    Ok(addr)
}

/// Appends the scalar raise messages, the wrapper message block and the
/// entry function's parameter-name block (raw UTF-8 bytes, in order); returns
/// the finished image with the baked message addresses.
pub(crate) fn append_raise_messages(
    static_bytes: &[u8],
    param_names: &[String],
    limits: &CompileLimits,
) -> Result<(Vec<u8>, BakedMessages), EmitError> {
    let limit = limits.max_static_bytes as u64;
    let mut image = static_bytes.to_vec();
    let scalar = append_message_block(
        &mut image,
        &crate::back::emit::scalar::RAISE_MESSAGES,
        limit,
    )?;
    let wrapper = append_message_block(&mut image, &WRAPPER_MESSAGES, limit)?;
    let mut scalar_addrs = [0u32; 5];
    for (slot, addr) in scalar_addrs.iter_mut().enumerate() {
        *addr = *scalar
            .get(slot)
            .ok_or(EmitError::Internal("missing raise message"))?;
    }
    let message_at = |position: usize| -> Result<u32, EmitError> {
        wrapper
            .get(position)
            .copied()
            .ok_or(EmitError::Internal("missing wrapper message"))
    };
    let wrapper_messages = WrapperMessages {
        arg: message_at(0)?,
        mixed: message_at(1)?,
        arity: message_at(2)?,
        i32: message_at(3)?,
        field: message_at(5)?,
    };
    let mut name_addrs = Vec::with_capacity(param_names.len());
    for name in param_names {
        if name.is_empty() {
            return Err(EmitError::Internal("empty parameter name"));
        }
        let addr = append_bytes(&mut image, name.as_bytes(), limit)?;
        let len = u32::try_from(name.len())
            .map_err(|_| EmitError::Internal("parameter name length overflow"))?;
        name_addrs.push((addr, len));
    }
    Ok((
        image,
        BakedMessages {
            scalar: scalar_addrs,
            wrapper: wrapper_messages,
            names: name_addrs,
        },
    ))
}

/// Appends one record descriptor per record-bearing node, reusing the
/// builder's layout (field-name bytes first, then `u32` count plus per field
/// `u32` len and `u32` ABSOLUTE name offset). Deterministic order = the
/// distinct-node order. Returns addresses parallel to `distinct` (`None` for
/// non-record nodes).
pub(crate) fn append_record_descriptors(
    image: &mut Vec<u8>,
    nodes: &[TypeNode],
    distinct: &[u32],
    limit: u64,
) -> Result<Vec<Option<u32>>, EmitError> {
    let mut addrs = Vec::with_capacity(distinct.len());
    for &node_index in distinct {
        let node = nodes
            .get(node_index as usize)
            .ok_or(EmitError::Internal("projection node out of range"))?;
        let TypeNodeKind::Record(fields) = &node.kind else {
            addrs.push(None);
            continue;
        };
        let mut name_offsets = Vec::with_capacity(fields.len());
        for field in fields {
            let offset = append_bytes(image, field.name.as_bytes(), limit)?;
            name_offsets.push(offset);
        }
        let count = u32::try_from(fields.len())
            .map_err(|_| EmitError::Internal("record field count overflow"))?;
        let descriptor_len = 4u64
            .checked_add(
                u64::from(count)
                    .checked_mul(8)
                    .ok_or(EmitError::Internal("descriptor length overflow"))?,
            )
            .ok_or(EmitError::Internal("descriptor length overflow"))?;
        // reserve + absolute descriptor address
        let offset = u64::try_from(image.len())
            .map_err(|_| EmitError::Internal("static image offset overflow"))?;
        let observed = offset
            .checked_add(descriptor_len)
            .ok_or(EmitError::Internal("static image length overflow"))?;
        if observed > limit {
            return Err(EmitError::LimitExceeded {
                code: "B001",
                limit,
                observed,
            });
        }
        let base = static_address(offset)?;
        image.extend_from_slice(&count.to_le_bytes());
        for (position, field) in fields.iter().enumerate() {
            let len = u32::try_from(field.name.len())
                .map_err(|_| EmitError::Internal("field name length overflow"))?;
            image.extend_from_slice(&len.to_le_bytes());
            let absolute = *name_offsets
                .get(position)
                .ok_or(EmitError::Internal("descriptor name offset overflow"))?;
            image.extend_from_slice(&absolute.to_le_bytes());
        }
        addrs.push(Some(base));
    }
    Ok(addrs)
}

/// Distinct type-node indices of the entry's projected aggregate parameters,
/// in first-use order (deterministic), after validating generation limits.
pub(crate) fn collect_projection_nodes(
    ir: &IrModule,
    entry: &IrFunction,
) -> Result<Vec<u32>, EmitError> {
    let name_limit = MAX_UNROLLED_NAME_BYTES as u64;
    for name in &entry.param_names {
        if name.len() > MAX_UNROLLED_NAME_BYTES {
            return Err(EmitError::LimitExceeded {
                code: "B001",
                limit: name_limit,
                observed: name.len() as u64,
            });
        }
    }
    let mut distinct: Vec<u32> = Vec::new();
    for (slot, kind) in entry.param_projections.iter().enumerate() {
        if !matches!(kind, ParamProjection::Cell | ParamProjection::OptionalCell) {
            continue;
        }
        // hand-built IR without nodes keeps the inline pass-through
        let Some(&node_index) = entry.param_type_nodes.get(slot) else {
            continue;
        };
        let node = ir
            .type_nodes
            .get(node_index as usize)
            .ok_or(EmitError::Internal("projection node out of range"))?;
        check_node_names(node, name_limit)?;
        let depth = node_depth(&ir.type_nodes, node_index)?;
        if depth > u64::from(PROJECTION_MAX_DEPTH) {
            return Err(EmitError::LimitExceeded {
                code: "B001",
                limit: u64::from(PROJECTION_MAX_DEPTH),
                observed: depth,
            });
        }
        if !distinct.contains(&node_index) {
            distinct.push(node_index);
        }
    }
    Ok(distinct)
}

/// Rejects record field names too long to unroll byte-by-byte.
fn check_node_names(root: &TypeNode, limit: u64) -> Result<(), EmitError> {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if let TypeNodeKind::Record(fields) = &node.kind {
            for field in fields {
                if field.name.len() as u64 > limit {
                    return Err(EmitError::LimitExceeded {
                        code: "B001",
                        limit,
                        observed: field.name.len() as u64,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Structural depth of the node tree rooted at `root` (iterative over the
/// node arena; children are arena indices).
fn node_depth(nodes: &[TypeNode], root: u32) -> Result<u64, EmitError> {
    let mut deepest = 0u64;
    let mut stack = vec![(root, 1u64)];
    while let Some((index, depth)) = stack.pop() {
        let node = nodes
            .get(index as usize)
            .ok_or(EmitError::Internal("projection node out of range"))?;
        if depth > deepest {
            deepest = depth;
        }
        match &node.kind {
            TypeNodeKind::Optional(child) | TypeNodeKind::List(child) => {
                stack.push((
                    *child,
                    depth
                        .checked_add(1)
                        .ok_or(EmitError::Internal("projection depth overflow"))?,
                ));
            }
            TypeNodeKind::Record(fields) => {
                let next = depth
                    .checked_add(1)
                    .ok_or(EmitError::Internal("projection depth overflow"))?;
                for field in fields {
                    stack.push((field.ty, next));
                }
            }
            _ => {}
        }
    }
    Ok(deepest)
}

/// Everything a projection helper needs besides the node arena.
pub(crate) struct ProjectionSpec<'a> {
    pub nodes: &'a [TypeNode],
    pub alloc_index: u32,
    pub raise_index: u32,
    /// Absolute address of the shared null cell.
    pub null_cell: u32,
    /// kind-2 "arg" (tag mismatch).
    pub arg_msg_addr: u32,
    /// kind-2 "i32" (i32 range).
    pub i32_msg_addr: u32,
    /// kind-2 "field" (missing record field).
    pub field_msg_addr: u32,
}

/// The shared projection helper signature.
pub(crate) fn projection_signature() -> (Vec<ValType>, Option<ValType>) {
    (vec![ValType::I32], Some(ValType::I32))
}

/// Declared local count of one projection helper.
pub(crate) fn projection_local_count() -> u32 {
    PROJECTION_MAX_DEPTH * 7
}

/// Raises a kind-2 wrapper error and never returns (`unreachable`).
fn raise(function: &mut Function, spec: &ProjectionSpec, message: u32, len: u32) {
    function.instruction(&Instruction::I32Const(2));
    function.instruction(&Instruction::I32Const(message as i32));
    function.instruction(&Instruction::I32Const(len as i32));
    function.instruction(&Instruction::Call(spec.raise_index));
    function.instruction(&Instruction::Unreachable);
}

/// Emits one projection helper for node `node`. Locals: param 0 is the source
/// cell, then `PROJECTION_MAX_DEPTH * 7` i32 slots; depth `d` uses slots
/// `1 + d*7 .. 1 + d*7 + 6` named 0=iter, 1=count, 2=base/region, 3=src cell,
/// 4=dst, 5=flag, 6=tmp.
pub(crate) fn emit_projection_helper(
    spec: &ProjectionSpec,
    nodes: &[TypeNode],
    node: u32,
    descriptor_addr: Option<u32>,
) -> Result<Function, EmitError> {
    let mut function = Function::new([(projection_local_count(), ValType::I32)]);
    let root = nodes
        .get(node as usize)
        .ok_or(EmitError::Internal("projection node out of range"))?;
    emit_node(&mut function, spec, root, descriptor_addr, 0, 0)?;
    function.instruction(&Instruction::End);
    Ok(function)
}

/// Per-depth scratch slot base: depth `d` uses slots `1 + d*7 .. 1 + d*7 + 6`
/// (0=iter, 1=count, 2=base/region, 3=src cell, 4=dst, 5=flag, 6=tmp).
fn slots(depth: u32) -> Result<u32, EmitError> {
    depth
        .checked_mul(7)
        .and_then(|offset| offset.checked_add(1))
        .ok_or(EmitError::Internal("projection depth slot overflow"))
}

/// Recursive generation-time descent (depth-capped; see the module docs):
/// emits the validator for one node reading cell local `src` and LEAVES the
/// projected cell pointer on the stack.
fn emit_node(
    function: &mut Function,
    spec: &ProjectionSpec,
    node: &TypeNode,
    descriptor_addr: Option<u32>,
    src: u32,
    depth: u32,
) -> Result<(), EmitError> {
    if depth >= PROJECTION_MAX_DEPTH {
        return Err(EmitError::LimitExceeded {
            code: "B001",
            limit: u64::from(PROJECTION_MAX_DEPTH),
            observed: u64::from(depth),
        });
    }
    match &node.kind {
        TypeNodeKind::I32 => {
            // decoded MessagePack integers are I64-tagged (see decode)
            require_tag(function, spec, src, TAG_I64, spec.i32_msg_addr, 3);
            // payload in [i32::MIN, i32::MAX] (i64 signed compare)
            function.instruction(&Instruction::LocalGet(src));
            function.instruction(&Instruction::I64Load(mem64(8)));
            function.instruction(&Instruction::I64Const(i64::from(i32::MIN)));
            function.instruction(&Instruction::I64GeS);
            function.instruction(&Instruction::LocalGet(src));
            function.instruction(&Instruction::I64Load(mem64(8)));
            function.instruction(&Instruction::I64Const(i64::from(i32::MAX)));
            function.instruction(&Instruction::I64LeS);
            function.instruction(&Instruction::I32And);
            function.instruction(&Instruction::I32Eqz);
            function.instruction(&Instruction::If(BlockType::Empty));
            raise(function, spec, spec.i32_msg_addr, 3);
            function.instruction(&Instruction::End);
            // pass-through: the decoded cell is already the canonical cell
            function.instruction(&Instruction::LocalGet(src));
        }
        TypeNodeKind::I64 => {
            require_tag(function, spec, src, TAG_I64, spec.arg_msg_addr, 3);
            function.instruction(&Instruction::LocalGet(src));
        }
        TypeNodeKind::F64 => {
            require_tag(function, spec, src, TAG_F64, spec.arg_msg_addr, 3);
            // the decoder guarantees finite floats
            function.instruction(&Instruction::LocalGet(src));
        }
        TypeNodeKind::Bool => {
            require_tag(function, spec, src, TAG_BOOL, spec.arg_msg_addr, 3);
            function.instruction(&Instruction::LocalGet(src));
        }
        TypeNodeKind::String => {
            require_tag(function, spec, src, TAG_STRING, spec.arg_msg_addr, 3);
            function.instruction(&Instruction::LocalGet(src));
        }
        TypeNodeKind::Bytes => {
            require_tag(function, spec, src, TAG_BYTES, spec.arg_msg_addr, 3);
            function.instruction(&Instruction::LocalGet(src));
        }
        TypeNodeKind::Null => {
            require_tag(function, spec, src, TAG_NULL, spec.arg_msg_addr, 3);
            function.instruction(&Instruction::LocalGet(src));
        }
        TypeNodeKind::Optional(child) => {
            function.instruction(&Instruction::LocalGet(src));
            function.instruction(&Instruction::I32Load(mem32(0)));
            function.instruction(&Instruction::I32Const(TAG_NULL));
            function.instruction(&Instruction::I32Eq);
            // `wasm-encoder`'s `BlockType::Result` gives the if an i32 result
            function.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
            function.instruction(&Instruction::I32Const(spec.null_cell as i32));
            function.instruction(&Instruction::Else);
            let child_node = spec
                .nodes
                .get(*child as usize)
                .ok_or(EmitError::Internal("projection child out of range"))?;
            emit_node(function, spec, child_node, descriptor_addr, src, depth + 1)?;
            function.instruction(&Instruction::End);
        }
        TypeNodeKind::List(child) => {
            require_tag(function, spec, src, TAG_LIST, spec.arg_msg_addr, 3);
            emit_list(function, spec, *child, descriptor_addr, src, depth)?;
        }
        TypeNodeKind::Record(fields) => {
            // the decoder represents wire maps as dynamic map cells
            require_tag(function, spec, src, TAG_DYNMAP, spec.arg_msg_addr, 3);
            let descriptor = descriptor_addr.ok_or(EmitError::Internal(
                "record projection without a baked descriptor",
            ))?;
            emit_record(function, spec, fields, descriptor, src, depth)?;
        }
    }
    Ok(())
}

/// Emits `tag(src) != expected -> raise(message)`; leaves nothing on the
/// stack.
fn require_tag(
    function: &mut Function,
    spec: &ProjectionSpec,
    src: u32,
    tag: i32,
    message: u32,
    len: u32,
) {
    function.instruction(&Instruction::LocalGet(src));
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::I32Const(tag));
    function.instruction(&Instruction::I32Ne);
    function.instruction(&Instruction::If(BlockType::Empty));
    raise(function, spec, message, len);
    function.instruction(&Instruction::End);
}

/// `alloc(size)` call: leaves the region pointer on the stack.
fn alloc(function: &mut Function, spec: &ProjectionSpec, size: i32) {
    function.instruction(&Instruction::I32Const(size));
    function.instruction(&Instruction::Call(spec.alloc_index));
}

/// Copies 16 bytes (payload/aux/tag) from cell `from` to cell `to`.
fn copy_cell(function: &mut Function, from: u32, to: u32) {
    function.instruction(&Instruction::LocalGet(to));
    function.instruction(&Instruction::LocalGet(from));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I64Store(mem64(8)));
    function.instruction(&Instruction::LocalGet(to));
    function.instruction(&Instruction::LocalGet(from));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::I32Store(mem32(4)));
    function.instruction(&Instruction::LocalGet(to));
    function.instruction(&Instruction::LocalGet(from));
    function.instruction(&Instruction::I32Load(mem32(0)));
    function.instruction(&Instruction::I32Store(mem32(0)));
}

/// `alloc(size)` with a runtime-computed size held in local `size_local`
/// (leaves the region pointer on the stack).
fn alloc_local(function: &mut Function, spec: &ProjectionSpec, size_local: u32) {
    function.instruction(&Instruction::LocalGet(size_local));
    function.instruction(&Instruction::Call(spec.alloc_index));
}

/// Projects `List(inner)`: validates the tag, allocates a fresh element
/// region and iterates `count` elements (EMITTED code is an iterative
/// `block { loop { ... } }`); leaves the new list cell on the stack.
fn emit_list(
    function: &mut Function,
    spec: &ProjectionSpec,
    child: u32,
    descriptor_addr: Option<u32>,
    src: u32,
    depth: u32,
) -> Result<(), EmitError> {
    let base = slots(depth)?;
    let iter = base;
    let count = base + 1;
    let region = base + 2;
    let src_elem = base + 3;
    let dst = base + 4;
    let tmp = base + 6;
    let child_node = spec
        .nodes
        .get(child as usize)
        .ok_or(EmitError::Internal("projection child out of range"))?;
    // count = aux(src)
    function.instruction(&Instruction::LocalGet(src));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalSet(count));
    // region = alloc(count << 4); counts are runtime-capped far below 2^28
    function.instruction(&Instruction::LocalGet(count));
    function.instruction(&Instruction::I32Const(4));
    function.instruction(&Instruction::I32Shl);
    function.instruction(&Instruction::LocalSet(tmp));
    alloc_local(function, spec, tmp);
    function.instruction(&Instruction::LocalSet(region));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(iter));
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    // i >= count -> done
    function.instruction(&Instruction::LocalGet(iter));
    function.instruction(&Instruction::LocalGet(count));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1));
    // src_elem = payload-low(src) + i * 16; dst = region + i * 16
    function.instruction(&Instruction::LocalGet(src));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalGet(iter));
    function.instruction(&Instruction::I32Const(16));
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(src_elem));
    function.instruction(&Instruction::LocalGet(region));
    function.instruction(&Instruction::LocalGet(iter));
    function.instruction(&Instruction::I32Const(16));
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(dst));
    // <inner> leaves the projected element cell; stash it, then copy it
    emit_node(
        function,
        spec,
        child_node,
        descriptor_addr,
        src_elem,
        depth + 1,
    )?;
    function.instruction(&Instruction::LocalSet(tmp));
    copy_cell(function, tmp, dst);
    // i += 1 and loop
    function.instruction(&Instruction::LocalGet(iter));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(iter));
    function.instruction(&Instruction::Br(0));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    // build the list cell: alloc 16, tag 8, aux count, payload = region
    alloc(function, spec, 16);
    function.instruction(&Instruction::LocalSet(tmp));
    function.instruction(&Instruction::LocalGet(tmp));
    function.instruction(&Instruction::I32Const(TAG_LIST));
    function.instruction(&Instruction::I32Store(mem32(0)));
    function.instruction(&Instruction::LocalGet(tmp));
    function.instruction(&Instruction::LocalGet(count));
    function.instruction(&Instruction::I32Store(mem32(4)));
    function.instruction(&Instruction::LocalGet(tmp));
    function.instruction(&Instruction::LocalGet(region));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::I64Store(mem64(8)));
    function.instruction(&Instruction::LocalGet(tmp));
    Ok(())
}

/// Scratch slots of one record's field scans (`slots(depth)` frame).
struct RecordFrame {
    pair: u32,
    count: u32,
    key: u32,
    value: u32,
    flag: u32,
    iter: u32,
    tmp: u32,
    region: u32,
}

/// Projects `Record(fields)`: allocates the field region, matches each wire
/// map key against the (generation-time) field names in sorted order, fills
/// omitted optional fields with the null cell, raises "field" for missing
/// required fields, then builds the record cell. Per-field key scans are
/// iterative loops; field order is the canonical name-sorted order, the same
/// order the lowering uses for the record's cell layout.
fn emit_record(
    function: &mut Function,
    spec: &ProjectionSpec,
    fields: &[RecordField],
    descriptor: u32,
    src: u32,
    depth: u32,
) -> Result<(), EmitError> {
    let base = slots(depth)?;
    let frame = RecordFrame {
        pair: base,
        count: base + 1,
        region: base + 2,
        key: base + 3,
        value: base + 4,
        flag: base + 5,
        iter: base + 6,
        // `tmp` shares the `key` slot: the key cell is dead by the time the
        // matched/missing paths need scratch
        tmp: base + 3,
    };
    // pair base = payload-low(src)
    function.instruction(&Instruction::LocalGet(src));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalSet(frame.pair));
    // count = aux(src) (pair count of the dynamic map)
    function.instruction(&Instruction::LocalGet(src));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::LocalSet(frame.count));
    let field_count = i32::try_from(fields.len())
        .map_err(|_| EmitError::Internal("record field count overflow"))?;
    let region_bytes = i32::try_from(
        fields
            .len()
            .checked_mul(16)
            .ok_or(EmitError::Internal("record region size overflow"))?,
    )
    .map_err(|_| EmitError::Internal("record region size overflow"))?;
    // region = alloc(fields.len() << 4)
    alloc(function, spec, region_bytes);
    function.instruction(&Instruction::LocalSet(frame.region));
    for (field_index, field) in fields.iter().enumerate() {
        emit_field(function, spec, field, field_index, &frame, depth)?;
    }
    // build the record cell: alloc 16, tag 9, aux fields.len(),
    // payload = (descriptor << 32) | region (region zero-extended)
    alloc(function, spec, 16);
    function.instruction(&Instruction::LocalSet(frame.tmp));
    function.instruction(&Instruction::LocalGet(frame.tmp));
    function.instruction(&Instruction::I32Const(TAG_RECORD));
    function.instruction(&Instruction::I32Store(mem32(0)));
    function.instruction(&Instruction::LocalGet(frame.tmp));
    function.instruction(&Instruction::I32Const(field_count));
    function.instruction(&Instruction::I32Store(mem32(4)));
    function.instruction(&Instruction::LocalGet(frame.tmp));
    function.instruction(&Instruction::I64Const(i64::from(descriptor) << 32));
    function.instruction(&Instruction::LocalGet(frame.region));
    function.instruction(&Instruction::I64ExtendI32U);
    function.instruction(&Instruction::I64Or);
    function.instruction(&Instruction::I64Store(mem64(8)));
    function.instruction(&Instruction::LocalGet(frame.tmp));
    Ok(())
}

/// One field's scan: a found-flag loop over the map pairs, then a
/// missing-field block (optional null fill or `raise field`). Slot roles:
/// `pair` holds the pair base, `key`/`value` the current key/value cells,
/// `flag` the found flag, `iter` the scan counter (reused as the destination
/// once a match `br`s out — the counter is dead then), `tmp` a scratch cell.
///
/// NOTE: nested record fields pass no descriptor of their own; a record
/// whose fields contain records is rejected loudly at emission this
/// milestone (`descriptor_addr must be Some` fires as an internal error).
fn emit_field(
    function: &mut Function,
    spec: &ProjectionSpec,
    field: &RecordField,
    field_index: usize,
    frame: &RecordFrame,
    depth: u32,
) -> Result<(), EmitError> {
    let name_bytes = field.name.as_bytes();
    let name_len = i32::try_from(name_bytes.len())
        .map_err(|_| EmitError::Internal("field name length overflow"))?;
    let child_node = spec
        .nodes
        .get(field.ty as usize)
        .ok_or(EmitError::Internal("projection child out of range"))?;
    let field_offset = i32::try_from(
        field_index
            .checked_mul(16)
            .ok_or(EmitError::Internal("record field offset overflow"))?,
    )
    .map_err(|_| EmitError::Internal("record field offset overflow"))?;
    // i = 0; found-flag = 0
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(frame.iter));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(frame.flag));
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    // i >= count -> not found in this map
    function.instruction(&Instruction::LocalGet(frame.iter));
    function.instruction(&Instruction::LocalGet(frame.count));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1));
    // key cell = pair + i * 32; value cell = key + 16
    function.instruction(&Instruction::LocalGet(frame.pair));
    function.instruction(&Instruction::LocalGet(frame.iter));
    function.instruction(&Instruction::I32Const(32));
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(frame.key));
    function.instruction(&Instruction::LocalGet(frame.key));
    function.instruction(&Instruction::I32Const(16));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(frame.value));
    // found = (klen == name_len); per byte: found &= load8u(kaddr + b) ==
    // name[b] (name bytes are GENERATION-TIME constants, not memory loads)
    function.instruction(&Instruction::LocalGet(frame.key));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::I32Const(name_len));
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::LocalSet(frame.flag));
    for (byte_index, byte) in name_bytes.iter().enumerate() {
        function.instruction(&Instruction::LocalGet(frame.flag));
        function.instruction(&Instruction::LocalGet(frame.key));
        function.instruction(&Instruction::I64Load(mem64(8)));
        function.instruction(&Instruction::I32WrapI64);
        function.instruction(&Instruction::I32Const(byte_index as i32));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::I32Load8U(byte_mem(0)));
        function.instruction(&Instruction::I32Const(i32::from(*byte)));
        function.instruction(&Instruction::I32Eq);
        function.instruction(&Instruction::I32And);
        function.instruction(&Instruction::LocalSet(frame.flag));
    }
    function.instruction(&Instruction::LocalGet(frame.flag));
    function.instruction(&Instruction::If(BlockType::Empty));
    // matched: <field.ty with the value cell> -> copy into dst, exit this
    // field's loop (the counter in `iter` is dead once we br out, so `iter`
    // is reused as the destination below)
    emit_node(function, spec, child_node, None, frame.value, depth + 1)?;
    function.instruction(&Instruction::LocalSet(frame.tmp));
    function.instruction(&Instruction::LocalGet(frame.region));
    function.instruction(&Instruction::I32Const(field_offset));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(frame.iter));
    copy_cell(function, frame.tmp, frame.iter);
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::LocalSet(frame.flag));
    function.instruction(&Instruction::Br(1));
    function.instruction(&Instruction::End);
    // i += 1 and loop
    function.instruction(&Instruction::LocalGet(frame.iter));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(frame.iter));
    function.instruction(&Instruction::Br(0));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    // missing-field handling: flag == 0 -> optional fill OR raise
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(frame.flag));
    function.instruction(&Instruction::BrIf(0));
    if matches!(child_node.kind, TypeNodeKind::Optional(_)) {
        // omitted optional field reads as the null cell: store it into
        // dst = region + field * 16 (computed again here)
        function.instruction(&Instruction::LocalGet(frame.region));
        function.instruction(&Instruction::I32Const(field_offset));
        function.instruction(&Instruction::I32Add);
        function.instruction(&Instruction::LocalSet(frame.iter));
        function.instruction(&Instruction::LocalGet(frame.iter));
        function.instruction(&Instruction::I32Const(spec.null_cell as i32));
        function.instruction(&Instruction::I32Store(mem32(0)));
        function.instruction(&Instruction::LocalGet(frame.iter));
        function.instruction(&Instruction::I32Const(0));
        function.instruction(&Instruction::I32Store(mem32(4)));
        function.instruction(&Instruction::LocalGet(frame.iter));
        function.instruction(&Instruction::I64Const(0));
        function.instruction(&Instruction::I64Store(mem64(8)));
    } else {
        raise(function, spec, spec.field_msg_addr, 5);
    }
    function.instruction(&Instruction::End);
    Ok(())
}
