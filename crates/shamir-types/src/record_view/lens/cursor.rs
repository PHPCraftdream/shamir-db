use super::RecordViewError;

// Test-only deterministic traversal accounting — counts skip_value/read_bin_len
// invocations per thread so tests can bound scan work without timing.
#[cfg(test)]
thread_local! {
    static SCAN_STEPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
#[inline]
pub(crate) fn bump_scan() {
    SCAN_STEPS.with(|c| c.set(c.get() + 1));
}

#[cfg(test)]
pub(crate) fn reset_scan_steps() {
    SCAN_STEPS.with(|c| c.set(0));
}

#[cfg(test)]
pub(crate) fn scan_steps() -> u64 {
    SCAN_STEPS.with(|c| c.get())
}

// ---------------------------------------------------------------------------
// Interned-key encoding — matches `InternerKey::serialize` (variable-width LE)
// and `eval_bytes::interned_key_bytes` exactly.
// ---------------------------------------------------------------------------

/// A minimal stack buffer for the variable-width interned key bytes.
/// Matches the encoding in `InternerKey::as_bytes_buf` / `eval_bytes::interned_key_bytes`.
pub(super) struct KeyBuf([u8; 8], usize);

impl KeyBuf {
    #[inline]
    pub(super) fn as_ref(&self) -> &[u8] {
        &self.0[..self.1]
    }
}

/// Encode a `u64` interned id to the same variable-width LE bytes that
/// `InternerKey::serialize` produces (1/2/4/8 bytes depending on magnitude).
#[inline]
pub(super) fn interned_key_bytes(id: u64) -> KeyBuf {
    let mut buf = [0u8; 8];
    let len = if id <= u8::MAX as u64 {
        buf[0] = id as u8;
        1
    } else if id <= u16::MAX as u64 {
        let b = (id as u16).to_le_bytes();
        buf[..2].copy_from_slice(&b);
        2
    } else if id <= u32::MAX as u64 {
        let b = (id as u32).to_le_bytes();
        buf[..4].copy_from_slice(&b);
        4
    } else {
        buf.copy_from_slice(&id.to_le_bytes());
        8
    };
    KeyBuf(buf, len)
}

// ---------------------------------------------------------------------------
// Low-level cursor primitives — read raw bytes with bounds checks. These are
// the inlined hot path; keeping them as standalone `&[u8]` + `&mut usize`
// functions (not a trait + `std::io::Cursor`) is what makes the scan cheap,
// matching the verified Stage-0 prototype.
// ---------------------------------------------------------------------------

/// Read one byte at `*pos`, advancing. Bounds-checked → [`RecordViewError`].
#[inline]
pub(super) fn read_u8(buf: &[u8], pos: &mut usize) -> Result<u8, RecordViewError> {
    let p = *pos;
    let b = *buf.get(p).ok_or(RecordViewError::Truncated(p))?;
    *pos = p + 1;
    Ok(b)
}

/// Read `N` big-endian bytes as a `u64`. Bounds-checked.
#[inline]
pub(super) fn read_be<const N: usize>(buf: &[u8], pos: &mut usize) -> Result<u64, RecordViewError> {
    let p = *pos;
    if p + N > buf.len() {
        return Err(RecordViewError::Truncated(p));
    }
    let mut v = 0u64;
    for i in 0..N {
        v = (v << 8) | buf[p + i] as u64;
    }
    *pos = p + N;
    Ok(v)
}

/// Read the map-header marker at `*pos` and return the entry count + the
/// updated cursor. Non-map markers → [`RecordViewError::NonMapTopLevel`].
#[inline]
pub(super) fn read_map_len(buf: &[u8], pos: &mut usize) -> Result<usize, RecordViewError> {
    let m = read_u8(buf, pos)?;
    match m {
        0x80..=0x8f => Ok((m & 0x0f) as usize),
        0xde => Ok(read_be::<2>(buf, pos)? as usize),
        0xdf => Ok(read_be::<4>(buf, pos)? as usize),
        other => Err(RecordViewError::NonMapTopLevel { got: other }),
    }
}

/// Read the bin-header marker at `*pos` and return the byte length of the
/// binary payload. Non-bin markers → [`RecordViewError::NonBinKey`] (used
/// for key reads in the storage form where keys are `InternerKey` serialised
/// as msgpack `bin`).
#[inline]
pub(super) fn read_bin_len(buf: &[u8], pos: &mut usize) -> Result<usize, RecordViewError> {
    #[cfg(test)]
    bump_scan();
    let start = *pos;
    let m = read_u8(buf, pos)?;
    match m {
        0xc4 => Ok(read_be::<1>(buf, pos)? as usize),
        0xc5 => Ok(read_be::<2>(buf, pos)? as usize),
        0xc6 => Ok(read_be::<4>(buf, pos)? as usize),
        _ => Err(RecordViewError::NonBinKey(start)),
    }
}

/// Read the str-header marker at `*pos` and return the byte length of the
/// string payload. Used for VALUE reads (not keys — keys are `bin` in the
/// storage form).
#[inline]
pub(super) fn read_str_len(buf: &[u8], pos: &mut usize) -> Result<usize, RecordViewError> {
    let start = *pos;
    let m = read_u8(buf, pos)?;
    match m {
        0xa0..=0xbf => Ok((m & 0x1f) as usize),
        0xd9 => Ok(read_be::<1>(buf, pos)? as usize),
        0xda => Ok(read_be::<2>(buf, pos)? as usize),
        0xdb => Ok(read_be::<4>(buf, pos)? as usize),
        _ => Err(RecordViewError::NonBinKey(start)),
    }
}

/// Borrow `len` bytes starting at `*pos`, advancing the cursor.
#[inline]
pub(super) fn borrow_bytes<'a>(
    buf: &'a [u8],
    pos: &mut usize,
    len: usize,
) -> Result<&'a [u8], RecordViewError> {
    let p = *pos;
    let end = p.checked_add(len).ok_or(RecordViewError::Truncated(p))?;
    if end > buf.len() {
        return Err(RecordViewError::Truncated(p));
    }
    let slice = &buf[p..end];
    *pos = end;
    Ok(slice)
}

/// Borrow `len` bytes as a `&str`, validating UTF-8.
#[inline]
pub(super) fn borrow_str<'a>(
    buf: &'a [u8],
    pos: &mut usize,
    len: usize,
) -> Result<&'a str, RecordViewError> {
    let bytes = borrow_bytes(buf, pos, len)?;
    std::str::from_utf8(bytes).map_err(|_| RecordViewError::InvalidUtf8(*pos - len))
}
