# Checkpoint — 2026-07-21 21:xx [fg3-in-flight]

## Session summary

Continuation of the long `/babygoal`-driven campaign ("реализуй задачи с помощью `/crush`, между задачами делай коммиты, покрой их тестами. Если /crush войдет в пиковые часы, то переключайся на агентов @sh"). This window worked through the entire release-infrastructure series (RI-1…RI-12, all done) and is now deep into the functional-correctness series (FG-1…FG-4/FG-5). Every task followed the established discipline: investigate → write a precise brief to `docs/dev-artifacts/prompts/correctness/` or `.../release/` → commit the brief → delegate (crush, or `@sh` on refusal/crash/quota) → zero-trust re-verify (full diff read + independent re-run of fmt/clippy/tests, never trusting the delegate's own envelope) → commit → mark task complete.

**RI-1 through RI-12 are DONE, verified, committed.** Highlights: RI-7 (actor-threading through validators/nested WASM) recovered from two transient crush crashes into the same session. RI-8 (resource profiles) found and fixed a real latent ktav-parse bug as a bonus. RI-9 (bootstrap-token lifecycle) discovered a SEPARATE, fully-built-but-never-wired challenge-response bootstrap wire protocol in `shamir-connect` and correctly left it undocumented-as-roadmap rather than wiring it up (out of scope). RI-10 (replication) reused the existing `subscriptions.state` field for a new `resync_required` signal with zero new API surface. RI-11 (restore) discovered fjall's own OS-level file lock already provides the "is the server running" check for free — no PID-file mechanism needed. RI-12 (README positioning) discovered the flagged over-claim no longer exists in the current tree (likely already fixed in an earlier session) — added the requested honest positioning section anyway since it was still missing. `/crush` hit a genuine peak-hours refusal (provider `zai`, 08:00–12:00) partway through this window — per the user's explicit `/goal` addendum, fell back to `@sh` (Sonnet-high) specifically, not the crush skill's own default fallback aliases. `/crush` later recovered and ran RI-10 forward, but by FG-1 it started hitting transient provider crashes (recovered into the same session 2-3× each time) and finally, mid-FG-2, hit a **hard weekly/monthly quota exhaustion** (resets 2026-07-23) — confirmed by the user's own message reinforcing "when crush runs out of limits, switch to @sh agents," so the campaign has been running on `@sh` exclusively since partway through FG-2.

**FG-1 (u64→Big promotion contract) is DONE, verified, committed** (`85cd5afe`). Fixed 6 real sites (not just the 2 the review named) including both Rust AND TS query-builder `lit_u64`/`litU64` helpers, which explicitly mirrored each other's OLD wrapping bug in their own doc comments. The brief's own mandatory empirical verification step (an honest "STOP and report if it doesn't work" escape hatch, deliberately built into every FG brief this window) found a REAL structural gap: an `Eq` filter via `lit_u64` against a promoted `Big` value does NOT match, because `RecordRef::scalar_at`/`ScalarRef` has no path for either an owned-`Cow` decimal string (lens) or a `Big` variant (tree). Filed as **task #750 (FG-6)**, not folded into FG-1, not blocking #742 by default.

**FG-2 (`with_version` full CAS contour) is DONE, verified, committed** (`c08c4f86`) — the largest task so far, took 4 `/crush` attempts (3 transient crashes recovered into the same session, then the quota exhaustion) plus one `@sh` continuation to finish. Delivered: `QueryResult.versions: Option<Vec<u64>>` threaded through all 19 `QueryRecord::Direct` push sites in `read_exec.rs` (every site either got a real version or an explicit documented exception); `UpdateOp`/`DeleteOp.expected_version` implementing a two-step CAS hybrid (immediate `MvccStore::version_of` check + `TxContext::record_read_shared` registration reusing the EXISTING SSI contour, not a duplicate mechanism); a genuine concurrent-CAS test (real `tokio::spawn` race, both at the engine level and through a real server e2e) proving "exactly one wins" — 5/5 reliable runs. The mandatory verification again found a real gap: `record_read_shared` (the CAS hybrid's SSI backstop) is a no-op under `Snapshot` isolation, and `RepoInstance::run_implicit_batch_tx` (the path EVERY plain non-transactional write uses) is hardcoded to `Snapshot` — so `expected_version` on an ordinary non-transactional write gets ONLY the immediate check, not the race-window backstop, and two concurrent non-tx writers CAN both succeed (empirically confirmed reproducible). Full CAS safety requires wrapping in `.transactional().isolation(Serializable)` — documented precisely in a new `OPTIMISTIC_CONCURRENCY.md` and in the test's own doc comment. Filed as **task #751 (FG-7)**.

**FG-3 (tx-scan read-your-own-writes) is IN FLIGHT right now** — the largest, deepest task of the whole campaign (core engine, MVCC/staging internals). Per the user's own instruction ("это ядро engine — сначала investigate"), I did the full architectural investigation myself before writing the brief (`docs/dev-artifacts/prompts/correctness/08-tx-scan-ryow-overlay.md`, committed `69e3a71c`) rather than delegating the design. Key findings baked into the brief: (1) a genuinely reusable 2-way sorted-merge overlay pattern already exists (`merge_overlay_stream` in `shamir-storage/src/storage_membuffer.rs`, task #530) and is the algorithm template; (2) the overlay source (`StagingStore::snapshot_ops`/`keys`/`staged_op`, `RecordKey`'s existing `Ord` impl) already exists and is already used by the point-read RYOW path (`read_one_tx`); (3) a SECOND, NOT-in-the-original-review gap: `execute_update_tx`/`execute_delete_tx` each have their OWN bespoke inline match-scan (index-path arm + list_stream-fallback arm) that does NOT route through `list_stream_tx`/`filter_stream_tx` at all — so the fix needs 6 integration points total (2 general streaming methods + 4 write-path match-scan arms), not just the 2 the review's C8 test example pointed at. The brief mandates the overlay merge stay strictly downstream of the EXISTING SSI-recording logic (`record_scan_reads`, left untouched) and that filtered scans re-evaluate the filter against overlay-injected rows (a staged row's new value may not match a filter its old value did). Delegated to `@sh` (crush quota still exhausted). **Current literal state**: the `@sh` sub-agent (id `a86db451ccb9f95c5`) is actively producing files — `git status` shows a new `crates/shamir-engine/src/table/tx_scan_overlay.rs` (the shared merge primitive) and `tx_scan_overlay_tests.rs`, plus modifications to `table_manager_streaming.rs`, `write_exec.rs`, `table/mod.rs`, and the `stream_tx_tests.rs` test file (presumably the C8 flip) — this is IN-PROGRESS, NOT yet zero-trust-verified or committed.

