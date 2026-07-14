Task: CLEANUP — fix stale/misleading doc comments identified by the
2026-07-06 audits (durability §3, concurrency §2-3) and remove
confirmed-dead code. This is a DOCUMENTATION + surgical dead-code-removal
task — no behavioral changes to live code paths.

⚠️ Scope discipline: fix ONLY the specific items listed below. Do NOT
attempt the larger architectural items mentioned in the audit sections
(merging `pre_commit_locked`/`pre_commit_locked_validate`, extracting a
shared `SingleFlightGuard`, removing the dead `group_commit.rs`
mechanism, fixing `bump_write_counter`'s panic-safety gap) — those are
separate, higher-risk tasks tracked independently. If you find yourself
touching logic beyond a doc-comment correction or a genuinely-dead
(zero-caller) function/field removal, STOP and note it in your report
instead of doing it.

## Doc-comment corrections (durability audit, Section 3)

For each item, read the CURRENT surrounding code first (line numbers
may have drifted since 2026-07-06) to confirm the comment is still
present and still stale, then correct the comment to describe the
ACTUAL current behavior:

1. **`crates/shamir-wal/src/lib.rs:33-55`** (confirm current lines) —
   the module-level doc describes a REMOVED KV-marker design
   (`__wal_active_` in info_store, "marker removed after"). Production
   uses file segments with no such markers (F5c/F6 cutover). Rewrite
   this module doc to describe the ACTUAL current file-segment design.
2. **`crates/shamir-engine/src/tx/drainer.rs:33-42`** (confirm current
   lines) — comment says "Scope of P1d-2a — additive, NOT wired...
   cutover is P1d-2b" and describes replay via `replay_v2_entry`/
   "truncate the inflight marker" — stale; the drainer has long been
   wired as the sole history-writer, with a Phase A/B/C structure.
   Update to describe the CURRENT flow.
3. **`crates/shamir-wal/src/wal_segment.rs:15`** (confirm current line)
   — "Wired to nothing yet" is false; this is a live production path.
   Correct the comment.
4. **`crates/shamir-tx/src/repo_tx_gate.rs:554-560`** (confirm current
   lines) — "under the current inline-materialize path this equals
   last_committed... P1d-2 will decouple" — the decoupling already
   happened; update to reflect the current (already-decoupled) state.
5. **`mark_durable`'s naming/semantics** — the function actually means
   "written to history (page cache)", not "durable on disk" (real
   fsync only happens at the truncation gate). Do NOT rename the
   function (a rename is a larger, riskier refactor touching every call
   site) — instead, add/correct an explicit doc comment on
   `mark_durable`'s definition (`crates/shamir-tx/src/repo_tx_gate.rs`,
   search `fn mark_durable`) stating this precise contract, so future
   readers don't assume disk-durability from the name.
6. **`crates/shamir-engine/src/table/interner_manager.rs:253-263`**
   (confirm current lines) — `persisted_high_water`'s doc claims
   "durably persisted to the chunk store" — untrue (it's RAM+buffer
   until an actual flush; see the codebase's own tracking of this
   class of issue). Correct the doc to state the ACTUAL guarantee
   (matches whatever the current, already-fixed-by-other-audits
   behavior is — check recent A8/A11 fixes in this same file/area for
   the current accurate contract before writing the correction).
