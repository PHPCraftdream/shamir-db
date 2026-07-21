# RI-10: Replication proto_ver validation + honest journal-gap handling + doc honesty

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context — already investigated, do not re-derive

Three independent items from the 2026-07-20/21 reviews. All are scoped to
**R1-level honesty and safety**, NOT full election/quorum/failover/
multi-primary/sharding/snapshot-reseed (those stay roadmap/R2+, out of
scope here).

### Item 1 — proto_ver is accepted unconditionally

`crates/shamir-server/src/db_handler/repl_handler.rs`'s `handle_repl`,
`ReplRequest::Hello { proto_ver: _, node_id: _ } =>` line — the comment
reads `// TODO(R1): negotiate proto_ver — for R0 we accept any.` The
follower's own Hello always sends `proto_ver: 1`
(`crates/shamir-server/src/replication/source.rs:74`, a magic literal, no
shared constant exists anywhere in the tree today — verified via grep).

### Item 2 — replication is documented as `реализовано` (implemented), not experimental

`docs/guide-docs/guide/08-interconnect.md`, "### Шаг 3: Leader-follower
replication — реализовано" (~line 157) states plainly "реализовано" with
no experimental qualifier and no limitations list. Reality (confirmed by
reading `supervisor.rs`'s own module doc + `admin_replication.rs`): ONE
shared `replicator` credential authenticates every subscription's upstream
connection (no per-subscription credential store — `TODO(386-c)` in
`server/bootstrap_mode.rs`... actually see `ReplicationConfig`'s own doc
comment in `crates/shamir-server/src/config.rs` for the exact wording of
this limitation, quote it precisely rather than paraphrasing); the
supervisor is **reconcile-driven, not event-driven** (its own module doc:
"For R1 the supervisor is reconcile-driven... TODO(386-c): subscribe to
the system repo changelog... replacing the explicit notify_changed() pokes
with an event-driven watch" — i.e. convergence happens on `reconcile()`
calls, not continuously); there is no leader election, no quorum, no
automatic failover, no multi-primary, no sharding. This is a legitimate,
useful **experimental async single-leader read-replica** feature — it
must be labeled that way, not implied to be a clustering/HA solution.

### Item 3 — journal gap is silently skipped (P0#4, scope EXPANDED for this task)

`crates/shamir-server/src/replication/follower_loop.rs` lines ~285-299
(step 6, "gap-reseed (§5.3 / R1 simplification)"): when the leader reports
`gap_at: Some(g)` with `g > from_version` (the follower requested a range
the leader no longer retains), the loop currently just `warn!`s, sets
`gap_skip_from = Some(g)`, and `continue`s — silently resuming from past
the gap. **The follower keeps running with permanently missing data and
nothing records or surfaces this.** This must become a honest STOP, not a
silent skip (full snapshot-based auto-reseed remains R2/out of scope —
this task only adds the stop + visibility).

**Key finding: the visibility mechanism already exists, reuse it.**
`crates/shamir-db/src/shamir_db/execute/admin_replication.rs` already has
`handle_replication_status`/`handle_list_subscriptions`, both of which
read the `subscriptions` table and echo back each row's `state` field
verbatim (currently `"active"` / `"paused"`, flipped by the existing
`Pause`/`Resume` admin actions via `set_via_implicit_tx`). **Do NOT build
a new HTTP/status endpoint** — instead, make a gap-detected follower loop
persist `state = "resync_required"` on ITS OWN subscription row, using the
SAME table/write path `Pause`/`Resume` already use. This makes the signal
visible via the existing admin surface with zero new API, and — because
`supervisor.rs`'s `reconcile()` only (re)starts subscriptions whose row has
`state == "active"` (`Subscription::is_active()`) — a subscription stuck
at `"resync_required"` naturally stays stopped across reconcile ticks,
exactly like `"paused"` does today. Recovery for R1 is: an operator
notices `resync_required` via `ListSubscriptions`/`ReplicationStatus`,
independently verifies/fixes the follower's data (out of scope — no
automated reseed procedure exists yet), then calls the EXISTING `Resume`
admin action to flip the state back to `"active"` and let `reconcile()`
restart the loop. Do not build a new distinct "resync" admin action —
`Resume` already does exactly the state flip needed; only the STOP +
visibility are new.

## The task — Part 1: proto_ver upper-bound validation

1. Add `pub const CURRENT_REPL_PROTO_VER: u32 = 1;` to
   `crates/shamir-query-types/src/wire/repl.rs` (co-located with
   `ReplRequest`/`ReplResponse`, doc comment explaining it's the highest
   protocol version this build speaks/accepts).
