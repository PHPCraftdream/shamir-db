/// Per-request durability level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// Ack after in-memory buffer; durable on background tick.
    Buffered,
    /// Flush durable backing before ack.
    Synced,
}

impl Durability {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Durability::Buffered => "buffered",
            Durability::Synced => "synced",
        }
    }
}
