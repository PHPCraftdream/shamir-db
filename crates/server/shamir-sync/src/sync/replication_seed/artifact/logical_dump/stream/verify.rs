use crate::replication_seed::artifact::logical_dump::header::LogicalDumpHeader;
use crate::replication_seed::artifact::logical_dump::stream::reader::DumpReader;
use crate::replication_seed::error::invalid;
use crate::replication_seed::SeedError;

/// Verified logical-dump summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DumpReport {
    pub bytes: u64,
    pub frames: u64,
    pub items: u64,
    pub sections: u64,
    pub header: LogicalDumpHeader,
    pub prefix_sha256: [u8; 32],
}

pub fn verify_dump<R: std::io::Read>(input: R) -> Result<DumpReport, SeedError> {
    let mut reader = DumpReader::new(input)?;
    while reader.next_item()?.is_some() {}
    reader.report()
}

pub fn verify_dump_file(path: impl AsRef<std::path::Path>) -> Result<DumpReport, SeedError> {
    let path = path.as_ref();
    let file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    preflight_artifact_size(size)?;
    verify_dump(file)
}

/// Reject oversized files before opening their contents or allocating buffers.
pub fn preflight_artifact_size(size: u64) -> Result<(), SeedError> {
    if size > crate::replication_seed::codec::ARTIFACT_LIMIT {
        return Err(invalid("logical-dump file exceeds 1 TiB"));
    }
    Ok(())
}
