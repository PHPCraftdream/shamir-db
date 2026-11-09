//! Independent reference MessagePack encoder for M1 test vectors.
//!
//! Deliberately NOT the rmp-based SDK codec: vectors must be built from
//! first principles so a shared encoding bug cannot hide. Emits the
//! shortest width for every value, maps in key-byte (canonical) order, and
//! mirrors the runtime codec's input contract (finite floats only, negative
//! zero normalized away).

/// `nil` (0xc0).
pub fn enc_nil() -> Vec<u8> {
    vec![0xc0]
}

/// `true` / `false` (0xc3 / 0xc2).
pub fn enc_bool(value: bool) -> Vec<u8> {
    vec![if value { 0xc3 } else { 0xc2 }]
}

/// Signed integer in the exact signed ranges, shortest per the MessagePack
/// spec (fixint, int8/16/32/64, uint8/16/32).
pub fn enc_i64(value: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(9);
    if (0..=127).contains(&value) {
        out.push(value as u8);
    } else if (-32..=-1).contains(&value) {
        out.push(0xe0 | (value as u8 & 0x1f));
    } else if (-128..=-33).contains(&value) {
        out.push(0xd0);
        out.push(value as i8 as u8);
    } else if (128..=255).contains(&value) {
        out.push(0xcc);
        out.push(value as u8);
    } else if (-32_768..=-129).contains(&value) {
        out.push(0xd1);
        out.extend_from_slice(&(value as i16).to_be_bytes());
    } else if (256..=65_535).contains(&value) {
        out.push(0xcd);
        out.extend_from_slice(&(value as u16).to_be_bytes());
    } else if (i64::from(i32::MIN)..=-32_769).contains(&value) {
        out.push(0xd2);
        out.extend_from_slice(&(value as i32).to_be_bytes());
    } else if (65_536..=i64::from(u32::MAX)).contains(&value) {
        out.push(0xce);
        out.extend_from_slice(&(value as u32).to_be_bytes());
    } else if value >= 0 {
        out.push(0xcf);
        out.extend_from_slice(&(value as u64).to_be_bytes());
    } else {
        out.push(0xd3);
        out.extend_from_slice(&value.to_be_bytes());
    }
    out
}

/// Finite f64 (0xcb, big endian); rejects non-finite input and normalizes
/// negative zero to positive zero, mirroring the runtime encoder.
pub fn enc_f64(value: f64) -> Vec<u8> {
    assert!(
        value.is_finite(),
        "reference encoder rejects non-finite floats"
    );
    let normalized = if value == 0.0 { 0.0 } else { value };
    let mut out = Vec::with_capacity(9);
    out.push(0xcb);
    out.extend_from_slice(&normalized.to_bits().to_be_bytes());
    out
}

/// String in the shortest width (fixstr below 32, str8, str16, str32).
pub fn enc_str(value: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + value.len());
    let len = value.len();
    if len <= 31 {
        out.push(0xa0 + len as u8);
    } else if len <= u8::MAX as usize {
        out.push(0xd9);
        out.push(len as u8);
    } else if len <= u16::MAX as usize {
        out.push(0xda);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0xdb);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
    out.extend_from_slice(value.as_bytes());
    out
}

/// Binary blob: bin has NO fix form, so the header is always a marker plus
/// a length (bin8 below 256, bin16 below 65,536, else bin32).
pub fn enc_bin(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + bytes.len());
    let len = bytes.len();
    if len <= u8::MAX as usize {
        out.push(0xc4);
        out.push(len as u8);
    } else if len <= u16::MAX as usize {
        out.push(0xc5);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0xc6);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
    out.extend_from_slice(bytes);
    out
}

/// Array in the shortest width (fixarray, array16, array32) over
/// pre-encoded elements.
pub fn enc_array(items: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    if items.len() <= 15 {
        out.push(0x90 + items.len() as u8);
    } else if items.len() <= u16::MAX as usize {
        out.push(0xdc);
        out.extend_from_slice(&(items.len() as u16).to_be_bytes());
    } else {
        out.push(0xdd);
        out.extend_from_slice(&(items.len() as u32).to_be_bytes());
    }
    for item in items {
        out.extend_from_slice(item);
    }
    out
}

/// UTF-8 payload of a pre-encoded MessagePack string (fixstr / str8 /
/// str16 / str32), used to sort map keys by payload bytes rather than full
/// encodings (two keys can encode to equal widths yet differ in payload).
pub fn key_payload(key: &[u8]) -> &[u8] {
    match key[0] {
        0xa0..=0xbf => &key[1..],
        0xd9 => &key[2..],
        0xda => &key[3..],
        0xdb => &key[5..],
        other => panic!("reference map key is not an encoded string: {other:#04x}"),
    }
}

/// Map with STRING keys in canonical UTF-8 key-byte order (fixmap, map16,
/// map32). Entries are `(key, value)` pairs of pre-encoded bytes.
pub fn enc_map_sorted(entries: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let mut sorted: Vec<&(Vec<u8>, Vec<u8>)> = entries.iter().collect();
    sorted.sort_by(|left, right| key_payload(&left.0).cmp(key_payload(&right.0)));
    let mut out = Vec::new();
    if sorted.len() <= 15 {
        out.push(0x80 + sorted.len() as u8);
    } else if sorted.len() <= u16::MAX as usize {
        out.push(0xde);
        out.extend_from_slice(&(sorted.len() as u16).to_be_bytes());
    } else {
        out.push(0xdf);
        out.extend_from_slice(&(sorted.len() as u32).to_be_bytes());
    }
    for (key, value) in sorted {
        out.extend_from_slice(key);
        out.extend_from_slice(value);
    }
    out
}

/// Positional call envelope from design section 3:
/// `{"0":a0,"1":a1,...,"args":[a0,a1,...]}` — numeric keys are authoritative
/// and the redundant `args` list is ignored by the consumer but still
/// depth-counted.
pub fn enc_call_envelope(args: &[i64]) -> Vec<u8> {
    let mut entries: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for (index, value) in args.iter().enumerate() {
        entries.push((enc_str(&index.to_string()), enc_i64(*value)));
    }
    let items: Vec<Vec<u8>> = args.iter().map(|value| enc_i64(*value)).collect();
    entries.push((enc_str("args"), enc_array(&items)));
    enc_map_sorted(&entries)
}
