/// Transaction isolation level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    /// Reads see a consistent snapshot; writes use last-writer-wins.
    Snapshot,
    /// Read-set validated at commit; concurrent write conflict aborts.
    Serializable,
}

impl Isolation {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Isolation::Snapshot => "snapshot",
            Isolation::Serializable => "serializable",
        }
    }
}