## Active goal

A `/goal` Stop hook is armed with this exact condition text (confirmed still binding this window, reinforced by an explicit user message mid-session: "когда у /crush кончатся лимиты, то переходи на агентов @'sh (agent)'"):

> реализуй задачи с помощью /crush , между задачами делай коммиты, покрой их тестами. Если /crush войдет в пиковые часы, то переключайся на агентов @sh

The user's reinforcement message generalizes the fallback trigger from "peak hours" specifically to ANY exhaustion of `/crush`'s capacity (peak-hours refusal, transient crash exhaustion, OR hard quota limits) — all three have now actually occurred this window, and in each case the fallback to `@sh` was followed correctly.

## TaskList

### in_progress
- #745 FG-3. Tx-scan read-your-own-writes: полный overlay staged write_set в потоковые сканы + флип C8-теста (blockedBy: none — blocks #742, #746)

### pending
- #742 RI-13. Frozen commit: полный локальный gate + push + зелёный удалённый CI (blockedBy: #745, #746, #747, #748, #749[no—#749 is post-alpha, does not block], #750, #751 — needs re-check of exact blockedBy list via TaskGet; only user may authorize the actual push/tag)
- #746 FG-4. KNOWN_LIMITATIONS.md (blockedBy: #739[done],#743[done],#744[done],#745 — waiting on FG-3)
- #747 FG-5 (пост-альфа). Server-side cursors / streaming результатов — explicitly deferred past alpha gate, does not block #742
- #748 RI-14. Update 2 stale TS e2e tests — blocks #742
- #749 RI-15 (пост-альфа). Глобальный inflight response-memory budget — deferred, does not block #742
- #750 FG-6. scalar_at/ScalarRef: Eq-фильтр и ORDER BY не видят Big-значения — found during FG-1 verification, not yet triaged as blocking or non-blocking #742
- #751 FG-7. expected_version на non-transactional write не защищён SSI backstop — found during FG-2 verification, not yet triaged as blocking or non-blocking #742

### recently completed
- #741 RI-12, #740 RI-11, #739 RI-10, #738 RI-9, #737 RI-8, #736 RI-7, #735 RI-6, #734 RI-5, #733 RI-4, #732 RI-3, #731 RI-2, #730 RI-1 (entire release-infrastructure series)
- #743 FG-1 (u64→Big promotion)
- #744 FG-2 (with_version CAS contour)

## Decisions

