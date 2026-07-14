# Checkpoint — 2026-07-10 [audit-remediation-500-inflight]

## Session summary

Continuation of a very long audit-remediation campaign against the 5-agent audit at `docs/dev-artifacts/audits/2026-07-06-*.md`. Standing instruction (from repeated `/babygoal` invocations, most recently updated mid-campaign): implement remaining tasks sequentially using `@oh` (Agent tool, Opus-based) sub-agents for implementation and `@fl` (Agent tool, Fable-based) for adversarial review — this replaced an earlier `/crush`+`@sh` pipeline the user explicitly asked to switch away from. After each `@fl` review, the orchestrator (me) fixes any genuine bugs directly (never re-delegates fixes), re-verifies independently (build/fmt/clippy/tests), and commits with a detailed message crediting what was found and fixed. A `/goal` Stop-hook ("решить все задачи" — solve all tasks) is active and has repeatedly fired to push the session to keep working through the TaskList rather than stop early.

**Completed and committed this session (chronological, each via the brief→`@oh`→verify→`@fl`→commit pipeline):** #495 (security residual: subscription cap, reactive query-limits, WASM epoch/wall-clock deadline, SSRF DNS-resolution guard — with finding 1d reverted after breaking legitimate resume, since TLS exporters are per-connection-unique under RFC 9266), #496 (perf: keyset O(N) pagination via `lookup_range_first_k`, MVCC `ts_index` GC, SQ8 precompute), #497 (client: TS `explain`/`async_index`/wire-type fixes, `executeWithTouch` Rust-vs-TS parity settled via a server-side characterization test proving Rust had the latent bug, typed `ShamirDbError`/timeouts in the TS SDK), #498 (RUSTSEC triage — NOT a code fix but a security-judgment pass: 4 of 13 advisories closed via lock-only `cargo update` within already-permitted semver ranges per explicit user scoping; 6 remaining need a manifest-level decision like `scc` 2.x→3.x, documented not fixed), #499 (posting-list `Arc<BTreeSet>`→`Arc<[RecordId]>`; investigation showed the audit's assumed BTreeSet-intersection hot path doesn't exist — real consumers only iterate/union — turning a "structural" finding into a low-risk representation swap, verified safe across all 4 storage backends).

**Currently in flight: #500** (WAL `SegmentSet::open` avoiding a full replay-per-sealed-segment just to compute `max_version`, via a new `NNNNNNNN.meta` sidecar file written at seal time). This re-examined and overturned an EARLIER deferral (from task #489) that had claimed "WAL format doesn't support backward-seek" — that reasoning applied to a different, more invasive idea; the actual fix (write new metadata forward at seal time) is purely additive. `@oh`'s first implementation had bench numbers up to 4.30x at 256 segments, but `@fl`'s FIRST review pass found a genuine, reachable crash-safety bug: a segment can be RE-ACTIVATED after a crash (chosen as the new active segment on reopen because it has the highest on-disk seq, if the crash happened before the next segment's file existed) while still carrying a stale sidecar from its prior sealed incarnation; if that segment is later poisoned-and-rotated or re-sealed with a failed sidecar rewrite, the stale (too-low) sidecar could survive and mislead a future open's `truncate_below` into deleting not-yet-durable data. Verdict was DO NOT SHIP. I (orchestrator) fixed this myself: added a `crate::segment_meta::remove_blocking` call immediately after opening the active segment in `SegmentSet::open` (sheds any stale sidecar before any new append can make it stale), the same removal added defensively in `rotate_after_poison`, and a new regression test `reactivated_segment_sheds_stale_sidecar` that plants a stale sidecar, reopens, asserts it's gone, pushes real appends past the stale value, forces a reseal, reopens again, and asserts a genuine replay sees the TRUE max_version. Re-verified independently: build/fmt/clippy clean, `./scripts/test.sh -p shamir-wal -p shamir-tx` → 431/431 passed. **A SECOND `@fl` review pass was just launched (as a background Agent, agentId `a91643ff24e0b1049`) to confirm the fix genuinely closes the gap and introduces nothing new — this had not yet returned when the session was interrupted for this checkpoint.** Do NOT read that agent's raw output file directly (it's a huge JSONL transcript) — wait for its background-task completion notification instead, per the tool's own warning.

