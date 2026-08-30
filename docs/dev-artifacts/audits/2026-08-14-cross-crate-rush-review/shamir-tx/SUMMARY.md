# shamir-tx — Consolidated 7-lens review (synthesis of the 2026-08-14 cross-crate review)

Crate: `crates/shamir-tx/` (MVCC transactional store — cells/version registry, history log +
drainer, ts-index, repo tx gate with SSI predicate validation, versioned overlay, changefeed,
staging + interner layers).

Review basis: the seven 2026-08-14 lens files under this directory — `correctness-tdd.md`,
`concurrency-lockfree.md`, `security-crypto.md`, `performance-hotpath.md`, `api-wire-protocol.md`,
`error-handling-lifecycle.md`, `style-claude-md.md` — synthesized read-only into this one file.
Structure/tone/rigor calibrated against the two completed exemplars,
`../shamir-client-node/SUMMARY.md` and `../shamir-transport-ipc/SUMMARY.md`; the workspace
`../SUMMARY.md` per-crate row (65 pre-dedup lens-tagged findings, "needs focused remediation")
was consulted for context only. During synthesis, five high-severity file:line references were
re-verified against source (mod.rs:664-679, repo_tx_gate.rs:586-589, mvcc_history.rs:450/:503,
changefeed.rs:397-423, mod.rs:766-799 — all confirmed); no build/test/lint was run and no source
file was modified. No new defects were found during spot-checking.

## Executive summary

Verdict: **needs focused remediation** (workspace scorecard #9 of 23; 0 critical / 7 high before
dedup). The crate is structurally strong — `scc` + `THasher` on every concurrent map, sanctioned
lock usage with inline contention models, exemplary A10/D2 proof-by-test discipline — but it
carries two MVCC version-regression defects (the ack-path publisher `finalize_reservation` sets
`cell.version` unconditionally; the public `publish_committed` plain-`store()` can regress the
reader-visible floor) plus one no-rollback durability illusion (a failed `history.transact` leaves
the cell bumped, so the record reads as deleted and a failed delete resurrects after restart).
Third, the changefeed journal has no format envelope, and every anomaly class — corrupt entry,
persist failure, overflow drop after a restart — surfaces as a silent hole with `gap_at: None`, so
replication/subscription consumers silently diverge. Fix those three clusters (plus the per-write
`vacuum_key` I/O inflation, the top performance-high) before anything else ships from this crate.

---

## 1. correctness-tdd

Lens verdict: strong TDD discipline overall (A10 TOCTOU keystone tests, D2 ordering
proof-by-test, proptest oracles), but the A2 max-monotonic fix was applied to
`publish_cell`/`seed_version` only — not to `finalize_reservation`, the publisher the live
ack-path actually uses. Positive: the A10 barrier/anchor-deferral suite (`a10_toctou_tests.rs`)
and `overlay_ordering_tests.rs` are exemplary; `version_codec` proptests, `ts_index_tests` scan
oracle, `get_at_many`/`get_current_many` per-key oracles, and `repo_tx_gate_tests` commit-window
bounds are all genuine regression guards.

### 1.1 — high — `finalize_reservation` is not max-monotonic: the ack-path publish can regress a cell below a newer committed version
- File:line: `crates/shamir-tx/src/mvcc_store/mod.rs:664-679` (unconditional
  `cell.version = version` at :668 — verified during synthesis); live call path
  `mvcc_history.rs:450-511` (`apply_committed_visible` → `finalize_reservation` at :503).
- Issue: `publish_cell` (:545-560) and `seed_version` (`mvcc_history.rs:262-277`) both carry the
  A2 strictly-greater-than guard precisely because a slower writer/drain can apply an OLDER
  version after a NEWER one landed. `finalize_reservation` sets `cell.version` unconditionally yet
  is the publisher on EVERY commit's ack path (`apply_committed_visible`, engine
  `commit_phases.rs:603`) — including plain `Snapshot` txs, which per
  `shamir-engine/src/tx/commit.rs:903-911` run Phase 5a WITHOUT `commit_lock`. Two concurrent
  commits touching the same key can therefore finalize out of order.
- Failure scenario: tx A allocates v100, tx B v101 for key K; B's Phase 5a lands first
  (`cell.version = 101`), then A's late Phase 5a runs `finalize_reservation(K, 100)` → cell
  regresses to 100. `get_current`/`get_current_bytes` takes the direct path (cur_v=100 ≤
  floor=101) and returns A's value though B committed later — a stale "current" read — and
  `version_of`/`try_reserve` stale-write checks are fed a regressed version (masked SSI
  write-write conflicts): exactly the anomaly class A2 named. The window self-heals only when the
  drainer's guarded `publish_cell` re-publishes 101.
- TDD gap: the A2 suite (`tests/mvcc_store_tests/publish_monotonic_tests.rs`) covers
  `publish_cell`/`seed_version` only; every `apply_committed_visible`/`finalize_reservation` test
  applies versions ascending on a single task. The comment at `publish_monotonic_tests.rs:46`
  asserts Phase 5a goes "finalize_reservation → publish_cell" — it does not; finalize is called
  directly, which is why the guard was missed.
- Suggested fix: in the Occupied branch write `cell.version` only when `version > cell.version`
  (`reserved_by = 0` clearing stays unconditional); add an out-of-order ack test (apply v11 then
  v10; assert cell stays 11) plus a two-task `apply_committed_visible` race test on
  `multi_thread`.

### 1.2 — medium — `prune_version_cache` can evict a cell holding a live SSI reservation, breaking "exactly one committer wins"
- File:line: `mvcc_store/mvcc_gc.rs:531-534`; reservation insert at `mod.rs:641-649`
  (`try_reserve` Vacant → `RecordCell { version: 0, reserved_by: txn_id }`).
- Issue: the retain predicate `c.version >= min_alive` ignores `c.reserved_by`. A freshly-claimed
  key's cell has `version: 0`, and `min_alive()` (no snapshots ⇒ `last_committed()`) is > 0 on any
  live repo, so a `gc_below`/`gc()` tick running between a Serializable committer's `try_reserve`
  (pre_commit) and its `finalize_reservation` (Phase 5a — a window spanning the WAL fsync) erases
  the claim.
- Failure scenario: tx A claims fresh key K; GC evicts the `{version: 0, reserved_by: A}` cell;
  tx B's `try_reserve(K)` sees Vacant and WINS; both committers proceed to WAL/publish →
  double-commit / lost update under Serializable. `concurrent_try_reserve_exactly_one_wins` proves
  atomicity but nothing re-proves it across a concurrent prune.
- Suggested fix: `retain_sync(|_k, c| c.version >= min_alive || c.reserved_by != 0)` + a test that
  claims a fresh key, runs `gc_below(last_committed)`, and asserts a second `try_reserve` loses.

