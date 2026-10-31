//! Cards RV-TRANSPORT-TCP-79910BA5 + RV-CLIENT-E16A04E5-D +
//! L4-RV-CLIENT-1C219006 allocation/wire oracles.
//!
//! ## Why a PER-THREAD allocation counter
//! The old `WINDOW_ALLOCS` was a process-global `AtomicUsize`: any other test
//! thread allocating inside our window polluted the sample. The counter is
//! now a `thread_local!` `Cell<usize>` incremented inside `alloc`; the tests
//! run on a `current_thread` runtime, so the ping, its reader task and the
//! peer side all live on THIS thread and only our own allocations count.
//!
//! ## Oracle window (item 79910BA5)
//! Old code: `write_frame` allocated a fresh `Zeroizing<Vec<u8>>` of
//! `4 + N` bytes for EVERY request (and, before that, `to_msgpack`
//! allocated the whole envelope again). New code encodes the envelope into
//! the reused thread-local scratch (prefix patched in place) and hands the
//! ALREADY length-prefixed buffer to `write_frame_prereserved`, which
//! performs no heap allocation.
//!
//! The window runs between the `after_writer_acquired` hook (the last point
//! before the write call) and the frame being fully readable on the peer
//! side (which proves `write_all` + `flush` returned). The only allocation
//! the production path still makes in that window is the `WriteCancelGuard`'s
//! `Arc<str>` cancellation cause — hence the tight bound of 1, which the OLD
//! code violates (that `Arc<str>` + `write_frame`'s `Zeroizing<Vec<u8>>`
//! frame buffer = at least 2).
//!
//! ## Warm-up
//! One throwaway ping roundtrip (peer answers a valid framed Pong) runs
//! FIRST so both thread-local scratch slots reach their high-water capacity;
//! the measured request must not pay growth allocations. The warm-up uses a
//! separate hook-less client — the slots are thread-local, so the growth
//! carries over; the measured client's one-shot hook stays armed for the
//! window.
//!
//! ## Item RV-CLIENT-E16A04E5-D (scratch held through the response wait)
//! Before the fix, both `ScratchReturn` guards were released only when
//! `roundtrip` returned — i.e. AFTER `await_pending_response`. Two
//! consequences: a concurrent request on the same thread found BOTH slots
//! empty (taken → 0-capacity Vec) and had to allocate ≥ 2 buffers, and the
//! secret-bearing request body stayed live (unwiped, outside the slot)
//! through the whole response wait. The fix drops both guards right after
//! the write phase, before any response await. The second oracle below
//! overlaps request 2's encode+write with request 1's response wait and
//! fails on the unfixed code (empty slots → growth allocations).
//!
//! ## Item L4-RV-CLIENT-1C219006 (interner pre-touch version gate)
//! `execute_with_receipts` used to pass `Some(self.interner_cache())`
//! unconditionally, so even a LEGACY peer (server_query_version < 2) got
//! `get_or_create` cache entries for every distinct repo. The gate now
//! mirrors `Client::execute`'s `should_advertise_interner_epochs` check; the
//! third oracle fails on the old code because the registry gains entries for
//! `(db, repo)` even with the v0 peer.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::pin::pin;

use shamir_connect::common::envelope::ResponseEnvelope;
use shamir_query_types::batch::{BatchRequest, BatchResponse};
use shamir_types::types::common::TMap;
use shamir_types::types::value::QueryValue;
use tokio::io::{duplex, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};

use crate::builder::batch::Batch;
use crate::builder::write::{insert, Insert};
use crate::client::{HookPoint, LifecycleHooks};
use crate::{Client, DbResponse};
use zeroize::Zeroizing;

/// Per-thread allocation counter wrapping the system allocator. Exactly ONE
/// `#[global_allocator]` exists per test binary (verified: shamir-client had
/// none before this test). Each thread counts its own allocations; a test
/// samples/resets ONLY its own thread's counter, so other tests' threads
/// cannot pollute the window.
struct CountingAlloc;

