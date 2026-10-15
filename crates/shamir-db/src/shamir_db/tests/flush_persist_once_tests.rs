//! Persist-once-per-flush-pass regression tests.
//!
//! `flush_buffers` drains every store (`Store::flush_drain`) and then issues
//! exactly ONE backend-wide durability barrier (`Repo::persist_once`), instead
//! of the old per-store `flush()` loop that fsynced fjall's single per-Database
//! journal once per store. These tests pin the fsync COUNT (via the
//! `FJALL_PERSIST_SYNC_ALL_COUNT` seam) and data durability across reopen.

#[cfg(feature = "fjall")]
mod fjall {
    use std::sync::atomic::Ordering;

    use shamir_query_builder::write::doc;
    use shamir_types::codecs::interned::{inner_value_to_query_value, query_value_to_inner};
    use tokio::runtime::Builder;

    use crate::engine::repo::{BoxRepoFactory, RepoInstance};
    use crate::engine::table::TableConfig;
    use crate::storage::storage_fjall::{
        reset_persist_sync_all_count, FJALL_PERSIST_SYNC_ALL_COUNT,
    };

    // fjall-gated; SERIAL-style isolation not needed (no global hook), but
    // serialize the persist counter with a local mutex if a second test is
    // added — this single test is fine without.
    #[test]
    fn flush_buffers_persists_once_per_pass_and_data_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repo");
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        let (rid, value) = runtime.block_on(async {
            let repo = RepoInstance::from_factory(
                "persist_once_repo".into(),
                BoxRepoFactory::fjall_raw(&path),
                vec![
                    TableConfig::new("a"),
                    TableConfig::new("b"),
                    TableConfig::new("c"),
                ],
            )
            .await
            .unwrap();
            let table = repo.get_table("a").await.unwrap();
            let interner = table.interner().get().await.unwrap();
            let value = query_value_to_inner(&doc().set("k", "v").build(), interner).unwrap();
            let rid = table.insert(&value).await.unwrap();

            // Settle the interner delta FIRST: the table write path does not
            // persist the interner, so without this the interner persist
            // inside `flush_buffers` would run its real chunk write + flush
            // (an extra counted persist). After this call the interner's
            // persist takes its `cur_len == last` no-op fast path.
            table.interner().persist().await.unwrap();

            // Reset AFTER the write: the write path (and interner persist)
            // may legitimately issue persists of their own — only the flush
            // pass itself is under count.
            reset_persist_sync_all_count();
            repo.flush_buffers().await.unwrap();
            let n = FJALL_PERSIST_SYNC_ALL_COUNT.load(Ordering::Acquire);
            assert_eq!(n, 1, "one full-database persist per flush pass, got {n}");
            (rid, value)
        });
        // All foreground handles were dropped above. Current-thread runtime
        // drop cancels background store owners synchronously, without reopen
        // retries.
        drop(runtime);

        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let repo = RepoInstance::from_factory(
                "persist_once_repo".into(),
                BoxRepoFactory::fjall_raw(&path),
                vec![
                    TableConfig::new("a"),
                    TableConfig::new("b"),
                    TableConfig::new("c"),
                ],
            )
            .await
            .expect("all previous stores must be released before reopen");
            repo.recover_v2_inflight().await.unwrap();
            let table = repo.get_table("a").await.unwrap();
            let reopened = table.get(rid).await.unwrap();
            assert_eq!(reopened, value);
            let interner = table.interner().get().await.unwrap();
            assert_eq!(
                inner_value_to_query_value(&reopened, interner).unwrap(),
                doc().set("k", "v").build()
            );
        });
        drop(runtime);
    }
}
