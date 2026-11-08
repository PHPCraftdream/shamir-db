use thiserror::Error;

pub(crate) const PAGE_BYTES: u64 = 65_536;
pub(crate) const GUARD_BYTES: u64 = 64;
pub(crate) const ERROR_START: u64 = GUARD_BYTES;
pub(crate) const ERROR_BYTES: u64 = 1_024;
pub(crate) const STATIC_START: u64 = ERROR_START + ERROR_BYTES;
pub(crate) const MAX_STATIC_BYTES: u64 = 4 * 1_024 * 1_024;
/// Frame-stack bytes; the workspace grows further by the cell budget so the
/// decoder cell region, the encoder sort window and the reject scratch stay
/// strictly below the heap base.
pub(crate) const WORKSPACE_BYTES: u64 = PAGE_BYTES;
pub(crate) const MAX_MEMORY_PAGES: u64 = 1_024;
pub(crate) const MAX_ADDRESS: u64 = 1_u64 << 31;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeCaps {
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub string_bytes: u64,
    pub key_bytes: u64,
    pub containers: u64,
    pub entries: u64,
    pub cells: u64,
    pub depth: u32,
}

impl Default for RuntimeCaps {
    fn default() -> Self {
        Self {
            input_bytes: 8 * 1_024 * 1_024,
            output_bytes: 8 * 1_024 * 1_024,
            string_bytes: 1_024 * 1_024,
            key_bytes: 64 * 1_024,
            containers: 100_000,
            entries: 100_000,
            cells: 1_000_000,
            depth: 32,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Layout {
    pub static_end: u64,
    pub workspace_start: u64,
    pub workspace_end: u64,
    pub heap_base: u64,
    pub minimum_pages: u64,
    pub maximum_pages: u64,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum LayoutError {
    #[error("static data exceeds the runtime cap")]
    StaticCap,
    #[error("layout arithmetic overflow")]
    Overflow,
    #[error("layout exceeds wasm32 address space")]
    AddressSpace,
    #[error("layout exceeds guest memory page cap")]
    MemoryCap,
}

impl Layout {
    /// `calculate` reserves `WORKSPACE_BYTES + cell_bytes` of workspace, where
    /// `cell_bytes` covers the decoder cell slots (`caps.cells * 16`), the
    /// encoder sort window (`caps.entries * 4`) and a 16-byte reject scratch.
    pub fn calculate(static_bytes: u64, cell_bytes: u64) -> Result<Self, LayoutError> {
        if static_bytes > MAX_STATIC_BYTES {
            return Err(LayoutError::StaticCap);
        }
        let static_end = STATIC_START
            .checked_add(static_bytes)
            .ok_or(LayoutError::Overflow)?;
        let workspace_start = align8(static_end)?;
        let workspace_end = workspace_start
            .checked_add(WORKSPACE_BYTES)
            .and_then(|sum| sum.checked_add(cell_bytes))
            .ok_or(LayoutError::Overflow)?;
        let heap_base = align8(workspace_end)?;
        let required_end = heap_base
            .checked_add(PAGE_BYTES)
            .ok_or(LayoutError::Overflow)?;
        if required_end > MAX_ADDRESS {
            return Err(LayoutError::AddressSpace);
        }
        let minimum_pages = ceil_div(required_end, PAGE_BYTES)?;
        if minimum_pages > MAX_MEMORY_PAGES {
            return Err(LayoutError::MemoryCap);
        }
        Ok(Self {
            static_end,
            workspace_start,
            workspace_end,
            heap_base,
            minimum_pages,
            maximum_pages: MAX_MEMORY_PAGES,
        })
    }
}

fn align8(value: u64) -> Result<u64, LayoutError> {
    value
        .checked_add(7)
        .map(|aligned| aligned & !7)
        .ok_or(LayoutError::Overflow)
}

fn ceil_div(value: u64, divisor: u64) -> Result<u64, LayoutError> {
    value
        .checked_add(divisor - 1)
        .map(|sum| sum / divisor)
        .ok_or(LayoutError::Overflow)
}
