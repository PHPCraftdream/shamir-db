mod decode;
mod encode;
mod frames;
mod primitives;
#[cfg(test)]
mod tests;

pub(crate) use decode::{emit_decode_root, DecodeEmit};
pub(crate) use encode::{emit_encode_root, EncodeEmit};
pub(crate) use frames::{
    CONTEXT_CELL_BASE_OFFSET, CONTEXT_FRAME_BASE_OFFSET, CONTEXT_HEADER_BYTES,
    CONTEXT_INPUT_LEN_OFFSET, CONTEXT_INPUT_PTR_OFFSET, CONTEXT_KEEP_ORDER_OFFSET,
    CONTEXT_OUTPUT_CAP_OFFSET, CONTEXT_OUTPUT_LEN_OFFSET, CONTEXT_OUTPUT_PTR_OFFSET,
    CONTEXT_RESERVED_OFFSET, CONTEXT_SCRATCH_OFFSET, FRAME_BYTES, FRAME_COUNT,
};
pub(crate) use primitives::{
    emit_append_byte, emit_check_range, emit_compare_bytes, emit_copy_cells, emit_read_be,
    emit_read_byte, emit_store_cell, emit_validate_utf8,
};
