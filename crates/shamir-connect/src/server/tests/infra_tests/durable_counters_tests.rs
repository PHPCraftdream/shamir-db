#[cfg(feature = "durable-fjall")]
mod inner {
    use crate::common::time::{ns, UnixNanos};
    use crate::server::durable_counters::{FaultPoint, FjallConsumedCounters};
    use crate::server::resume::{AdvanceOutcome, ConsumedCounterStore};
    use tempfile::TempDir;

    fn fresh_store() -> (TempDir, FjallConsumedCounters) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("counters.redb");
        let store = FjallConsumedCounters::open(&path).unwrap();
        (dir, store)
    }

    #[test]
    fn first_advance_accepts() {
        let (_dir, s) = fresh_store();
        let uid = [1u8; 16];
        let fam = [2u8; 16];
        assert_eq!(s.try_advance(&uid, &fam, 1), AdvanceOutcome::Accepted);
        assert_eq!(s.peek(&uid, &fam), Some(1));
    }

    #[test]
    fn replay_same_counter_rejected() {
        let (_dir, s) = fresh_store();
        let uid = [1u8; 16];
        let fam = [2u8; 16];
        assert_eq!(s.try_advance(&uid, &fam, 5), AdvanceOutcome::Accepted);
        assert_eq!(
            s.try_advance(&uid, &fam, 5),
            AdvanceOutcome::Replayed,
            "replay must reject"
        );
        assert_eq!(
            s.try_advance(&uid, &fam, 4),
            AdvanceOutcome::Replayed,
            "lower must reject"
        );
    }

    #[test]
    fn higher_counter_accepts_and_advances() {
        let (_dir, s) = fresh_store();
        let uid = [1u8; 16];
        let fam = [2u8; 16];
        assert_eq!(s.try_advance(&uid, &fam, 1), AdvanceOutcome::Accepted);
        assert_eq!(s.try_advance(&uid, &fam, 2), AdvanceOutcome::Accepted);
        assert_eq!(s.try_advance(&uid, &fam, 100), AdvanceOutcome::Accepted);
        assert_eq!(s.peek(&uid, &fam), Some(100));
    }

    /// Spec §6.2 — durability across restart: counter state survives
    /// closing + reopening the database.
    #[test]
    fn counter_state_survives_restart() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("counters.redb");

        let uid = [0xa1u8; 16];
        let fam = [0xb2u8; 16];

        // First boot: advance counter to 7.
        {
            let s1 = FjallConsumedCounters::open(&path).unwrap();
            assert_eq!(s1.try_advance(&uid, &fam, 7), AdvanceOutcome::Accepted);
            // s1 dropped → file closed.
        }

        // Second boot: peek must see 7. Replay of counter 7 must reject.
        {
            let s2 = FjallConsumedCounters::open(&path).unwrap();
            assert_eq!(s2.peek(&uid, &fam), Some(7));
            assert_eq!(
                s2.try_advance(&uid, &fam, 7),
                AdvanceOutcome::Replayed,
                "post-restart replay must reject"
            );
            assert_eq!(
                s2.try_advance(&uid, &fam, 6),
                AdvanceOutcome::Replayed,
                "post-restart older must reject"
            );
            assert_eq!(
                s2.try_advance(&uid, &fam, 8),
                AdvanceOutcome::Accepted,
                "post-restart higher must accept"
            );
        }
    }

    #[test]
    fn distinct_families_are_independent() {
        let (_dir, s) = fresh_store();
        let uid = [1u8; 16];
        let fam_a = [0xaau8; 16];
        let fam_b = [0xbbu8; 16];
        assert_eq!(s.try_advance(&uid, &fam_a, 1), AdvanceOutcome::Accepted);
        assert_eq!(s.try_advance(&uid, &fam_b, 1), AdvanceOutcome::Accepted);
        // Family A advance to 2 doesn't affect family B.
        assert_eq!(s.try_advance(&uid, &fam_a, 2), AdvanceOutcome::Accepted);
        assert_eq!(s.peek(&uid, &fam_a), Some(2));
        assert_eq!(s.peek(&uid, &fam_b), Some(1));
    }

    #[test]
    fn gc_drops_idle_entries() {
        let (_dir, s) = fresh_store();
        let uid = [1u8; 16];
        let fam = [2u8; 16];
        assert_eq!(s.try_advance(&uid, &fam, 1), AdvanceOutcome::Accepted);
        assert!(s.peek(&uid, &fam).is_some());

        // GC with a far-future cutoff drops all entries.
        let far = UnixNanos::now().as_u64() + 48 * ns::HOUR;
        s.gc(far);
        assert!(s.peek(&uid, &fam).is_none());
    }

    /// RV-CONNECT-2FADEBC4: a malformed persisted value (length != 16) must
    /// not panic `unpack_value`. `try_advance` must treat it as corruption
    /// (StorageFailed), and `peek` must return None.
    #[test]
    fn rv_counters_2fadebc4_malformed_persisted_value_does_not_panic() {
        let (_dir, s) = fresh_store();
        let uid = [7u8; 16];
        let fam = [8u8; 16];
        s.write_raw(&uid, &fam, &[0u8; 8]);
        assert_eq!(
            s.peek(&uid, &fam),
            None,
            "malformed value must read as None, not panic"
        );
        assert_eq!(
            s.try_advance(&uid, &fam, 1),
            AdvanceOutcome::StorageFailed,
            "malformed persisted value is corruption, not a replay"
        );
    }

    /// RV-CONNECT-ECE3E157: every real fjall storage failure must map to
    /// `StorageFailed`, not `Replayed`. Fault-injects each real branch
    /// (get / insert / persist) on a fresh store; a control run proves the
    /// store itself accepts and rejects normally.
    #[test]
    fn rv_counters_ece3e157_durable_store_failures_map_to_storage_failed() {
        for point in [FaultPoint::Get, FaultPoint::Insert, FaultPoint::Persist] {
            let (_dir, s) = fresh_store();
            s.set_fault(point);
            assert_eq!(
                s.try_advance(&[3u8; 16], &[4u8; 16], 1),
                AdvanceOutcome::StorageFailed,
                "fault point {:?} must map to StorageFailed",
                point
            );
        }

        // Control: healthy store accepts, and monotonicity rejection is
        // Replayed (distinct from StorageFailed).
        let (_dir, s) = fresh_store();
        let uid = [5u8; 16];
        let fam = [6u8; 16];
        assert_eq!(s.try_advance(&uid, &fam, 5), AdvanceOutcome::Accepted);
        assert_eq!(s.try_advance(&uid, &fam, 4), AdvanceOutcome::Replayed);
    }
}
