#[cfg(test)]
mod tests {
    use crate::codecs::basic::bincode;
    use crate::types::common::new_set;
    use crate::types::record_id::RecordId;
    use shamir_collections::TFxSet;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_record_id_uniqueness() {
        let mut ids = new_set();
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

    #[test]
    fn try_from_bytes_round_trip() {
        let rid = RecordId::new();
        let bytes = rid.to_bytes();
        let restored = RecordId::try_from_bytes(&bytes).unwrap();
        assert_eq!(rid, restored);
    }

    #[test]
    fn try_from_bytes_wrong_length_returns_none() {
        assert!(RecordId::try_from_bytes(&[0u8; 15]).is_none());
        assert!(RecordId::try_from_bytes(&[0u8; 17]).is_none());
        assert!(RecordId::try_from_bytes(&[]).is_none());
    }

    /// L13: `from_ts` with a fixed timestamp produces 1000 unique ids
    /// that share the same upper 8 bytes (ts) but differ in the lower
    /// 8 bytes (random tail).
    #[test]
    fn from_ts_produces_unique_ids_with_same_timestamp() {
        let ts = RecordId::now_micros();
        let n = 1000;
        let mut ids: TFxSet<RecordId> = TFxSet::default();
        let mut tails: TFxSet<[u8; 8]> = TFxSet::default();

        for _ in 0..n {
            let id = RecordId::from_ts(ts);
            // Upper 8 bytes must be identical (same ts).
            assert_eq!(
                &id.as_bytes()[..8],
                &RecordId::from_ts(ts).as_bytes()[..8],
                // This checks layout only — both share the same ts input,
                // so the first 8 bytes MUST match. We actually verify
                // against a reference encoding below.
            );
            assert!(ids.insert(id), "duplicate RecordId in batch");

            let mut tail = [0u8; 8];
            tail.copy_from_slice(&id.as_bytes()[8..]);
            assert!(tails.insert(tail), "duplicate random tail in batch");
        }

        // All ids share the same timestamp prefix.
        let expected_prefix = &ids.iter().next().unwrap().as_bytes()[..8];
        for id in &ids {
            assert_eq!(&id.as_bytes()[..8], expected_prefix);
        }
    }

    /// L13: `from_ts` preserves the byte layout — 8B BE timestamp
    /// (relative to epoch) followed by 8B random.
    #[test]
    fn from_ts_preserves_byte_layout() {
        let ts: i64 = 1_800_000_000_000_000; // some future µs
        let id = RecordId::from_ts(ts);
        let expected_epoch: i64 = 1_769_817_600_000_000;
        let relative = ts.saturating_sub(expected_epoch);
        assert_eq!(
            &id.as_bytes()[..8],
            &relative.to_be_bytes(),
            "upper 8 bytes must be BE-encoded relative timestamp"
        );
    }

    /// L13: `RecordId::new()` still works (delegates to `from_ts`).
    #[test]
    fn new_delegates_to_from_ts_correctly() {
        let before = RecordId::now_micros();
        let id = RecordId::new();
        let after = RecordId::now_micros();

        let expected_epoch: i64 = 1_769_817_600_000_000;
        let mut ts_bytes = [0u8; 8];
        ts_bytes.copy_from_slice(&id.as_bytes()[..8]);
        let relative = i64::from_be_bytes(ts_bytes);
        let absolute = relative + expected_epoch;

        assert!(
            absolute >= before && absolute <= after,
            "timestamp {absolute} not in [{before}, {after}]"
        );
    }
}
