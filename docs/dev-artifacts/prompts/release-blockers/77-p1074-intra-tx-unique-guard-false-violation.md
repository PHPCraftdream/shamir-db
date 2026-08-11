# Brief 77 — #1074 (MEDIUM): intra-tx unique-guard check false-rejects a key released and reclaimed within the same transaction

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## The defect (regression against #1039, already landed this session)

`crates/shamir-engine/src/tx/pre_commit.rs:600-634` (the intra-tx unique
guard check #1039 added):

```rust
let mut seen: TFxMap<(u64, bytes::Bytes), RecordId> = TFxMap::default();
for g in &tx.unique_guards {
    let key = (g.table_token, g.index_key.clone());
    if let Some(&prior_owner) = seen.get(&key) {
        if prior_owner != g.owner {
            return Err(TxError::UniqueViolation { key: g.index_key.clone() });
        }
        continue;
    }
    seen.insert(key, g.owner);
    // ... durable-state check against the FIRST-seen owner only ...
}
```

`tx.unique_guards` (`crates/shamir-tx/src/tx_context.rs:66`, `UniqueGuard { table_token, index_key, owner, .. }`,
pushed into `Vec<UniqueGuard>` at `tx_context.rs:251`) is a chronological
list of CLAIMS made during staging — one guard is pushed every time an
insert/update assigns a value to a unique-indexed field (verified call
sites: `crates/shamir-engine/src/table/table_manager_tx_ops.rs:424, 573,
767, 908, 1015, 1059`, all `for index_key in
self.index_manager.unique_keys_for(&new_view) { ... owner: id ... }`).
**DELETE never pushes a guard. An UPDATE that moves a record OFF a unique
key never pushes any "release" marker for the old key either** — it only
pushes a NEW guard for whatever key the updated value now has.

So `tx.unique_guards` is a list of point-in-time claims, not a snapshot of
final per-key ownership — but the check above treats the FIRST owner seen
for a key as authoritative and rejects any LATER guard for the same key
with a different owner, even when the first owner legitimately vacated
that key later in the same transaction.

**False-rejection scenario (legal transaction, used to commit fine before
#1039)**:
```
INSERT A {email: "x"}   -> guard (K_x, A)
UPDATE A SET email = "z" -> guard (K_z, A)   -- A no longer claims K_x
INSERT B {email: "x"}   -> guard (K_x, B)
```
`seen` sees `(K_x, A)` first, then later `(K_x, B)` — different owner —
`UniqueViolation`, even though the final state (`K_x -> B`, `K_z -> A`) is
completely valid. Same false rejection for `INSERT A{x}` -> `DELETE A` ->
`INSERT B{x}` (DELETE pushes no guard at all, so the check still only
ever sees `(K_x, A)` then `(K_x, B)`).

This is fail-closed (no data corruption — a legal transaction is
incorrectly aborted), so MEDIUM not CRITICAL, but it IS a real regression:
before #1039 this transaction pattern committed correctly.

**Why #1039's own tests missed it**: all 4 tests in
`crates/shamir-engine/src/tx/tests/base_index_tx_tests.rs`
(`intra_tx_*`) cover "two live claims on one key" (correctly rejected)
and "the same record claiming its own key twice" (correctly allowed) —
none cover a key being released (via UPDATE-off or DELETE) and then
reclaimed by a DIFFERENT record within the same transaction.

## The fix — check FINAL ownership, not claim history

The task's own analysis (independently re-verified against the code)
offers two directions; use whichever you determine is more correct after
investigating the actual data available at this point in `pre_commit.rs`:

**Option (a)**: thread release information alongside `tx.unique_guards`
so `seen` can reflect "who owns this key as of the LAST staged operation
touching it," not "who claimed it first." Concretely: `unique_guards` is
pushed in chronological staging order (verify this order is preserved
end-to-end — check whether anything reorders `unique_guards` between
staging and `pre_commit`), so **iterating the guards list and
UNCONDITIONALLY overwriting `seen[key] = g.owner` on every occurrence**
(instead of "insert once, then compare") would already correctly resolve
the false-rejection scenario above — the LAST guard for a key wins,
which is exactly the release-then-reclaim case. The problem this
alone does NOT solve: it also silently masks the GENUINE conflict #1039
exists to catch (`INSERT A{x}` -> `INSERT B{x}`, neither releases) — with
naive overwrite, that would also just silently resolve to
`seen[K_x] = B` with no error, breaking #1039's own positive case. You
need a way to distinguish "A legitimately moved off K_x before B claimed
it" from "A never moved off K_x and B is a genuine duplicate" — which
requires knowing whether A's OWN claim on K_x was superseded by A's own
LATER activity (a different guard from A on a different key, or A being
deleted), not just "a later guard from someone else exists."

**Option (b)**: use `tx.index_write_set` (`Vec<(u64, IndexWriteOp)>`,
`crates/shamir-tx/src/tx_context.rs:111`) instead of `tx.unique_guards`
for this check — `IndexWriteOp::SetPosting { key, value, provenance }` /
`RemovePosting { key, provenance }` (`crates/shamir-tx/src/index_write_op.rs:90-107`)
is the actual ordered sequence of storage mutations Phase 5c will apply,
which DOES include removals (an UPDATE-off or DELETE that vacates a
unique key produces a `RemovePosting` for that key — verify this claim
directly by reading how unique-index writes populate `index_write_set`
in `table_manager_tx_ops.rs`, the same call sites cited above, and
confirm whether unique-index postings are tagged/distinguishable from
regular-index postings in this same vec, e.g. via `provenance` or by
which `table_token`/key namespace they fall under — you may need to
correlate against `tx.unique_guards`' `(table_token, index_key)` set to
know WHICH `index_write_set` entries are for unique indexes specifically,
since `index_write_set` also carries regular-index and index2 writes).
Walking this ordered list and tracking `current_owner: Map<(table_token,
key), Option<RecordId>>` (Some on SetPosting, None on RemovePosting) — set
by set, remove by remove, in order — gives you the TRUE final per-key
state for this transaction, which is both correct for the false-rejection
scenario AND still correctly flags a genuine unreleased double-claim
(since a genuine conflict never produces a `RemovePosting` for the
first owner's claim on that key).

**Recommendation**: option (b) is more principled (uses the actual
authoritative write sequence rather than re-deriving intent from a
claims-only list), but investigate BOTH before committing to an approach,
and clearly state in your final report which you chose and why the
rejected alternative was insufficient. Whichever you choose, the
per-key durable-state check against committed storage (the existing
`info_store.get(...)` call, unchanged in spirit) must still run for
whatever the FINAL live owner of each key turns out to be — this task is
about fixing what "seen" represents, not removing the cross-tx check.

## Tests — the required minimum (missing from #1039, must be added now)

New tests in `crates/shamir-engine/src/tx/tests/base_index_tx_tests.rs`
(mirror the existing `intra_tx_*` tests' style/harness):

1. `INSERT A{x}` -> `UPDATE A` (moves off `x`) -> `INSERT B{x}` in ONE
   transaction -> commit succeeds, final state `K_x -> B` (and the old
   key `A` moved to now points at `A`, if you assert on that too).
2. `INSERT A{x}` -> `DELETE A` -> `INSERT B{x}` in ONE transaction ->
   commit succeeds, final state `K_x -> B`.
3. **Regression guard — do not silently break #1039's positive case**:
   `INSERT A{x}` -> `INSERT B{x}` with NEITHER releasing `x` (the
   original #1039 scenario) must STILL be correctly rejected with
   `TxError::UniqueViolation`. Run the EXISTING `intra_tx_*` tests too
   and confirm they still pass unmodified.

**Every new test must FAIL on the current HEAD (before your fix)** —
verify this yourself: temporarily revert your `pre_commit.rs` change
locally, confirm tests 1 and 2 go red with `UniqueViolation`, then
restore the fix and confirm all three (plus the pre-existing #1039 tests)
are green. Report this outcome explicitly.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-tx
```

Paste the actual final summary line from each `./scripts/test.sh`
invocation (pass/fail counts) — literal output, not a paraphrase. List
every test you added/touched by name with individual pass/fail status,
and the outcome of the mandatory revert-and-check. If anything fails,
fix it before reporting done — everything you report must be something
you personally watched pass, with the command's actual output as
evidence.
