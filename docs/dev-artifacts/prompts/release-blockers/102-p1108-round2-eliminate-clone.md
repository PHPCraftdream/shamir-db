# #1111 MEDIUM — #1108's incremental cache still O(N^2): cache.released.clone() grows per row, called per row

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

Found by a follow-up adversarial `@oh` review of the `#1105`-`#1109`
remediation wave (commit range `59e8c532..HEAD`). Confirmed personally by
reading the code and independently re-running the bench before this brief
was written.

File: `crates/shamir-engine/src/table/table_manager_tx_ops.rs`, the last
line of `released_unique_keys_in_tx` (the `#1108` fix, currently around
line 149 — re-read the function fresh, it may have shifted).

## The bug

`#1108`'s incremental fold made the WALK itself `O(new suffix only)`
instead of `O(whole index_write_set)` per call — a real improvement. But
the function still ends with `cache.released.clone()` — returning an OWNED
CLONE of the accumulated `released` set on EVERY call. Since `released`
grows by roughly one key per row in the natural worst-case workload (an
UPDATE batch changing the unique-indexed column's value on every row —
exactly the workload `#1108`'s own new bench, `p1108_released_unique_growth.rs`,
exercises), and this function is called once per row, the total cost across
N rows is `O(1+2+...+N) = O(N^2)` in the clone alone — the SAME asymptotic
complexity class as before `#1108`, just with a much smaller per-element
constant (a `HashSet<Vec<u8>>` key clone, ~28 bytes, vs. the old full
match/compare walk per `index_write_set` entry).

The `#1108` commit message and CHANGELOG entry both claim to "close #1099's
residual O(N^2)" — this is inaccurate; the fix reduced the constant factor
substantially (why the bench numbers looked much better) but did NOT remove
the `O(N^2)` driver.

### Confirmed evidence

Re-running the SAME bench at HEAD gives (a real, independent measurement —
verify these are still representative before trusting them, hardware/load
varies):

| n | time | ratio per doubling |
|---|------|---------------------|
| 400 | 25.22 ms | — |
| 800 | 73.32 ms | 2.91× |
| 1600 | 239.39 ms | 3.27× |

Linear would be 2.0×, and the ratio RISES with n — the signature of a
quadratic term taking over, not of "nonzero per-row constants." A
least-squares fit `T(n) = 34.4µs·n + 71.5ns·n²` reproduces all three points
within 0.6%; the quadratic term is 77% of the n=1600 runtime. An isolated
micro-measurement of a `HashSet<Vec<u8>>`-growing-by-one-~28-byte-key-
cloned-per-row pattern reproduces a matching `78–101 ns·n²` term, confirming
attribution to the clone specifically — NOT to
`validate_unique_for_*_with_released` (only does `.contains(...)`, O(1)),
`record_unique_guard` (a `Vec::push`, O(1)), or `stage_mutation` (O(1)),
all confirmed O(1) per row.

This means the CHANGELOG's own explanation for the residual super-linearity
("other genuine per-row O(1)-but-nonzero-constant costs remain...") is
WRONG — O(1) per-row costs cannot produce a RISING 2.9–3.27× ratio; only a
real quadratic term can. Correct that CHANGELOG text once the real fix
lands.

## Fix direction

Split the single function into a mutation step and a read step, so callers
can take an IMMUTABLE BORROW of the cached set instead of an owned clone:

```rust
// Refresh the cache (takes &mut tx), returns nothing:
fn refresh_released_unique_cache(tx: &mut TxContext, table_token: u64) { /* the existing fold logic, minus the final .clone() */ }
```

Caller pattern (6 call sites in `table_manager_tx_ops.rs` — re-grep for
`released_unique_keys_in_tx`, line numbers have shifted since `#1108`
landed; check each site's exact surrounding code individually, do not
assume they're all structurally identical without checking):

```rust
refresh_released_unique_cache(tx, table_token);
let released = &tx.released_unique_cache[&table_token].released;
// (released_unique_cache.entry(table_token).or_default() inside refresh
// guarantees the entry exists after refresh runs — indexing is safe here.
// If you prefer not to rely on that invariant implicitly, use
// .get(&table_token).map(|c| &c.released) and handle the impossible-in-
// practice None case however this codebase's conventions prefer.)
let is_record_touched = |rid: &RecordId| -> bool {
    tx.write_set.get(&table_token).is_some_and(|s| s.staged_op(rid.as_bytes()).is_some())
};
```

Both `released` and the `is_record_touched` closure end up as IMMUTABLE
borrows of `tx` after `refresh_released_unique_cache`'s mutable borrow ends
— these coexist fine under Rust's borrow rules. Trying to return a
reference directly from a single combined `&mut tx -> &Set` function would
NOT work here, because the caller also needs a separate immutable borrow of
`tx.write_set` for the closure at the same time — hence the two-step split.

Update all 6 call sites to this two-step pattern.

## Required test/bench

Re-run `p1108_released_unique_growth.rs` (the existing `#1108` bench)
before and after this fix (temporarily revert just this change and re-run
for genuine "before" numbers, matching this session's established
technique — see `#1099`'s and `#1108`'s own bench methodology). Confirm the
AFTER curve's doubling ratio is close to 2.0× (genuinely near-linear), not
still rising toward 3×+. Report real numbers, not a rounded claim — if it's
still somewhat super-linear, say so honestly and explain what's left,
rather than claiming a clean win.

Also re-run `p1096_tx_aware_unique_check.rs` in full (especially
`released_unique_keys_in_tx_walks_correctly`, whose call site will need
updating to the new two-step pattern) to confirm no correctness regression
from the refactor — the LIVE/RELEASED semantics must be byte-identical to
before this change, only the calling convention differs.

## Also address (LOW, same review pass, same struct — fold in since you'll already be touching TxContext)

`TxContext.released_unique_cache` is retained for the WHOLE transaction
lifetime (a copy of every unique index key the tx sets or releases), but
`TxContext::approx_bytes`'s doc says it deliberately counts
`index_write_set` toward the tx's staging-size cap and does NOT count this
new cache field — so a large transaction's real memory footprint now
exceeds what the cap sees by roughly a constant factor per released/live
key. Investigate whether this is worth accounting for in `approx_bytes` (a
rough addition based on the cache's key/value byte sizes) — if it's a
meaningfully-sized addition to the cap's accuracy, add it; if it's
negligible relative to `index_write_set`'s own already-counted size for
realistic workloads, document that reasoning briefly in
`approx_bytes`'s doc instead of silently leaving the gap unexplained.

Correct the `#1108` CHANGELOG entry's explanation of the residual
super-linearity once the real fix lands, replacing the incorrect
"O(1)-per-row-constants" explanation with an honest accounting of what
remains super-linear (if anything) after THIS fix.

## Gate

```
cargo fmt -p shamir-engine -p shamir-tx -- --check
cargo clippy -p shamir-engine -p shamir-tx --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-tx --full
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo bench -p shamir-engine --bench p1108_released_unique_growth
```

(Use forward slashes for `CARGO_TARGET_DIR` on this system — backslashes get
mis-parsed by the shell here.)

All must pass clean. Report real before/after bench numbers (not rounded/
cherry-picked), and confirm `released_unique_keys_in_tx_walks_correctly`
and the rest of `p1096_tx_aware_unique_check.rs` pass unchanged.
