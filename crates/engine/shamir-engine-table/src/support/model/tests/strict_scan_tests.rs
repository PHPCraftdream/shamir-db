//! The node-failure oracles for the strict partition scan: ordinary scans keep
//! the lenient skip, the journal-only strict twin refuses.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

use super::super::table::Table;

#[tokio::test]
async fn lenient_scan_skips_a_corrupted_key_and_the_strict_twin_refuses_it() {
    tokio::time::timeout(
        Duration::from_secs(120),
        Box::pin(async {
            let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
            let table = Table::new(Arc::clone(&store));
            let first = table.insert(&InnerValue::Map(new_map())).await.unwrap();
            let second = table.insert(&InnerValue::Map(new_map())).await.unwrap();
            // A row whose PHYSICAL key is not a 16-byte record id. Written through the
            // store seam, never by hand-editing a file.
            store
                .set(
                    RecordKey::from_slice(b"short"),
                    Bytes::from_static(b"\x81\xa1a\x01"),
                )
                .await
                .unwrap();

            let mut lenient = Box::pin(table.list_stream(16));
            let mut seen = Vec::new();
            while let Some(batch) = lenient.next().await {
                for (id, _) in batch.expect("the lenient scan must not fail on a bad key") {
                    seen.push(id);
                }
            }
            seen.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            let mut expected = vec![first, second];
            expected.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            assert_eq!(
                seen, expected,
                "an ordinary scan must still SKIP the corrupted key and return every real row"
            );

            let mut strict = Box::pin(table.list_stream_strict(16));
            let mut refused = false;
            while let Some(batch) = strict.next().await {
                match batch {
                    // A successful batch before the corrupted key is expected here and
                    // asserts nothing: the refusal must surface at the corrupted key.
                    Ok(_) => {}
                    Err(DbError::Codec(detail)) => {
                        refused = true;
                        assert!(
                            detail.contains("16-byte record id"),
                            "the strict refusal must name the key shape, not the key bytes: {detail}"
                        );
                        assert!(
                            !detail.contains("short"),
                            "the strict refusal must not leak the key bytes: {detail}"
                        );
                    }
                    Err(other) => panic!("unexpected strict-scan error: {other}"),
                }
            }
            assert!(
                refused,
                "the strict twin must refuse a malformed physical key instead of skipping it"
            );
        }),
    )
    .await
    .expect("lenient/strict scan scenario timed out");
}