thread_local! {
    static WINDOW_ALLOCS: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            WINDOW_ALLOCS.with(|c| c.set(c.get() + 1));
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn window_reset() {
    WINDOW_ALLOCS.with(|c| c.set(0));
}

fn window_load() -> usize {
    WINDOW_ALLOCS.with(Cell::get)
}

/// ALLOCATION-FREE peer-side frame read for the measured window: the 4-byte
/// prefix goes into a STACK buffer and the body into a Vec PRE-RESERVED in
/// setup (the allocating `read_frame` is banned inside the window). Returns
/// the payload length.
async fn read_frame_no_alloc(reader: &mut (impl AsyncRead + Unpin), body: &mut Vec<u8>) -> usize {
    let mut prefix = [0u8; 4];
    reader.read_exact(&mut prefix).await.expect("frame prefix");
    let len = u32::from_be_bytes(prefix) as usize;
    body.clear();
    body.resize(len, 0); // within pre-reserved capacity: no allocation
    reader
        .read_exact(&mut body[..len])
        .await
        .expect("frame body");
    len
}

/// tokio's `duplex` grows its internal pipe buffer lazily on the first write.
/// Pushing one chunk through before the client is built means the measured
/// window sees only the production path's own allocations; the consumed
/// space is reused by the next, smaller write without reallocating.
async fn prewarm_pipe(
    writer: &mut (impl AsyncWrite + Unpin),
    reader: &mut (impl AsyncRead + Unpin),
) {
    let chunk = [0u8; 512];
    writer.write_all(&chunk).await.expect("pre-warm write");
    let mut sink = [0u8; 512];
    reader.read_exact(&mut sink).await.expect("pre-warm read");
}

/// Write a [`ResponseEnvelope`] carrying an encoded [`DbResponse`] payload.
async fn write_response_frame(writer: &mut (impl AsyncWrite + Unpin), rid: u32, payload: Vec<u8>) {
    let env = ResponseEnvelope::ok(Some(rid), payload);
    let bytes = env.to_msgpack().expect("encode response envelope");
    write_frame(writer, &bytes).await.expect("write_frame");
}

/// A valid framed Pong answer so a warm-up ping roundtrip completes.
async fn write_pong(writer: &mut (impl AsyncWrite + Unpin), rid: u32) {
    let payload = rmp_serde::to_vec_named(&DbResponse::Pong).expect("encode pong");
    write_response_frame(writer, rid, payload).await;
}

fn empty_batch_response() -> BatchResponse {
    BatchResponse {
        id: QueryValue::Null,
        results: TMap::default(),
        execution_plan: Vec::new(),
        edge_provenance: TMap::default(),
        execution_time_us: 0,
        transaction: None,
        interner_delta: TMap::default(),
    }
}

/// A valid framed `DbResponse::Batch` answer for `execute_with_receipts`.
async fn write_batch_response(writer: &mut (impl AsyncWrite + Unpin), rid: u32) {
    let payload = rmp_serde::to_vec_named(&DbResponse::Batch {
        response: empty_batch_response(),
    })
    .expect("encode batch response");
    write_response_frame(writer, rid, payload).await;
}

fn two_repo_batch() -> BatchRequest {
    let mut batch = Batch::new();
    batch.insert("w1", insert("items").row(QueryValue::Null));
    batch.insert(
        "w2",
        Insert::with_repo("hot", "sessions").row(QueryValue::Null),
    );
    batch.build()
}

/// Warm-up: one FULL throwaway ping roundtrip on a hook-less client so both
/// thread-local scratch slots reach their high-water capacity. The measured
/// request on a SECOND client (same thread ⇒ same thread-local slots) must
/// not pay growth allocations.
async fn warm_up_scratch_slots() {
    let (client_write, mut server_read) = duplex(64 * 1024);
    let (client_read, mut server_write) = duplex(64 * 1024);
    let client = Client::for_test(client_read, client_write, LifecycleHooks::default());
    client.set_next_request_id_for_test(1);
    let warm = client.ping();
    let peer = async {
        let _frame = read_frame(&mut server_read, MAX_FRAME_SIZE_DEFAULT)
            .await
            .expect("warm-up request frame");
        write_pong(&mut server_write, 1).await;
    };
    let (res, ()) = tokio::join!(warm, peer);
    res.expect("warm-up ping roundtrip ok");
    drop(client);
}

// ─── Card RV-TRANSPORT-TCP-79910BA5 ─────────────────────────────────────────

/// The write phase (writer acquired → frame fully on the wire) must allocate
/// at most the `WriteCancelGuard` cause string — no frame buffer, no payload
/// copy. Old code allocates `write_frame`'s fresh `Zeroizing<Vec<u8>>` here,
/// so this test FAILS on the old implementation.
#[tokio::test(flavor = "current_thread")]
async fn rv_client_79910ba5_write_phase_allocates_no_frame_buffer() {
    // A current-thread runtime keeps the sampling window single-threaded and
    // deterministic: no other task can allocate inside it.
    warm_up_scratch_slots().await;

    let (mut client_write, mut server_read) = duplex(64 * 1024);
    prewarm_pipe(&mut client_write, &mut server_read).await;
    let (client_read, _server_write) = duplex(64 * 1024);
    let (hook, reached, release) = HookPoint::new();
    let client = Client::for_test(
        client_read,
        client_write,
        LifecycleHooks {
            after_writer_acquired: Some(hook),
            ..Default::default()
        },
    );
    client.set_next_request_id_for_test(2);

    // Measured request: parks in the hook, writes, then awaits a response
    // that never comes (dropped after the sample).
    let mut measured = pin!(async {
        let _ = client.ping().await;
    });

    // The peer branch is driven by the SAME select as the measured window,
    // so its read buffer is allocated BEFORE the window opens — otherwise it
    // would count as an in-window allocation and break the tight bound.
    let mut body = Vec::with_capacity(4096);
    let window = tokio::select! {
        w = async {
            // Ping acquired the writer and parked in the hook.
            reached.await.expect("writer acquired hook fired");
            // ── sample window starts ─────────────────────────────────────
            window_reset();
            release.send(()).expect("release write phase");
            // The frame is fully readable ⇒ write_all + flush returned.
            // ALLOCATION-FREE peer read (stack prefix + pre-reserved body).
            let len = read_frame_no_alloc(&mut server_read, &mut body).await;
            assert!(len > 0, "request frame bytes reached the peer");
            // ── sample window closes: capture BEFORE anything else runs ──
            window_load()
        } => w,
        r = &mut measured => panic!("measured ping resolved during window: {r:?}"),
    };

    assert!(
        window <= 1,
        "write phase allocated {window} times; bound 1 (WriteCancelGuard \
         Arc<str>) — old code allocates >= 2 (that Arc<str> PLUS \
         write_frame's Zeroizing<Vec<u8>>(4+N) frame buffer)"
    );

    // The parked request future drops at the end of this test (clippy
    // forbids a redundant `drop` on the non-Drop `Pin<&mut _>`): dropping it
    // mid-flight arms the WriteCancelGuard and shuts the lifecycle down; the
    // Client's own Drop then aborts the reader. The test is over, this is
    // expected.
}

// ─── Card RV-CLIENT-E16A04E5-D ──────────────────────────────────────────────

/// The secret-bearing body slot must be handed back zeroized (len 0, whole
/// allocation zero) once the write phase ends, while the request is still
/// awaiting its response. The unfixed code keeps the buffer outside the slot
/// (0-capacity default left behind) until the response wait is over.
#[tokio::test(flavor = "current_thread")]
async fn rv_client_e16a04e5_d_body_slot_zeroized_before_response_wait() {
    warm_up_scratch_slots().await;

    let secret: &[u8] = b"pw-e16a04e5d-secret";
    let (client_write, mut server_read) = duplex(64 * 1024);
    let (client_read, _server_write) = duplex(64 * 1024);
    let client = Client::for_test(client_read, client_write, LifecycleHooks::default());

    // Box-pinned so the parked request can be dropped EXPLICITLY (releasing
    // its borrow of `client`) before the client is closed.
    let mut send = Box::pin(async {
        let _ = client
            .create_scram_user("u", Zeroizing::new("pw-e16a04e5d-secret".into()), vec![])
            .await;
    });

    tokio::select! {
        () = async {
            // Plain allocating read is fine here — allocation is not measured.
            let frame = read_frame(&mut server_read, MAX_FRAME_SIZE_DEFAULT)
                .await
                .expect("request frame");
            // Oracle teeth: the secret really WAS in the request body on the
            // wire, i.e. it really lived in the body slot in flight.
            assert!(
                frame.windows(secret.len()).any(|w| w == secret),
                "request body must carry the secret while in flight"
            );
            // The request is now parked in await_pending_response (the peer
            // will never answer). The body slot must already be returned:
            assert_eq!(
                client.request_body_scratch_len_for_test(),
                0,
                "body slot must be empty (len 0) before the response wait"
            );
            let capacity = client.request_body_scratch_capacity_for_test();
            assert!(
                capacity > 0,
                "body slot must be RESTORED before the response wait; \
                 unfixed code holds it until roundtrip returns, leaving the \
                 0-capacity default Vec in the slot"
            );
            // SAFETY: capacity captured on this same thread one line above and
            // the parked request cannot grow the slot; after the ScratchReturn
            // drop every capacity byte was zeroize-written (initialized).
            let bytes =
                unsafe { client.request_body_scratch_capacity_bytes_for_test(capacity) };
            assert!(
                bytes.iter().all(|&byte| byte == 0),
                "body slot allocation must be fully zeroed while parked"
            );
            assert!(
                !bytes.windows(secret.len()).any(|w| w == secret),
                "secret bytes must be absent from the returned body slot"
            );
        } => (),
        r = &mut send => panic!(
            "secret request resolved before any response arrived: {r:?}"
        ),
    }

    // Drop the parked request mid-flight (WriteCancelGuard shutdown is
    // expected), which releases its borrow, then close the client cleanly.
    drop(send);
    client.close().await;
}

/// Allocation count of request 2's encode+write on its own client. When
/// `park_first` is set, request 1 (another client, SAME thread ⇒ same
/// thread-local scratch slots) is parked in its response wait first; the
/// slots are only available to request 2 if request 1 returned them after
/// its write phase. Request 2's encode happens BEFORE writer acquisition
/// (api.rs: encode → register → writer), so it never waits on request 1.
async fn second_request_window(park_first: bool) -> usize {
    warm_up_scratch_slots().await;

    // Request 1: own client, own one-shot hook.
    let (w1, mut sr1) = duplex(64 * 1024);
    let (r1, _sw1) = duplex(64 * 1024);
    let (hook1, reached1, release1) = HookPoint::new();
    let client1 = Client::for_test(
        r1,
        w1,
        LifecycleHooks {
            after_writer_acquired: Some(hook1),
            ..Default::default()
        },
    );
    client1.set_next_request_id_for_test(21);

    // Phase 1 (park_first only): request 1 runs until its frame is FULLY READ
    // by the allocation-free peer — write phase done, response still pending.
    // Without `park_first` request 1 never starts (the future stays pending).
    let mut ping1 = pin!(async {
        if park_first {
            let _ = client1.ping().await;
        } else {
            std::future::pending::<()>().await;
        }
    });
    let mut body = Vec::with_capacity(4096);
    if park_first {
        tokio::select! {
            () = async {
                reached1.await.expect("request 1 writer acquired");
                release1.send(()).expect("release request 1 write");
                let len = read_frame_no_alloc(&mut sr1, &mut body).await;
                assert!(len > 0, "request 1 frame reached the peer");
            } => (),
            r = &mut ping1 => panic!("request 1 resolved before its response: {r:?}"),
        }
    }
    // Request 1 is now parked in await_pending_response (slots: RELEASED on
    // the fixed code, still HELD on the unfixed code).

    // Request 2: a second ping on a second client, SAME thread ⇒ same
    // thread-local scratch slots. Its encode+write overlaps request 1's
    // response wait.
    let (w2, mut sr2) = duplex(64 * 1024);
    let (r2, _sw2) = duplex(64 * 1024);
    let (hook2, reached2, release2) = HookPoint::new();
    let client2 = Client::for_test(
        r2,
        w2,
        LifecycleHooks {
            after_writer_acquired: Some(hook2),
            ..Default::default()
        },
    );
    client2.set_next_request_id_for_test(22);

    let mut ping2 = pin!(async {
        let _ = client2.ping().await;
    });
    // ── window opens BEFORE request 2 is first polled ───────────────────
    window_reset();
    let window = tokio::select! {
        w = async {
            reached2.await.expect("request 2 writer acquired");
            release2.send(()).expect("release request 2 write");
            let len = read_frame_no_alloc(&mut sr2, &mut body).await;
            assert!(len > 0, "request 2 frame reached the peer");
            // ── window closes: capture BEFORE anything else runs ────────
            window_load()
        } => w,
        r = &mut ping1 => panic!("request 1 resolved during window: {r:?}"),
        r = &mut ping2 => panic!("request 2 resolved during window: {r:?}"),
    };

    // Both parked futures drop on return (see the 79910BA5 oracle's note
    // about mid-flight drops).
    window
}

/// Request 2's allocation count must not depend on request 1 being parked in
/// its response wait. The fixed code returns both scratch slots after the
/// write phase, so request 2 finds them warm and the two counts match (one
/// of slack for incidental noise). The UNFIXED code holds the slots until
/// the response arrives: request 2 then grows two empty Vecs from scratch
/// (the 4-byte prefix and a ~45-byte envelope alone take several doubling
/// reallocs), so the parked count exceeds the baseline by well over one.
/// The A/B difference cancels the allocations both runs share (oneshot, scc
/// pending map, `WriteCancelGuard` cause, first duplex write).
#[tokio::test(flavor = "current_thread")]
async fn rv_client_e16a04e5_d_concurrent_request_reuses_slots_during_response_wait() {
    let baseline = second_request_window(false).await;
    let parked = second_request_window(true).await;
    assert!(
        parked <= baseline + 1,
        "request 2 allocated {parked} times while request 1 awaited its \
         response, but only {baseline} with the slots free: the scratch \
         slots were still held through the response wait"
    );
}

// ─── Card L4-RV-CLIENT-1C219006 ─────────────────────────────────────────────

/// A legacy peer (default `server_query_version` 0 for `Client::for_test`)
/// must NOT get interner-cache entries pre-touched by
/// `execute_with_receipts`; a v2 peer MUST. On the OLD code the registry was
/// passed unconditionally, so the first assertion fails (the legacy exchange
/// `get_or_create`s entries for every distinct repo).
#[tokio::test(flavor = "current_thread")]
async fn rv_client_1c219006_execute_with_receipts_legacy_peer_skips_interner_pretouch() {
    let (client_write, mut server_read) = duplex(64 * 1024);
    let (client_read, mut server_write) = duplex(64 * 1024);
    let client = Client::for_test(client_read, client_write, LifecycleHooks::default());
    client.set_next_request_id_for_test(41);

    // Legacy exchange (v0): the peer answers with a valid framed Batch.
    let fut = client.execute_with_receipts("db", two_repo_batch(), None);
    let peer = async {
        let _req = read_frame(&mut server_read, MAX_FRAME_SIZE_DEFAULT)
            .await
            .expect("legacy request frame");
        write_batch_response(&mut server_write, 41).await;
    };
    let (out, ()) = tokio::join!(fut, peer);
    let (_response, receipts) = out.expect("legacy execute_with_receipts ok");
    assert!(receipts.is_empty(), "empty BatchResponse ⇒ no receipts");

    assert!(
        client
            .interner_cache()
            .peek_for_test("db", "main")
            .is_none(),
        "legacy peer (v0): interner registry must NOT be pre-touched for \
         (db, main)"
    );
    assert!(
        client.interner_cache().peek_for_test("db", "hot").is_none(),
        "legacy peer (v0): interner registry must NOT be pre-touched for \
         (db, hot)"
    );

    // Positive control: the SAME client flipped to a v2 peer — the gate now
    // passes and the pre-touch DOES create the entries.
    client.set_server_query_version_for_test(2);
    client.set_next_request_id_for_test(42);
    let fut = client.execute_with_receipts("db", two_repo_batch(), None);
    let peer = async {
        let _req = read_frame(&mut server_read, MAX_FRAME_SIZE_DEFAULT)
            .await
            .expect("v2 request frame");
        write_batch_response(&mut server_write, 42).await;
    };
    let (out, ()) = tokio::join!(fut, peer);
    out.expect("v2 execute_with_receipts ok");

    assert!(
        client
            .interner_cache()
            .peek_for_test("db", "main")
            .is_some(),
        "v2 peer: pre-touch must create (db, main)"
    );
    assert!(
        client.interner_cache().peek_for_test("db", "hot").is_some(),
        "v2 peer: pre-touch must create (db, hot)"
    );
}