2. `crates/shamir-server/src/replication/source.rs:74` — replace the
   magic-literal `proto_ver: 1` with `proto_ver: CURRENT_REPL_PROTO_VER`
   (import it from `shamir_query_types::wire::repl`).
3. `crates/shamir-server/src/db_handler/repl_handler.rs`'s `handle_repl`,
   `ReplRequest::Hello { proto_ver, node_id: _ } =>` arm: if
   `proto_ver > CURRENT_REPL_PROTO_VER`, return
   `ReplResponse::Error { leader_epoch: self.leader_epoch, code: "proto_ver_unsupported".into(), message: format!("follower proto_ver {proto_ver} exceeds this leader's supported version {CURRENT_REPL_PROTO_VER}") }`
   instead of calling `self.handle_hello(session).await`. Accept any
   `proto_ver <= CURRENT_REPL_PROTO_VER` (forward-compat: an OLDER
   follower speaking a lower version is still accepted — only a NEWER,
   unrecognized version is rejected; this is deliberately an upper-bound
   check, not full bidirectional negotiation). Remove the stale
   `// TODO(R1): negotiate proto_ver` comment, replace with an accurate
   one-line note of what's actually implemented (upper-bound reject) and
   that full negotiation is unnecessary until a second protocol version
   is ever introduced.
4. Test: a `repl_handler` test (find the existing test module for this
   file — check `crates/shamir-server/src/db_handler/tests/` for the
   sibling-test convention already used for this handler) proving a
   `Hello` with `proto_ver: CURRENT_REPL_PROTO_VER + 1` gets
   `ReplResponse::Error { code: "proto_ver_unsupported", .. }`, and a
   `Hello` with `proto_ver: CURRENT_REPL_PROTO_VER` (or lower, e.g. `0`)
   still succeeds normally.

## The task — Part 2: journal gap → resync_required (STOP, not skip)

1. `crates/shamir-server/src/replication/error.rs`: add a new terminal
   variant `JournalGap { gap_at: u64, from_version: u64 }` to `ReplError`
   (mirror `StaleLeaderEpoch`'s doc-comment style: state plainly that this
   is now a TERMINAL condition — the loop must stop, not skip-and-continue
   — and that full snapshot reseed remains R2). Update the existing
   `LeaderGap` variant's doc comment too if you keep it (check whether
   `LeaderGap` is used anywhere today — grep — if it's unused dead code,
   you may repurpose/rename it to `JournalGap` instead of adding a
   parallel variant; if `LeaderGap` IS already used somewhere for a
   different purpose, add `JournalGap` as a distinct new variant and leave
   `LeaderGap` alone).
2. `crates/shamir-server/src/replication/follower_loop.rs`, step 6 (the
   gap-handling block, ~lines 285-299): replace the
   `warn! + gap_skip_from = Some(g) + continue` behavior with
   `return Err(ReplError::JournalGap { gap_at: g, from_version })`
   when `g > from_version` — log at `warn!` or `error!` immediately before
   returning (state clearly in the log message that the loop is stopping,
   not skipping). The events in this same reply (if any events happen to
   accompany a gap response — check whether the leader ever sends events
   alongside `gap_at` in the same `Pull` response, per `repl_handler.rs`'s
   `handle_pull`, and if so make sure they are NOT applied before the
   early return) must NOT be applied. Remove the now-stale module-doc
   bullet describing "gap-reseed... shifting the cursor" and replace with
   an accurate one describing the STOP behavior.