### 1.3 — medium — `ts_index_rebuild` marks the index ready even when the history scan errored
- File:line: `mvcc_store/mod.rs:398-429` (`Err(_) => continue` at :406-409; unconditional
  `ts_index_ready.store(true)` at :428). *(also flagged by error-handling-lifecycle #6, low)*
- Issue: a transient storage error during the one-time rebuild is swallowed per batch, but the
  ready flag is still set — `version_at_or_before_ts` (`mvcc_history.rs:87-97`) will never retry,
  so every subsequent as-of-by-ts query silently resolves against a permanently incomplete index
  (returns an older version than the true answer). With a durable backend, an I/O hiccup at first
  query poisons the index for the process lifetime; the documented fallback only fires when the
  index is *empty*, not *partial*.
- Suggested fix: track whether any batch errored; store `ready = true` only on a clean pass (leave
  false to retry on next query) and `log::warn!` once. Test with the existing `PausableStore`/
  fault-injection pattern (`tests/mvcc_store_tests/test_stores.rs`).

### 1.4 — low — `write_committed_batch_to_history` trusts caller-ascending `pass` for the floor advance
- File:line: `mvcc_store/mvcc_history.rs:295-299` (contract: "MUST be in ascending commit_version
  order") and :393-396 (`pass.last()` assumed max for `publish_committed_max`).
- Issue: the ordering contract is unenforced. With an unsorted `pass`, `pass.last()` may be below
  the true max; the CAS-max prevents regression but under-advances the floor for one drain round
  (brief visibility lag, no corruption). No test feeds an unsorted pass.
- Suggested fix: `debug_assert!(pass.windows(2).all(|w| w[0].0 <= w[1].0))` and/or fold to the
  true max (`pass.iter().map(|(v,_)| *v).max()`) — O(n) on an off-hot-path batch builder.

### 1.5 — low — `min_alive()` check-then-scan-then-read is not atomic: narrow TOCTOU against a just-registering snapshot
- File:line: `repo_tx_gate.rs:651-670`; consumers `mvcc_gc.rs:156` (scan-path `min_alive`) and
  :193-201 (anchor computed from it). (Distinct from the *cost* defect at the same site — 4.3.)
- Issue: the A10 in-flight barrier is checked once at entry. A reader whose whole
  `register_snapshot` (barrier inc → floor read → register → barrier dec) lands between the
  `active_snapshots_opening` check, the `iter_sync` scan, and the `last_committed()` fallback is
  missed by the scan while `last_committed` may already have advanced past its floor; `min_alive()`
  then returns a value ABOVE the live snapshot's true floor and the scan-path vacuum can compute
  its single anchor against it — in principle reclaiming the version the just-registered snapshot
  resolves (reader sees the key vanish as `None`). The window is a few instructions wide —
  theoretical, but exactly the A10 class the barrier was built to close.
- Suggested fix: double-check — if `snapshots_opening()` is true after the scan, return 0
  (conservative), mirroring the barrier semantics the fast path already enforces.

### 1.6 — nit — Stale doc: `predicate_conflicts` claims non-tx writes never reach `last_committed`
- File:line: `repo_tx_gate.rs:793-798`.
- Issue: the comment justifies the `last_committed` upper bound with "non-tx writes … do NOT call
  `publish_committed`", but since the VersionGuard cutover `set_versioned`/`delete_versioned`
  advance the floor via `guard.commit()` → watermark fetch_max. Behaviour remains
  conservative-correct; only the rationale is wrong — a future maintainer could "fix" the bound
  based on it.
- Suggested fix: update the comment to the actual post-cutover floor semantics.

### 1.7 — low — *(primary: 2.1)* `publish_committed` plain-store regression
- Flagged here as a public-API footgun (no production callers); full write-up and the
  high/latent rating live under concurrency (2.1), which is the primary lens for this defect.

### 1.8 — low — *(primary: 2.2)* `release_locks` never removes empty `KeyLock` entries
- Correctness lens noted the unbounded locks-map growth low ("documented intentional; memory
  growth, not correctness"); the full write-up (waiter-safety analysis included) lives under 2.2.

### 1.9 — nit — *(primary: 6.4)* `Result<_, String>` on `apply_id_remap` (`tx_context.rs:913-916`)
- One site of the crate-wide thiserror/String-error cluster; see 6.4.

---

## 2. concurrency-lockfree

Lens verdict: strong pillar adherence — every concurrent map is `scc::*` keyed with `THasher`, no
`parking_lot`, no `std::sync::Mutex` on any live path (the single instance is the sanctioned
dead-scaffolding exception), every `tokio::sync::Mutex` is the sanctioned guard-across-`.await`
case with an inline contention model, and the #589 sync-accessor discipline is uniform with
dedicated H1/H2 regression tests. Positive: the sync/async accessor discipline, `PredicateSet`/
`VersionedOverlay` O(1) len mirrors, `drain_exclusive`'s try-lock back-off (#1032), and the
sanctioned O(N) ack annotations on all four `scc::*::len()` sites in `mvcc_store/mod.rs`.

### 2.1 — high (latent) — `publish_committed` plain `store()` can regress `last_committed_version`; its safety contract is unsound
- File:line: `repo_tx_gate.rs:576-589` (verified during synthesis: plain `store(version, Release)`
  at :586-589; doc claims "monotonic ordering is guaranteed by the lock").
- Issue: `commit_mutex` only serialises tx commits **against each other** — the crate's own
  non-tx write paths (`set_versioned`/`set_versioned_many`/`delete_versioned` via
  `VersionGuard` → `fetch_max`, and `write_committed_*_to_history` → `publish_committed_max`)
  publish the same atomic WITHOUT holding `commit_lock`, and allocate versions from the same
  counter. `commit_lock` therefore does not establish the global monotonicity the doc promises.
  Everything else in the gate enforces monotonicity; this one public method is the only site that
  can move the reader-visible floor backwards. *(also flagged by correctness-tdd #4, low)*
- Failure scenario: tx T takes `commit_lock`, assigns v=95; concurrently a non-tx writer assigns
  v=96, completes, and fetch_maxes the floor to 96 (write acked). T then calls
  `publish_committed(95)` → floor drops 96→95. Readers hit the R3 cap in `get_current_bytes`
  (`v > floor` → range-scan at floor) and resolve the **pre-write** value: the acked non-tx write
  becomes invisible; a fresh snapshot opens at 95 and misses it (read-your-write break across the
  non-tx/tx boundary).
- Suggested fix: make `publish_committed` body identical to `publish_committed_max` (or delete it
  and migrate the ~20 test callers). At minimum correct the doc: monotonicity holds only if
  *every* publisher takes `commit_lock`, which is already false in-crate.

### 2.2 — medium — `MvccStore::locks` registry never evicts empty entries — unbounded growth under pessimistic workloads
- File:line: `mvcc_store/mvcc_locks.rs:219-239` (insertion at :75-82; struct at
  `key_lock.rs:78-90`); field `mvcc_store/mod.rs:139-143`. *(also flagged by correctness-tdd #7,
  low, and performance-hotpath #6, medium)*
- Issue: `lock_key` entry-inserts an `Arc<KeyLock>` (tokio `Mutex<KeyLockState>` + `Notify` +
  `Vec<Holder>`) per key on first pessimistic acquire; `release_locks` removes holders but
  explicitly keeps the empty entry ("cheap; GC is intentionally not done here"). Grep confirms no
  removal/retain path exists. For Level-3 workloads locking ever-fresh keys (the common shape —
  lock by monotonic `RecordId`), the map grows without bound for the life of the table — a
  pillar-3 violation; the "cheap" comment addresses per-entry size, not cardinality.
  `locks_len()` exists only to assert emptiness when Level-3 is unused.
- Failure scenario: a long-running server with `IsolationLevel::Pessimistic` txs touching N
  distinct keys accumulates N dead `KeyLock`s forever; memory grows linearly with total distinct
  locked keys, with progressively worse probe locality.
- Suggested fix: naive removal is unsound (a waiter parked on `notify` holds an `Arc<KeyLock>`
  clone; removing the map entry lets a new requester insert a fresh `KeyLock` and both txs "hold"
  the key via two independent mutexes). Add a `waiters: AtomicUsize` per `KeyLock` incremented
  around the parked `select!`, and in `release_locks` remove the entry only when
  `holders.is_empty() && waiters == 0`; alternatively sweep in the existing GC tick.

### 2.3 — low — O(N) tree traversals use `range(..).count()` — sidesteps the `len()` disallowed-methods gate without the ack attribute
- File:line: `repo_tx_gate.rs:898-904` (`commit_log_len`), `:886-896` (`prune_commit_log_below`).
- Issue: pillar 3 bans `scc::*::len()` via `disallowed-methods` and requires legitimate O(N) uses
  to carry the `#[allow] // O(N) ack` attribute. Both functions compute cardinality with
  `range(..).count()` — semantically identical full traversals the lint cannot see — and carry
  only prose docs. `prune_commit_log_below` additionally walks the pruned range **twice** (count,
  then `remove_range_sync`) on the engine's GC tick. Off hot path / telemetry-facing, hence low.
- Suggested fix: add the sanctioned `O(N) ack` attribute (matching `pending_ts_len`/`ts_index_len`
  /`locks_len`), and drop the pre-count pass if `remove_range_sync`'s return can serve the count.

### 2.4 — low — A10 in-flight barrier can starve GC (commit-write log / version cache growth) under sustained snapshot-open churn
- File:line: `repo_tx_gate.rs:110-127` (barrier field), `:636-657` (`min_alive()` returns 0 while
  openers in flight).
- Issue: while `active_snapshots_opening > 0`, `min_alive()` returns 0, making `gc_below`,
  `prune_version_cache`, `vacuum_key`'s fast path and `prune_commit_log_below` all no-ops — the
  deliberate, *safe* (over-retention) A10 choice, but there is no bound on how long the counter
  can stay non-zero: on a busy server a snapshot is always mid-registration at tick time, so
  `commit_write_log` (one entry per Serializable/footprint commit) and the `cells` cache grow
  unboundedly between quiet windows. Each barrier window is short, so sustained starvation needs
  extreme churn — residual risk, not a live defect.
- Failure scenario: continuous high-rate snapshot opens (one short tx per request) with
  Serializable commits: GC ticks repeatedly observe `min_alive() == 0`, `commit_write_log` never
  prunes, memory climbs with commit count until an occasional quiet tick lets GC catch up.
- Suggested fix: pinned-floor-aware barrier — each opener publishes the `last_committed` value
  captured *before* incrementing the barrier into an AtomicU64 min-CAS; `min_alive()` returns
  `min(registered_min, in_flight_captured_min)` instead of a blanket 0.

### 2.5 — nit — Stale "cannot early-return" comment on `validate_read_set`'s `iter_sync`
- File:line: `tx_context.rs:770-776`.
- Issue: the comment asserts scc's synchronous visitor "cannot early-return", yet the closure
  immediately below returns `false` on the first conflict — precisely scc's early-stop protocol
  (and `append_ri_barrier_deps` at :748-753 relies on returning `true` to continue). The code is
  right; the comment is wrong and will mislead future edits around this lock-free iteration.
- Suggested fix: reword to state that returning `false` stops the iteration (first conflict wins).

### 2.6 — medium — *(primary: 4.4)* `record_conflicts` linear interval scan under `commit_lock`
- Concurrency lens flagged the commit-lock hold-time/critical-section angle (its O(W×P×K) analysis
  and sort-without-binary-search observation); the full write-up lives under 4.4.

### 2.7 — medium — *(primary: 4.1)* `vacuum_key` scan path duplicates `lookup_ts` per reclaimed version
- Concurrency lens flagged the duplicated awaited history reads as a pillar-3 "repeated lookups"
  violation; the full write-up (fast + scan path, batching fix) lives under 4.1.

### 2.8 — low — *(primary: 4.3)* `min_alive()` full-map traversal per call
- Concurrency lens flagged the O(S) traversal + bucket-lock contention vs the hot
  `bump_refcount`/`SnapshotGuard::drop` path; the full write-up lives under 4.3.

### 2.9 — nit — *(primary: 4.7)* `history_of` resolves commit timestamps with N sequential awaited gets
- Concurrency lens flagged the batching miss on the admin/diagnostic path; see 4.7.

---

## 3. security-crypto

Lens verdict: no authentication/HMAC/TLS/crypto code, zero `unsafe` blocks, no secret material,
no string-assembled query surface (binary keys — no injection grammar); the crate sits behind the
server's auth boundary and its stored inputs are checksummed at the storage layer. The theme
reduces to panic-on-input robustness at `pub` APIs, untrusted/corrupt durable-input handling, and
the key-encoding boundary. Positive: test coverage for the codec boundary is strong (round-trip +
separator-rejection + proptest); the two `.lock().unwrap()` sites (`repo_tx_gate.rs:753/761`) sit
on `pending_commits`, explicitly sanctioned dead scaffolding — not re-litigated.

### 3.1 — medium — `LayeredInterner::touch_sync` panics on the same failure its sibling merge path treats as a recoverable error
- File:line: `layered_interner.rs:82-86` (Direct branch) vs `:256-263`
  (`commit_interner_overlay`). *(also flagged by error-handling-lifecycle #7, low)*
- Issue: both call sites invoke `Interner::touch_ind` (returns `Result<TouchInd, &'static str>`,
  shamir-types `interner.rs:138`) on field names — user-supplied data reaching this crate from the
  engine's write path. The overlay path maps `Err` to `DbError::Codec` and propagates with `?`;
  `touch_sync`'s Direct (non-tx write) branch does
  `.expect("Interner::touch_ind is infallible for valid input")` — a message that itself concedes
  invalid input exists. This is not an invariant violation; it is an input-conditioned failure the
  sibling path already models as `Err`, converted on the non-tx path into a process-wide panic.
- Failure scenario: a client write whose field name trips `touch_ind`'s failure mode (whatever
  "invalid input" covers — e.g. pathological length) crashes the whole server process on the
  non-tx path, where the tx path returns a clean `Codec` error — remote DoS by input, contingent
  on `Interner`'s out-of-crate failure modes.
- Suggested fix: return `Result<u64, DbError>` from `touch_sync`, mirroring
  `commit_interner_overlay`. If the interner truly cannot fail on any reachable input, delete the
  expect's "for valid input" hedge and pin the Ok-for-all-inputs property with a test.

### 3.2 — low — `version_codec`'s separator invariant is doc-only, its probability claim is wrong, its prop tests dodge the case — and `vacuum_key`'s scan path guards prefix-matched entries with the wrong `cur_v`
- File:line: `version_codec.rs:10-30` (invariant + "negligible" claim; stale "tests below" ref);
  `mvcc_store/mvcc_gc.rs:162-199` (prefix scan), `:185` + `:212-214` (single-`cur_v` SACRED
  check). *(also flagged by api-wire-protocol #8, low, and the version_codec sub-item of
  style-claude-md #4)*
- Issue: (a) the invariant ("original key must not contain `0xFF` + 8 trailing bytes") is enforced
  nowhere — `encode_version_key` is `pub` and accepts any `&[u8]`. (b) The doc's probability claim
  is wrong: P(byte `len-9` is `0xFF`) is **1/256**, not "negligible"; the doc also
  self-contradicts ("cannot appear" vs "negligible"), and the prop tests exclude `0xFF`
  "so that the invariant … is upheld by construction" (`version_codec_tests.rs:53-67, :110-117`) —
  the suite never exercises the fragile case it documents, and the "verified by … tests below"
  reference is stale. (c) `vacuum_key`'s scan path prefix-scans `key || 0xFF` and applies ONE
  `cur_v = current_version(key)` SACRED check to every collected entry: if key `A` and
  `A||0xFF||W` coexist, the longer key's entries fall inside the shorter key's scan and are
  reclaimed against the *shorter* key's guard — `gc_below` (`mvcc_gc.rs:329`) and `purge_below_ts`
  (`:426`) correctly re-derive `cur_v` per decoded key; `vacuum_key` is the outlier.
- Failure scenario: a future caller stores variable-length user keys where `A` and `A||0xFF||W`
  coexist; a retention-triggered `vacuum_key(A, …)` deletes `A||0xFF||W`'s live current version —
  silent data loss attributed to GC. Not reachable with today's fixed-width keyspaces;
  defense-in-depth against the crate's own documented invariant being violated downstream.
- Suggested fix: (1) `debug_assert!` in `encode_version_key` that `key[len-9] != VERSION_SEP`
  (len ≥ 9); (2) correct the probability arithmetic and state the real invariant (fixed 16-byte
  RecordIds; variable-length keys must be engine-typed encodings that never end in `0xFF`+8); fix
  the stale "tests below" reference; (3) group scan-path entries by decoded original key and check
  each against `current_version(orig)` exactly as `gc_below` does.

### 3.3 — low — `StagedRow::as_inner` panics on malformed staged bytes; the invariant is a doc comment on a `pub` API
- File:line: `staging_store.rs:31-49` (`.expect` at :46-49), `:135-137` (`pub fn set` accepting
  arbitrary `Bytes`). *(also flagged by error-handling-lifecycle #8, low)*
- Issue: `StagedRow` documents "always holds already-encoded msgpack `Bytes`" but nothing
  enforces it, and `as_inner()` does `InnerValue::from_bytes(...).expect(...)`. The crate's own
  remap path (`id_remap.rs:77-80`) handles the *identical* decode failure by returning `Err` —
  the same malformed payload either panics or errors depending on which API touches it first.
  The panic detonates at a later read-your-own-write (cold path), far from the buggy staging call.
- Failure scenario: an engine caller stages bytes not produced by `query_value_to_storage_bytes`
  (a future write path, a test fixture, a WAL-replay shortcut); every staged read/commit-remap
  touching that row panics the process. Requires a caller bug, not attacker input — hence low.
- Suggested fix: make `as_inner` infallible by construction (store `InnerValue` alongside the
  bytes, or validate at `StagingStore::set`), or return `Result`/`Option` — matching
  `remap_inner_value_bytes`' existing behavior for the same decode.

### 3.4 — low — Changefeed journal keys carry no repo namespace — shared stores silently cross-wire event streams
- File:line: `changefeed.rs:426-430` (`version_key`), `:232-257` (`RepoChangefeed::new`).
- Issue: journal records are keyed by the bare 8-byte BE `commit_version` — no repo id, no table
  token. The "one store per repo" assumption exists only in the wiring; `new` accepts any
  `Arc<dyn ChangelogStore>` and nothing detects or rejects sharing. Two repos' feeds on one store
  overwrite each other's entries (`put` is an upsert) with no error and no gap marker.
- Failure scenario: a deployment/test harness reuses one changelog store for two repos; repo B's
  event at version V overwrites repo A's at V; A's subscribers receive B's records
  (cross-tenant record disclosure if the repos have different readers) and neither feed reports a
  gap.
- Suggested fix: include a per-repo discriminator in the journal key (e.g. hash of the repo name
  as prefix), or at minimum `debug_assert!`/document the exclusive-store contract at `new`.

### 3.5 — nit — Non-keyed `THasher` on engine-supplied keys — the "no untrusted hash inputs" premise is enforced only upstream
- File:line: `tx_context.rs:233` (`read_set`), `:240` (`cas_set`), `mvcc_store/mod.rs:138`
  (`cells`), `:143` (`locks`).
- Issue: pillar 4 trades SipHash DoS protection for Fx speed "and we don't accept untrusted hash
  inputs here". Within shamir-tx that premise holds only because upstream constrains record keys
  to producer-generated random 16-byte ids — the crate's own `pub` APIs accept arbitrary
  `Bytes`/`RecordKey` with no validation. A future feature letting clients choose record ids
  converts client-chosen keys into Fx-hash collisions (O(n²) probe chains) — a CPU-DoS surface.
  Recorded so the premise is known to live outside this crate.
- Suggested fix: none while keys are system-generated; if client-chosen ids ever land, add a keyed
  hasher or key-shape validation at the boundary.

### 3.6 — medium — *(primary: 5.1)* Corrupt changefeed journal entries are silently skipped
- Security lens flagged the silent-omission contradiction of CF-1's own contract (and the missing
  corrupt-entry test case); the full write-up (envelope + gap signal + fix) lives under 5.1.

---

## 4. performance-hotpath

Lens verdict: core hot paths largely honour pillar 3 — lock-free `scc` structures with `THasher`,
O(1) atomic len mirrors where recognised, one batched `history.transact` per write/batch,
vectored `get_many` reads. Residual risk concentrates in the per-write `vacuum_key` tail
(unbatched + duplicated round-trips), the GC family (materialises the whole history store), and
O(S)/O(K) helper scans — several half-acknowledged in the code's own comments as future work.

### 4.1 — high — `vacuum_key`: unbatched + duplicated per-version I/O on the write hot path
- File:line: `mvcc_store/mvcc_gc.rs:100-121` (fast path), `:229-250` (scan path); call sites
  `mvcc_store/mod.rs:830`, `:934-936`, `:1022-1024`, `:1084`. *(also flagged by
  concurrency-lockfree #4, medium — the duplicated `lookup_ts` half)*
- Issue: `vacuum_key` runs after EVERY non-tx write (`set_versioned`, `delete_versioned`, per-key
  inside both `set_versioned_many` variants). Even the L6 fast path (CurrentOnly default) performs
  up to 3 sequential round-trips per overwrite once a deferred anchor exists: `lookup_ts` (:105)
  plus two `remove_no_flag` awaits (:109-111) — none folded into the `history.transact` that just
  landed the new version. The scan path is worse per reclaimed version: `lookup_ts` is awaited
  **twice** (:229 age check, :245 reclaim bookkeeping) plus two more sequential removes
  (:246-247), all inside the per-write tail.
- Failure scenario: default retention + steady overwrites of a hot key: each write = 1 transact +
  1 get + 2 removes (4 sequential I/O ops) where 2 would do. With `keep_history`/`max_count`,
  each write pays a full prefix scan plus up to 4 sequential ops × reclaimed versions — write
  latency grows linearly with per-key version count, exactly the hidden O(N)-per-op cost pillar 3
  bans. For `set_versioned_many` this repeats per key with no cross-key batching.
- Suggested fix: (a) reuse the ts fetched in the age check instead of the second `lookup_ts`;
  (b) collect scan-path removals into ONE `history.transact(Vec<KvOp>)` per vacuum; (c) in the
  fast path, append the prev-anchor's two deletes to the SAME transact as the incoming write;
  (d) consider moving scan-path vacuum into the GC tick.

### 4.2 — high (memory) / medium (CPU) — `gc_below` / `purge_below_ts` materialise the whole history store before deleting
- File:line: `mvcc_store/mvcc_gc.rs:305-322` (`gc_below` Phase 1), `:396-473` (`purge_below_ts`).
- Issue: Phase 1 streams the ENTIRE history store into `TFxMap<Vec<u8>, Vec<(u64, Bytes)>>`: one
  `orig.to_vec()` allocation plus one `phys_key` clone per visited row, all held simultaneously.
  `purge_below_ts` buffers ALL versions of every key — including current, snapshot-pinned,
  anchored, ts-ineligible rows — before Phase 2 filters. Peak transient memory is O(total history
  entries), unbounded relative to store size.
- Failure scenario: a GC tick (or T4 purge) on a large table buffers every below-threshold key +
  physical key at once: a 100M-entry history causes an RSS spike of that magnitude plus heavy
  allocator churn; long-lived servers pay it every tick.
- Suggested fix: the store iterates key-major — stream with a one-key lookahead (accumulate only
  the current key's entries, sort, delete, release) instead of building the global `per_key` map.
  At minimum filter eligibility during the scan and evict map entries chunk-wise.

### 4.3 — medium — `min_alive()`: full-map iteration on the write tail and GC paths
- File:line: `repo_tx_gate.rs:651-670`; hot caller `mvcc_gc.rs:156`. *(also flagged by
  concurrency-lockfree #6, low)*
- Issue: `min_alive` iterates the whole `active_snapshots` map per call. The vacuum scan path
  calls it on every write while any snapshot is live (a live snapshot sets `vacuum_needs_scan`),
  and `gc_below`/`prune_version_cache` call it per tick — O(S) per write (S = distinct pinned
  snapshot versions), taking bucket read locks that contend with the hot
  `bump_refcount`/`SnapshotGuard::drop` path. The same shape pillar 3 bans, already fixed twice
  in this crate with atomic mirrors (`predicate_set.rs:85-86`, `versioned_overlay.rs:58-59`).
- Failure scenario: N concurrent long-lived readers + a hot write key: every write walks N map
  entries only to learn nothing new (the min changes only on snapshot open/drop).
- Suggested fix: AtomicUsize occupancy mirror for the emptiness fast-path, and serve `min_alive`
  from an ordered structure (`scc::TreeIndex<u64, refcount>`) or cache the computed min,
  invalidated on snapshot open/drop — O(1)/O(log S) instead of O(S).

### 4.4 — medium — `record_conflicts`: linear interval scan over already-sorted keys, inside the commit critical section
- File:line: `repo_tx_gate.rs:1009-1018` (linear scan), `:1089-1091` (the sort that makes binary
  search "free"), `:876-883` (call site under `commit_lock`); single-dep variant `:803-824`,
  batch entry `:860-884`. *(also flagged by concurrency-lockfree #3, medium)*
- Issue: the `IndexRange` arm does `inserted_index_keys.iter().any(key_in_interval)` — O(K) per
  (record, dep) pair. `predicate_conflicts_batch` runs for Serializable commits under
  `commit_lock` (per its own calling-contract doc), so total cost is O(W × P × K) inside the
  serialised commit window, directly extending the hold time of the repo-wide commit mutex and
  serialising all Serializable commits behind it. `build_footprint_from_tx` explicitly
  `sort_unstable()`s each table's keys ("frees a future binary_search optimisation") but the
  validation never uses the order.
- Failure scenario: a Serializable tx with several range predicates validating against a busy
  commit window with wide footprints stretches the lock-held critical section linearly,
  throttling commit throughput for every table in the repo.
- Suggested fix: two `partition_point` calls over the sorted vec (lower/upper bound, preserving
  inclusive/exclusive `Bound` semantics) — O(log K) per pair; the 9-byte prefix check can hoist to
  one slice-level comparison since all keys in one footprint share the table's index ids. Keep the
  linear path for `TableScan` (single map probe, already O(1)).

### 4.5 — medium — Stream group-by: per-version-row key allocation + O(K) `shift_remove` per group
- File:line: `mvcc_store/version_entry.rs:193` (per-row copy), `:124` (`shift_remove`), `:295`
  (leftover pop — already reverse order).
- Issue: (a) `Bytes::copy_from_slice(orig)` executes for EVERY decoded version row but is consumed
  only when the key run changes — R keys × V versions performs R×V heap allocs where R suffice.
  (b) `flush_group` uses `TMap::shift_remove` — IndexMap's order-preserving O(K) memmove — once
  per history key group, i.e. O(N_keys × K_overlay) on a full stream. The leftover drain already
  emits in reverse index order, so `swap_remove` (O(1)) would not make ordering observably worse.
- Failure scenario: `current_stream` (list/scan/replication/migration) over a large table while a
  bursty overlay window holds many undrained keys: avoidable per-row allocations plus memmoves
  quadratic in the overlay key count.
- Suggested fix: (a) move the `orig_bytes` copy inside the key-change branch; (b) switch to
  `swap_remove` and document the (already reversed) leftover order.

### 4.6 — low — Vectored reads: sequential per-key awaits on fallback/cold slots + redundant re-probe
- File:line: `mvcc_store/mod.rs:1202-1207` (`get_at_many` Phase 3), `:1595-1615`
  (`get_current_many` Phase 3).
- Issue: Fallback/Cold/FloorExceeded slots are resolved one awaited call at a time; `get_at_many`
  also re-runs `current_version(&keys[i])` (:1203) already computed in Phase 1. The doc's "cold is
  the minority in steady state" assumption (:1529-1531) does not hold during post-restart warm-up
  or cache-pruned read-mostly workloads, where every key pays a sequential `seek_latest_version`
  range scan.
- Suggested fix: store `cur_v` in the `Slot` enum (drops the re-probe); resolve the fallback/cold
  subset with bounded concurrency or chunking. The intentionally unbatched range-scan fallback
  itself is documented and fine.

### 4.7 — low (borderline low/medium in source) — `history_of` resolves commit timestamps with N sequential `lookup_ts` point-reads
- File:line: `mvcc_store/mvcc_history.rs:208-219`. *(also flagged by concurrency-lockfree #8, nit)*
- Issue: Phase 3 loops `lookup_ts(version).await` per timeline entry — V sequential round-trips
  for a key with V versions — where one `history.get_many` over the `ts_key(version)` set (the
  batching seam the crate already uses at `mod.rs:1189`, `:1590`) would collapse them. Off-hot
  admin/diagnostic path (T4).
- Suggested fix: collect the versions, issue a single `get_many(ts_keys)`, assemble
  `VersionEntry`s in order.

### 4.8 — low — `VersionedOverlay::gc_upto`: full-tree collect-then-remove
- File:line: `versioned_overlay.rs:170-203`.
- Issue: iterates the ENTIRE tree (visiting entries above the threshold too) and materialises a
  `Vec` of every qualifying entry (cloning each `RecordKey`) before issuing `remove_sync` per
  entry — a transient O(K) allocation and a double walk per drainer tick. The overlay is
  window-bounded by design, but the window grows with drainer lag under write bursts; the doc
  itself defers a version-major index (:168-169, "P1e may optimise").
- Suggested fix: derive a version-major upper bound for range iteration, or remove during
  iteration with a cursor (re-probing by last-removed key); chunk the collect if cursor removal is
  impractical.

### 4.9 — low — `project_event`: per-record heap clone of the table-name String
- File:line: `changefeed.rs:453-477`.
- Issue: `table.clone()` executes per staged op although it is loop-invariant per
  `(token, staging)` group — a 10k-row single-table commit pays 10k identical `String`
  allocations on the commit path.
- Suggested fix: clone once per token, or have `RecordChange` carry an index into a per-event
  table list (also shrinks the serialised event).

### 4.10 — low — Changefeed journal writer: one sequential `put` await per event (no batching seam)
- File:line: `changefeed.rs:557-614` (`journal_writer_loop`/`persist_one`), `:151-158`
  (`ChangelogStore` — single-item `put` only).
- Issue: the module doc says the background writer "batches pending events", but the drain is a
  loop of one-at-a-time `store.put` awaits (`WRITER_BATCH` caps the count per loop, not the I/O
  shape). Sustained commit bursts drain at one round-trip per event, the 4096-deep channel fills,
  and events are dropped (bounded by design, but the drop rate is a throughput artefact, not a
  policy choice).
- Suggested fix: add a batched `put_many` to `ChangelogStore` and drain up to `WRITER_BATCH`
  events per call; keep drop-on-overflow as the backstop.

### 4.11 — low — `remap_inner_value_bytes` re-encodes rows that changed nothing
- File:line: `id_remap.rs:73-81` (with `tx_context.rs:913-929`).
- Issue: whenever a tx created any new field name (remap non-empty), EVERY staged row is fully
  decoded, walked, and re-encoded — including rows referencing no overlay ids, whose re-encode
  output is byte-identical to the input. Commit-path CPU is O(N × row size) even for the unchanged
  majority.
- Suggested fix: have `remap_value` report whether any key was rewritten and return the original
  `Bytes` untouched when not (or pre-scan the remap's id set against the row's u64 keys).

### 4.12 — nit — `set_versioned_many` / `_append_only`: duplicate key vector
- File:line: `mvcc_store/mod.rs:880`, `:982`.
- Issue: `keys: Vec<RecordKey> = items.iter().map(|(k,_)| k.clone()).collect()` — an N-key clone
  per batch used only by the trailing vacuum loop, while `items` is never consumed (later loops
  borrow it).
- Suggested fix: drop the `keys` vec; iterate `&items` in the vacuum loop.

### 4.13 — medium — *(primary: 2.2)* Pessimistic `locks` registry never evicts empty entries
- Performance lens flagged the unbounded-growth/memory-locality angle; the full write-up
  (waiter-accounting fix) lives under 2.2.

---

## 5. api-wire-protocol

Lens verdict: the physical key codecs (`version_codec`, ts-key namespace, changefeed journal
keys) are well-reasoned and genuinely property-tested, and the builder-only query rule is
trivially satisfied (the crate sits below the query layer; zero `serde_json` usage; wire names
snake_case and pinned by test). The weak spot is the changefeed wire format and its read API.

### 5.1 — high — Durable journal events have no schema/version envelope; decode failures are silently skipped
- File:line: `changefeed.rs:86-101` (`ChangelogEvent`), `:542-546` (`serialize_event`),
  `:409-411` (corrupt-entry skip — verified during synthesis), `:397-405` (store error → empty
  read). *(also flagged by security-crypto #1, medium — the CF-1 "silent omission is not
  acceptable" contradiction at :414-415, and the missing corrupt-entry test case)*
- Issue: `ChangelogEvent` is serialized with bare `rmp_serde::to_vec` into the *durable* per-repo
  journal — an artifact that survives restarts and format upgrades. No version field, no format
  tag, no envelope: adding/renaming/removing a field silently changes the on-disk layout. Worse,
  `read_from` skips entries that fail msgpack decode with only a `log::warn!`, and `gap_at` is
  populated only from `first_gap_version` (emit-time overflow drops) — so a corrupt or
  format-incompatible entry produces exactly the hole class the module's own comment rules out,
  with no resync signal. On decode failure the 8-byte journal key is always still decodable, so a
  gap marker is always available.
- Failure scenario: an upgrade adds a field to `ChangelogEvent`; a not-yet-upgraded replica (or a
  post-upgrade rollback) calls `read_from`: every entry fails `from_slice`, is skipped with a
  warn, and the caller receives empty/sparse `JournalRead { gap_at: None }` — indistinguishable
  from an empty journal. Downstream replication/subscription silently diverges with no error
  surfaced through the API.
- Suggested fix: wrap the payload in an envelope (`{ v: u8, event: ChangelogEvent }` or a leading
  format-tag byte); on decode failure set `gap_at = Some(corrupt_key_version)` (decode the 8-byte
  key regardless of payload corruption) so consumers take the documented full-resync path — or
  track a `first_decode_failure_version` watermark with the same min-CAS discipline as
  `first_gap_version`.

### 5.2 — medium — `SORTED_TAG` posting-key layout duplicated across crates with an illusory test pin
- File:line: `predicate_set.rs:159-181`; `tests/predicate_set_tests.rs:196`; mirror of
  `shamir-engine/src/index/sorted_index_manager.rs:60/:574`.
- Issue: `SORTED_TAG`/`SORTED_PREFIX_LEN` are "kept local so shamir-tx stays decoupled", with the
  comment claiming a pin by `key_in_interval_prefix_tag_matches` — but that test only asserts the
  local constant equals `0x80`; it cannot fail if the *engine-side* constant or key layout drifts.
  The pin protects the wrong crate; the coupling is real (the predicate layer must interpret
  posting keys exactly as the engine composes them).
- Failure scenario: the engine changes `SORTED_TAG` or its posting-key layout; shamir-tx still
  compiles, still passes its local pin, and `key_in_interval` returns `false` for every posting —
  `predicate_conflicts` finds no phantom, Serializable txs stop aborting on phantoms. Completely
  silent (missing aborts, no error anywhere).
- Suggested fix: move the tag byte / prefix layout into a crate both depend on (`shamir-types` or
  `shamir-collections`), or add a cross-crate test in `shamir-engine` asserting its constant equals
  the re-exported `shamir_tx::SORTED_TAG`. Fix the comment to say the current pin is local-only.

### 5.3 — medium — CF-1 gap signal is volatile (in-memory only); `read_from` never checks contiguity
- File:line: `changefeed.rs:190` (`first_gap_version` field), `:414-422` (`gap_at` computation).
- Issue: the contract is "`gap_at = Some(v)` ⇒ the journal is not contiguous, resync". But
  `first_gap_version` is a plain in-memory atomic: it resets on restart, so both overflow drops
  and the documented crash-window tail loss become undetectable after a process restart.
  Additionally, `read_from` holds the returned events (each carrying `commit_version`) yet never
  verifies their contiguity — a cheap check that would catch every hole regardless of origin.
- Failure scenario: a burst overflows the 4096-deep journal channel and drops commit_version 100
  (`gap_at = Some(100)` correctly signalled in-process). The process restarts (routine deploy)
  before the consumer catches up; on the new process `first_gap_version == 0`, so `read_from(1, …)`
  returns events 1..99,101.. with `gap_at: None`. A consumer honouring the contract trusts an
  unbroken history and permanently misses v100.
- Suggested fix: in `read_from`, scan the returned events' `commit_version`s and synthesise
  `gap_at` from the first hole (covers corrupt-skips and crash-window losses too); optionally
  persist a durable gap tombstone key at the dropped version.

### 5.4 — medium — `read_from` has no error channel — store failure and corruption are indistinguishable from "empty"
- File:line: `changefeed.rs:397-413` (verified during synthesis: store error →
  `JournalRead { events: vec![], gap_at: None }`).
- Issue: a `ChangelogStore::range_from` error is logged and returns an empty read; corrupt entries
  are skipped. The caller cannot distinguish "no events yet", "storage broken", and "entries
  dropped as undecodable". CLAUDE.md's error rules (return `Result`, propagate with `?`) are
  sidestepped entirely on this read path.
- Failure scenario: a misconfigured or failed changelog store makes every pull look like an empty
  feed. Monitoring built on `JournalRead` sees nothing wrong; replication falls behind with zero
  signal.
- Suggested fix: return `Result<JournalRead, ChangefeedError>` (or at minimum add
  `truncated: bool` / `decode_failures: usize` to `JournalRead`) so the three cases are
  distinguishable.

### 5.5 — medium — `read_from` re-demands the store the constructor already consumed; no same-store guarantee
- File:line: `changefeed.rs:232` (`new(store: Arc<dyn ChangelogStore>)`), `:390-395`
  (`read_from(&self, store: &Arc<dyn ChangelogStore>, …)`).
- Issue: `new` moves the store `Arc` into the background writer task; `read_from` then requires
  the caller to pass a *second* handle to the same store. Nothing enforces it is the same store:
  passing a different `ChangelogStore` silently returns wrong/empty results with no error. The
  caller must also keep a duplicate `Arc` alive for the feed's whole lifetime or reads break.
- Failure scenario: a caller builds the feed from the repo's changelog store but later passes a
  fresh/other store handle (easy in DI or test wiring) — `read_from` reads the wrong journal and
  the API reports success with empty results.
- Suggested fix: keep `Arc<dyn ChangelogStore>` in `Self` (clone into the writer task) and drop
  the `store` parameter from `read_from`.

### 5.6 — low — Empty-`Bytes`-as-tombstone sentinel is unguarded on the public write path
- File:line: `mvcc_store/mod.rs:766` (`set_versioned`), `:724-728` (`resolve_read`:
  `Ok(val) if val.is_empty() => Ok(None)`), `:1058-1060` (the only documentation of the
  convention, at `delete_versioned`).
- Issue: the convention "empty value bytes = tombstone" is sound for msgpack records (never
  zero-length) but is documented only on `delete_versioned`, while
  `set_versioned`/`set_versioned_many`/`set_versioned_many_append_only` accept arbitrary `Bytes`
  with no `debug_assert!` rejecting empty values. A non-record caller writing `Bytes::new()`
  creates an implicit delete.
- Failure scenario: any present or future caller stores a legitimately empty blob through
  `set_versioned`: every read path (`resolve_read`, `get_current_bytes`, `get_at_many`,
  `current_stream`) interprets it as a tombstone and the row vanishes from scans — silently, since
  nothing rejected the write.
- Suggested fix: `debug_assert!(!value.is_empty())` on the three `set_versioned*` entry points
  (and the `KvOp::Set` arm of `apply_committed_visible`), plus a contract line on
  `set_versioned`'s doc.

### 5.7 — low — `tx_id = 0` "non-tx write" sentinel is unenforced against 0-seeded id allocators
- File:line: `changefeed.rs:491-494` (`project_event`), `:513-514` (`nontx_event`);
  `repo_wal_manager.rs:30-40`.
- Issue: the external changefeed contract ("0 = non-tx write", per LIVE_SUBSCRIPTIONS.md) relies
  on every real tx id being ≥ 1. `RepoTxGate::fresh()` seeds 1, but
  `RepoWalManager::new(initial_txn_id, …)` accepts any `u64` — a 0 seed mints a real tx whose
  projected event is indistinguishable from a non-tx write.
- Failure scenario: a recovery path seeds `RepoWalManager` with 0; the first real transaction gets
  id 0; its `ChangelogEvent.tx_id == 0` and every downstream consumer classifies a genuine tx as a
  non-tx write.
- Suggested fix: `debug_assert!` (or clamp) in the id allocators so 0 is never handed out, or
  document the precondition on `project_event`/`nontx_event`.

### 5.8 — nit — Dead public group-commit API remains in the exported surface
- File:line: `lib.rs:69` (`pub use pending_commit::PendingCommit`);
  `repo_tx_gate.rs:752-763` (`enqueue_pending`/`drain_pending`).
- Issue: F-79/#906 sanction the dead `pending_commits` field as scaffolding, but the crate still
  *exports* `PendingCommit` and both zero-caller accessors — inviting use of a path whose
  contention model is explicitly documented as "never audited for a live call".
- Suggested fix: demote `enqueue_pending`/`drain_pending` to `pub(crate)` (or `#[doc(hidden)]`)
  and drop `PendingCommit` from `lib.rs` re-exports until group-commit is revived.

### 5.9 — nit — `serde_bytes_compat` deserializer accepts any sequence, not just byte arrays
- File:line: `changefeed.rs:104-116`.
- Issue: `serialize` emits `serialize_bytes` (msgpack bin) but `deserialize` goes through
  `Vec::<u8>::deserialize`, which also accepts a msgpack array-of-integers encoding. Asymmetric:
  round-trip of self-produced payloads is fine (and tested), but hand-crafted/adversarial journal
  entries get a second accepted encoding for the same field.
- Suggested fix: implement a small `Visitor` calling `deserialize_byte_buf` so only the bin
  encoding is accepted.

### 5.10 — low — *(primary: 6.4)* Stringly-typed `Result<_, String>` across the public API
- The api lens enumerated the trait/config/staging sites (`ChangelogStore` :154-157, `serialize_event`
  :542, `Retention::validate` retention.rs:60, `set_retention` mod.rs:500, `rewrite_set_bytes`
  staging_store.rs:249-251, `apply_id_remap` tx_context.rs:916); the consolidated write-up lives
  under 6.4.

### 5.11 — low — *(primary: 3.2)* `VERSION_SEP` invariant doc/probability/prop-test findings
- Folded into 3.2, which carries the full three-part analysis (invariant unenforced, wrong
  probability claim, test dodge, stale reference) plus the `vacuum_key` guard consequence.

---

## 6. error-handling-lifecycle

Lens verdict: error-path discipline is genuinely strong in places — the
`VersionGuard`/`CellReservationGuard`/`SnapshotGuard` RAII design makes abort-marking hold "by
construction", and a fault-injection double (`FailingStore`) exists. But the bump-first write
paths perform no compensating rollback when the durable transact fails, `thiserror` is declared
but never used, several GC/recovery paths swallow storage errors inconsistently with their
siblings, the changefeed gap invariant has an undetected hole, and error-path tests stop at two
functions on fresh keys. Note: `RepoTxGate::pending_commits.lock().unwrap()`
(repo_tx_gate.rs:753/761) is **not** flagged — CLAUDE.md sanctions it as dead scaffolding.

### 6.1 — high — Failed `history.transact` leaves `RecordCell` advanced with no rollback — prior version permanently masked on point reads
- File:line: `mvcc_store/mod.rs:766-832` (`set_versioned`: `publish_cell` at :785, `?` at :799 —
  verified during synthesis; `old_v` already captured at :782); same shape `:853-938`
  (`set_versioned_many`, publish loop :885, `?` :898) and `:1035-1086` (`delete_versioned`,
  publish :1051, `?` :1063).
- Issue: all three non-tx write paths execute `publish_cell(key, new_v)` *before* the single
  durable `history.transact(...)` (deliberate — the MVCC-2 snapshot invariant requires
  publish-before-log). On transact failure the `?` propagates immediately; the `VersionGuard` drop
  correctly marks the version `Aborted` and advances the watermark, but the cell map is left at
  `new_v` with **no compensating restore** (verified: no rollback/revert/compensate exists
  anywhere in `mvcc_store/`). `publish_cell`'s A2 max-monotonic guard means nothing short of a
  successful rewrite of the key ever corrects it, and `prune_version_cache` only evicts cells with
  `version < min_alive` — a cell at the aborted (now watermark-past) version is never evicted.
- Failure scenario: *failed SET:* key `k` has committed version 5; `set_versioned(k, v6)` hits
  disk-full/IO in `transact`; caller gets `Err`. Every later `get_current`/`get_current_bytes`:
  `cur_v = 6`, floor ≥ 6, no `get_at` fallback; overlay miss; `history.get(k::6)` → `NotFound` →
  **`Ok(None)` — the record reads as deleted** though version 5 is intact in the log. Meanwhile
  `current_stream` (which group-bys the *log*, not the cell) still emits it — point and stream
  reads disagree indefinitely. *Failed DELETE:* the cell bumps to the tombstone version but the
  tombstone never lands; point reads return `None` — **the delete appears to have succeeded**
  in-process, then the record resurrects after restart. A durability illusion. The doc's
  "cancel-safe: NO … caller must retry or WAL-replay" covers cancellation, not the *error* path:
  non-tx writes have no WAL entry to replay, and a caller surfacing the `Err` (normal engine
  behavior) never retries — the divergence is permanent until the same key is rewritten.
- Suggested fix: on `transact` failure restore the cell before propagating — capture `old_v` per
  key (already captured for vacuum), then on the error branch run an explicit
  `restore_cell(key, old_v)` that unconditionally sets `cell.version = old_v` (a plain `entry_sync`
  write — *not* `publish_cell`, whose max-monotonic guard would refuse the regression). For
  `set_versioned_many`, roll back all batch keys to their captured `old_versions`. Alternatively
  defer the publish loop to after transact success on the batch paths, keeping the pre-log publish
  only where the MVCC-2 invariant demonstrably requires it.

### 6.2 — medium — `vacuum_key` scan path silently swallows prefix-scan stream errors — inconsistent with its siblings
- File:line: `mvcc_store/mvcc_gc.rs:173-181` (`batch.unwrap_or_default()` at :174).
- Issue: the retention-aware vacuum's phase-1 scan treats every stream error as an empty batch.
  The two sibling GC paths — `gc_below` (:312) and `purge_below_ts` (:408) — both propagate with
  `batch?`. The truncation errs toward over-retention rather than data loss (deletions target only
  *collected* entries), so this is not a correctness hole — but under a persistent read error,
  vacuum silently stops reclaiming anything with **zero log output** while `gc_below` on the same
  store correctly reports errors. The deletions being best-effort (documented `let _ =`) is fine;
  the *decision-input* scan being silently lossy is not.
- Failure scenario: a backend returns intermittent `DbError::Storage` from `scan_prefix_stream`.
  Every write still succeeds, but the vacuum collects a partial/empty list each time and quietly
  deletes nothing; no warning; disk fills; the only symptom is history growth.
- Suggested fix: propagate (make `vacuum_key` return `DbResult<()>` — its callers already return
  `DbResult`), or at minimum `log::warn!` per errored batch and skip the reclaim pass for that key
  (a partial list must not drive deletions at all). Match `gc_below`/`purge_below_ts`.

### 6.3 — medium — Changefeed journal gap detection has an undetected hole on persist failures
- File:line: `changefeed.rs:632-655` (`persist_one`, error branch :645-651); related
  `journal_send` `Closed` branch :336-339.
- Issue: CF-1's `first_gap_version` is updated **only** on `TrySendError::Full` (:310-334). When
  the background writer's `store.put()` fails, the event is dropped with a `log::warn!` and *no*
  gap marker — and the next successful persist advances `last_persisted_version` via `fetch_max`
  **past the hole**, so the CF-2 watermark cannot expose it either. `read_from` then returns
  `gap_at: None` over a journal silently missing a version — directly violating the module's own
  contract ("Conservative over-signal is acceptable; silent omission is not"). The
  `TrySendError::Closed` branch is even quieter: no counter, no gap marker, no log (a panicked
  writer task closes the channel and every subsequent commit's journal event vanishes with zero
  observability).
- Failure scenario: a replication consumer resumes from `read_from(v)` after a transient store
  failure dropped version `v`'s journal write; `gap_at` is `None`, events on both sides of the
  hole are returned, and the consumer trusts an unbroken history — missing exactly one committed
  transaction with no signal to trigger the documented full-snapshot resync.
- Suggested fix: in `persist_one`'s error branch run the same min-CAS loop on
  `first_gap_version` that `journal_send` uses for `Full`. For `Closed`, bump `journal_dropped`
  (or a dedicated counter) so a dead writer is at least countable.

### 6.4 — medium — `thiserror` declared but never used — six APIs return `Result<_, String>`, including a public trait
- File:line: `Cargo.toml:23` (dependency, zero uses); `changefeed.rs:154` & `:157`
  (`ChangelogStore::put`/`range_from`), `changefeed.rs:542` (`serialize_event`),
  `staging_store.rs:249` (`rewrite_set_bytes`), `tx_context.rs:913-916` (`apply_id_remap`),
  `mvcc_store/mod.rs:500` (`set_retention`), `mvcc_store/retention.rs:60`
  (`Retention::validate`). *(also flagged by api-wire-protocol #6, low, and correctness-tdd #9,
  nit)*
- Issue: CLAUDE.md mandates `thiserror` for library error enums; the crate defines no error enum
  of its own and threads `String` through six APIs. `ChangelogStore` is the worst offender: a
  **public trait** whose `Result<(), String>` / `Result<Vec<Bytes>, String>` shape forces
  stringly-typed errors onto every implementor (engine-side production store included), making
  error-kind matching impossible and pushing `format!`-based error construction into callers
  (`serialize_event`, `apply_id_remap`'s `.map_err(|e| format!("remap: {e}"))`).
- Suggested fix: introduce `#[derive(thiserror::Error)]` enums for the crate's failure kinds and
  convert the six sites, starting with the public `ChangelogStore` trait (its failures are already
  only ever logged — a small `ChangelogStoreError` with `#[from]` is a drop-in); then
  `RetentionError`, `RemapError`.

### 6.5 — medium — Error-path tests stop at two functions on fresh keys — the documented batch-abort and drain error claims are untested
- File:line: `tests/mvcc_store_tests/error_tests.rs` (entire file); fault double at
  `test_stores.rs:10-108`.
- Issue: the crate has an excellent `FailingStore` double (`fail_get`/`fail_remove`/`fail_set`)
  but it is exercised by exactly three tests, all on a **fresh key** against
  `set_versioned`/`delete_versioned`. Untested error paths, each carrying a documented behavioral
  claim only a test can pin: (a) `set_versioned_many`/`_append_only` transact failure — the
  guard-vector comment (mod.rs:869-873) claims "every guard drops un-committed and marks its
  version Aborted, so the contiguous watermark advances past the whole failed batch instead of
  wedging at the first version"; no test asserts the watermark advances or the overlay stays
  empty; (b) no pre-existing-key failure test — precisely why 6.1 is invisible to the suite:
  `set_versioned_propagates_archive_read_error` asserts `get_current == None` on a key that *never
  existed*, where `None` is correct for the wrong reason; (c) `write_committed_to_history` /
  `write_committed_batch_to_history` / `drain_to_history` `?` propagation and the
  `drain_exclusive` backoff-returns-`Ok` contract (#1032) — only the deferral is tested
  (`write_committed_batch_tests.rs:363`); (d) `get_at_many`/`get_current_many` mid-assembly `?`
  propagation (note `FailingStore` doesn't override `get_many`); (e) `vacuum_key` scan-error
  behavior (6.2) and journal persist-failure gap behavior (6.3).
- Suggested fix: extend `error_tests.rs`: (1) failed `set_versioned` on a key with a committed
  prior version, asserting the prior value is still readable (will currently fail — 6.1);
  (2) failed `set_versioned_many`, asserting `gate.last_committed()` advances past the batch,
  overlay stays empty, `durable_watermark() <= last_committed()`; (3) failed `drain_to_history`
  mid-version, asserting propagation and `drain_exclusive` release; (4) `get_at_many` with
  `fail_get` armed, asserting propagation.

### 6.6 — low — `apply_committed_ops` doc contradicts the code's error-path ordering
- File:line: `mvcc_store/mvcc_history.rs:413-431`.
- Issue: the doc states "Ordering: history FIRST (durable landing), then visible (overlay + cell)
  — matching the pre-split contract where a failed history `transact` (`?`) left no
  reader-visible state." The code does the **opposite**: `apply_committed_visible` first (:427),
  then `write_committed_to_history(...).await?` (:428). The inner comment (:417-426) correctly
  explains the intentional swap (pending_ts must be stamped before the drain half consumes it) and
  admits "a history error propagates via `?` (the cell/overlay are then ahead …)". The stale outer
  paragraph is exactly what a maintainer auditing error-path state reads first, and it describes
  the opposite guarantee.
- Suggested fix: rewrite the doc's ordering paragraph to match the code: visible-first (ts stamp)
  → history `?` → on error the cell/overlay are intentionally ahead, mirroring the production
  ack-path until the drainer catches up.

### 6.7 — nit — `lookup_ts` swallows all `history.get` errors as "unknown age" with no log
- File:line: `mvcc_store/mod.rs:1624-1636`.
- Issue: `Err(_) => None` is the conservative direction (unknown-ts versions are KEPT by
  vacuum/purge), so it is safe — but a persistent storage error silently disables the entire
  age-retention axis and, in `history_of`, silently degrades every `ts_millis` to `None`.
- Suggested fix: a single rate-limited `log::debug!`/`warn!` on the error arm would make "age
  retention quietly stopped working" diagnosable.

### 6.8 — nit — `RepoChangefeed::new` panics outside a tokio runtime; writer task is detached
- File:line: `changefeed.rs:249-255`.
- Issue: `tokio::spawn(journal_writer_loop(...))` panics if called off-runtime (e.g. from `new()`
  during engine construction before the runtime exists), and the returned `JoinHandle` is
  discarded so a writer panic is observable only via the CF-2 watermark stall. Engine-side call
  sites are runtime-hosted today.
- Suggested fix: a `#[track_caller]`-documented contract, or a builder taking the runtime's
  handle.

### 6.9 — low — *(primary: 1.3)* `ts_index_rebuild` swallows stream errors and unconditionally marks ready
- Folded into 1.3 (same defect, mod.rs:398-429); the error-handling lens additionally proposed
  surfacing a `ts_index_degraded: AtomicBool` next to `ts_index_len()`.

### 6.10 — low — *(primary: 3.1)* `touch_sync` `expect` vs propagated sibling
- Folded into 3.1 (same defect, layered_interner.rs:82-86 vs :256-263).

### 6.11 — low — *(primary: 3.3)* `StagedRow::as_inner` panic on unvalidated staged bytes
- Folded into 3.3 (same defect, staging_store.rs:46-49); the error-handling lens additionally
  proposed a `try_as_inner` variant.

---

## 7. style-claude-md

Lens verdict: mostly disciplined on the tests-that-matter — no inline `#[cfg(test)] mod tests {}`
blocks anywhere, all three `tests/mod.rs` files are pure manifests, tests split by topic, no
TODO/FIXME noise. The one bright-line breach is `mvcc_store/mod.rs` itself. Positive: the crate
demonstrates the compliant `changefeed.rs` + `changefeed/tests/` layout it otherwise deviates
from.

### 7.1 — high — `mvcc_store/mod.rs` is a full implementation file, not a re-export manifest
- File:line: `mvcc_store/mod.rs:1-1638`.
- Issue: CLAUDE.md: "`mod.rs` files contain re-exports only. Types and logic live in sibling
  files." This mod.rs contains the module doc, `pub(super) const TS_TAG` (:68), `pub(crate) fn
  ts_key` (:71) and `decode_ts_key` (:82), `pub(crate) struct RecordCell` (:94), `pub struct
  MvccStore` (:125) with all field docs, and roughly 1,400 lines of `impl MvccStore`. The sibling
  files (`mvcc_history.rs`, `mvcc_gc.rs`, `drain.rs`, `mvcc_locks.rs`, …) correctly hold extension
  `impl MvccStore` blocks — the pattern is right, but the anchor struct and its core impl live in
  the one file the rules reserve for wiring.
- Failure scenario: no runtime failure; this is the maintainability cost the rule exists to
  prevent — the crate's most-edited type has its `git blame` diluted in a file whose diffs should
  only ever be module wiring, and logic changes masquerade as module-structure changes in review.
- Suggested fix: move the struct + core impl into `mvcc_store/store.rs` (or adopt the crate's own
  `changefeed.rs` + `changefeed/` layout, i.e. `src/mvcc_store.rs` + `src/mvcc_store/`), leaving
  `mod.rs` as docs + `mod`/`pub use` only. Purely mechanical; no API change (paths stay
  `crate::mvcc_store::*`).

### 7.2 — medium — Mid-function `use` statements in production code (8 sites, 5 files)
- File:line: `mvcc_store/mod.rs:399-400` (inside `ts_index_rebuild`), `:1389` (inside
  `snapshot_stream_impl`); `mvcc_history.rs:88`, `:103,105` (the latter a `#[cfg(test)]` fn —
  nearest to the cfg-gated exception, but both imports hoist cleanly); `mvcc_gc.rs:301` and
  `:397` (identical `decode_version_key` import duplicated in two fn bodies);
  `tx_context.rs:512` and `:670` (`scc::hash_map::Entry`); `layered_interner.rs:96`
  (`Entry::{Occupied, Vacant}`).
- Issue: CLAUDE.md ("Imports at the top"): none of the three documented exceptions apply to
  these. It compounds: because `StreamExt` is not at the top of `mvcc_store/mod.rs`, line 1436 is
  forced into a fully-qualified `futures::StreamExt::map(...)` call — the missing header import
  leaks into call sites.
- Suggested fix: hoist each to the file header (test files carry the same pattern at lower stakes;
  fix opportunistically).

### 7.3 — medium — Test placement deviates from the documented per-module `tests/` layout
- File:line: `src/tests/` (wired from `lib.rs:33-34`), `src/tests/mvcc_store_tests/`.
- Issue: CLAUDE.md prescribes one `tests/` directory **per module** (e.g.
  `shamir-types/src/types/tests/`), wired via the parent module's `#[cfg(test)] mod tests;`.
  shamir-tx concentrates tests for ~14 modules into a single crate-root `src/tests/`, and nests
  MvccStore's 28-file suite at `src/tests/mvcc_store_tests/` rather than
  `src/mvcc_store/tests/`. Only `changefeed` follows the documented shape. Mitigations: all
  `tests/mod.rs` files are re-export-only manifests, files are topical, zero inline test modules,
  shared fixtures properly factored.
- Failure scenario: doc/code divergence — an engineer (or agent) following CLAUDE.md looks for
  `src/<module>/tests/` and doesn't find it; the mvcc suite is doubly-nested away from the module
  it tests.
- Suggested fix: migrate incrementally (start with `mvcc_store_tests/` → `src/mvcc_store/tests/`;
  new tests go per-module), or amend CLAUDE.md to bless the crate-root layout — one of the two
  should move so the documented standard and the code agree.

### 7.4 — low — Stale / self-contradictory doc comments (rename & refactor drift)
- File:line: `lib.rs:8-31` (Status block says both "Stage 2 (in progress)" and "complete";
  landed-primitives list omits 10+ shipped modules); `repo_tx_gate.rs:33` ("same lifetime
  discipline as `MvccStore::version_cache` (mvcc_store.rs:446)" — the field is now `cells`/
  `RecordCell` (#532), the file is `mvcc_store/mod.rs`; same vintage `:643`, `:888`;
  `mvcc_gc.rs:531` still names the method `prune_version_cache`); `version_entry.rs:27` (cites
  removed `MvccStore::record_ts` — broken intra-doc link); `version_codec.rs:29-30` ("property
  tests below" — tests live in `src/tests/version_codec_tests.rs`; folded into 3.2's fix);
  `pending_commit.rs:10-14` (describes the group-commit leader removed in F-54/#865 — dead
  machinery).
- Issue: misleading docs about current structure (wrong field names, dead paths, wrong file
  pointers); broken rustdoc links ship silently since `doctest = false`.
- Suggested fix: sweep the `version_cache` vocabulary to `cells`/`RecordCell` (or rename to
  `prune_cells`), refresh the lib.rs status block, repoint the stale file/method references.

### 7.5 — low — One-file-one-export stretched in `changefeed.rs` and `repo_tx_gate.rs`
- File:line: `changefeed.rs` (659 lines: event wire types `RecordChange` :58, `ChangeOp` :73,
  `ChangelogEvent` :87 + projection helpers vs runtime `ChangelogStore` :152, `RepoChangefeed`
  :166, `JournalRead` :199 + writer loop); `repo_tx_gate.rs` (~1,100 lines: gate family
  `RepoTxGate` :62, `SnapshotGuard` :209, `OpeningBarrier` :269 vs Phase-C conflict family
  `TableWriteFootprint` :35, `CommitWriteRecord` :49, `record_conflicts` :1004,
  `build_footprint_from_tx` :1044).
- Issue: two distinguishable families per file that change for different reasons (the gate/conflict
  coupling via `commit_write_log` is defensible).
- Suggested fix: direction, not defect — natural splits are `changefeed/event.rs` + runtime, and a
  conflict-validation sibling for the gate. Split when either file is next substantively edited;
  do not churn just for this.

### 7.6 — nit — `metrics.rs` has no test coverage in this crate
- File:line: `metrics.rs` (`TxMetrics` :7, `TxMetricsSnapshot` :92).
- Issue: no `metrics_tests.rs` and no test file references `TxMetrics`/`TxMetricsSnapshot`; the
  snapshot/diff arithmetic is pure and trivially testable. (`pending_commit.rs` is likewise
  unreferenced by tests, but that follows from its documented-dead status — see 5.8.)
- Suggested fix: a small `metrics_tests.rs` covering `snapshot()`/delta math; fold into the
  dead-scaffolding decision for `PendingCommit`.

---

## Finding counts

Raw lens-tagged total (as filed, matching the workspace SUMMARY's pre-dedup row): **65** =
0 critical / 7 high / 21 medium / 26 low / 11 nit. 14 findings are the same root-cause defect as
a primary-lens finding (flagged in 2-3 lens files each) and fold into it, per the workspace dedup
convention.

| Severity | Lens-tagged findings | Deduped distinct defects | Dedup groups (members listed) |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 7 | 7 | 1.1 (finalize_reservation) · 2.1 + correctness#4 (publish_committed) · 4.1 + concurrency#4 (vacuum_key I/O) · 4.2 (GC materialization) · 5.1 + security#1 (journal envelope/corrupt-skip) · 6.1 (no-rollback) · 7.1 (mod.rs) |
| medium | 21 | 17 | 1.2 · 1.3 + error#6 (ts_index ready) · 2.2 + correctness#7 + perf#6 (locks registry) · 3.1 + error#7 (touch_sync) · 4.3 + concurrency#6 (min_alive cost) · 4.4 + concurrency#3 (record_conflicts) · 4.5 (stream group-by) · 5.2 (SORTED_TAG) · 5.3 (volatile gap) · 5.4 (no error channel) · 5.5 (same-store) · 6.2 (vacuum swallow) · 6.3 (persist-failure gap) · 6.4 + api#6 + correctness#9 (thiserror) · 6.5 (error tests) · 7.2 (imports) · 7.3 (test layout) |
| low | 26 | 18 | 1.4 (pass ordering) · 1.5 (min_alive TOCTOU — distinct from 4.3) · 2.3 (range(..).count()) · 2.4 (A10 starvation) · 3.2 + api#8 (version_codec invariant) · 3.3 + error#8 (StagedRow) · 3.4 (journal namespace) · 4.6 (vectored reads) · 4.7 + concurrency#8 (history_of) · 4.8 (gc_upto) · 4.9 (project_event clone) · 4.10 (journal writer) · 4.11 (remap re-encode) · 5.6 (tombstone sentinel) · 5.7 (tx_id=0) · 6.6 (apply_committed_ops doc) · 7.4 (stale docs) · 7.5 (file split) |
| nit | 11 | 9 | 1.6 (predicate_conflicts doc) · 2.5 (validate_read_set comment) · 3.5 (THasher premise) · 4.12 (keys vec) · 5.8 (dead exports) · 5.9 (serde_bytes_compat) · 6.7 (lookup_ts swallow) · 6.8 (off-runtime spawn) · 7.6 (metrics tests) |
| **total** | **65** | **51** | 14 secondary-lens entries fold into their primaries |

Deduplicated defect census: **0 critical, 7 high, 17 medium, 18 low, 9 nit = 51 distinct
defects** (65 lens-tagged findings). No high findings merge with each other — each high is a
distinct defect.

## Fix Plan

**P0 — before anything else ships from this crate**

1. **Restore the cell when the durable write fails.** Add `restore_cell(key, old_v)` on the
   `history.transact` error branch of `set_versioned`/`set_versioned_many`/`delete_versioned`
   (plain `entry_sync` write, *not* `publish_cell`), plus the pre-existing-key failure test from
   6.5. Closes **6.1** (durability illusion) and the 6.5-(1) test gap.
2. **Make the ack-path publisher max-monotonic.** Guard `finalize_reservation`'s Occupied branch
   with `version > cell.version`; add the out-of-order ack test and the two-task
   `apply_committed_visible` race test. Closes **1.1** — closes the stale-read/masked-SSI-conflict
   window on every commit.
3. **Delete or delegate `publish_committed`.** One-line-grade: make its body identical to
   `publish_committed_max` (or `pub(crate)`/remove and migrate the ~20 test callers), and correct
   the unsound `commit_lock` doc. Closes **2.1** (+ correctness#4).
4. **Changefeed journal integrity envelope.** Add the format/version envelope to
   `ChangelogEvent`; set `gap_at` on decode failure; add the `read_from` contiguity scan over
   returned `commit_version`s; mark gaps on `persist_one` failure and count `Closed`. Closes
   **5.1** (+ security#1), **5.3**, **6.3** — replication stops silently diverging.
5. **De-duplicate and batch `vacuum_key` I/O.** Reuse the age-check ts, fold scan-path removals
   into one `transact` per vacuum, fold the fast-path anchor deletes into the incoming write's
   transact. Closes **4.1** (+ concurrency#4) — the per-write I/O inflation on every non-tx write.

**P1 — soon**

6. **Protect SSI reservations from GC.** `retain_sync` predicate keeps `reserved_by != 0` cells +
   the claim-then-`gc_below`-then-`try_reserve` test. Closes **1.2** (double-commit window).
7. **Stream the GC passes.** One-key-lookahead streaming in `gc_below`/`purge_below_ts` instead of
   materialising the whole history store. Closes **4.2**.
8. **Stop caching known-bad state.** `ts_index_rebuild` marks ready only on a clean pass (1.3 +
   error#6); `vacuum_key` propagates or warns on scan errors instead of `unwrap_or_default`
   (**6.2**).
9. **Expand error-path tests** per 6.5-(2)-(4): batch-abort watermark advance, `drain_to_history`
   failure + lock release, `get_at_many` propagation. Closes **6.5**.
10. **Adopt thiserror**, starting with the public `ChangelogStore` trait, then
    `RetentionError`/`RemapError`. Closes **6.4** (+ api#6, correctness#9) and unblocks **5.4**.
11. **Fix the changefeed read API shape**: `Result`/`truncated`/`decode_failures` on
    `JournalRead` (**5.4**) and keep the store `Arc` in `Self` so `read_from` loses the re-demanded
    parameter (**5.5**).
12. **Share `SORTED_TAG`** via `shamir-types`/`shamir-collections` or a cross-crate pin test.
    Closes **5.2** — phantom-detection loss is currently silent.
13. **Commit-window cost**: `partition_point` for `record_conflicts` (**4.4** + concurrency#3) and
    a cached-min/`TreeIndex` `min_alive` (**4.3** + concurrency#6, which also narrows 1.5's
    exposure).
14. **`touch_sync` returns `Result`** (or pins infallibility with a test). Closes **3.1**
    (+ error#7) — removes the non-tx-path process-panic on input.
15. **Split `mvcc_store/mod.rs`** into `store.rs` + wiring manifest. Closes **7.1** (rated high on
    CLAUDE.md bright-line weight; mechanical, no runtime effect — sequenced here rather than P0
    for that reason).
16. **Stale-doc sweep (one docs-only commit):** lib.rs status block, `version_cache` vocabulary,
    `apply_committed_ops` ordering paragraph, `predicate_conflicts` floor rationale,
    `validate_read_set` early-return comment, version_codec doc. Closes **1.6, 2.5, 6.6, 7.4**
    (and 3.2's doc half).

**P2 — backlog**

17. **Locks-registry eviction** with per-`KeyLock` waiter accounting (or GC-tick sweep). Closes
    **2.2** (+ correctness#7, perf#6).
18. **Version-codec hardening triple**: encode-time `debug_assert`, corrected probability/invariant
    doc, per-key `cur_v` grouping in `vacuum_key`'s scan path. Closes **3.2** (+ api#8).
19. **`StagedRow` decode discipline**: validate at `set` or add `try_as_inner`/`Result`. Closes
    **3.3** (+ error#8).
20. **Journal keyspacing**: per-repo discriminator in `version_key` (or documented/
    asserted exclusive-store contract). Closes **3.4**.
21. **Remaining hot-path polish**: stream group-by copy-in-branch + `swap_remove` (**4.5**),
    vectored-read slot `cur_v` + chunked cold resolution (**4.6**), `history_of` via one
    `get_many` (**4.7** + concurrency#8), `gc_upto` range-bound/cursor removal (**4.8**),
    `project_event` table clone-per-token (**4.9**), batched `put_many` journal writer (**4.10**),
    remap skip-unchanged rows (**4.11**), drop the `keys` vec (**4.12**).
22. **Barrier/starvation hardening**: pinned-floor-aware A10 barrier so GC can run under churn
    (**2.4**); post-scan `snapshots_opening()` re-check in `min_alive` (**1.5**); `pass`
    ascending-order `debug_assert`/max-fold (**1.4**).
23. **Contract guards & hygiene**: empty-value `debug_assert!` on `set_versioned*` (**5.6**),
    `tx_id != 0` in id allocators (**5.7**), demote dead group-commit exports (**5.8**),
    `deserialize_byte_buf` visitor (**5.9**), THasher-premise doc note (**3.5**), rate-limited
    `lookup_ts` error log (**6.7**), runtime-hosting contract for `RepoChangefeed::new` (**6.8**),
    `metrics_tests.rs` (**7.6**), imports-at-top hoist (**7.2**), test-layout migration or
    CLAUDE.md amendment (**7.3**), opportunistic `changefeed`/`repo_tx_gate` family split
    (**7.5**).
