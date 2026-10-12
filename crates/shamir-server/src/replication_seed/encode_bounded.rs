use std::io::{self, Write};

use serde::Serialize;

use super::error::invalid;
use super::SeedError;

/// Serialization refuses before extending the buffer beyond its byte budget.
struct BoundedBuffer {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for BoundedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let size = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("serialization byte overflow"))?;
        if size > self.limit {
            return Err(io::Error::other("serialization byte budget exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn encode_bounded<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, SeedError> {
    let mut buffer = BoundedBuffer {
        bytes: Vec::new(),
        limit,
    };
    rmp_serde::encode::write_named(&mut buffer, value).map_err(|e| invalid(e.to_string()))?;
    Ok(buffer.bytes)
}
