use core::str;

/// Wire parsing failure; intentionally carries no attacker-controlled data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    /// Invalid MessagePack shape, type, or bounds.
    Invalid,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Value<'a> {
    UInt(u64),
    Str(&'a str),
    Bin(&'a [u8]),
    Nil,
    Array { count: usize, bytes: &'a [u8] },
    Map { count: usize, bytes: &'a [u8] },
}

pub(super) struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], WireError> {
        let end = self.at.checked_add(len).ok_or(WireError::Invalid)?;
        let value = self.bytes.get(self.at..end).ok_or(WireError::Invalid)?;
        self.at = end;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }

    fn be(&mut self, len: usize) -> Result<u64, WireError> {
        let bytes = self.take(len)?;
        let mut value = 0_u64;
        for byte in bytes {
            value = (value << 8) | u64::from(*byte);
        }
        Ok(value)
    }

    pub(super) fn uint(&mut self) -> Result<u64, WireError> {
        let marker = self.byte()?;
        match marker {
            0x00..=0x7f => Ok(u64::from(marker)),
            0xcc => self.be(1),
            0xcd => self.be(2),
            0xce => self.be(4),
            0xcf => self.be(8),
            _ => Err(WireError::Invalid),
        }
    }

    pub(super) fn string(&mut self) -> Result<&'a str, WireError> {
        let marker = self.byte()?;
        let len = match marker {
            0xa0..=0xbf => usize::from(marker & 0x1f),
            0xd9 => usize::try_from(self.be(1)?).map_err(|_| WireError::Invalid)?,
            0xda => usize::try_from(self.be(2)?).map_err(|_| WireError::Invalid)?,
            0xdb => usize::try_from(self.be(4)?).map_err(|_| WireError::Invalid)?,
            _ => return Err(WireError::Invalid),
        };
        str::from_utf8(self.take(len)?).map_err(|_| WireError::Invalid)
    }

    pub(super) fn binary(&mut self) -> Result<&'a [u8], WireError> {
        let marker = self.byte()?;
        let len = match marker {
            0xc4 => usize::try_from(self.be(1)?).map_err(|_| WireError::Invalid)?,
            0xc5 => usize::try_from(self.be(2)?).map_err(|_| WireError::Invalid)?,
            0xc6 => usize::try_from(self.be(4)?).map_err(|_| WireError::Invalid)?,
            _ => return Err(WireError::Invalid),
        };
        self.take(len)
    }

    fn value(&mut self, depth: u8) -> Result<Value<'a>, WireError> {
        if depth > 3 {
            return Err(WireError::Invalid);
        }
        let start = self.at;
        let marker = self.byte()?;
        match marker {
            0xc0 => Ok(Value::Nil),
            0x00..=0x7f | 0xcc..=0xcf => {
                self.at = start;
                Ok(Value::UInt(self.uint()?))
            }
            0xa0..=0xbf | 0xd9..=0xdb => {
                self.at = start;
                Ok(Value::Str(self.string()?))
            }
            0xc4..=0xc6 => {
                self.at = start;
                Ok(Value::Bin(self.binary()?))
            }
            0x90..=0x9f => {
                let count = usize::from(marker & 0x0f);
                let body_start = self.at;
                self.skip_values(count, depth + 1)?;
                Ok(Value::Array {
                    count,
                    bytes: &self.bytes[body_start..self.at],
                })
            }
            0xdc | 0xdd => {
                let count = usize::try_from(self.be(if marker == 0xdc { 2 } else { 4 })?)
                    .map_err(|_| WireError::Invalid)?;
                if count > 4 {
                    return Err(WireError::Invalid);
                }
                let body_start = self.at;
                self.skip_values(count, depth + 1)?;
                Ok(Value::Array {
                    count,
                    bytes: &self.bytes[body_start..self.at],
                })
            }
            0x80..=0x8f => {
                let count = usize::from(marker & 0x0f);
                self.skip_pairs(count, depth + 1)?;
                Ok(Value::Map {
                    count,
                    bytes: &self.bytes[start..self.at],
                })
            }
            0xde | 0xdf => {
                let count = usize::try_from(self.be(if marker == 0xde { 2 } else { 4 })?)
                    .map_err(|_| WireError::Invalid)?;
                if count > 12 {
                    return Err(WireError::Invalid);
                }
                self.skip_pairs(count, depth + 1)?;
                Ok(Value::Map {
                    count,
                    bytes: &self.bytes[start..self.at],
                })
            }
            _ => Err(WireError::Invalid),
        }
    }

    fn skip_values(&mut self, count: usize, depth: u8) -> Result<(), WireError> {
        for _ in 0..count {
            self.value(depth)?;
        }
        Ok(())
    }

    fn skip_pairs(&mut self, count: usize, depth: u8) -> Result<(), WireError> {
        if count > 12 {
            return Err(WireError::Invalid);
        }
        for _ in 0..count {
            self.string()?;
            self.value(depth)?;
        }
        Ok(())
    }

    pub(super) fn map(
        &mut self,
        expected: &'static [&'static str],
    ) -> Result<Vec<(&'static str, Value<'a>)>, WireError> {
        let count = self.map_header()?;
        self.map_counted(count, expected)
    }

    pub(super) fn map_counted(
        &mut self,
        count: usize,
        expected: &'static [&'static str],
    ) -> Result<Vec<(&'static str, Value<'a>)>, WireError> {
        if count != expected.len() {
            return Err(WireError::Invalid);
        }
        let mut pairs = Vec::with_capacity(expected.len());
        let mut seen = 0_u16;
        for _ in 0..count {
            let key = self.string()?;
            let index = expected
                .iter()
                .position(|candidate| *candidate == key)
                .ok_or(WireError::Invalid)?;
            let bit = 1_u16 << index;
            if seen & bit != 0 {
                return Err(WireError::Invalid);
            }
            seen |= bit;
            pairs.push((expected[index], self.value(1)?));
        }
        if seen.count_ones() as usize != expected.len() {
            return Err(WireError::Invalid);
        }
        Ok(pairs)
    }

    pub(super) fn map_header(&mut self) -> Result<usize, WireError> {
        let marker = self.byte()?;
        match marker {
            0x80..=0x8f => Ok(usize::from(marker & 0x0f)),
            0xde => usize::try_from(self.be(2)?).map_err(|_| WireError::Invalid),
            0xdf => usize::try_from(self.be(4)?).map_err(|_| WireError::Invalid),
            _ => Err(WireError::Invalid),
        }
    }

    pub(super) fn take_public(&mut self, len: usize) -> Result<&'a [u8], WireError> {
        self.take(len)
    }
    pub(super) fn byte_public(&mut self) -> Result<u8, WireError> {
        self.byte()
    }

    pub(super) fn finish(self) -> Result<(), WireError> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(WireError::Invalid)
        }
    }
}

pub(super) fn uint_value(value: Value<'_>, max: u64) -> Result<u64, WireError> {
    match value {
        Value::UInt(number) if number <= max => Ok(number),
        _ => Err(WireError::Invalid),
    }
}

pub(super) fn string_value(value: Value<'_>) -> Result<&str, WireError> {
    match value {
        Value::Str(text) => Ok(text),
        _ => Err(WireError::Invalid),
    }
}

pub(super) fn bin_value<const N: usize>(value: Value<'_>) -> Result<[u8; N], WireError> {
    match value {
        Value::Bin(bytes) if bytes.len() == N => bytes.try_into().map_err(|_| WireError::Invalid),
        _ => Err(WireError::Invalid),
    }
}
