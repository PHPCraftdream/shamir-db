//! `list_activity` — the `ListActivity` paging worker (R4b-1, D1/D4/D13).
//!
//! Inherent method on [`ShamirDbHandler`], routed by `handle_decoded` as
//! the single, standalone, non-transactional op of an admitted batch,
//! AFTER admission (this call's own in-flight entry is registered and its
//! `exec_id` is passed in so the worker can exclude it from the `Requests`
//! section — otherwise every page could observe itself as a freshly
//! minted row and the walk would never exhaust). Control rows are ALSO
//! excluded by their static op kind (`admin:list_activity`) so operator
//! listings never appear in any page of any worker, documented per the
//! orchestrator ruling.
//!
//! Rendering order and sources (D1 — no new source of truth):
//! sessions → requests → transactions → cursors;
//! - sessions from [`SessionStore::activity_snapshot`] PLUS the token
//!   service's `TokenSessionStore::activity_snapshot` (both global, all
//!   dbs; both store `Arc<Session>` under the shared handle mint, so
//!   handles are unique across the union). Stateless REST never holds a
//!   store session, so its handle-0 sentinel entries are skipped —
//!   stateless REST is NOT enumerated as sessions;
//! - requests from a key pass over [`ActivityRequests::for_each`] plus ONE
//!   indexed lock-free [`ActivityRequests::get`] (`u64` key) lookup per
//!   SELECTED id (no duplicate second scan), with a filter recheck on
//!   each fetched entry;
//! - transactions from [`TxRegistry::for_each_activity`] for the key pass
//!   plus the indexed [`TxRegistry::activity_row`] for rendering, with an
//!   Own+db RECHECK on each selected row (tx handles are minted per repo
//!   and can be reused across repos — a stale key must never render
//!   another user's row);
//! - cursors from [`CursorRegistry::activity_snapshot`], whose rows are
//!   `(cursor_id, Arc<CursorActivity>)` pairs carrying the owning user id
//!   directly (stable id — no session-owner indirection needed).
//!
//! Paging (D4): each section is one keyset walk. The section's rows (or
//! their numeric keys) are DEDUPLICATED (a pinned scc iteration may visit
//! an id twice) and sorted ascending, then the first `remaining` rows are
//! rendered in that exact sorted order. Whether the section OVERFLOWED is
//! decided against the full deduplicated key count `> remaining` — the
//! `remaining+1`-th key is a pure lookahead and is NEVER rendered. A full
//! page emits ONE continuation anchored either at the last RENDERED key
//! of the overflowing section (keyset `> last_key` on the next call —
//! resuming from the first unrendered key would silently drop that row)
//! or, when the page filled exactly at a section boundary, at the top of
//! the next section with no skipped key. A resumed `All` walk starts at
//! the resume section — earlier sections are never re-rendered. The
//! token re-states the request's scope and kind filter, so a page can
//! never drift into a different scope or filter mid-chain.
//!
//! Every reader is lock-free (epoch guards / atomics only): the worker
//! takes NO lock any writer takes and touches NO transaction or cursor
//! mutable state (no `ctx` mutex, no pagination mutex). A momentarily
//! torn snapshot is acceptable and documented (D4): a row that vanishes
//! between the key pass and the render pass is skipped, never mis-served.
//!
//! Memory discipline: the copyable projections (ids, handles, user ids,
//! state words, timestamps) are collected during the filter pass; strings
//! are cloned ONLY for the rows actually rendered (two-pass scan for the
//! callback-based registries; the session and cursor snapshot APIs copy
//! `Arc`s only). No per-row rescan, no full DTO list retained beyond the
//! page.
//!
//! Own filter (D13): rows are matched by the directory-minted 16-byte
//! `user_id` (stable across that user's separate sessions) — never by the
//! bearer token and never by the mutable display username.
//!
//! Privacy (D3): rows carry ids, actor, db, static op-kind names, timing
//! and state only. The bearer token, its prefix, filter values, literals
//! and row data can never enter a row by construction. Session `Arc`s
//! (which contain credential material) are used internally only and never
//! rendered, logged or Debug-formatted.

use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use shamir_collections::TFxMap;
use shamir_connect::common::TransportKind;
use shamir_connect::server::session::Session;
use shamir_connect::server::TokenSessionStore;
use shamir_query_types::admin::activity::{
    ActivityKind, ActivityPage, ActivityRequestState, ActivityRow, ActivityScope, ActivityTxState,
    ListActivityReq, RequestExecId,
};
use shamir_query_types::wire::DbResponse;
use shamir_types::access::{Action, ResourcePath};

