#[cfg(test)]
mod tests {
    use crate::codecs::basic::bincode;
    use crate::types::record_id::RecordId;
    use std::collections::HashSet;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_record_id_uniqueness() {
        let mut ids = HashSet::new();
        for _ in 0..100_000 {
            assert!(ids.insert(RecordId::new()));
        }
    }

    #[test]
    fn test_record_id_ordering() {
        let id1 = RecordId::new();
        thread::sleep(Duration::from_micros(10));
        let id2 = RecordId::new();
        assert!(id1 < id2, "id1 should be less than id2");
    }

    #[test]
    fn test_string_roundtrip() {
        let id = RecordId::new();
        let s = id.to_string();
        let reconstructed_id: RecordId = s.parse().unwrap();
        assert_eq!(id, reconstructed_id);
    }

    #[test]
    fn test_system_record_id_logic() {
        // Short name
        let id_short = RecordId::system("users");
        assert!(id_short.is_system());
        assert_eq!(&id_short.0[0..4], &[0, 0, 0, 0]);
        assert_eq!(&id_short.0[4..9], b"users");
        assert_eq!(id_short.0[9], 0); // Padding

        // 12-byte name
        let id_exact = RecordId::system("123456789012");
        assert_eq!(&id_exact.0[4..16], b"123456789012");

        // Long name (truncation)
        let id_long = RecordId::system("123456789012-extra");
        assert_eq!(
            id_exact, id_long,
            "Long name should be truncated to same ID"
        );

        // Determinism
        let id_again = RecordId::system("users");
        assert_eq!(id_short, id_again);

        // User ID should not be system
        let user_id = RecordId::new();
        assert!(!user_id.is_system());
    }

    #[test]
    fn test_roundtrip() {
        let id = RecordId::new();
        let bytes = bincode::to_bytes(&id).unwrap();
        let deserialized: RecordId = bincode::from_bytes(&bytes).unwrap();
        assert_eq!(id, deserialized);
    }

    #[test]
    fn test_bincode_roundtrip() {
        let id = RecordId::new();
        let bytes = bincode::to_bytes(&id).unwrap();
        let id2 = bincode::from_bytes::<RecordId>(&bytes).unwrap();
        assert_eq!(id2.0, id.0);
    }
}
