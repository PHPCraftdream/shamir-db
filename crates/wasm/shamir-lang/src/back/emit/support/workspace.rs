use crate::back::runtime::{
    Cell, CellTag, Layout, RuntimeCaps, RuntimeError, CONTEXT_HEADER_BYTES, FRAME_BYTES,
    FRAME_COUNT, MAX_ADDRESS, MAX_STATIC_BYTES, PAGE_BYTES,
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

/// Design hard ceilings; exceeding any of them is a caller bug (the cap
/// values themselves are the supported envelope), rejected before layout.
const MAX_INPUT_BYTES: u64 = 8 * 1_024 * 1_024;
const MAX_OUTPUT_BYTES: u64 = 8 * 1_024 * 1_024;
const MAX_STRING_BYTES: u64 = 1_024 * 1_024;
const MAX_KEY_BYTES: u64 = 65_536;
const MAX_CONTAINERS: u64 = 100_000;
const MAX_ENTRIES: u64 = 100_000;
const MAX_CELLS: u64 = 1_000_000;
const MAX_DEPTH: u64 = 32;

/// Every cell tag must survive an encode/decode round trip before the wrapper
/// embeds the null-cell constants, keeping the cell codec exercised in
/// production.
const ALL_CELL_TAGS: [CellTag; 15] = [
    CellTag::Unused,
    CellTag::Null,
    CellTag::Bool,
    CellTag::I32,
    CellTag::I64,
    CellTag::F64,
    CellTag::String,
    CellTag::Bytes,
    CellTag::List,
    CellTag::Record,
    CellTag::Validation,
    CellTag::Batch,
    CellTag::Request,
    CellTag::Response,
    CellTag::DynamicMap,
];

pub(crate) fn validate_caps(caps: &RuntimeCaps) -> Result<(), RuntimeError> {
    // design hard ceilings first: a cap above these is a configuration bug
    if caps.input_bytes > MAX_INPUT_BYTES
        || caps.output_bytes > MAX_OUTPUT_BYTES
        || caps.string_bytes > MAX_STRING_BYTES
        || caps.key_bytes > MAX_KEY_BYTES
        || caps.containers > MAX_CONTAINERS
        || caps.entries > MAX_ENTRIES
        || caps.cells > MAX_CELLS
        || u64::from(caps.depth) > MAX_DEPTH
    {
        return Err(RuntimeError::CapAboveCeiling);
    }
    let within_limit = |value: u64| value <= MAX_ADDRESS;
    if !within_limit(caps.input_bytes)
        || !within_limit(caps.output_bytes)
        || !within_limit(caps.string_bytes)
        || !within_limit(caps.key_bytes)
        || !within_limit(caps.containers)
        || !within_limit(caps.entries)
        || !within_limit(caps.cells)
        || !within_limit(u64::from(caps.depth))
    {
        return Err(RuntimeError::CapsBeyondAddressSpace);
    }
    Ok(())
}

pub(crate) fn validate_cell_codec() -> Result<[u8; 16], RuntimeError> {
    for &tag in ALL_CELL_TAGS.iter() {
        let cell = Cell::new(tag, 0, 0);
        let Some(decoded) = Cell::decode(cell.encode()) else {
            return Err(RuntimeError::CellTemplate);
        };
        if decoded.tag() != tag || decoded.auxiliary() != 0 || decoded.payload() != 0 {
            return Err(RuntimeError::CellTemplate);
        }
    }
    Ok(Cell::new(CellTag::Null, 0, 0).encode())
}
