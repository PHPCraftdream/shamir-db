use super::super::common::{no_flush_config, rk};
use crate::membuffer_clear_race_hook::ClearRaceHook;
use crate::storage_in_memory::InMemoryRepo;
use crate::storage_membuffer::MemBufferStore;
use crate::types::{Repo, Store};
use bytes::Bytes;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

// Inject a writer before snapshot removal and check both telemetry and visibility.
#[tokio::test]
async fn drain_clear_race_does_not_mask_acked_write() {
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();

    // Inject K into dirty before the drain removes its snapshot; K has
    // not been written to the backing store.
    let racing_key = rk(b"racing-key-535");
    let racing_val = Bytes::from_static(b"racing-value-535");
    let store_slot: Arc<Mutex<Weak<MemBufferStore>>> = Arc::new(Mutex::new(Weak::new()));
    let fired = Arc::new(AtomicBool::new(false));

    let hook = {
        let store_slot = Arc::clone(&store_slot);
        let fired = Arc::clone(&fired);
        let racing_key = racing_key.clone();
        let racing_val = racing_val.clone();
        ClearRaceHook::install(Arc::new(move || {
            if fired.swap(true, Ordering::SeqCst) {
                return;
            }
            if let Some(store) = store_slot.lock().unwrap().upgrade() {
                store.inject_racing_dirty_write(racing_key.clone(), racing_val.clone());
            }
        }))
    };

    let buffered = Arc::new(MemBufferStore::new_with_clear_race_hook(
        inner_store,
        no_flush_config(),
        hook,
    ));
    *store_slot.lock().unwrap() = Arc::downgrade(&buffered);

    // Prime dirty so the drain reaches the removal hook.
    let jkey = buffered
        .insert(Bytes::from_static(b"prime-j"))
        .await
        .unwrap();
    let drained = buffered.drain_once_for_test(usize::MAX).await.unwrap();
    assert_eq!(drained, 1, "the single drain should have flushed only J");

    assert!(
        buffered.dirty_nonempty_flag(),
        "clear-race masked an ACKed write: dirty holds K but the dirty count is zero"
    );
    let got = buffered.get(racing_key).await.unwrap();
    assert_eq!(
        got.as_ref(),
        b"racing-value-535",
        "racing write must remain visible after snapshot removal"
    );
    let jkey_owned = jkey;
    assert!(
        buffered.get(jkey_owned).await.is_ok(),
        "primed key J should have been flushed to inner"
    );
}
