use xxhash_rust::xxh3::xxh3_128;

/// XXH3-128 (seed 0) of a module's bytes: `[high word, low word]`.
///
/// Identity and corruption check only. Nothing may use it for lookup, dedup,
/// caching, permissions or `definer` decisions: XXH3-128 is not collision
/// resistant against an adversary. It deliberately implements neither `Hash`
/// nor `Ord`, so it cannot be used as a map key by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleHash([u64; 2]);

impl ModuleHash {
    /// Hashes the full module bytes.
    pub fn of(bytes: &[u8]) -> Self {
        let h = xxh3_128(bytes);
        Self([(h >> 64) as u64, h as u64])
    }

    /// Rebuilds a hash from its `[high, low]` words.
    pub fn from_words(words: [u64; 2]) -> Self {
        Self(words)
    }

    /// The `[high, low]` words.
    pub fn words(&self) -> [u64; 2] {
        self.0
    }

    /// 32 lowercase hex characters, high word first.
    pub fn to_hex(&self) -> String {
        let [hi, lo] = self.0;
        format!("{hi:016x}{lo:016x}")
    }

    /// Strict inverse of [`ModuleHash::to_hex`]: exactly 32 characters of
    /// `0-9a-f`; no sign, whitespace or prefix.
    pub fn from_hex(s: &str) -> Result<Self, String> {
        if s.len() != 32 {
            return Err(format!(
                "module hash must be 32 hex characters, got {}",
                s.len()
            ));
        }
        if !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err("module hash must contain only lowercase hex digits".to_string());
        }
        let (hi, lo) = s.split_at(16);
        let parse = |part: &str| {
            u64::from_str_radix(part, 16).map_err(|e| format!("invalid module hash: {e}"))
        };
        Ok(Self([parse(hi)?, parse(lo)?]))
    }
}
