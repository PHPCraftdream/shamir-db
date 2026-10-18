//! Index status for synchronization tracking

/// Status of index synchronization with disk
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IndexStatus {
    /// Index matches disk state
    Actual = 0,
    /// Index was modified, needs to be saved
    Pending = 1,
    /// Index is being saved to disk
    Saving = 2,
}

impl IndexStatus {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Actual,
            1 => Self::Pending,
            _ => Self::Saving,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }
}
