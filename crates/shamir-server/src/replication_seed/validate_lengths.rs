use super::error::invalid;
use super::SeedError;

/// Validate every MessagePack prefix before serde can reserve a container.
/// Each array element needs one remaining byte; each map pair needs two.
/// Arrays and maps allow 65,536 elements/pairs, matching RecordShape's cap.
/// No allocations, at most 64 nested containers, and a caller-bounded slice.
pub(crate) fn validate_lengths(bytes: &[u8]) -> Result<(), SeedError> {
    let mut position = 0;
    value(bytes, &mut position, 0)?;
    if position != bytes.len() {
        return Err(invalid("trailing MessagePack bytes"));
    }
    Ok(())
}

fn take<'a>(bytes: &'a [u8], position: &mut usize, len: usize) -> Result<&'a [u8], SeedError> {
    let end = position
        .checked_add(len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| invalid("MessagePack length exceeds remaining bytes"))?;
    let result = &bytes[*position..end];
    *position = end;
    Ok(result)
}

fn length(bytes: &[u8], position: &mut usize, width: usize) -> Result<usize, SeedError> {
    let mut len = 0usize;
    for byte in take(bytes, position, width)? {
        len = len
            .checked_mul(256)
            .and_then(|n| n.checked_add(usize::from(*byte)))
            .ok_or_else(|| invalid("MessagePack length overflow"))?;
    }
    Ok(len)
}

fn value(bytes: &[u8], position: &mut usize, depth: usize) -> Result<(), SeedError> {
    if depth >= 64 {
        return Err(invalid("MessagePack depth exceeds bounds"));
    }
    let marker = take(bytes, position, 1)?[0];
    let (members, payload) = match marker {
        0x00..=0x7f | 0xc0 | 0xc2 | 0xc3 | 0xe0..=0xff => (0, 0),
        0x80..=0x8f => (usize::from(marker & 15) * 2, 0),
        0x90..=0x9f => (usize::from(marker & 15), 0),
        0xa0..=0xbf => (0, usize::from(marker & 31)),
        0xcc | 0xd0 => (0, 1),
        0xcd | 0xd1 => (0, 2),
        0xca | 0xce | 0xd2 => (0, 4),
        0xcb | 0xcf | 0xd3 => (0, 8),
        0xc4 | 0xd9 => (0, length(bytes, position, 1)?),
        0xc5 | 0xda => (0, length(bytes, position, 2)?),
        0xc6 | 0xdb => (0, length(bytes, position, 4)?),
        0xdc => (length(bytes, position, 2)?, 0),
        0xdd => (length(bytes, position, 4)?, 0),
        0xde | 0xdf => {
            let width = if marker == 0xde { 2 } else { 4 };
            let count = length(bytes, position, width)?;
            (
                count
                    .checked_mul(2)
                    .ok_or_else(|| invalid("map length overflow"))?,
                0,
            )
        }
        // Seed codecs contain no extension values; fail closed before decoding.
        _ => return Err(invalid("unsupported MessagePack marker")),
    };
    if members > bytes.len() - *position {
        return Err(invalid("MessagePack length exceeds remaining bytes"));
    }
    let count = if matches!(marker, 0x80..=0x8f | 0xde | 0xdf) {
        members / 2
    } else {
        members
    };
    if count > 65_536 {
        return Err(invalid(
            "MessagePack container exceeds 65536 elements/pairs",
        ));
    }
    take(bytes, position, payload)?;
    for _ in 0..members {
        value(bytes, position, depth + 1)?;
    }
    Ok(())
}
