//! FG-5c — idiomatic [`futures::Stream`] wrapper over the server-side
//! cursor wire protocol (FG-5a/FG-5b).
//!
//! [`Client::stream_cursor`](crate::Client::stream_cursor) opens a cursor
//! and returns a [`CursorStream`] that yields one [`QueryRecord`] at a time,
//! internally issuing `FetchNext` calls as the consumer polls — results no
//! longer need to materialize as one big `Vec` on the CLIENT side either
//! (mirroring FG-5b's server-side fix for the same underlying problem).
//!
//! # Cleanup — no network call in `Drop`
//!
//! Unlike [`SubscriptionHandle`](crate::subscription::SubscriptionHandle),
//! there is no client-side registry entry to clean up locally when a
//! `CursorStream` is dropped — the only thing that would help the server
//! reclaim the cursor early is a network round-trip (`CancelCursor`), and
//! `Drop` cannot run one: `Client` is `!Clone`, and a fire-and-forget
//! `tokio::spawn` at drop time would need an ambient Tokio runtime (no
//! guarantee one exists) with no error-reporting path if it failed. So
//! `CursorStream` does **not** implement `Drop` for network cleanup:
//!
//! - Draining the stream to completion (`has_more == false` on the last
//!   page) needs no explicit close — the server already auto-closes an
//!   exhausted cursor (FG-5b: `fetch_next` removes it from the registry on
//!   the last page).
//! - Calling [`CursorStream::close`] sends `CancelCursor` deterministically
//!   for early, intentional release.
//! - Dropping the stream early WITHOUT calling `close()` leaves the cursor
//!   open server-side until the idle-timeout reaper reclaims it (FG-5b
//!   default: 60s) — the SAME backstop-for-abandoned-resources philosophy
//!   FG-5b's own design already documents, not an afterthought here either.

use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll};

use futures::stream::{self, Stream};

use shamir_query_types::read::{QueryRecord, ReadQuery};
use shamir_query_types::wire::{CursorId, DbResponse};

use crate::builder::cursor::{cancel_cursor, create_cursor, fetch_next};
use crate::error::ClientError;
use crate::Client;

/// Evolving state driving the `unfold`-based inner stream.
///
/// `Init` issues `CreateCursor` on the first poll (deferring the network
/// call until the stream is actually polled, rather than at
/// `Client::stream_cursor` call time). `Buffered` holds the current page's
/// not-yet-yielded records plus whether another `FetchNext` should be
/// attempted once the buffer drains. `Exhausted` is the terminal state —
/// once reached, `unfold` returns `None` forever after (no further network
/// calls, no panics on re-poll).
enum State<'a> {
    Init {
        client: &'a Client,
        db: String,
        // Boxed: `ReadQuery` is large (`clippy::large_enum_variant` fires
        // otherwise) — this variant is only ever alive for the single
        // `CreateCursor` round-trip, so the extra indirection is a
        // negligible, one-time cost.
        query: Box<ReadQuery>,
        page_size: u32,
    },
    Buffered {
        client: &'a Client,
        cursor_id: CursorId,
        page_size: u32,
        remaining: std::vec::IntoIter<QueryRecord>,
        has_more: bool,
    },
    Exhausted,
}

/// One step of the `unfold` state machine: `Some((item, next_state))` to
/// yield `item` and continue, or `None` to end the stream.
///
/// Loops (rather than recursing) over empty-but-`has_more` pages — a
/// pathological but legal server response shape (e.g. a page whose rows
/// were all filtered client-invisible) — until either a record is found or
/// the cursor genuinely exhausts.
///
/// On any wire-level error (`ClientError` from `roundtrip`, or an
/// unexpected `DbResponse` variant) the step yields `Err(..)` as the
/// stream's next (and final) item; the state collapses to `Exhausted` so
/// the stream ends cleanly afterward instead of retrying or panicking.
async fn step(mut state: State<'_>) -> Option<(Result<QueryRecord, ClientError>, State<'_>)> {
    loop {
        state = match state {
            State::Init {
                client,
                db,
                query,
                page_size,
            } => {
                let req = create_cursor(db, *query, page_size);
                match client.roundtrip(&req).await {
                    Ok(DbResponse::CursorPage {
                        cursor_id,
                        page,
                        has_more,
                    }) => State::Buffered {
                        client,
                        cursor_id,
                        page_size,
                        remaining: page.records.into_iter(),
                        has_more,
                    },
                    Ok(other) => {
                        return Some((
                            Err(ClientError::Protocol(format!(
                                "expected CursorPage, got {other:?}"
                            ))),
                            State::Exhausted,
                        ))
                    }
                    Err(e) => return Some((Err(e), State::Exhausted)),
                }
            }
            State::Buffered {
                client,
                cursor_id,
                page_size,
                mut remaining,
                has_more,
            } => {
                if let Some(rec) = remaining.next() {
                    return Some((
                        Ok(rec),
                        State::Buffered {
                            client,
                            cursor_id,
                            page_size,
                            remaining,
                            has_more,
                        },
                    ));
                }
                if !has_more {
                    return None;
                }
                let req = fetch_next(cursor_id, Some(page_size));
                match client.roundtrip(&req).await {
                    Ok(DbResponse::CursorPage { page, has_more, .. }) => State::Buffered {
                        client,
                        cursor_id,
                        page_size,
                        remaining: page.records.into_iter(),
                        has_more,
                    },
                    Ok(other) => {
                        return Some((
                            Err(ClientError::Protocol(format!(
                                "expected CursorPage, got {other:?}"
                            ))),
                            State::Exhausted,
                        ))
                    }
                    Err(e) => return Some((Err(e), State::Exhausted)),
                }
            }
            State::Exhausted => return None,
        };
    }
}

