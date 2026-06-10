#[cfg(feature = "durable-redb")]
mod inner {
    use crate::common::time::ns;
    use crate::server::durable_counters::RedbConsumedCounters;
    use crate::server::resume::ConsumedCounterStore;
    use tempfile::TempDir;

    fn fresh_store() -> (TempDir, RedbConsumedCounters) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("counters.redb");
        let store = RedbConsumedCounters::open(&path).unwrap();
        (dir, store)
    }

    #[test]
    fn first_advance_accepts() {
        let (_dir, s) = fresh_store();
        let uid = [1u8; 16];
        let fam = [2u8; 16];
        assert!(s.try_advance(&uid, &fam, 1));
        assert_eq!(s.peek(&uid, &fam), Some(1));
    }

    #[test]
    fn replay_same_counter_rejected() {
        let (_dir, s) = fresh_store();
        let uid = [1u8; 16];
        let fam = [2u8; 16];
        assert!(s.try_advance(&uid, &fam, 5));
        assert!(!s.try_advance(&uid, &fam, 5), "replay must reject");
        assert!(!s.try_advance(&uid, &fam, 4), "lower must reject");
    }

    #[test]
    fn higher_counter_accepts_and_advances() {
        let (_dir, s) = fresh_store();
        let uid = [1u8; 16];
        let fam = [2u8; 16];
        assert!(s.try_advance(&uid, &fam, 1));
        assert!(s.try_advance(&uid, &fam, 2));
        assert!(s.try_advance(&uid, &fam, 100));
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
            let s1 = RedbConsumedCounters::open(&path).unwrap();
            assert!(s1.try_advance(&uid, &fam, 7));
            // s1 dropped → file closed.
        }

        // Second boot: peek must see 7. Replay of counter 7 must reject.
        {
            let s2 = RedbConsumedCounters::open(&path).unwrap();
            assert_eq!(s2.peek(&uid, &fam), Some(7));
            assert!(
                !s2.try_advance(&uid, &fam, 7),
                "post-restart replay must reject"
            );
            assert!(
                !s2.try_advance(&uid, &fam, 6),
                "post-restart older must reject"
            );
            assert!(
                s2.try_advance(&uid, &fam, 8),
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
        assert!(s.try_advance(&uid, &fam_a, 1));
        assert!(s.try_advance(&uid, &fam_b, 1));
        // Family A advance to 2 doesn't affect family B.
        assert!(s.try_advance(&uid, &fam_a, 2));
        assert_eq!(s.peek(&uid, &fam_a), Some(2));
        assert_eq!(s.peek(&uid, &fam_b), Some(1));
    }

    #[test]
    fn gc_drops_idle_entries() {
        let (_dir, s) = fresh_store();
        let uid = [1u8; 16];
        let fam = [2u8; 16];
        s.try_advance(&uid, &fam, 1);
        assert!(s.peek(&uid, &fam).is_some());

        // GC with a far-future cutoff drops all entries.
        let far = crate::common::time::UnixNanos::now().as_u64() + 48 * ns::HOUR;
        s.gc(far);
        assert!(s.peek(&uid, &fam).is_none());
    }
}