**Not yet committed**: all of task #500's changes are still uncommitted in the working tree (`crates/shamir-wal/{Cargo.toml,src/lib.rs,src/segment_set.rs,src/tests/segment_set_tests.rs,benches/wal_startup_open.rs,src/segment_meta.rs}`) — waiting on the second `@fl` pass before writing the commit message and committing, per the established pipeline (never commit before the review that was launched actually returns).

Two flaky/load-sensitive tests were investigated and correctly dismissed as NOT regressions from this session's dependency-lock updates (task #498): `vr5_cofilter_sees_staged_and_filters_residual` (already known-flaky under nextest's full-suite parallelism, task #492's own territory) and `argon2id_concurrency_cap_bounds_parallel_calls` (a real-time thread-overlap measurement, self-diagnosing "workers likely did not overlap enough" under system load — filed as its own follow-up task #521 for a more robust synchronization-barrier design, since the current design relies on wall-clock timing to prove overlap).

Also mid-session: a user global-effort-level change (`/effort high`) landed via a local command block, unrelated to the campaign itself — noted for completeness, not an in-repo change.

## Active goal

`/goal`: **"решить все задачи"** (solve all tasks) — session-scoped Stop hook, ACTIVE, has fired multiple times this session pushing continued work, NOT YET satisfied (large pending backlog remains).

## TaskList

### in_progress
- #500 PERF: WAL segment-open avoid full replay-for-max-version (finding 2.1) — sidecar implementation + crash-safety fix done and independently verified by orchestrator (431/431 tests); SECOND `@fl` review pass in flight (background agent `a91643ff24e0b1049`), not yet returned; NOT YET COMMITTED

### pending
- #501 PERF: Interner segmented-spine to avoid full reverse-vec clone (finding 2.3)
- #502 PERF: investigate fjall per-op spawn_blocking overhead (finding 3.3, deferred from #490)
- #503 PERF-RADICAL-STRUCTURAL step 2: RecordKey alias cutover to KeyBytes (mechanical, no logic change)
- #504 PERF-RADICAL-STRUCTURAL step 3: alloc-free hot-path key constructors (blocked by #503)
- #505 PERF-RADICAL-STRUCTURAL step 4: sweep in-memory backend key maps for residual Bytes::copy_from_slice (blocked by #503)
- #506 PERF-RADICAL-STRUCTURAL step 5 (optional, measure-first): raise KeyBytes INLINE_CAP or add a posting-key tier (blocked by #504)
- #507 CLEANUP: fix stray backslash comment typos in read_exec.rs (found during #492 review)
- #508 TEST: add real fault-injection regression test for WalGroupCommit::append_many all-or-nothing (finding 1.6 residual)
- #509 FLAKE: oversample_higher_yields_at_least_as_many intermittent failure (found during #494 verification)
- #511 BUG: trusted_pure_scalar_backs_functional_index fails (found during #495 verification, unrelated to security diff)
- #512 SECURITY: design a correct fix for resumption-ticket channel-binding (finding 1d, reverted attempt in #495)
- #513 FIX: subscription cap slot leaks when bridge task exits on its own (found during #495 review)
- #514 FIX: SSRF guard has DNS-rebind TOCTOU + missing octal/short IP forms (found during #495 review)
- #515 PERF: MemBuffer merge-overlay scan (finding 5, deferred from #496)
- #516 PERF: fused SQ8 rescore + weighted-SIMD distance kernels (finding 4 items a/b, deferred from #496)
- #517 PERF: keyset pagination has no record-id tiebreaker for ORDER BY value ties (found during #496 review)
- #518 PERF: lookup_range_first_k can return a short page on stale index entries (found during #496 review)
- #519 CLIENT: node-binding typed error .code/.retryable (found during #497 review, needs napi-rs 3.x)
- #520 CLIENT: Rust client roundtrip has no request timeout (found during #497 review)
- #521 FLAKE: argon2id_concurrency_cap_bounds_parallel_calls intermittent failure under load (found during #498 verification)