7. **`crates/shamir-engine/src/tx/recovery.rs:409-412`** (confirm
   current lines — NOTE: this file was recently modified by the A11
   fix, so line numbers have shifted; find the actual comment by
   searching for text like "leaving a partial history write inflight
   is safe") — "the WAL marker is untouched" is false post-F6/CRIT-1
   fixes; correct to describe the actual current safety argument (or
   remove the claim if it's simply no longer applicable).
8. **`crates/shamir-engine/src/tx/pre_commit.rs:488-491, 518-521`**
   (confirm current lines) — "failed wal.begin ⇒ nothing durable" is
   incorrect in the case of a partial window write (per the audit's
   1.6 finding elsewhere in the durability doc — do NOT fix 1.6 itself
   here, only correct THIS comment's claim to be accurate about the
   partial-write case, or note explicitly that this claim assumes 1.6
   is not yet fixed if that's still true).
9. **`crates/shamir-tx/src/repo_wal_manager.rs:62-63`** (confirm
   current lines) — "cancel-safe: yes" for `begin_grouped` is
   technically true (the future parks) but semantically misleading —
   cancellation does NOT undo the append; the entry becomes durable
   and resurrects on restart while the caller believes it was
   cancelled. Expand the comment to state this precisely (the
   `commit_tx` code already documents this honestly elsewhere — mirror
   that phrasing).

## Doc-comment corrections (concurrency audit, Section 2)

10. **`crates/shamir-tx/src/layered_interner.rs:163-165`** (confirm
    current lines) — "Must be called under RepoTxGate::commit_mutex —
    no internal synchronisation" is stale: in the current P2c/lock-free
    path it's called in `pre_commit_prelock` OUTSIDE the mutex,
    correct only because `touch_ind`'s CAS is idempotent (a fact
    nowhere stated as a contract). Correct the comment to state the
    ACTUAL safety argument (CAS-idempotency of `touch_ind`), not a
    false mutex requirement.
11. **`crates/shamir-tx/src/repo_tx_gate.rs:495-497`**
    (`predicate_conflicts_batch`, confirm current lines) and
    **`crates/shamir-tx/src/staging_store.rs:157`** (confirm current
    line) — both claim "Called ... UNDER commit_lock" — the lock-free
    commit path does NOT do this. Correct both comments to state the
    actual current calling contract (which paths hold the lock, which
    don't, and why it's still safe).
12. **`crates/shamir-tx/src/repo_tx_gate.rs:62-70`** (confirm current
    lines) — describes `commit_mutex` as serializing "the commit
    section" broadly — actually only the AsyncIndex path takes it.
    Correct to state this precisely.
13. **`crates/shamir-engine/src/tx/commit.rs:158-160`** (HEAD, confirm
    current lines) — "`is_empty()` is lock-free on scc::HashMap (atomic
    length check)" — `scc` has NO atomic length (that's why `len()` is
    banned by `clippy.toml`'s `disallowed-methods`); `is_empty` walks
    buckets until the first entry. Correct the comment to describe the
    ACTUAL cost/behavior (bucket-walk, not an atomic check).
14. **`crates/shamir-tx/src/mvcc_store/mvcc_history.rs:242-243`**
    (confirm current lines) — "`upsert_async` ... advances
    monotonically rather than silently keeping a stale value" is false:
    `upsert_async` unconditionally overwrites, including backward (this
    was exactly A2's bug, already fixed elsewhere in this campaign —
    confirm the CURRENT code's actual monotonicity guarantee, likely
    now provided by explicit max-comparison logic added by the A2 fix,
    and correct this stale comment to describe what ACTUALLY provides
    monotonicity now).
15. **`crates/shamir-tx/src/repo_tx_gate.rs:391-399`** and
    **`crates/shamir-engine/src/table/table_manager_changefeed.rs:36`**
    (confirm current lines) — the justification for a Relaxed read of
    `active_serializable_count` ("tx opened AFTER the write committed")
    does not actually hold under the "floor read → counter increment"
    vs "version assigned after counter check" interleaving described in
    the audit. The audit notes the CONSEQUENCE is bounded (T-before-W
    serial order remains valid — a blind write) but the COMMENT's
    justification is wrong. Correct the comment to state the accurate
    justification (bounded consequence via blind-write serial
    ordering), rather than the incorrect one. Do NOT change the actual
    Relaxed-read code/add a Dekker-style re-check — that's a separate,
    riskier fix if ever needed; this task is comment-only here.
16. **`crates/shamir-engine/src/repo/repo_instance.rs:70-72`** (confirm
    current lines) — "a lost race just drops a redundant un-spawned
    Drainer" describes a race that cannot happen:
    `std::sync::OnceLock::get_or_init`'s closure runs EXACTLY once by
    construction. Correct/remove the comment describing the
    non-existent race.

## Confirmed-dead code removal (verify zero callers before removing)

17. **`crates/shamir-tx/src/repo_tx_gate.rs`, `publish_committed`**
    (the PLAIN, non-monotonic setter — confirm current line, search
    `pub fn publish_committed(` distinct from
    `publish_committed_max`) — per the audit, this has NO live callers
    on the hot path anymore (superseded by `publish_committed_max`
    everywhere). **Before removing**: grep the ENTIRE workspace for
    `.publish_committed(` (not `_max`) to confirm zero call sites
    (including tests). If genuinely zero, remove the function. If any
    caller remains (even in tests), do NOT remove — report what you
    found instead.
18. **`crates/shamir-engine/src/table/interner_manager.rs`,
    `InternerManager::save_new_keys`** (confirm current name/location)
    — per the audit, this is currently a dead API (0 callers) that
    would be DANGEROUS if ever called (advances `last_persisted_len`
    without checking id density, which could prematurely satisfy the
    A5 truncate gate). **Before removing**: grep the entire workspace
    for its usage to confirm zero callers (including tests). If
    genuinely dead, remove it entirely (per the audit's own
    recommendation: "delete or lock down with an assert" — prefer
    DELETE since it's confirmed dead and removal is simpler/safer than
    adding an assert to code nobody calls). If any caller exists,
    report it and do NOT remove — instead, note the finding for a
    follow-up assert-based fix.
19. **`crates/shamir-tx/src/versioned_overlay.rs`, `gc_upto(durable,
    floor)`** (confirm current signature/location) — the audit notes
    the SECOND parameter is ALWAYS `u64::MAX` at every call site (see
    `gc_overlay_to`'s usage), making the min-logic dead. **Before
    changing**: grep for every call site of `gc_upto` to confirm this
    is genuinely true everywhere. If confirmed, simplify the signature
    to drop the now-always-`u64::MAX` parameter (update all call sites
    accordingly) — this is a small, mechanical signature simplification,
    not a behavior change (since the parameter's value was constant).
    If ANY call site passes something other than `u64::MAX`, do NOT
    simplify — report what you found.

## What NOT to do (explicitly out of scope for this task)

- Do NOT extract a shared `SingleFlightGuard` pattern (concurrency §3
  item 6) — separate task.
- Do NOT remove the dead `group_commit.rs` mechanism (concurrency §3
  item 1) — separate task, and per this campaign's history it may
  already be scheduled for a future cutover (P1d-2b) rather than
  deletion; do not touch it.
- Do NOT merge `pre_commit_locked`/`pre_commit_locked_validate`
  (concurrency §3 item 3) — separate, higher-risk task.
- Do NOT fix `bump_write_counter`'s panic-safety gap (concurrency §2
  item 1) — separate task (needs a Drop-guard pattern, real logic
  change, not cleanup).
- Do NOT fix `RecordCounter::persist`'s dirty-flag masking (concurrency
  §2 item 4) — separate task (real logic fix, not cleanup), even
  though the audit calls it "trivial" — keep this task doc-only +
  confirmed-dead-code-only.
- Do NOT touch the fire-and-forget interner-checkpoint spawns
  (concurrency §2 item 5) or `bm25.rs`'s underflow risk (concurrency §2
  item 7) — separate tasks.

