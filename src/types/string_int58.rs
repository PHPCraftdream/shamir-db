/// Base58 string-based integer that grows dynamically
/// Compact, URL-safe, human-readable IDs like "1", "2", "4k9"
/// Supports only one operation: increment in base58 positional system
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StringInt58 {
    bytes: Vec<u8>, // Raw bytes of base58 string, most significant first
}

impl StringInt58 {
    const BASE58: &'static [u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

    // Static lookup table: char -> next char in base58
    const NEXT_CHAR: [u8; 256] = {
        let mut table = [0u8; 256];
        let mut i = 0;
        while i < 57 {
            table[Self::BASE58[i] as usize] = Self::BASE58[i + 1];
            i += 1;
        }
        // Last char 'z' wraps - will be handled as overflow
        table
    };

    pub fn new() -> Self {
        Self { bytes: vec![b'1'] } // "1"
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        if s.is_empty() {
            return None;
        }

        // Validate all characters are valid base58
        for b in s.bytes() {
            if !Self::BASE58.contains(&b) {
                return None;
            }
        }

        Some(Self {
            bytes: s.bytes().collect(),
        })
    }

    /// Simple increment: replace each char with next in BASE58, carry when needed
    pub fn increment(&mut self) {
        let mut pos = self.bytes.len() - 1;
        loop {
            let current = self.bytes[pos];
            let next = Self::NEXT_CHAR[current as usize];

            if next != 0 {
                // Normal case: just replace with next char
                self.bytes[pos] = next;
                return;
            }

            // Overflow at this position: wrap to '1' and carry left
            self.bytes[pos] = b'1';
            if pos == 0 {
                // Need new digit at front: "z" -> "21"
                self.bytes.insert(0, b'2');
                return;
            }
            pos -= 1;
        }
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes).unwrap()
    }
}

impl std::str::FromStr for StringInt58 {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str(s).ok_or("Invalid base58 string")
    }
}

impl Default for StringInt58 {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for StringInt58 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sequence() {
        let mut id = StringInt58::new();
        assert_eq!(id.as_str(), "1");

        id.increment();
        assert_eq!(id.as_str(), "2");

        for _ in 2..9 {
            id.increment();
        }
        assert_eq!(id.as_str(), "9");

        id.increment();
        assert_eq!(id.as_str(), "A");

        id.increment();
        assert_eq!(id.as_str(), "B");

        for _ in 11..58 {
            id.increment();
        }
        assert_eq!(id.as_str(), "z");

        id.increment();
        assert_eq!(id.as_str(), "21");

        id.increment();
        assert_eq!(id.as_str(), "22");
    }

    #[test]
    fn test_base58_boundaries() {
        let id = StringInt58::from_str("z").unwrap();
        assert_eq!(id.as_str(), "z");

        let id = StringInt58::from_str("21").unwrap();
        assert_eq!(id.as_str(), "21");

        let id = StringInt58::from_str("22").unwrap();
        assert_eq!(id.as_str(), "22");
    }

    #[test]
    fn test_conversion_round_trip() {
        // Test that parsing and stringify are consistent
        let test_cases = ["1", "2", "9", "A", "z", "21", "22", "1z"];
        for expected in test_cases {
            let id = StringInt58::from_str(expected).unwrap();
            assert_eq!(
                id.as_str(),
                expected,
                "Expected {} to parse as itself",
                expected
            );
        }

        // Test round trip via increment
        let mut id = StringInt58::new();
        for _ in 0..10 {
            let str_val = id.as_str().to_string();
            let id2 = StringInt58::from_str(&str_val).unwrap();
            assert_eq!(id, id2);
            id.increment();
        }
    }

    #[test]
    fn test_size_comparison() {
        assert_eq!(StringInt58::from_str("1").unwrap().as_str().len(), 1);
        assert_eq!(StringInt58::from_str("21").unwrap().as_str().len(), 2);
        assert_eq!(StringInt58::from_str("4k9").unwrap().as_str().len(), 3);
    }

    #[test]
    fn test_increment_sequence() {
        let mut id = StringInt58::from_str("z").unwrap();

        id.increment();
        assert_eq!(id.as_str(), "21");

        id.increment();
        assert_eq!(id.as_str(), "22");
    }

    #[test]
    fn test_from_str_invalid() {
        assert!(StringInt58::from_str("").is_none());
        assert!(StringInt58::from_str("0").is_none()); // 0 not in base58
        assert!(StringInt58::from_str("l").is_none()); // l not in base58
        assert!(StringInt58::from_str("O").is_none()); // O not in base58
        assert!(StringInt58::from_str("I").is_none()); // I not in base58
    }

    #[test]
    fn test_display() {
        let id = StringInt58::from_str("z").unwrap();
        assert_eq!(format!("{}", id), "z");
    }

    #[test]
    fn test_large_increment() {
        let mut id = StringInt58::from_str("zz").unwrap();

        // After increment "zz" -> "211"
        id.increment();
        assert_eq!(id.as_str(), "211");
    }

    #[test]
    fn test_increment_carry_chain() {
        let mut id = StringInt58::from_str("z").unwrap();
        assert_eq!(id.as_str(), "z");

        id.increment();
        assert_eq!(id.as_str(), "21");

        // Increment to cause multiple carries
        let mut id = StringInt58::from_str("zz").unwrap();
        id.increment();
        assert_eq!(id.as_str(), "211");
    }

    #[test]
    fn test_equality() {
        let id1 = StringInt58::from_str("1").unwrap();
        let id2 = StringInt58::from_str("2").unwrap();
        let id3 = StringInt58::from_str("1").unwrap();

        assert_ne!(id1, id2);
        assert_eq!(id1, id3);
        assert_ne!(id2, id3);
    }
}