### recently completed
- #499 PERF-RADICAL-3.2 (posting-list sorted-slice, commit eb34e955)
- #498 RUSTSEC triage (commit 07add530)
- #497 HIGH-client residual (commit 6c2297cf)
- #496 HIGH-perf residual (commit b89172da)
- #495 HIGH-security residual (commit b41a4842)
- #494 MEDIUM-durability residual (commit 83fe85f3)
- #493 bench-scale-tool migration (already-done, no commit needed)
- #492 FLAKE vr5_cofilter (commit 30d976c0)
- #491 KeyBytes SSO type step 1 (commit aff20ae8)
- #490 PERF-RADICAL-5 (commit de4aa759)

## Decisions

- Switched sub-agent pipeline mid-campaign from `/crush` + `@sh` to `@oh` (Agent tool) + `@fl` (Agent tool) per explicit user instruction — same verification discipline applies (orchestrator never trusts an agent's self-report, always independently rebuilds/retests/re-reviews).
- When `@fl` finds a genuine crash-safety or correctness bug (not a style nit or accepted trade-off), the orchestrator fixes it directly and sends the FIX through a second `@fl` review pass before committing — never re-delegates the fix itself. This has now happened twice this session (task #494's WAL replay-path wiring bug, task #500's stale-sidecar-on-reactivation bug).
- For RUSTSEC advisory remediation (task #498), the user drew an explicit line: lock-only `cargo update -p <crate>` within already-permitted Cargo.toml semver ranges is allowed without extra confirmation; any manifest-level version-requirement change (e.g. `scc` needing a 2.x→3.x major bump) is NOT allowed in this pass and must be documented + deferred instead.
- Load-sensitive test failures (timing-based thread-overlap tests, full-suite-parallelism-only timeouts) are investigated by re-running in isolation before being accepted as flakes — never dismissed on a hunch. Two more confirmed this session (argon2id concurrency test, vr5_cofilter recurrence).
- Structural/high-complexity audit findings continue to get an investigation-first treatment (write a design doc or at minimum investigate real consumer usage before implementing) rather than jumping straight to the audit's own suggested fix — this has twice now revealed the audit's stated premise didn't match the actual codebase (task #499's non-existent BTreeSet-intersection hot path; task #500's overturned "format bump required" deferral).

## Open questions

- Task #500: will the second `@fl` review pass return SHIP IT, or find something else? Needs to be checked (via the background-agent completion notification, NOT by reading its raw output file) before writing the commit message.
- The large pending backlog (#501-#521, ~20 tasks) has no re-prioritization signal from the user yet — proceeding in numeric order unless redirected.
- Whether task #502 (fjall spawn_blocking) will need the same kind of "investigate first" treatment given its audit-rated "средняя-высокая" (medium-high) complexity, similar to how #499/#500 both had their audit premises partially overturned by investigation.

## Repo state

```
 M CLAUDE.md
 M bench-iters.txt
 M crates/shamir-wal/Cargo.toml
 M crates/shamir-wal/src/lib.rs
 M crates/shamir-wal/src/segment_set.rs
 M crates/shamir-wal/src/tests/segment_set_tests.rs
?? crates/shamir-wal/benches/wal_startup_open.rs
?? crates/shamir-wal/src/segment_meta.rs
(+ many stray *.log files in repo root from gate runs — untracked, not part of any commit, pre-existing clutter from earlier sessions)
```

```
4fcdcc44 docs(prompts): brief for #500 WAL segment max_version footer/sidecar
eb34e955 perf(index): posting-list representation Arc<BTreeSet> -> Arc<[RecordId]> (audit 3.2)
3cfbc2eb docs(prompts): brief for #499 PERF-RADICAL-3.2 posting-list sorted-slice investigation
07add530 chore(deps): patch 4 of 13 RUSTSEC advisories via lock-only updates (task #498)
6c2297cf fix(client): wire-type drift + executeWithTouch parity + error typing + timeouts (audit 1.3/1.4/2.1/2.2)
```
