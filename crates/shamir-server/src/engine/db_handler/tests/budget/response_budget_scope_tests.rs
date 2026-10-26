//! Regression for RV-SERVER-FB3EC046: the response budget guard stashed by
//! `ShamirDbHandler::execute` must be handed off to the per-request dispatch
//! task's writer phase — never dropped when the dispatch future resolves.
//!
//! Production AND this test both call the shared seam
//! [`byte_budget::run_with_handoff`]: the dispatch future runs inside the
//! guard slot, and the stashed guard is taken in that SAME scoped task the
//! moment the future resolves, so the guard can ride
//! `WriterMsg::{Reply,ReplyAndClose}` until the socket write completes.
//!
//! HONEST LIMIT: this test pins the shared helper, so reintroducing an
//! out-of-scope `take_stashed_guard()` in `run_with_handoff` itself fails
//! here — but a direct revert of `request_loop.rs` back to hand-rolling
//! `run_with_guard_slot` + `take_stashed_guard` at the call site would NOT
//! be discriminated by this test (byte_budget.rs would be untouched).

use crate::byte_budget::{run_with_handoff, stash_guard, ByteBudget};

#[tokio::test]
async fn rv_server_fb3ec046_guard_outlives_scope_until_writer_write() {
    let budget = ByteBudget::new(Some(8));

    // Spawned exactly like `request_loop.rs::join_set.spawn(async move { ... })`:
    // one task-local scope per dispatched request. The "dispatch stub"
    // acquires a reservation and stashes it (mirroring `execute`); the
    // returned tuple is what the writer arm consumes.
    let inner = budget.clone();
    let guard = tokio::spawn(async move {
        run_with_handoff(async {
            let guard = inner.acquire(8).await;
            stash_guard(guard);
        })
        .await
        .1
        .expect("request reservation must be handed to the writer phase")
    })
    .await
    .expect("dispatch task must not panic");

    // The guard is now owned by the "writer" (queued response / socket write
    // in flight): the reservation must still be charged.
    assert_eq!(budget.used(), 8, "writer-owned response remains charged");
    drop(guard);
    // Writer completed the write: the release happens only here.
    assert_eq!(budget.used(), 0, "writer completion releases reservation");
}
