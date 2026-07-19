# Deadlock hazard — `LayeredInterner` overlay mixes scc `_async`/`_sync` lock acquisition (hygiene-only, per-tx map)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing. This has caused repeated
problems in prior sessions and must not happen again.

## Background — read the three prior commits in this same sweep first

`git show 7a4abf62` (`#589` `cells`-map fix) and the two already-landed
commits in this task series (search `git log --oneline --grep "H1+H2"` and
`--grep "H3"` and `--grep "H4+H5"`) establish the mechanism and doc-comment
style. Short version: `scc::HashMap`'s `_async` lock-wait is a lock-HANDOFF
(the bucket lock is granted directly to a suspended waiter task, which then
holds it while sitting in tokio's run queue until re-polled); `_sync`
accessors instead park the calling OS thread while the bucket is locked.
Mixing the two on one map risks a whole-runtime deadlock under the right
interleaving.

**This is the LOWEST-priority and LOWEST-risk fix in this sweep** — read
the risk assessment below carefully before starting; this is a hygiene /
future-proofing change, not an active production hazard.

## The site — `crates/shamir-tx/src/layered_interner.rs`

`LayeredInterner::Layered { overlay, .. }` wraps `overlay:
scc::HashMap<String, u64, THasher>` — this is `TxContext::interner_overlay`
(`crates/shamir-tx/src/tx_context.rs:134`), i.e. **one instance per
transaction**, holding that tx's newly-minted (name → id) mappings until
commit.

- `touch` (async, `layered_interner.rs:46-70`) — uses
  `overlay.entry_async(key.to_string()).await` (~line 61). **Grep the
  codebase**: this async `touch` is called ONLY from
  `crates/shamir-tx/src/tests/layered_interner_tests.rs` — confirm this
  yourself (`grep -rn '\.touch(' crates/ | grep -v touch_sync`) — no
  PRODUCTION call site uses it directly today.
- `touch_sync` (sync, `layered_interner.rs:78-...`) — uses
  `overlay.entry_sync(key.to_string())` (~line 93). THIS is what production
  code actually calls: `crates/shamir-engine/src/table/write_helpers.rs:348`
  and `crates/shamir-engine/src/table/write_exec.rs:903`.
- `get_id` (async, `layered_interner.rs:113-123`) — uses
  `overlay.read_async(key, |_, v| *v).await` (~line 121). Called from
  PRODUCTION code: `crates/shamir-engine/src/validator/validator_db.rs:363`.
- `get_str` (sync, `layered_interner.rs:131-...`) — uses
  `overlay.iter_sync(...)` (~line 142) for the reverse (id → string) scan
  when `id >= OVERLAY_ID_BASE`.
- `commit_interner_overlay` (async, `layered_interner.rs:198-...`) — uses
  `overlay.iter_async(...)` (~line 206) at commit time.

So today: `touch_sync` (production writes) and `get_str` (production
reverse lookups) are already sync; `get_id` (production reads, called from
the validator path) and `commit_interner_overlay` (commit-time drain) are
async; `touch`'s async form is test-only.

## Why this is downgraded to hygiene (read this before "fixing" it as if it were HIGH risk)

The overlay map is **per-`TxContext`** — one instance exists per
transaction, accessed by operations belonging to THAT tx. A tx's operations
(writes via `touch_sync`, validator reads via `get_id`, then the Phase-1
merge via `commit_interner_overlay`) execute SEQUENTIALLY on one logical
task's timeline for that tx — there is no current call path where two
DIFFERENT tasks concurrently touch the SAME `TxContext`'s
`interner_overlay` instance. **No plausible `#589`-style cross-task
interleaving exists TODAY.** This is a landmine for FUTURE intra-tx
parallelism (e.g., a future feature that runs multiple batch ops within one
tx concurrently, sharing one `TxContext`) — fix it for convention
consistency and future-proofing, not because it is an active production
risk. Do not write test code that tries to force a "regression" here the
way the H1-H5 tests did — there is no realistic interleaving to exercise
(the map is genuinely single-task-at-a-time today), so an elaborate stress
test would be theatre, not signal.

## The fix

1. Convert `get_id`'s `overlay.read_async(...).await` to
   `overlay.read_sync(...)` — matches `get_str`'s already-sync convention
   on the same map, and per this workspace's established rule ("every
   lock-acquiring op on a map that has ANY synchronous accessor must
   ITSELF be synchronous"), closes the mixing. The closure is a plain
   dereference — no suspension, trivially safe to convert.
2. `touch`'s async form: since it has NO production call site (test-only),
   the cleanest fix is to make its internals delegate to the sync entry
   accessor too — i.e. change its body to use `overlay.entry_sync(...)`
   instead of `overlay.entry_async(...).await` (mirroring `touch_sync`'s
   own body almost exactly). Do NOT delete the async `touch` function
   itself (tests call it, and removing a used test helper is out of
   scope) — just make its internal lock acquisition synchronous, same
   shape as the `get_id` fix. If, on reading the two function bodies side
   by side, `touch` and `touch_sync` end up doing IDENTICAL work once both
   use `entry_sync`, consider (your judgement) whether `touch` should
   simply become a thin `async fn touch(&self, key: &str) -> u64 { self
   .touch_sync(key) }` wrapper instead of duplicating the entry-acquire
   logic — either is acceptable, prefer whichever is less code duplication
   without changing either function's public signature (both must remain
   present since tests call `touch` specifically).