use super::continuation::{fresh_sequence, section_order, ActivityContinuation};
use super::requests::REQUEST_STATE_RUNNING;
use crate::cursor_registry::CursorActivitySnapshot;
use crate::engine::db_handler::handler::{session_actor, ShamirDbHandler};
use crate::tx_registry::TxActivityState;

/// Default page cap: rendered rows per call (D4 global cap).
const DEFAULT_PAGE_CAP: u32 = 256;
/// Hard maximum page cap: a larger requested `limit` is clamped, not
/// rejected (D4). `limit: 0` is rejected as `invalid_request`.
const MAX_PAGE_CAP: u32 = 256;

/// Static op-kind name of the listing operation itself (`BatchOp::
/// activity_name`); control rows are excluded from every `Requests`
/// section so an operator listing can never observe listings.
const LIST_ACTIVITY_OP_KIND: &str = "admin:list_activity";

/// Handle sentinel meaning "no minted handle" (stateless REST token
/// paths); such entries are never enumerated as sessions or owners.
const UNSTAMPED_HANDLE: u64 = 0;

/// Wall-clock now in nanoseconds since `UNIX_EPOCH` (the clock the
/// sessions stamp `created_at_ns` / `last_activity_ns` with).
fn unix_now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn error(code: &str, message: &str) -> DbResponse {
    DbResponse::Error {
        code: code.to_owned(),
        message: message.to_owned(),
    }
}

/// Resolve the effective page cap; `None` = the requested limit is zero
/// (typed `invalid_request`), excessive values clamp to [`MAX_PAGE_CAP`].
fn page_cap(limit: Option<u32>) -> Option<u32> {
    match limit {
        Some(0) => None,
        Some(n) => Some(n.min(MAX_PAGE_CAP)),
        None => Some(DEFAULT_PAGE_CAP),
    }
}

/// Stable display label for a session's transport (row field only).
fn transport_label(kind: TransportKind) -> &'static str {
    match kind {
        TransportKind::Tcp => "tcp",
        TransportKind::WebSocket => "ws",
        TransportKind::Unix => "ipc",
    }
}

/// Request-scoped context shared by every render helper: the request's
/// scope and kind filter (so continuation tokens always re-state exactly
/// what the caller asked for), the targeted database, the Own-filter
/// identity and the caller's own exec id to exclude from `Requests`.
struct PageContext<'a> {
    scope: ActivityScope,
    kind: ActivityKind,
    db: &'a str,
    own: bool,
    caller_user_id: [u8; 16],
    exclude: Option<RequestExecId>,
}

/// Per-section paging outcome.
struct SectionOutcome {
    /// Number of rows this section contributed to the page.
    rendered: u32,
    /// True when the section still had rows beyond its contribution:
    /// decided against the full deduplicated key count, NOT the cap+1
    /// lookahead slice.
    overflowed: bool,
    /// Keyset boundary: the last rendered key, or the section's incoming
    /// resume key when nothing was rendered (page already full).
    last_key: u64,
}

impl PageContext<'_> {
    /// Apply one section's outcome to the page: advance the remaining
    /// budget and, on overflow, emit the chain's continuation — anchored
    /// inside THIS section at [`SectionOutcome::last_key`].
    ///
    /// P1 hard paging cap: the subtraction is SATURATING. `rendered` is
    /// bounded by the section's own `min(remaining, …)` selection, so the
    /// budget can never legitimately go negative; the saturating form
    /// guarantees that even a torn-snapshot renderer overshoot (a future
    /// section contributing past a boundary) clamps the budget at 0 and
    /// every later section renders nothing — the page can never exceed
    /// [`MAX_PAGE_CAP`] rows under any interleaving.
    fn apply(
        &self,
        render: &mut PageRender,
        section: ActivityKind,
        outcome: SectionOutcome,
        sequence: u64,
    ) {
        render.remaining = render.remaining.saturating_sub(outcome.rendered);
        if outcome.overflowed {
            render.continuation = Some(ActivityContinuation::new(
                self.scope,
                self.kind,
                section,
                outcome.last_key,
                sequence,
            ));
        }
    }
}

/// One page's rendering state.
struct PageRender {
    remaining: u32,
    rows: Vec<ActivityRow>,
    continuation: Option<ActivityContinuation>,
}

