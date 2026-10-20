use crate::meta::repo_identity::RepoIdentity;
use serde::{Deserialize, Serialize};

/// The advancing bookmark and its upstream incarnation are inseparable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FenceRecord {
    pub bookmark: u64,
    pub lineage_id: [u8; 16],
    pub history_epoch: u64,
    pub observed_identity: Option<RepoIdentity>,
}
