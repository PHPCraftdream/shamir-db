mod alloc;
mod cell;
mod codec;
mod error;
mod layout;
#[cfg(test)]
mod tests;

pub(crate) use alloc::emit_allocator;
pub(crate) use cell::{Cell, CellTag};
pub(crate) use codec::{
    emit_append_byte, emit_check_range, emit_compare_bytes, emit_copy_cells, emit_decode_root,
    emit_encode_root, emit_read_be, emit_read_byte, emit_store_cell, emit_validate_utf8,
    DecodeEmit, EncodeEmit,
};
pub(crate) use codec::{
    CONTEXT_CELL_BASE_OFFSET, CONTEXT_FRAME_BASE_OFFSET, CONTEXT_HEADER_BYTES,
    CONTEXT_INPUT_LEN_OFFSET, CONTEXT_INPUT_PTR_OFFSET, CONTEXT_OUTPUT_CAP_OFFSET,
    CONTEXT_OUTPUT_LEN_OFFSET, CONTEXT_OUTPUT_PTR_OFFSET, CONTEXT_RESERVED_OFFSET,
    CONTEXT_SCRATCH_OFFSET, FRAME_BYTES, FRAME_COUNT,
};
pub(crate) use error::emit_error_raiser;
pub use error::RuntimeError;
pub use layout::RuntimeCaps;
pub(crate) use layout::{Layout, MAX_ADDRESS, MAX_STATIC_BYTES, PAGE_BYTES, STATIC_START};
