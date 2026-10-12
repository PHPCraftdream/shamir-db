use std::io::{self, Write};

use serde::Serialize;

use crate::replication_seed::error::invalid;
use crate::replication_seed::SeedError;

/// Serialization refuses before extending the buffer beyond its byte budget.
struct BoundedBuffer {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl Write for BoundedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let size = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("serialization byte overflow"))?;
        if size > self.limit {
            self.exceeded = true;
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
        exceeded: false,
    };
    // rmp_serde replaces the writer's io error with a generic message, so the
    // budget refusal is recovered from the flag.
    if let Err(error) = rmp_serde::encode::write_named(&mut buffer, value) {
        if buffer.exceeded {
            return Err(invalid("serialization byte budget exceeded"));
        }
        return Err(invalid(error.to_string()));
    }
    Ok(buffer.bytes)
}
