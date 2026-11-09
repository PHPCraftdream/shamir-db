use shamir_connect::server::token::OwnerFloors;
use std::sync::Arc;

use crate::user_directory::FjallUserDirectory;

/// Adapts the durable user-directory cache to token owner-floor verification.
pub struct DirectoryOwnerFloors {
    directory: Arc<FjallUserDirectory>,
}

impl DirectoryOwnerFloors {
    /// Create an adapter retaining the process-wide user directory.
    pub fn new(directory: Arc<FjallUserDirectory>) -> Self {
        Self { directory }
    }
}

impl OwnerFloors for DirectoryOwnerFloors {
    fn floor_if_present(&self, user_id: &[u8; 16]) -> Option<u64> {
        self.directory.floor_if_present(user_id)
    }
}
