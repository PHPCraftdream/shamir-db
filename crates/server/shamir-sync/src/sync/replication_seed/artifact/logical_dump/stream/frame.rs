use std::io::{Read, Write};

use sha2::{Digest, Sha256};

use crate::replication_seed::error::invalid;
use crate::replication_seed::SeedError;

pub(crate) const MAGIC: &[u8; 8] = b"SDBLDMP\n";
pub(crate) const VERSION: u16 = 1;
pub(crate) const SUPPORTED_FLAGS: u16 = 0;
pub(crate) const CHECKSUM_BYTES: usize = 32;
pub(crate) const FRAME_OVERHEAD: usize = 4 + CHECKSUM_BYTES;

pub(crate) struct Frame {
    pub payload: Vec<u8>,
    pub wire: Vec<u8>,
}

pub(crate) fn encode_frame(sequence: u64, payload: &[u8]) -> Result<Vec<u8>, SeedError> {
    if payload.len() > crate::replication_seed::codec::FRAME_LIMIT
        || payload.len() as u64 > crate::replication_seed::codec::CHUNK_LIMIT
    {
        return Err(invalid("logical-dump frame exceeds 16 MiB"));
    }
    let length = u32::try_from(payload.len()).map_err(|_| invalid("frame length overflow"))?;
    let mut wire = Vec::with_capacity(payload.len() + FRAME_OVERHEAD);
    wire.extend_from_slice(&length.to_be_bytes());
    wire.extend_from_slice(payload);
    let mut checksum = Sha256::new();
    checksum.update(sequence.to_be_bytes());
    checksum.update(length.to_be_bytes());
    checksum.update(payload);
    wire.extend_from_slice(&checksum.finalize());
    Ok(wire)
}

pub(crate) fn write_all<W: Write>(output: &mut W, bytes: &[u8]) -> Result<(), SeedError> {
    output.write_all(bytes)?;
    Ok(())
}

pub(crate) fn read_counted<R: Read>(input: &mut R, buffer: &mut [u8]) -> Result<(), SeedError> {
    input.read_exact(buffer)?;
    Ok(())
}

pub(crate) fn read_frame<R: Read>(
    input: &mut R,
    offset: u64,
    sequence: u64,
) -> Result<Frame, SeedError> {
    let mut length_bytes = [0u8; 4];
    read_counted(input, &mut length_bytes)?;
    let length = u32::from_be_bytes(length_bytes) as usize;
    if length == 0
        || length > crate::replication_seed::codec::FRAME_LIMIT
        || length as u64 > crate::replication_seed::codec::CHUNK_LIMIT
    {
        return Err(invalid("logical-dump frame length is outside limits"));
    }
    let wire_size = (length + FRAME_OVERHEAD) as u64;
    if offset
        .checked_add(wire_size)
        .is_none_or(|total| total > crate::replication_seed::codec::ARTIFACT_LIMIT)
    {
        return Err(invalid("logical-dump artifact exceeds 1 TiB"));
    }
    let mut payload = vec![0; length];
    read_counted(input, &mut payload)?;
    let mut received_checksum = [0u8; CHECKSUM_BYTES];
    read_counted(input, &mut received_checksum)?;
    let mut checksum = Sha256::new();
    checksum.update(sequence.to_be_bytes());
    checksum.update(length_bytes);
    checksum.update(&payload);
    let calculated: [u8; CHECKSUM_BYTES] = checksum.finalize().into();
    if calculated != received_checksum {
        return Err(invalid("logical-dump frame checksum mismatch"));
    }
    let mut wire = Vec::with_capacity(length + FRAME_OVERHEAD);
    wire.extend_from_slice(&length_bytes);
    wire.extend_from_slice(&payload);
    wire.extend_from_slice(&received_checksum);
    Ok(Frame { payload, wire })
}
