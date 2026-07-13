# Checkpoint — 2026-07-10 [audit-remediation-520-inflight]

## Session summary

Continuation of the long-running audit-remediation campaign against
`docs/audits/2026-07-06-*.md`. This is a very long session; the
narrative below covers from where the previous checkpoint
(`2026-07-10-audit-remediation-503-regrouped.md`) left off.

**Methodology changes made this session (both explicitly requested and
confirmed by the user):**
1. **Lighter per-task gate.** Per-task verification is `cargo check
   --workspace --all-targets` + a SCOPED `./scripts/test.sh -p <touched
   crate(s)>` only, with immediate commit — NOT the full
   build+fmt+clippy+test gate used for #500/#501/#502. A single
   **FINAL-GATE** task (#529) at the very end of the whole remaining
   series runs the full `cargo fmt --all -- --check` +
   `cargo clippy --workspace --all-targets -- -D warnings` +
   `./scripts/test.sh --full`, fixing everything found in one pass.
2. **Task regrouping**, written up in
   `docs/roadmap/2026-07-10-audit-remediation-regroup.md` (committed):
   related pending tasks were merged into 6 grouped tasks G1-G6
   (#525-#528, #530-#531), each covering what were originally 2-3
   separate review-follow-up tasks. All absorbed originals were marked
   `deleted` (confirmed explicitly to the user when asked). The user
   also asked to look for MORE grouping opportunities a second time,
   which produced G5 (#530) and G6 (#531) plus dropping standalone #507
   (folded into FINAL-GATE's own scope) — 16 pending tasks → 13.

**Tasks completed and committed this session (in order, each via the
established brief→`@oh`→independent-verification→[`@fl`-review-when-
warranted]→commit pipeline):**
- **#503** (commit `83e1def9`) — RecordKey alias cutover `Bytes`→`KeyBytes`
  (structural step 2), 99 files, mechanical. `@fl`-reviewed, SHIP IT WITH
  NITS (nits noted, not blocking).
- **G1/#525** (commit `9e976f7b`) — alloc-free hot-path key constructors
  (structural step 3+4). `@oh` honestly reported the bench numbers as
  noise (no fabricated win claimed) — good discipline, accepted as-is.
  One item explicitly flagged-not-silently-fixed (re-keying MvccStore's
  Bytes-keyed coordination maps would break public API) → filed as
  follow-up **#532**.
- **G2/#526** (commit `6bbe1a0e`) — keyset-seek short-page-on-stale-index
  fix. Deliberately scoped OUT the record-id tie-breaker half of the
  original finding (would require a client-visible wire-protocol change
  to `Pagination::After`'s `key` field) → filed as its own design task
  **#533**.
- **G3/#527** (commit `cf3d507c`) — subscription-cap slot leak +
  SSRF DNS-rebind/octal gaps. **`@fl`'s FIRST review pass found a real
  bug the orchestrator introduced/inherited**: a spawn-vs-insert race
  where a fast-self-exiting bridge task's cleanup guard could fire
  BEFORE the handler's `registry.insert()` ran, reintroducing the exact
  leak the fix was meant to close. Fixed directly by the orchestrator
  (split `insert()` into `reserve_pending`+`attach_handle`, closing the
  race by construction). **Second `@fl` pass: SHIP IT.**
- **G4/#528** (commit `2d9ea01b`) — three test flakes/failures.
  **Discovered a genuine PRODUCTION bug** while fixing
  `trusted_pure_scalar_backs_functional_index` (a STABLE, not flaky,
  failure): `TableManager::create_index_v2`'s index2 pipeline
  (functional/fts/vector indexes) never backfilled pre-existing table
  rows — only rows inserted AFTER index creation got indexed. Fixed with
  `backfill_index2_backend`. `@fl` review found the fix's own doc
  comment overclaimed safety (there IS a residual lost-write race, not
  closed by this fix, though it's a strict improvement over the
  prior always-broken state) — reworded the comment honestly, filed
  **#534** to properly close the race (register-first ordering isn't
  simply portable since FTS's `BumpFtsStats` ops aren't idempotent).
  Also fixed two independent flaky-test root causes (unseedable HNSW
  RNG for the ANN oversample test; wall-clock-timing-dependent argon2id
  concurrency-cap test, redesigned with a `std::sync::Barrier`).
- **G5/#530** (commit `81c0c853`) — MemBuffer merge-overlay scans (all 4
  scan methods, not just the order-insensitive 2 the brief permitted
  scoping down to — investigation showed all 4 backends already yield
  sorted output, so the full fix generalized cleanly) + SQ8 fused
  rescore (allocation-free, ×2-3 measured). SIMD weighted-kernel part
  explicitly deferred (no safe reusable template existed). `@fl`-reviewed
  with a hand-traced merge-correctness walkthrough (forward AND reverse)
  — SHIP IT WITH NITS (a misleading test-helper comment fixed; a missing
  ordering-contract doc added to `Store::iter_stream`; a PRE-EXISTING
  `dirty_nonempty` clear-race — confirmed not introduced by this diff —
  filed as **#535**).
- **G6/#531** (commit `aa76c1d3`) — real fault-injection test for
  `WalGroupCommit::append_many`'s all-or-nothing claim (a `#[cfg(test)]`-
  gated one-shot fault knob added to `MemSink`, zero production-path
  change) + strengthened `reactivated_segment_sheds_stale_sidecar`'s
  sibling test to exercise `rotate_after_poison` directly (was only
  covering `SegmentSet::open`'s shed before).
- **#506** (commit `5a917ad8`) — investigated directly (no agent),
  concluded the posting-key (41B) heap-fallback in `KeyBytes` is
  write-path-only (never on the hot read/compare path since task #499),
  not worth the `unsafe` union work to raise `INLINE_CAP`. Deferred, no
  code change, documented in a design doc.
- **#512** (commit `684a41e4`) — investigated directly (no agent),
  concluded raw TLS-exporter-equality channel binding is
  cryptographically impossible per RFC 9266 (exporter values are
  per-connection-unique BY DESIGN, even across legitimate resumption —
  can't distinguish legit reconnect from stolen-ticket replay). The real
  fix (mutual-TLS-anchored ticket binding) is a substantial new
  capability this codebase doesn't have, out of scope. Documented that
  the EXISTING `ConsumedCounterStore` monotonic ticket-rotation counter
  already provides a reasonable mitigation (stolen-ticket use causes the
  legitimate party's next resume to loudly fail, not silently coexist).
  Updated the inline code comment to point at the new design doc.

**Currently IN FLIGHT: #520** (Rust client has no connect/request
timeout, parity with the TS client's `requestTimeoutMs`/
`connectTimeoutMs` from task #497). Wrote brief 52
(`docs/prompts/audit/52-client-520-rust-request-timeout.md`, committed
`4bd4b2a9`), launched `@oh` as a background agent — **this had NOT yet
returned when the session was interrupted for this checkpoint.**
`git status` shows the following files already modified (mid-agent-run,
NOT yet independently verified or committed by the orchestrator):
`crates/shamir-client/src/client.rs`, `crates/shamir-client/src/error.rs`,
`crates/shamir-client/src/tests/{ambient_sync_tests,interner_cache_tests,
v2_passthrough_tests}.rs`, `crates/shamir-client/tests/smoke.rs`,
`crates/shamir-client-node/src/lib.rs`,
`crates/shamir-server/{benches/wire_latencies.rs,src/access_tree.rs,
src/replication/prod_factory.rs,tests/{access_tree_e2e,duplex_e2e,
quickstart_e2e}.rs}`. This matches the brief's expected mechanical
blast radius (13 `ConnectOptions` construction call sites needing the
two new timeout fields added).

The `/goal` Stop-hook ("решить все задачи") from earlier in this
campaign is presumed still active (not re-confirmed this window). The
`/babysit` cron has been ticking throughout, correctly reporting "still
running #<task>" at each tick since no new commit had landed between
ticks for whichever task was in flight at the time.

## Active goal

`/goal`: **"решить все задачи"** (solve all tasks) — presumed still
active; not re-verified via CronList this window. If stale:
```
/goal решить все задачи
```

## TaskList

### in_progress
- #520 CLIENT: Rust client roundtrip has no request timeout — `@oh` background agent ran, NOT yet returned/verified/committed at checkpoint time

### pending
- #519 CLIENT: node-binding typed error .code/.retryable (needs napi-rs 3.x version bump → needs explicit user permission, not yet asked; does NOT block FINAL-GATE)
- #523 PERF: add fjall-backend bench (point get/set/scan, real tempdir) — prerequisite for #524
- #524 PERF: prototype sharded worker-loop batching for fjall point-ops  (blockedBy: #523)
- #529 FINAL-GATE: full fmt+clippy+test --full + fix everything found  (blockedBy: #523, #524, #532, #533, #534, #535 — NOT #519)
- #532 PERF: re-key MvccStore's Bytes-keyed coordination maps to RecordKey (found during G1/#525 review)
- #533 DESIGN: record-id tie-breaker for keyset pagination on ORDER BY value ties (found during G2/#526 scoping, needs a client-wire-protocol decision)
- #534 SECURITY/CORRECTNESS: close index2 CREATE INDEX lost-write race + crash-orphan-postings window (found during G4/#528 review)
- #535 FIX: MemBuffer dirty_nonempty flag has a clear-race that can mask an ACKed write (found during G5/#530 review, pre-existing bug)

### recently completed (this window, in order)
- #512 channel-binding design decision (commit 684a41e4)
- #506 KeyBytes INLINE_CAP deferred (commit 5a917ad8)
- G6/#531 WAL test-hardening (commit aa76c1d3)
- G5/#530 MemBuffer merge-overlay + SQ8 fused rescore (commit 81c0c853)
- G4/#528 index2 backfill bug fix + 2 flake fixes (commit 2d9ea01b)
- G3/#527 subscription-leak race fix + SSRF gaps (commit cf3d507c)
- G2/#526 keyset short-page fix (commit 6bbe1a0e)
- G1/#525 alloc-free key constructors (commit 9e976f7b)
- #503 RecordKey alias cutover (commit 83e1def9)

## Decisions

- **Per-task gate lightened** (explicit user request, weighed and chosen
  over both "full gate every time" and "check-only, no tests at all"):
  `cargo check` + scoped test per task, full fmt/clippy/test--full
  deferred to one FINAL-GATE task at the series' end.
- **Task regrouping** — 6 grouped tasks (G1-G6) replacing ~13 originally
  separate review-follow-ups; user explicitly asked twice to find more
  grouping opportunities.
- **G2/#526's tie-breaker scoped out** rather than implemented blindly:
  extending `Pagination::After`'s seek key is a client-wire-protocol
  change (the TS client's `key: WireValue[]` is an already-shipped
  contract), not a mechanical backend fix — filed as design task #533.
- **#512's channel-binding "fix" concluded infeasible as originally
  conceived** — RFC 9266 makes exporter-value equality unable to
  distinguish legitimate resume from stolen-ticket replay; existing
  ticket-rotation counter is the correct mitigation class already in
  place. No code change; documented as a considered decision, not a punt.
- **#506 deferred** — posting-key heap fallback is write-path-only
  (never hot-path), not worth `unsafe` union work without a measured
  signal justifying it.
- **When `@fl` finds a genuine bug, the orchestrator fixes it directly**
  and sends the fix through a SECOND review pass before committing —
  happened again this window for G3/#527's spawn-vs-insert race.
- **G4/#528's index2 backfill bug** is the second time this campaign a
  TEST-FIXING task uncovered a genuine, previously-unknown PRODUCTION
  correctness bug (not just a flaky/broken test) — matches the
  established pattern of trusting fresh investigation over assumed
  premises.

## Open questions

- Task #520: did `@oh`'s background run finish? Check via the
  completion notification (do NOT poll `git status` repeatedly or tail
  the agent's raw JSONL transcript) — resume verification (cargo check
  + scoped `./scripts/test.sh -p shamir-client -p shamir-server`, spot-
  check the diff for correctness, especially the 13 mechanically-updated
  `ConnectOptions` call sites and whether `shamir-server`'s own
  replication client got a sensible non-None default) once it returns.
- Task #519 (napi-rs 3.x bump): still blocked on an explicit user
  decision not yet asked this session.
- Whether the `/goal` Stop-hook is still armed — not reconfirmed this
  window.

## Repo state

```
 M CLAUDE.md
 M bench-iters.txt
 M crates/shamir-client-node/src/lib.rs
 M crates/shamir-client/src/client.rs
 M crates/shamir-client/src/error.rs
 M crates/shamir-client/src/tests/ambient_sync_tests.rs
 M crates/shamir-client/src/tests/interner_cache_tests.rs
 M crates/shamir-client/src/tests/v2_passthrough_tests.rs
 M crates/shamir-client/tests/smoke.rs
 M crates/shamir-server/benches/wire_latencies.rs
 M crates/shamir-server/src/access_tree.rs
 M crates/shamir-server/src/replication/prod_factory.rs
 M crates/shamir-server/tests/access_tree_e2e.rs
 M crates/shamir-server/tests/duplex_e2e.rs
 M crates/shamir-server/tests/quickstart_e2e.rs
 (^ all from @oh's in-flight #520 work — NOT yet independently verified
    or committed by the orchestrator)
 ?? (many stray *.log files accumulated across this entire campaign's
    gate runs in the repo root — pre-existing clutter, not part of any
    pending commit)
```

```
4bd4b2a9 docs(prompts): brief for #520 Rust client connect/request timeout
684a41e4 docs(security): close #512 -- resumption-ticket channel-binding fix confirmed infeasible
5a917ad8 docs(design): defer task #506 (KeyBytes INLINE_CAP / posting-key tier)
aa76c1d3 test(wal): real fault-injection + poison-rotation sidecar hardening (G6/#531)
da0d3f66 docs(prompts): brief for G6/#531 WAL fault-injection + sidecar test hardening
81c0c853 perf(storage,index): MemBuffer merge-overlay scans + SQ8 fused rescore (G5/#530)
2d9ea01b fix(engine,funclib): index2 CREATE INDEX backfill + two deterministic-flake test fixes (G4/#528)
```