impl PageRender {
    fn new(cap: u32) -> Self {
        Self {
            remaining: cap,
            rows: Vec::with_capacity(cap as usize),
            continuation: None,
        }
    }
}

impl ShamirDbHandler {
    /// Serve one bounded `ListActivity` page (design §2, §5.1–§5.2).
    ///
    /// Called by `handle_decoded` after admission with the caller's own
    /// minted `exec_id` (when the batch minted one) so the listing can
    /// exclude itself from its own `Requests` section.
    ///
    /// Permission (D13/D19) runs BEFORE the continuation token is parsed:
    /// `scope: All` requires `ActivityView` + `Action::Manage` through the
    /// normal `authorize_access` path (never the coarse admin gate — the
    /// op is coarse-gate-exempt); `scope: Own` is open to any authenticated
    /// session and is filtered server-side by stable user id. Read-only on
    /// a follower: no write path is touched and no node-mode gate applies.
    pub(in crate::engine::db_handler) async fn list_activity(
        &self,
        session: &Session,
        db_name: &str,
        req: &ListActivityReq,
        exec_id: Option<RequestExecId>,
    ) -> DbResponse {
        // 1. Permission gate FIRST — before any token parse or scan.
        if req.scope == ActivityScope::All {
            let actor = session_actor(session);
            if self
                .db
                .authorize_access(&actor, &ResourcePath::ActivityView, Action::Manage)
                .await
                .is_err()
            {
                return error(
                    "permission_denied",
                    "scope All requires ActivityView manage permission",
                );
            }
        }

        // 2. Page cap (form validation before any registry work).
        let Some(cap) = page_cap(req.limit) else {
            return error("invalid_request", "limit must be at least 1");
        };

        // 3. Continuation: parse the incoming token (after the permission
        //    gate) or mint a fresh chain sequence. The token must re-state
        //    the request's own scope and kind filter — it can never widen
        //    what this call is allowed to see, and a resumed `All` walk
        //    starts at the resume section, never re-rendering earlier ones.
        let mut resume: Option<ActivityContinuation> = None;
        let sequence;
        match &req.continuation {
            None => sequence = fresh_sequence(),
            Some(token) => match ActivityContinuation::decode(token) {
                Ok(decoded) => {
                    if decoded.scope != req.scope || decoded.filter != req.kind {
                        return error(
                            "invalid_request",
                            "continuation token does not match the request scope/kind",
                        );
                    }
                    sequence = decoded.sequence;
                    resume = Some(decoded);
                }
                Err(e) => {
                    return error("invalid_request", e.message());
                }
            },
        }

        let ctx = PageContext {
            scope: req.scope,
            kind: req.kind,
            db: db_name,
            own: req.scope == ActivityScope::Own,
            caller_user_id: session.user_id,
            exclude: exec_id,
        };

        // Skip every section BEFORE the resume section (a resumed `All`
        // walk must not start from Sessions again).
        let order = section_order(req.kind);
        let start = resume
            .map(|r| order.iter().position(|s| *s == r.section).unwrap_or(0))
            .unwrap_or(0);

        // 4. Render sections in the fixed order until the page is full.
        let mut render = PageRender::new(cap);
        for section in &order[start..] {
            let last_key = resume
                .filter(|r| r.section == *section)
                .map_or(0, |r| r.last_key);
            let outcome = match section {
                ActivityKind::Sessions => self.render_sessions(&mut render, &ctx, last_key),
                ActivityKind::Requests => self.render_requests(&mut render, &ctx, last_key),
                ActivityKind::Transactions => self.render_transactions(&mut render, &ctx, last_key),
                ActivityKind::Cursors => self.render_cursors(&mut render, &ctx, last_key),
                // `All` is a filter, never a section.
                ActivityKind::All => unreachable!("section_order never yields All"),
            };
            ctx.apply(&mut render, *section, outcome, sequence);
            if render.continuation.is_some() {
                break;
            }
        }

        DbResponse::ActivityPage {
            page: ActivityPage::new(render.rows, render.continuation.map(|c| c.encode())),
        }
    }

