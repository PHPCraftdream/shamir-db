use std::collections::BTreeSet;
use std::str;

use crate::replication_seed::error::invalid;
use crate::replication_seed::SeedError;

const MAX_DEPTH: usize = 64;
const MAX_CONTAINER: usize = 65_536;
const MAX_WORKING: usize = 64 * 1024 * 1024;

/// Validate canonical named MessagePack before a serde visitor can allocate.
pub(crate) fn validate_canonical_lengths(
    bytes: &[u8],
    require_sorted: bool,
) -> Result<(), SeedError> {
    let mut scanner = CanonicalScanner {
        bytes,
        position: 0,
        working: 0,
        require_sorted,
    };
    scanner.value(0)?;
    if scanner.position != bytes.len() {
        return Err(invalid("trailing MessagePack bytes"));
    }
    Ok(())
}

struct CanonicalScanner<'a> {
    bytes: &'a [u8],
    position: usize,
    working: usize,
    require_sorted: bool,
}

impl CanonicalScanner<'_> {
    fn take(&mut self, len: usize) -> Result<&[u8], SeedError> {
        let end = self
            .position
            .checked_add(len)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| invalid("MessagePack length exceeds remaining bytes"))?;
        let result = &self.bytes[self.position..end];
        self.position = end;
        Ok(result)
    }

    fn length(&mut self, width: usize) -> Result<usize, SeedError> {
        self.take(width)?.iter().try_fold(0usize, |value, byte| {
            value
                .checked_mul(256)
                .and_then(|value| value.checked_add(usize::from(*byte)))
                .ok_or_else(|| invalid("MessagePack length overflow"))
        })
    }

    fn charge(&mut self, bytes: usize) -> Result<(), SeedError> {
        self.working = self
            .working
            .checked_add(bytes)
            .filter(|size| *size <= MAX_WORKING)
            .ok_or_else(|| invalid("MessagePack working budget exceeded"))?;
        Ok(())
    }

    fn value(&mut self, depth: usize) -> Result<(), SeedError> {
        if depth >= MAX_DEPTH {
            return Err(invalid("MessagePack depth exceeds bounds"));
        }
        self.charge(64)?;
        let marker = self.take(1)?[0];
        match marker {
            0x00..=0x7f | 0xc0 | 0xc2 | 0xc3 | 0xe0..=0xff => {}
            0x80..=0x8f => self.map(usize::from(marker & 15), depth)?,
            0x90..=0x9f => self.array(usize::from(marker & 15), depth)?,
            0xa0..=0xbf => self.string(usize::from(marker & 31))?,
            0xcc | 0xd0 => {
                self.take(1)?;
            }
            0xcd | 0xd1 => {
                self.take(2)?;
            }
            0xca | 0xce | 0xd2 => {
                self.take(4)?;
            }
            0xcb | 0xcf | 0xd3 => {
                self.take(8)?;
            }
            0xc4 => {
                let len = self.length(1)?;
                self.payload(len)?;
            }
            0xc5 => {
                let len = self.length(2)?;
                self.payload(len)?;
            }
            0xc6 => {
                let len = self.length(4)?;
                self.payload(len)?;
            }
            0xd9 => {
                let len = self.length(1)?;
                self.string(len)?;
            }
            0xda => {
                let len = self.length(2)?;
                self.string(len)?;
            }
            0xdb => {
                let len = self.length(4)?;
                self.string(len)?;
            }
            0xdc | 0xdd => {
                let width = if marker == 0xdc { 2 } else { 4 };
                let count = self.length(width)?;
                self.array(count, depth)?;
            }
            0xde | 0xdf => {
                let width = if marker == 0xde { 2 } else { 4 };
                let count = self.length(width)?;
                self.map(count, depth)?;
            }
            _ => return Err(invalid("unsupported MessagePack marker")),
        }
        Ok(())
    }

    fn array(&mut self, count: usize, depth: usize) -> Result<(), SeedError> {
        if count > MAX_CONTAINER || count > self.bytes.len().saturating_sub(self.position) {
            return Err(invalid("MessagePack array exceeds bounds"));
        }
        self.charge(count.saturating_mul(64))?;
        for _ in 0..count {
            self.value(depth + 1)?;
        }
        Ok(())
    }

    fn map(&mut self, count: usize, depth: usize) -> Result<(), SeedError> {
        if count > MAX_CONTAINER
            || count.saturating_mul(2) > self.bytes.len().saturating_sub(self.position)
        {
            return Err(invalid("MessagePack map exceeds bounds"));
        }
        self.charge(count.saturating_mul(128))?;
        let require_sorted = self.require_sorted;
        let mut names = BTreeSet::new();
        let mut previous: Option<Vec<u8>> = None;
        for _ in 0..count {
            let marker = self.take(1)?[0];
            let len = match marker {
                0xa0..=0xbf => usize::from(marker & 31),
                0xd9 => self.length(1)?,
                0xda => self.length(2)?,
                0xdb => self.length(4)?,
                _ => return Err(invalid("named MessagePack map key is not a string")),
            };
            self.charge(len.saturating_mul(3))?;
            let key = self.take(len)?;
            str::from_utf8(key)
                .map_err(|_| invalid("named MessagePack field is not valid UTF-8"))?;
            if !names.insert(key.to_vec()) {
                return Err(invalid("duplicate named MessagePack field"));
            }
            if require_sorted && previous.as_deref().is_some_and(|last| last >= key) {
                return Err(invalid("named MessagePack fields are not canonical"));
            }
            if require_sorted {
                previous = Some(key.to_vec());
            }
            self.value(depth + 1)?;
        }
        Ok(())
    }

    fn string(&mut self, len: usize) -> Result<(), SeedError> {
        self.charge(len.saturating_mul(3))?;
        let value = self.take(len)?;
        str::from_utf8(value).map_err(|_| invalid("MessagePack string is not valid UTF-8"))?;
        Ok(())
    }

    fn payload(&mut self, len: usize) -> Result<(), SeedError> {
        self.charge(len.saturating_mul(3))?;
        self.take(len)?;
        Ok(())
    }
}