/// A [`Stream`] of [`QueryRecord`]s backed by a server-side cursor
/// (FG-5a/FG-5b), obtained via
/// [`Client::stream_cursor`](crate::Client::stream_cursor).
///
/// Yields records one at a time, issuing `FetchNext` internally as the
/// buffer of the current page drains. See the module doc comment for the
/// cleanup contract (`close()` vs. bare `drop`).
///
/// The inner stream is boxed (`Pin<Box<dyn Stream<...> + 'a>>`) purely so
/// this type can carry `close()` as an inherent method alongside `Stream` —
/// `impl Stream<Item=...>` returned from a function is otherwise opaque
/// with no room for extra methods. Boxing one stream per cursor is a
/// negligible cost.
pub struct CursorStream<'a> {
    client: &'a Client,
    /// Set by the `unfold` closure the moment `CreateCursor` succeeds —
    /// `close()` needs the id, but `unfold`'s own state lives inside the
    /// opaque combinator, so it is mirrored out here via a shared cell.
    /// `None` until the first page has been fetched (a `close()` before
    /// any poll is then a no-op — nothing to cancel).
    cursor_id: Arc<StdMutex<Option<CursorId>>>,
    inner: Pin<Box<dyn Stream<Item = Result<QueryRecord, ClientError>> + 'a>>,
}

impl<'a> CursorStream<'a> {
    pub(crate) fn new(client: &'a Client, db: &str, query: ReadQuery, page_size: u32) -> Self {
        let cursor_id: Arc<StdMutex<Option<CursorId>>> = Arc::new(StdMutex::new(None));
        let cursor_id_for_stream = cursor_id.clone();

        let initial = State::Init {
            client,
            db: db.to_string(),
            query: Box::new(query),
            page_size,
        };

        // Wrap `step` so every successfully-opened cursor's id is mirrored
        // into `cursor_id` above.
        let inner = stream::unfold(initial, move |state| {
            let cursor_id_cell = cursor_id_for_stream.clone();
            async move {
                let next = step(state).await;
                if let Some((_, State::Buffered { cursor_id, .. })) = &next {
                    let mut guard = cursor_id_cell.lock().unwrap_or_else(|p| p.into_inner());
                    *guard = Some(*cursor_id);
                }
                next
            }
        });

        Self {
            client,
            cursor_id,
            inner: Box::pin(inner),
        }
    }

    /// The server-assigned cursor id, once known.
    ///
    /// `None` until the first page has been fetched (the underlying
    /// `CreateCursor` round-trip has not completed yet — e.g. the stream
    /// has never been polled, or the very first poll is still in flight).
    /// Exposed primarily for tests that need to drive a raw follow-up
    /// request against the same id (see
    /// `src/tests/cursor_stream_tests.rs`); ordinary callers don't need it —
    /// `close()` already knows it internally.
    pub fn cursor_id(&self) -> Option<CursorId> {
        *self.cursor_id.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Explicitly release the server-side cursor early.
    ///
    /// Sends `CancelCursor` deterministically — the intentional-release
    /// counterpart to letting an abandoned stream idle-timeout server-side
    /// (see the module doc comment). A no-op (`Ok(())`, no network call) if
    /// no page has been fetched yet (the cursor was never created).
    pub async fn close(mut self) -> Result<(), ClientError> {
        let Some(cursor_id) = self.cursor_id() else {
            return Ok(());
        };

        // Stop polling for further pages before canceling — the caller is
        // done with this stream.
        self.inner = Box::pin(stream::empty());

        let req = cancel_cursor(cursor_id);
        match self.client.roundtrip(&req).await? {
            DbResponse::CursorClosed { .. } => Ok(()),
            other => Err(ClientError::Protocol(format!(
                "expected CursorClosed, got {other:?}"
            ))),
        }
    }
}

impl<'a> Stream for CursorStream<'a> {
    type Item = Result<QueryRecord, ClientError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}
