pub(crate) const FRAME_BYTES: u32 = 32;
pub(crate) const FRAME_COUNT: u32 = 2_048;
pub(crate) const CONTEXT_INPUT_PTR_OFFSET: u32 = 0;
pub(crate) const CONTEXT_INPUT_LEN_OFFSET: u32 = 4;
pub(crate) const CONTEXT_OUTPUT_PTR_OFFSET: u32 = 8;
pub(crate) const CONTEXT_OUTPUT_LEN_OFFSET: u32 = 12;
pub(crate) const CONTEXT_FRAME_BASE_OFFSET: u32 = 36;
pub(crate) const CONTEXT_CELL_BASE_OFFSET: u32 = 40;
pub(crate) const CONTEXT_SCRATCH_OFFSET: u32 = 44;
pub(crate) const CONTEXT_RESERVED_OFFSET: u32 = 48;
/// Output-buffer capacity in bytes, written by the wrapper before encoding;
/// `emit_append_byte` reads it as the defensive append bound.
pub(crate) const CONTEXT_OUTPUT_CAP_OFFSET: u32 = 52;
/// Header bytes reserved at the workspace start below the frame stack. Frame
/// zero sits at `frame_base - FRAME_BYTES`, so `frame_base` stored in the
/// context must stay at least `CONTEXT_HEADER_BYTES` above the workspace
/// start or the root frame would clobber the header (root cell pointer).
pub(crate) const CONTEXT_HEADER_BYTES: u32 = 96;