## Verification requirement (no TDD needed — this is docs + dead code)

Since this task changes NO live behavior (doc comments + provably-dead
code with zero callers), there is no Red/Green cycle. Instead:
1. For every doc-comment correction, show a brief before/after diff
   snippet in your report so the correction can be spot-checked for
   accuracy.
2. For every proposed dead-code removal, show the EXACT grep command
   and its output confirming zero callers, before removing.
3. Run the full existing test suite for every touched crate to confirm
   NOTHING broke (a genuinely-dead removal should never break a test;
   if it does, the code wasn't actually dead — revert that specific
   removal and report it).

## Test scope command

```
./scripts/test.sh -p shamir-tx
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-wal
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-tx -p shamir-engine -p shamir-wal -- --check
cargo clippy -p shamir-tx -p shamir-engine -p shamir-wal --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- For each of the 16 doc-comment items (1-16): confirm it still
  existed as described, what you changed it to (brief before/after),
  or note if the underlying code had already changed enough that the
  comment no longer applies / was already fixed by a prior task in
  this campaign (in which case, state that and move on without forcing
  a change).
- For each of the 3 dead-code items (17-19): the exact grep command
  used, its output, and whether the removal/simplification was
  performed or skipped (with reason if skipped).
- Full test/gate results (exact commands + pass/fail) for every
  touched crate.
- A list of anything from the audit's §2/§3 sections you deliberately
  did NOT touch (per the "what NOT to do" list above), for the record.