3. `crates/shamir-server/src/replication/supervisor.rs`'s
   `start_subscription`, the `tokio::spawn` block (~lines 285-289): on
   `Err(ReplError::JournalGap { gap_at, from_version })` specifically
   (distinguish from the generic `Err(e) => warn!(...)` branch that
   handles everything else, including `StaleLeaderEpoch`), call a NEW
   small helper (see item 4) to persist `state = "resync_required"` on
   `sub_name`'s row, then warn-log the outcome. Do NOT change behavior
   for any other `ReplError` variant — this task only handles
   `JournalGap` specially; a generic error stays exactly as today
   (warn-log, loop ends, subscription row untouched, reconcile will
   silently not-restart it because the registry still holds the dead
   entry — a PRE-EXISTING gap in loop-liveness detection, NOT part of
   this task's scope, do not attempt to fix it here).
4. Add a small helper — the natural home is
   `crates/shamir-db/src/shamir_db/execute/admin_replication.rs`
   (or an adjacent file in the same module if that file is getting large
   — your call), reusing the EXACT SAME lookup + `set_via_implicit_tx`
   pattern already used by `SubAction::Pause`/`SubAction::Resume` (read
   the existing code at ~lines 350-402 of that file first). Expose it as
   a `pub` method on `ShamirDb` (or wherever the existing pause/resume
   logic's owning type lives — check whether it's a method on
   `ShamirAdminExecutor` that would need a NEW public entry point
   reachable from `shamir-server`, since `supervisor.rs` only holds
   `Arc<ShamirDb>`, not an executor — verify this and choose whichever
   placement compiles cleanly and doesn't require plumbing new
   cross-crate access). Signature shape:
   `pub async fn mark_subscription_resync_required(&self, name: &str, gap_at: u64, from_version: u64) -> DbResult<()>`
   — sets `state` to the literal string `"resync_required"` on the
   subscription row named `name` (not-found → treat as a no-op, log a
   warning, do not error the caller — the subscription may have been
   dropped concurrently).
5. Tests (MANDATORY — this is the review's P0#4 finding):
   - `follower_loop.rs`'s existing test module (`replication::tests::
     follower_loop_tests` — find it, mirror its harness) — a test proving:
     given a mock `ReplSource` whose `pull` reply carries
     `gap_at: Some(g)` with `g > from_version`, `run_follower_loop` returns
     `Err(ReplError::JournalGap { .. })` and does NOT call
     `apply_replicated` for any event in that same reply (assert on a
     counter/spy, or assert the bookmark is unchanged after the call).
   - `supervisor_tests` (or wherever `supervisor.rs`'s tests live) — a
     test proving: after a follower loop returns `JournalGap`, the
     subscription's row in `system/subscriptions` has `state ==
     "resync_required"`, AND a subsequent `reconcile()` call does NOT
     restart the loop (mirrors how a `"paused"` subscription is already
     tested, if such a test exists — mirror its shape).
   - An admin-level test (wherever `handle_replication_status`/
     `handle_list_subscriptions` are already tested — find it) proving
     `resync_required` shows up verbatim in both responses' `state`
     field, and that the existing `Resume` action successfully flips it
     back to `"active"` (proving the existing recovery path still works
     for this new state value, with no new admin action needed).

## The task — Part 3: honest experimental positioning in docs

1. `docs/guide-docs/guide/08-interconnect.md`, "### Шаг 3: Leader-follower
   replication — реализовано" (~line 157): change the heading to
   "### Шаг 3: Leader-follower replication — реализовано (Experimental)"
   and add a short paragraph immediately after the ASCII diagram stating
   plainly, in the same terse style as the rest of this doc: single shared
   `replicator` credential per server (no per-subscription credential
   store yet), reconcile-driven convergence (not event-driven — a catalog
   change is only picked up on the next `reconcile()`/`notify_changed()`
   call, not continuously watched), no leader election / quorum / automatic
   failover / multi-primary / sharding, and (new, from Part 2 of this
   task) a journal gap now stops the follower and marks the subscription
   `resync_required` rather than silently skipping missing data — recovery
   is a manual operator step (verify/fix, then `Resume`), full automated
   snapshot reseed is roadmap (R2).
2. Grep the rest of `docs/guide-docs/` and the top-level `README.md` for
   any OTHER place replication is described as a finished/production
   feature without the same experimental caveat (the brief's own
   investigation found none in `README.md` directly, but verify
   independently — do not skip this check) and apply the same honesty
   fix if found.
3. `CHANGELOG.md`: one bullet under `## [Unreleased]` — proto_ver
   upper-bound rejection, journal-gap-now-stops-and-flags-resync_required
   behavior change (this is a BEHAVIORAL change: a follower that
   previously silently skipped a gap and kept running now stops — call
   this out explicitly, since an operator relying on the old
   keep-running-anyway behavior will see a new stopped state after
   upgrading).

## Out of scope

- Full proto_ver negotiation (range advertisement, downgrade dance) —
  only the upper-bound reject described in Part 1.
- Automated snapshot-based reseed after a gap — stays R2/roadmap. This
  task only adds the STOP + `resync_required` flag + manual `Resume`
  recovery path.
- A new dedicated "resync" admin action distinct from the existing
  `Resume` — do not add one.
- Leader election, quorum, automatic failover, multi-primary, sharding —
  no code changes toward any of these; Part 3 only documents their
  absence.
- Fixing the pre-existing loop-liveness gap noted in Part 2 item 3 (a
  dead JoinHandle isn't detected/respawned by `reconcile()` today for ANY
  error, not just `JournalGap`) — out of scope, do not touch.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-server --full` green, including every new/
  extended test above.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` clean.
- Report literal command output for all of the above, plus a summary of
  exactly which file ended up owning `mark_subscription_resync_required`
  and why (per item 4's "verify this and choose whichever placement
  compiles cleanly" instruction).
