use crate::back::runtime::{
    Layout, RuntimeCaps, RuntimeError, CONTEXT_HEADER_BYTES, FRAME_BYTES, FRAME_COUNT,
    MAX_STATIC_BYTES, PAGE_BYTES,
};

/// Bytes occupied by one runtime cell.
const CELL_SLOT_BYTES: u64 = 16;
/// Bytes occupied by one encoder pair-index slot.
const SORT_INDEX_BYTES: u64 = 4;
/// Bytes of the shared codec reject-message scratch slot.
const SCRATCH_BYTES: u64 = 16;

/// Static image length plus the workspace addresses derived from the caps.
pub(crate) struct Workspace {
    pub(crate) layout: Layout,
    pub(crate) context_base: u32,
    pub(crate) frame_base: u32,
    pub(crate) cell_base: u32,
    pub(crate) sort_base: u32,
    pub(crate) scratch: u32,
}

fn workspace_address(value: u64, memory_bytes: u64) -> Result<u32, RuntimeError> {
    if value > memory_bytes {
        return Err(RuntimeError::RuntimeTooLarge {
            limit: memory_bytes,
            observed: value,
        });
    }
    match i32::try_from(value) {
        Ok(address) => Ok(address as u32),
        Err(_) => Err(RuntimeError::RuntimeTooLarge {
            limit: memory_bytes,
            observed: value,
        }),
    }
}

/// Layout math shared by the M1 runtime path and the compiled path.
pub(crate) fn compute_workspace(
    static_len: u64,
    caps: &RuntimeCaps,
) -> Result<Workspace, RuntimeError> {
    if static_len > MAX_STATIC_BYTES {
        return Err(RuntimeError::RuntimeTooLarge {
            limit: MAX_STATIC_BYTES,
            observed: static_len,
        });
    }
    // Workspace budget above the 64 KiB frame stack: decoder cell slots, the
    // encoder pair-index window and the reject scratch, all below the heap.
    let cell_slot_total = caps
        .cells
        .checked_mul(CELL_SLOT_BYTES)
        .ok_or(RuntimeError::CapsBeyondAddressSpace)?;
    let sort_slot_total = caps
        .entries
        .checked_mul(SORT_INDEX_BYTES)
        .ok_or(RuntimeError::CapsBeyondAddressSpace)?;
    let cell_bytes = cell_slot_total
        .checked_add(sort_slot_total)
        .and_then(|total| total.checked_add(SCRATCH_BYTES))
        .ok_or(RuntimeError::CapsBeyondAddressSpace)?;
    let layout = Layout::calculate(static_len, cell_bytes)?;
    let memory_bytes = layout.maximum_pages * PAGE_BYTES;
    let cell_base = layout
        .workspace_start
        .checked_add(u64::from(FRAME_COUNT * FRAME_BYTES))
        .ok_or(RuntimeError::CapsBeyondAddressSpace)?;
    let sort_base = cell_base
        .checked_add(cell_slot_total)
        .ok_or(RuntimeError::CapsBeyondAddressSpace)?;
    let scratch = sort_base
        .checked_add(sort_slot_total)
        .ok_or(RuntimeError::CapsBeyondAddressSpace)?;
    let context_base = workspace_address(layout.workspace_start, memory_bytes)?;
    // frame 0 sits one FRAME_BYTES below this base, above the context header
    let frame_base = context_base + CONTEXT_HEADER_BYTES;
    let cell_base = workspace_address(cell_base, memory_bytes)?;
    let sort_base = workspace_address(sort_base, memory_bytes)?;
    let scratch = workspace_address(scratch, memory_bytes)?;
    Ok(Workspace {
        layout,
        context_base,
        frame_base,
        cell_base,
        sort_base,
        scratch,
    })
}