- **`/crush` fallback trigger generalized**: peak-hours refusal, transient mid-turn provider crashes (recovered into the SAME session, never a fresh session), AND hard weekly/monthly quota exhaustion all route to `@sh` specifically — confirmed by the user's own reinforcement message this window, not just my own inference.
- **FG-6 and FG-7 (both real structural gaps found via each brief's own mandatory "STOP and report if it doesn't work" verification step) were NOT folded into FG-1/FG-2's commits** — filed as independent, dedicated follow-up tasks instead, matching this campaign's established discipline (same pattern as RI-14/RI-15 from the release-infra series). Neither has been explicitly triaged yet as blocking vs. non-blocking #742 — this is an open item, see below.
- **FG-1's filter-literal fix reuses the EXISTING `QueryValue::Big`↔`Str` cross-type equality bridge** (a decimal string, not a new `FilterValue::Big` wire variant) — deliberately avoided a bigger wire-schema change.
- **FG-2's CAS validation reuses the EXISTING SSI read-set contour** (`TxContext::record_read_shared`/`validate_read_set`) rather than building a second revalidation mechanism — per the user's own "не дублировать" instruction.
- **FG-3's architecture was investigated and decided by the orchestrator BEFORE delegating** (not left to the delegate), per the user's explicit "engine core, investigate first" instruction — the brief locks in the exact merge algorithm, the exact 6 integration points, and the exact overlay/filter interaction rules.
- **RI-12's "removal" work turned out to be unnecessary** (the flagged over-claim doesn't exist in the current README) — the task was completed by ADDING the requested positive positioning content instead, done directly (not delegated) as a small, single-file, low-risk edit, matching the earlier RI-2 precedent for trivial fixes.

## Open questions

- **FG-6 and FG-7's blocking status relative to #742 (RI-13, frozen commit) has not been explicitly decided.** Both are real, honestly-surfaced correctness gaps in features the user explicitly wanted "fully delivered, not hidden" (FG-1's u64 contract, FG-2's CAS contour) — an argument could be made either way: (a) they don't block, since the CORE deliverable (no silent corruption / a working CAS proof under the documented usage pattern) is done and verified, matching the FG-5/RI-15 "follow-up, not blocker" precedent; or (b) they DO block, since the user's own framing ("довести целиком, не прятать флаг" for FG-2, "the largest of the four options... the only semantically-correct one" for FG-3) suggests a low tolerance for partial delivery on these specific features. This has not been raised to the user yet — worth surfacing explicitly once FG-3 lands, alongside whatever FG-3's own verification turns up (which may add a THIRD such gap to the same open question).
- No other outstanding questions requiring a stop — FG-3 is mechanically in flight (`@sh` agent alive) and the rest of the TaskList is a clear, already-decided backlog.

## Repo state

```
 M crates/shamir-engine/src/table/mod.rs
 M crates/shamir-engine/src/table/table_manager_streaming.rs
 M crates/shamir-engine/src/table/tests/mod.rs
 M crates/shamir-engine/src/table/tests/stream_tx_tests.rs
 M crates/shamir-engine/src/table/write_exec.rs
?? crates/shamir-engine/src/table/tests/tx_scan_overlay_tests.rs
?? crates/shamir-engine/src/table/tx_scan_overlay.rs
?? docs/checkpoints/ (this file + prior-session checkpoints, untracked)
```

(All of the above is the FG-3 `@sh` sub-agent's IN-PROGRESS, NOT-YET-VERIFIED output — do not trust it until the diff is read end-to-end and the full gate is independently re-run, per this campaign's zero-trust discipline.)

```
69e3a71c docs(prompts): brief for FG-3 -- tx-scan read-your-own-writes full overlay
c08c4f86 feat(engine,query-types,query-builder,ts-client): with_version full CAS contour (FG-2)
7629601f docs(prompts): brief for FG-2 -- with_version full CAS contour
85cd5afe fix(types,query-types,query-builder,ts-client): unified u64->Big promotion contract (FG-1)
e34ce958 docs(prompts): brief for FG-1 -- unified u64->Big promotion contract, all levels + both builders
8c2a3d92 docs(readme): honest positioning statement + comparison table (RI-12)
91e7d266 feat(server): restore command with manifest checksums + ticket invalidation (RI-11)
4e57244b docs(prompts): brief for RI-11 -- restore command, manifest checksums, ticket invalidation
```

## Active timers

A babysit cron (job id `53635720` as of the last known check — re-verify via `CronList` at the start of any new session, since job ids rotate and crons auto-expire after 7 days) has been ticking every 15 minutes throughout this entire window (spanning the RI series into the FG series), correctly reporting "still running #<current-task-id>" on each tick while whichever crush/`@sh` session is active. A `/goal` Stop hook is ALSO armed (see Active goal above) — both mechanisms watch the same underlying TaskList-completion condition from different angles. `/crush`'s own weekly/monthly quota resets 2026-07-23 — worth re-attempting `/crush` first (per the standing "never skip crush proactively" rule) on any NEW task started after that date, rather than defaulting straight to `@sh`.
