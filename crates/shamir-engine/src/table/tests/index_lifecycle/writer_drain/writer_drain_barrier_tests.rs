use crate::table::writer_drain_barrier::WriterDrainBarrier;

#[tokio::test]
async fn drain_returns_immediately_when_no_writers_active() {
    let b = WriterDrainBarrier::new();
    assert_eq!(b.active_count(), 0);
    // Must not hang — single SeqCst load, immediate return.
    b.drain().await;
}

#[tokio::test]
async fn drain_waits_until_all_writers_exit() {
    let b = WriterDrainBarrier::new();
    let g1 = b.enter_writer();
    let g2 = b.enter_writer();
    assert_eq!(b.active_count(), 2);

    // drain must block while writers are active. Spawn it and confirm it
    // does not finish.
    let b2 = WriterDrainBarrier::clone(&b);
    let drain = tokio::spawn(async move { b2.drain().await });
    tokio::task::yield_now().await;
    assert!(!drain.is_finished(), "drain must wait for active writers");

    drop(g1);
    tokio::task::yield_now().await;
    assert!(
        !drain.is_finished(),
        "drain must wait until ALL writers exit"
    );

    drop(g2);
    drain.await.expect("drain completes once counter hits 0");
}

#[tokio::test]
async fn guard_decrement_returns_counter_to_zero_for_drain() {
    // Structural: enter + drop returns the counter to 0 so a subsequent
    // drain observes it (the SeqCst load reads the SeqCst-stored 0).
    let b = WriterDrainBarrier::new();
    {
        let _g = b.enter_writer();
        assert_eq!(b.active_count(), 1);
    }
    assert_eq!(b.active_count(), 0);
    b.drain().await;
}

#[tokio::test]
async fn clone_shares_the_same_counter() {
    // Clones must observe the same counter (mirrors TableManager's
    // Arc-shared barrier flags) — a writer on one clone must be visible
    // to a drain on another.
    let a = WriterDrainBarrier::new();
    let b = a.clone();
    let _g = a.enter_writer();
    assert_eq!(
        b.active_count(),
        1,
        "clone must share the same Arc<AtomicUsize>"
    );
}
