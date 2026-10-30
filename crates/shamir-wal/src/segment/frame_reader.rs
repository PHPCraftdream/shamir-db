//! Bounded incremental reader for length-prefixed WAL frames.

use std::io::{self, Read};

use shamir_storage::error::{DbError, DbResult};

use super::wal_segment::wal_io_error;

/// Maximum encoded payload, excluding the length and checksum. Existing WAL
/// frames above this limit fail recovery without truncation; new writes reject
/// them as well. This deliberately trades compatibility for bounded allocation.
pub(crate) const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

pub(crate) enum FrameRead {
    End,
    Incomplete {
        offset: u64,
        header: bool,
    },
    Frame {
        offset: u64,
        end: u64,
        payload: Vec<u8>,
        crc_ok: bool,
    },
}

pub(crate) struct FrameReader<R> {
    reader: R,
    length: u64,
    offset: u64,
    context: String,
}

impl<R: Read> FrameReader<R> {
    pub(crate) fn new(reader: R, length: u64, context: String) -> Self {
        Self {
            reader,
            length,
            offset: 0,
            context,
        }
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> DbResult<bool> {
        match self.reader.read_exact(buf) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
            Err(e) => Err(wal_io_error(self.context.clone(), e)),
        }
    }

    pub(crate) fn next(&mut self) -> DbResult<FrameRead> {
        let offset = self.offset;
        let remaining = self.length.saturating_sub(offset);
        if remaining == 0 {
            return Ok(FrameRead::End);
        }
        if remaining < 4 {
            return Ok(FrameRead::Incomplete {
                offset,
                header: true,
            });
        }
        let mut header = [0; 4];
        if !self.read_exact(&mut header)? {
            return Ok(FrameRead::Incomplete {
                offset,
                header: true,
            });
        }
        let len = u32::from_le_bytes(header) as usize;
        if len > MAX_FRAME_BYTES {
            return Err(DbError::Storage(format!(
                "WAL frame at offset {offset} exceeds {MAX_FRAME_BYTES}-byte payload limit"
            )));
        }
        if (len as u64) + 8 > remaining {
            return Ok(FrameRead::Incomplete {
                offset,
                header: false,
            });
        }
        let mut payload = Vec::new();
        payload.try_reserve_exact(len).map_err(|e| {
            DbError::Storage(format!(
                "WAL frame allocation at offset {offset} failed: {e}"
            ))
        })?;
        payload.resize(len, 0);
        let mut crc = [0; 4];
        if !self.read_exact(&mut payload)? || !self.read_exact(&mut crc)? {
            return Ok(FrameRead::Incomplete {
                offset,
                header: false,
            });
        }
        let end = offset + len as u64 + 8;
        self.offset = end;
        let crc_ok = crc32fast::hash(&payload) == u32::from_le_bytes(crc);
        Ok(FrameRead::Frame {
            offset,
            end,
            payload,
            crc_ok,
        })
    }
}