    /// Gauge source (§5.4): the live registry mirrors as one snapshot.
    /// Exact mirrors — never `len()` on an scc map. Installed as the
    /// per-server observability source at boot (weak handle; after
    /// shutdown the gauges render zeros). Sessions count BOTH the
    /// dispatch session store and the token session store (stateless REST
    /// holds no store session, so it contributes nothing).
    pub(crate) fn activity_counts(&self) -> crate::observability::ActivityMetricsSnapshot {
        crate::observability::ActivityMetricsSnapshot {
            requests: self.activity_requests.in_flight() as u64,
            sessions: (self.session_store.as_ref().map_or(0, |s| s.len())
                + self.token_service_sessions().map_or(0, |s| s.len()))
                as u64,
            transactions: self.tx_registry.activity_count() as u64,
            cursors: self.cursor_registry.len() as u64,
        }
    }

    /// The token service's session store, when one is attached.
    /// `None` in handlers constructed without token administration.
    fn token_service_sessions(&self) -> Option<&TokenSessionStore> {
        self.token_service
            .as_ref()
            .map(|svc| svc.sessions().as_ref())
    }

    // ----- sessions ------------------------------------------------------

    fn render_sessions(
        &self,
        render: &mut PageRender,
        ctx: &PageContext<'_>,
        last_key: u64,
    ) -> SectionOutcome {
        // Sessions are global (§2.1): every live session of both stores is
        // listed regardless of the targeted db. Snapshots of `Arc`s only —
        // no credential material leaves this function.
        let mut sessions: Vec<_> = self
            .session_store
            .as_ref()
            .map(|store| store.activity_snapshot())
            .unwrap_or_default();
        let tokens = self
            .token_service_sessions()
            .map(|store| store.activity_snapshot())
            .unwrap_or_default();
        sessions.extend(tokens);

        // Sort by the minted handle, deduplicate (handles are unique by
        // the shared mint; the dedup guards torn-snapshot double visits),
        // then filter and take the first `remaining` IN ORDER. The
        // overflow check uses the full filtered count — never the cap+1
        // lookahead.
        sessions.sort_by_key(|s| s.activity_handle());
        sessions.dedup_by_key(|s| s.activity_handle());
        let matching: Vec<_> = sessions
            .iter()
            .filter(|s| {
                let handle = s.activity_handle();
                handle != UNSTAMPED_HANDLE
                    && handle > last_key
                    && (!ctx.own || s.user_id == ctx.caller_user_id)
            })
            .collect();
        let overflowed = matching.len() > render.remaining as usize;
        let rendered = matching.len().min(render.remaining as usize);
        let selected = &matching[..rendered];

        let selected_handles: Vec<u64> = selected.iter().map(|s| s.activity_handle()).collect();

        // Per-session open tx/cursor counts for the SELECTED sessions only
        // (the documented exception to "scan only requested types"): one
        // lock-free pass over each count registry, no per-row rescan.
        let (tx_counts, cursor_counts) = if rendered > 0 {
            (
                self.tx_counts_by_session(&selected_handles),
                self.cursor_counts_by_session(&selected_handles),
            )
        } else {
            (TFxMap::default(), TFxMap::default())
        };

        // Render: clone strings only for the selected sessions.
        let now_ns = unix_now_ns();
        for s in selected {
            let handle = s.activity_handle();
            render.rows.push(ActivityRow::Session {
                session_handle: handle,
                user: s.username.clone(),
                transport: transport_label(s.transport_kind).to_owned(),
                created_at_ns: s.created_at_ns,
                idle_ns: now_ns.saturating_sub(s.last_activity_ns.load(Ordering::Relaxed)),
                open_transactions: tx_counts.get(&handle).copied().unwrap_or(0),
                open_cursors: cursor_counts.get(&handle).copied().unwrap_or(0),
            });
        }

        SectionOutcome {
            rendered: rendered as u32,
            overflowed,
            last_key: boundary(selected.iter().map(|s| s.activity_handle()), last_key),
        }
    }

    /// One lock-free pass over the tx projection counting open rows per
    /// session handle. Only the SELECTED session handles are counted
    /// (membership test against the sorted selection), and each visited tx
    /// is deduplicated by its own tx key before counting — a pinned
    /// `HashIndex` iteration may revisit an entry during a resize, and an
    /// un-deduplicated pass would overcount such a session.
    fn tx_counts_by_session(&self, selected: &[u64]) -> TFxMap<u64, usize> {
        let mut pairs: Vec<(u64, u64)> = Vec::new();
        self.tx_registry.for_each_activity(|meta| {
            let session = meta.session_handle();
            if session != UNSTAMPED_HANDLE && selected.binary_search(&session).is_ok() {
                pairs.push((session, meta.handle()));
            }
            true
        });
        pairs.sort_unstable();
        pairs.dedup();
        let mut counts = TFxMap::default();
        for (session, _) in pairs {
            *counts.entry(session).or_insert(0) += 1;
        }
        counts
    }

