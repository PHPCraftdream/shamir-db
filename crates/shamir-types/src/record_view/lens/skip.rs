use super::cursor::{read_be, read_u8};
use super::{RecordViewError, MAX_MSGPACK_DEPTH};

// ---------------------------------------------------------------------------
// The skip + read primitives over the FULL marker set.
// ---------------------------------------------------------------------------

/// Skip ONE MessagePack value (any type) starting at `*pos`. Self-delimiting:
/// reads the marker, advances past the value's payload, recursing into nested
/// maps/arrays/ext. This is the O(1)-skip the lens uses to pass over fields it
/// does not match. Bounds-checked at every step; never panics.
///
/// `depth` bounds recursion — deeper than [`MAX_MSGPACK_DEPTH`] returns
/// [`RecordViewError::DepthExceeded`] (matches the canonical decoder's guard).
pub(crate) fn skip_value(buf: &[u8], pos: &mut usize, depth: usize) -> Result<(), RecordViewError> {
    if depth > MAX_MSGPACK_DEPTH {
        return Err(RecordViewError::DepthExceeded(RecordViewError::depth_cap()));
    }
    let start = *pos;
    let m = read_u8(buf, pos)?;
    match m {
        // positive fixint / negative fixint / nil / true / false — payload
        // already consumed (single-byte marker).
        0x00..=0x7f | 0xe0..=0xff | 0xc0 | 0xc2 | 0xc3 => Ok(()),
        // reserved marker — the canonical decoder rejects it; so do we.
        0xc1 => Err(RecordViewError::ReservedMarker(start)),
        // unsigned / signed ints with explicit width.
        0xcc | 0xd0 => {
            *pos = pos
                .checked_add(1)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xcd | 0xd1 => {
            *pos = pos
                .checked_add(2)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xce | 0xd2 => {
            *pos = pos
                .checked_add(4)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xcf | 0xd3 => {
            *pos = pos
                .checked_add(8)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        // floats.
        0xca => {
            *pos = pos
                .checked_add(4)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xcb => {
            *pos = pos
                .checked_add(8)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        // strings.
        0xa0..=0xbf => {
            *pos = pos
                .checked_add((m & 0x1f) as usize)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xd9 => {
            let len = read_be::<1>(buf, pos)? as usize;
            *pos = pos
                .checked_add(len)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xda => {
            let len = read_be::<2>(buf, pos)? as usize;
            *pos = pos
                .checked_add(len)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xdb => {
            let len = read_be::<4>(buf, pos)? as usize;
            *pos = pos
                .checked_add(len)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        // binary.
        0xc4 => {
            let len = read_be::<1>(buf, pos)? as usize;
            *pos = pos
                .checked_add(len)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xc5 => {
            let len = read_be::<2>(buf, pos)? as usize;
            *pos = pos
                .checked_add(len)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xc6 => {
            let len = read_be::<4>(buf, pos)? as usize;
            *pos = pos
                .checked_add(len)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        // arrays — skip each element.
        0x90..=0x9f => skip_seq(buf, pos, (m & 0x0f) as usize, depth),
        0xdc => {
            let n = read_be::<2>(buf, pos)? as usize;
            skip_seq(buf, pos, n, depth)
        }
        0xdd => {
            let n = read_be::<4>(buf, pos)? as usize;
            skip_seq(buf, pos, n, depth)
        }
        // maps — skip each (key, value) pair.
        0x80..=0x8f => skip_map(buf, pos, (m & 0x0f) as usize, depth),
        0xde => {
            let n = read_be::<2>(buf, pos)? as usize;
            skip_map(buf, pos, n, depth)
        }
        0xdf => {
            let n = read_be::<4>(buf, pos)? as usize;
            skip_map(buf, pos, n, depth)
        }
        // fixext — type byte + fixed-length payload.
        0xd4 => {
            *pos = pos
                .checked_add(1 + 1)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xd5 => {
            *pos = pos
                .checked_add(1 + 2)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xd6 => {
            *pos = pos
                .checked_add(1 + 4)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xd7 => {
            *pos = pos
                .checked_add(1 + 8)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xd8 => {
            *pos = pos
                .checked_add(1 + 16)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        // ext — length-prefixed payload + type byte.
        0xc7 => {
            let n = read_be::<1>(buf, pos)? as usize;
            *pos = pos
                .checked_add(1 + n)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xc8 => {
            let n = read_be::<2>(buf, pos)? as usize;
            *pos = pos
                .checked_add(1 + n)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
        0xc9 => {
            let n = read_be::<4>(buf, pos)? as usize;
            *pos = pos
                .checked_add(1 + n)
                .ok_or(RecordViewError::Truncated(start))?;
            Ok(())
        }
    }?;
    // Post-skip bounds check: any fixed-width arm above (`*pos += k`) may have
    // overrun a truncated buffer; catch it here in one place.
    if *pos > buf.len() {
        return Err(RecordViewError::Truncated(start));
    }
    Ok(())
}

#[inline]
fn skip_seq(buf: &[u8], pos: &mut usize, n: usize, depth: usize) -> Result<(), RecordViewError> {
    for _ in 0..n {
        skip_value(buf, pos, depth + 1)?;
    }
    Ok(())
}

#[inline]
fn skip_map(buf: &[u8], pos: &mut usize, n: usize, depth: usize) -> Result<(), RecordViewError> {
    for _ in 0..n {
        skip_value(buf, pos, depth + 1)?; // key
        skip_value(buf, pos, depth + 1)?; // value
    }
    Ok(())
}