3. `commit_interner_overlay`'s `overlay.iter_async(...)` — this runs
   EXACTLY ONCE per tx, at commit time, after all of that tx's own writes/
   reads have already completed (nothing else touches this specific
   `TxContext`'s overlay concurrently with commit) — convert it to
   `iter_sync` for full consistency with the rest of this map's accessors,
   even though the commit-time single-shot nature makes it especially low
   risk either way.
4. Add a "DEADLOCK FIX (same class as #589, commit `7a4abf62`)"-style doc
   comment at each converted site, but ALSO note explicitly (unlike the
   H1-H5 comments) that this map is per-`TxContext`/effectively
   single-task today — this is a convention/future-proofing fix, not a
   fix for an active reachable hazard — so a future reader doesn't
   over-read the urgency. Mirror the established comment STYLE from the
   prior three commits but adjust the RISK framing honestly for this site.

## Stale comment cleanup (unrelated map, same report section, cheap to fix while here)

`crates/shamir-tx/src/mvcc_store/mvcc_history.rs:459-460` has a stale
comment: `"Update the in-memory cell for every touched key (CRIT-2:
entry_async modify-or-insert)."` — the code immediately below this comment
(around line 480) already calls the SYNCHRONOUS `finalize_reservation`
(itself already fixed to be sync, with its own comment "finalize_reservation
is synchronous (scc `entry`, no I/O)"). The `entry_async` reference is
comment-only drift from an EARLIER fix that already landed correctly in the
code — just the comment text above it never got updated. Fix: reword the
stale sentence to describe what the code ACTUALLY does now (synchronous
`finalize_reservation`), removing the incorrect `entry_async` reference.
This is NOT a functional change — purely a comment correction.

## Tests

Given the hygiene-only nature (see risk section above), do not force an
elaborate concurrency stress test. Instead:

1. Existing `crates/shamir-tx/src/tests/layered_interner_tests.rs` tests
   must continue to pass UNCHANGED (they exercise `touch`/`touch_sync`/
   `get_id`/`get_str`/`commit_interner_overlay` — confirm none of their
   assertions depended on the async/sync distinction itself, only on the
   returned VALUES, which this fix does not change).
2. If you judge it worthwhile, add ONE small test (or extend an existing
   one) documenting WHY the conversion was made — e.g. a comment-level
   note in the test module, or a trivial test asserting `touch` and
   `touch_sync` now behave identically (same id allocation semantics) —
   this is optional polish, not a hard requirement, given there is no
   regression risk to pin.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh @tx @engine --full` green (check `scripts/test.sh`'s
  `scope_args` for the exact scope names if these don't match; report
  which scope you actually used).
- `cargo fmt --all -- --check` clean (or scoped to `shamir-tx`, report
  which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) `get_id`, `touch`, and `commit_interner_overlay`
  no longer use any `_async` accessor on `overlay`, (b) no existing test's
  observable behavior changed (this is a pure lock-mechanism conversion on
  an uncontended-today map), (c) the `mvcc_history.rs:459-460` comment now
  accurately describes the synchronous `finalize_reservation` call it
  precedes.

**After this task lands, the ENTIRE H1-H6 finding set from the
2026-07-17 concurrency-deadlock-hazards research report is closed** (H1+H2,
H3, H4+H5 already landed in this same sweep; this is H6, the last).

## Out of scope

- Do NOT touch `MvccStore::cells` (#589), `RepoTxGate::active_snapshots`/
  `MvccStore::locks` (H1+H2), the vector-index maps or `rid_map` (H3 and
  its follow-up task), or `per_table_mvcc`/`token_names` (H4+H5) — all
  already fixed in prior commits of this same sweep.
- Do NOT invent a concurrency stress test for `layered_interner.rs` that
  doesn't correspond to any REAL current call path — per the risk section
  above, no cross-task interleaving exists today; a test that artificially
  forces two tasks to share one `TxContext`'s overlay would not reflect
  reality and would be misleading, not a genuine regression guard.
- Do NOT raise any test timeout to paper over a hang — if you observe a
  real hang during testing (unlikely given this site's low risk, but if it
  happens), root-cause it rather than loosening a timeout.