    /// One lock-free pass over the cursor snapshot counting open cursors
    /// per session handle (one `Arc` clone per row, zero string clones).
    /// Same discipline as [`Self::tx_counts_by_session`]: selected
    /// sessions only, deduplicated by cursor id so a resize double-visit
    /// cannot inflate a count.
    fn cursor_counts_by_session(&self, selected: &[u64]) -> TFxMap<u64, usize> {
        let mut pairs: Vec<(u64, u64)> = Vec::new();
        for row in self.cursor_registry.activity_snapshot() {
            let session = row.meta.session_handle;
            if session != UNSTAMPED_HANDLE && selected.binary_search(&session).is_ok() {
                pairs.push((session, row.cursor_id));
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        let mut counts = TFxMap::default();
        for (session, _) in pairs {
            *counts.entry(session).or_insert(0) += 1;
        }
        counts
    }

    // ----- requests ------------------------------------------------------

    fn render_requests(
        &self,
        render: &mut PageRender,
        ctx: &PageContext<'_>,
        last_key: u64,
    ) -> SectionOutcome {
        // The P1 hard cap: budget clamped SATURATINGLY in `PageContext::
        // apply`, so the renderers here can never push a page past
        // [`MAX_PAGE_CAP`] rows under any interleaving.
        let mut keys: Vec<u64> = Vec::new();
        self.activity_requests.for_each(|entry| {
            let id = entry.id.0;
            if id <= last_key
                || entry.op_kind == LIST_ACTIVITY_OP_KIND
                || entry.db != ctx.db
                || (ctx.own && entry.user_id != ctx.caller_user_id)
            {
                return;
            }
            if let Some(excluded) = ctx.exclude {
                if entry.id == excluded {
                    return;
                }
            }
            keys.push(id);
        });
        let (selected, overflowed) = select_window(keys, render.remaining);

        // Render pass: ONE indexed lock-free lookup (`ActivityRequests::
        // get`) per SELECTED id — no duplicate second scan. Each fetched
        // entry gets a filter RECHECK (D4 torn-snapshot guard): a key
        // selected above must never render a row that now fails the db,
        // Own, control-row or self-exclusion filter. The `selected` slice
        // is ascending and distinct, so rows are emitted IN SORTED ID
        // ORDER with no re-sort; a vanished batch contributes no row.
        let mut emitted = 0usize;
        for &id in &selected {
            let Some(entry) = self.activity_requests.get(id) else {
                continue;
            };
            if entry.op_kind == LIST_ACTIVITY_OP_KIND
                || entry.db != ctx.db
                || (ctx.own && entry.user_id != ctx.caller_user_id)
            {
                continue;
            }
            if let Some(excluded) = ctx.exclude {
                if entry.id == excluded {
                    continue;
                }
            }
            let state = if entry.state.load(Ordering::Acquire) == REQUEST_STATE_RUNNING {
                ActivityRequestState::Running
            } else {
                ActivityRequestState::Done
            };
            render.rows.push(ActivityRow::Request {
                request_exec_id: entry.id,
                session_handle: entry.session_handle,
                user: entry.username.clone(),
                db: entry.db.clone(),
                op_kind: entry.op_kind.to_owned(),
                state,
                elapsed_ns: entry.elapsed_ns(),
            });
            emitted += 1;
        }

        SectionOutcome {
            rendered: emitted as u32,
            overflowed,
            last_key: boundary(selected.iter().copied(), last_key),
        }
    }

    // ----- transactions --------------------------------------------------

    fn render_transactions(
        &self,
        render: &mut PageRender,
        ctx: &PageContext<'_>,
        last_key: u64,
    ) -> SectionOutcome {
        // Key pass: copyable fields only (handles, user ids; the db name
        // is compared without cloning).
        let mut keys: Vec<u64> = Vec::new();
        self.tx_registry.for_each_activity(|meta| {
            let handle = meta.handle();
            if handle <= last_key || meta.db() != ctx.db {
                return true;
            }
            if ctx.own && meta.user_id() != &ctx.caller_user_id {
                return true;
            }
            keys.push(handle);
            true
        });
        let (selected, overflowed) = select_window(keys, render.remaining);

        // Render pass: INDEXED lookups of the selected handles IN SORTED
        // ORDER with an Own+db RECHECK on each fetched row — tx handles
        // are minted per repo and can be reused across repos, so a key
        // selected above must never render a row that now belongs to
        // another user or another database (cross-user leak guard). A row
        // that changed or vanished between the passes is skipped (D4).
        let mut emitted = 0usize;
        for &handle in &selected {
            let Some(meta) = self.tx_registry.activity_row(handle) else {
                continue;
            };
            if meta.db() != ctx.db || (ctx.own && meta.user_id() != &ctx.caller_user_id) {
                continue;
            }
            render.rows.push(ActivityRow::Transaction {
                tx_handle: meta.handle(),
                session_handle: meta.session_handle(),
                user: meta.username().to_owned(),
                state: tx_state(meta.state()),
                idle_ns: meta.idle_ns(),
            });
            emitted += 1;
        }

        SectionOutcome {
            rendered: emitted as u32,
            overflowed,
            last_key: boundary(selected.iter().copied(), last_key),
        }
    }

    // ----- cursors -------------------------------------------------------

    fn render_cursors(
        &self,
        render: &mut PageRender,
        ctx: &PageContext<'_>,
        last_key: u64,
    ) -> SectionOutcome {
        // The snapshot is cheap by design (one `Arc` clone per row, zero
        // string clones, unsorted); strings are cloned here ONLY for the
        // rows actually rendered.
        let mut rows: Vec<CursorActivitySnapshot> = self
            .cursor_registry
            .activity_snapshot()
            .into_iter()
            .filter(|row| {
                row.cursor_id > last_key
                    && row.meta.db == ctx.db
                    && (!ctx.own || row.meta.user_id == ctx.caller_user_id)
            })
            .collect();
        // Sort by cursor id and deduplicate (a pinned scc iteration may
        // visit an id twice), then take the first `remaining` IN ORDER.
        rows.sort_by_key(|row| row.cursor_id);
        rows.dedup_by_key(|row| row.cursor_id);
        let overflowed = rows.len() > render.remaining as usize;
        let rendered = rows.len().min(render.remaining as usize);
        for row in &rows[..rendered] {
            let meta = &row.meta;
            render.rows.push(ActivityRow::Cursor {
                cursor_id: row.cursor_id,
                session_handle: meta.session_handle,
                user: meta.username.clone(),
                table: meta.table.clone(),
                pinned_version: meta.pinned_version,
                open_ns: meta.created.elapsed().as_nanos() as u64,
            });
        }

        SectionOutcome {
            rendered: rendered as u32,
            overflowed,
            last_key: boundary(rows[..rendered].iter().map(|row| row.cursor_id), last_key),
        }
    }
}

/// Deduplicate, sort ascending, and window the section's candidate keys
/// against the page's remaining budget. This is the PRODUCTION selection
/// helper every key-pass section uses (requests, transactions) — exercised
/// directly by `activity/tests/paging_tests.rs` with duplicate ids, exact
/// limits and keyset boundaries.
///
/// Returns the bounded, DISTINCT, ascending-selected keys plus the
/// overflow flag, which is decided against the full deduplicated key
/// count (never the truncated selection) so the `remaining+1`-th key is a
/// pure lookahead: it decides the continuation but is NEVER rendered.
pub(super) fn select_window(mut keys: Vec<u64>, remaining: u32) -> (Vec<u64>, bool) {
    keys.sort_unstable();
    keys.dedup();
    let overflowed = keys.len() > remaining as usize;
    keys.truncate(remaining as usize);
    (keys, overflowed)
}

/// Keyset boundary for a section: the last RENDERED key when the section
/// contributed rows, otherwise the incoming resume key (so an exactly
/// full page continues at the top of the next section with no skipped
/// key, and an overflowing-but-unrenderable section restarts at its own
/// resume boundary).
fn boundary<I: Iterator<Item = u64>>(rendered_keys: I, last_key: u64) -> u64 {
    rendered_keys.last().unwrap_or(last_key)
}

fn tx_state(state: TxActivityState) -> ActivityTxState {
    match state {
        TxActivityState::Active => ActivityTxState::Active,
        TxActivityState::AwaitingCommit => ActivityTxState::AwaitingCommit,
        TxActivityState::ExpiredPendingReap => ActivityTxState::ExpiredPendingReap,
    }
}
