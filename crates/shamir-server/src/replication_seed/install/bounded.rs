use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::replication_seed::error::invalid;
use crate::replication_seed::SeedError;

pub(super) fn read_bounded(path: &Path, cap: u64) -> Result<Vec<u8>, SeedError> {
    let file = File::open(path)?;
    let expected = file.metadata()?.len();
    if expected > cap {
        return Err(invalid("seed file byte budget exceeded"));
    }
    let limit = cap
        .checked_add(1)
        .ok_or_else(|| invalid("seed read cap overflow"))?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    let actual = u64::try_from(bytes.len()).map_err(|e| invalid(e.to_string()))?;
    if actual > cap || actual != expected {
        return Err(invalid("seed file length changed or exceeded budget"));
    }
    Ok(bytes)
}
