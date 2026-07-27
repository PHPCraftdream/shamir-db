# Brief for F-40 (#848, P1) — FK footprint/isolation discovery must fail CLOSED; scope the explicit-Snapshot gap

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

A readonly review (`docs/dev-artifacts/research/2026-07-27-new-wave-readonly-review.md`,
finding P1-1) raised two DISTINCT concerns bundled under one finding.
**Read `crates/shamir-engine/src/query/batch/query_runner.rs`'s
`require_footprint_if_fk_child` (~line 289-334) and
`implicit_tx_isolation_for_fk_parent` (~line 345-395, post-F-35) in full
first** — this task fixes concern (1) precisely, and produces a DECISION
MEMO (not a rushed implementation) for concern (2).

### Concern 1 (MANDATORY fix): fail-open discovery

**Confirmed by reading the code**:
- `require_footprint_if_fk_child`: on `resolver.resolve_repo(...)` error OR
  `cache.get_or_build_by_parent(...)` error, it `log::warn!`s and
  `return`s — meaning `tx.require_footprint_for(table_token)` is simply
  never called. The write proceeds WITHOUT the footprint requirement.
- `implicit_tx_isolation_for_fk_parent`: on the equivalent errors, it
  returns `shamir_tx::IsolationLevel::Snapshot` — the LESS protective
  isolation, on an error condition.

Both treat "I couldn't determine whether this table is FK-relevant" as
"assume it isn't" — the wrong direction for a correctness-gating
mechanism. **Fix: on either error, fail CLOSED** — treat the unknown case
as "yes, apply the protection":
- `require_footprint_if_fk_child`: call `tx.require_footprint_for(table_token)`
  on the error paths too (widen the footprint unconditionally when
  discovery itself fails), not just on a confirmed `is_fk_child` hit.
- `implicit_tx_isolation_for_fk_parent`: return
  `shamir_tx::IsolationLevel::Serializable` on the error paths too (the
  MORE protective isolation), not `Snapshot`.

Update both functions' doc comments (they currently describe the OLD
fallback-to-permissive behavior as intentional "defense-in-depth" —
correct this framing to reflect the new fail-closed behavior) and the
`log::warn!` messages if they need adjusting to still make sense under
the new behavior.

**Why this is safe (state whether you agree or found a reason it isn't,
in your summary)**: `resolve_repo` failing here almost certainly means
the SAME resolve will also fail moments later for the actual write this
hook precedes — so the operation is very likely to fail downstream
regardless, making the "extra" footprint/Serializable-upgrade a
no-practical-cost safety margin, not a source of new spurious aborts on
otherwise-healthy repos. A cache-build failure is similarly rare and
would also likely recur on the very next real FK-discovery call the
write path needs anyway.

**Test**: a deterministic test (inject a `resolve_repo`/cache-build
failure — check whether `fk_race_closure_tests.rs`/
`fk_reverse_cache_race_tests.rs` already have an injectable-failure
resolver you can reuse or adapt) proving: on a resolve/build failure,
`require_footprint_if_fk_child` DOES call `require_footprint_for`, and
`implicit_tx_isolation_for_fk_parent` DOES return `Serializable` — not
the old permissive fallback.

## Concern 2 (INVESTIGATE + MEMO, do NOT rush an implementation): the explicit-Snapshot RI gap

The review separately notes: an EXPLICIT client transaction that stays at
Snapshot isolation (`.transactional()` with no `'serializable'` opt-in)
doing a parent-side DELETE/UPDATE against an FK-actionable table has NO
automatic protection — `implicit_tx_isolation_for_fk_parent`'s upgrade
logic ONLY runs for the IMPLICIT (autocommit) delete/update arms; an
explicit transaction's isolation level is fixed at `begin_implicit_batch_tx`/
tx-open time, chosen by the CLIENT, long before `query_runner.rs` sees the
individual op inside it.

**This residual is ALREADY documented** in `KNOWN_LIMITATIONS.md` as an
accepted, open gap (confirm this yourself — grep for the FK/Snapshot
residual entry) — so this is not a silently-hidden lie, just an
unclosed correctness gap the review wants scoped for future work.

**Do NOT implement an isolation-upgrade or RI-barrier mechanism for this
in this task** — investigate first, because the two options the review
names have real, non-obvious tradeoffs that deserve the same kind of
timeboxed design spike F-28 Step 3 did before implementing S3-C (see
`docs/dev-artifacts/research/f28-s3-mechanism-decision.md` for that
precedent's shape and rigor level — mirror it):

1. **Auto-upgrade the explicit transaction's isolation to Serializable**
   when it's about to perform an FK-actionable parent mutation.
   Investigate: is an explicit tx's isolation level fixed permanently at
   `begin`-time in this codebase, or can it be escalated mid-flight
   safely? (Read how `IsolationLevel` is threaded through `TxContext`/the
   commit pipeline — check whether SI/SSI read-set tracking that already
   happened under Snapshot semantics could be safely re-validated
   retroactively under Serializable's stricter rules, or whether
   escalating "loses" tracking that should have started at Serializable
   from the very first read.) This is very likely NOT a small, safe
   change — state your finding precisely either way.
2. **A narrower, isolation-independent "RI barrier"**: a commit-time
   re-check specific to FK-actionable parent tables that doesn't touch
   the transaction's overall isolation level at all — e.g. a targeted
   re-scan of the FK-relevant child tables just before commit, comparing
   against what the plan saw, regardless of SI/SSI machinery. Investigate
   whether this is expressible without a deep commit-pipeline change, and
   what its false-abort/performance profile would look like.

**Deliverable for concern 2**: a decision memo at
`docs/dev-artifacts/research/f40-explicit-snapshot-ri-gap-memo.md`,
matching `f28-s3-mechanism-decision.md`'s rigor (options considered,
tradeoffs, a recommendation, and what a follow-up implementation task
would need to do) — NOT code. If, after genuinely investigating, you find
option 1 or 2 is actually a small, safe, well-contained change after all,
say so explicitly in the memo and you MAY implement it in this same task
— but only if it's genuinely small and safe, not as a default expectation.
Do not force an implementation to appear complete if the honest
conclusion is "this needs its own dedicated task."

## Constraints

- Concern 1 (fail-closed fix) is mandatory, tested, and committed as code.
- Concern 2 is a memo by default; code only if the investigation
  genuinely concludes it's safe and small.
- Do NOT touch `FkReverseCache`'s internals (F-35/F-36 already landed) —
  only the two `query_runner.rs` functions' error-handling branches for
  concern 1.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- fk
./scripts/test.sh -p shamir-engine --full
```
