//! Writer vs Inspect mode (decision D9, design 3.3).

/// Writer: the caller will write to the data dir (serve, token, seed
/// install, restore target). Inspect: the check itself must never create
/// or delete a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampMode {
    Writer,
    Inspect,
}
