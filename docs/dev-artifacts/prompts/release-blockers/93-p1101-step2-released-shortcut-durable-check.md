# #1101 HIGH — Step 2's "released -> continue" shortcut skips the durable check for a same-tx net-release, letting it silently delete an unrelated concurrently-committed posting

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background — found by a 4th `@oh` review round of `#1097`, confirmed by the orchestrator's own reading

`crates/shamir-engine/src/tx/pre_commit.rs`'s Step 2 (~line 813-877,
`# Step 2 — Durable-state (cross-tx) check`) validates every unique key
this tx claims (via `tx.unique_guards`) against the CURRENT durable
posting owner, to catch a concurrent tx that already claimed the same key.
For each key it computes an `owner` to compare against `existing`:

```rust
let owner = if let Some(&final_owner) = live.get(&key) {
    final_owner
} else if released.contains(&key) {
    continue; // key vacated by this tx — no claim to validate
} else {
    g.owner // self-write: no index op, guard's owner is authoritative
};
```

`released` is Step 1's FINAL-STATE set: a key is in it if the LAST
unique-family op this tx staged for that key (in `tx.index_write_set`,
staging order) was a `RemovePosting` — i.e. the tx's own net effect on
that key is "released", even if it went through intermediate
Set→Remove/Remove→Set transitions within the same tx (e.g. `INSERT
R{email:"x"}` then `DELETE R` in the same tx — a legitimate net-release,
still leaves BOTH a `SetPosting(K,R)` and a `RemovePosting(K)`
**physically present** in `tx.index_write_set`, in that order).

**The bug**: `continue` skips the durable check ENTIRELY for a
net-released key — but `tx.index_write_set` still carries both ops, and
`materialize.rs`'s Phase 5c (`apply_index_ops_at_commit` /
`apply_index_batch`) applies EVERY op in staging order with
last-write-wins semantics, unconditionally, regardless of what Step 2
did or didn't check. If a DIFFERENT, concurrently-committed tx durably
claimed key `K` for an unrelated record `B` between this tx's snapshot
and this check, Phase 5c's blind apply of this tx's stale
`SetPosting(K,R)`-then-`RemovePosting(K)` pair FIRST overwrites `B`'s
genuine posting with `R` (wrong), THEN removes it entirely (also wrong —
`K` ends up incorrectly free, `B`'s real posting is gone, and `B`'s data
row still claims `K` with no posting to back it — the same index/data
divergence class `#1097` exists to prevent).

### Reproduction shape (construct and confirm it fails on current `master` first — the Red step)

```text
tx0: INSERT R{email:"x"}; COMMIT                         // "x" -> R
tx2: BEGIN (snapshot sees R owning "x")
tx1: DELETE R; INSERT B{email:"x"}; COMMIT                // durable owner is now B
tx2: INSERT C{email:"x"}; DELETE C (same tx, same key)    // nets to "released" for key "x" —
                                                            // index_write_set still has
                                                            // SetPosting("x",C) then RemovePosting("x")
tx2: COMMIT
```

Before `tx2`'s `DELETE C`, `INSERT C{email:"x"}`'s stage-time
`validate_unique_for_create` (durable-only, or released+touched-tolerant
per `#1096`) may or may not let this stage depending on exact
timing/ordering — investigate what actually happens at stage time first;
the point of interest is Step 2's behavior AT COMMIT once the key nets to
`released`. Since `key ("x")` is in `released` after Step 1's walk (last
op was `RemovePosting` for `C`'s own claim), Step 2's `continue` skips
checking `B`'s current durable ownership entirely. At Phase 5c,
`SetPosting("x", C)` then `RemovePosting("x")` still apply in order,
clobbering then erasing `B`'s durable posting.

**Confirm the actual observable symptom empirically** (does `B`'s
posting end up `None` when it should still point at `B`? does
`lookup_by_unique_index("x")` return `None` after `tx2` commits, even
though `B`'s data row still legitimately holds `email:"x"`?) — report
what you find, don't assume the reproduction sketch above is exactly
right; adjust it if the actual staging/validation behavior differs
(e.g. if stage-time validation rejects `INSERT C{email:"x"}` before it
ever reaches this shape, find the SIMPLEST net-release-within-one-tx
scenario that actually reaches Step 2 with a populated `released` entry
for a key whose durable owner changed concurrently — an
`INSERT`-then-immediate-`DELETE` of the same record is the most direct
shape, but a same-tx `UPDATE ... SET email=X` then `UPDATE ... SET
email=Y` (moving the SAME record's key on, then back off) may also net
to `released` for the intermediate key and could be simpler to construct
correctly — investigate both, pick whichever reproduces cleanly).

## Why this needs a different fix shape than `#1096`'s `released_and_touched` tolerance

The EXISTING `released_and_touched` check (~line 860-865, in the `owner`
branch reached when `live.get(&key)` or `g.owner` produce a non-skipped
`owner`) already solves a structurally similar problem for KEYS THAT ARE
STILL BEING VALIDATED: it tolerates a durable-owner mismatch when the
current durable owner is a record THIS TX has itself staged a write for
(`tx.write_set.get(&table_token).is_some_and(|s|
s.staged_op(existing_id.as_bytes()).is_some())`). That's exactly the
right tolerance test to reuse here — the `released.contains(&key) =>
continue` branch just needs to stop skipping and instead apply an
equivalent check, not invent a new one.

## What to investigate before implementing

1. Read Step 1 (~line 656-812) and Step 2 (~line 813-877) in full,
   including the `live`/`released`/`ever_released` bookkeeping and the
   existing `released_and_touched` tolerance logic, to have the complete
   mental model before touching anything.
2. Confirm empirically (via the reproduction, instrumented if needed —
   temporary `eprintln!`/debug probes are fine for investigation, revert
   them before the final diff) that the `released.contains(&key) =>
   continue` branch is genuinely reached with a stale durable owner in
   play, and that Phase 5c's blind last-write-wins apply is what
   actually corrupts `B`'s posting.
3. Decide the fix shape. Two directions the prior `@oh` review named,
   investigate both and justify your choice:
   - **(a)** Replace the `continue` with a real durable check: read
     `info_store.get(g.index_key)` and require EITHER `NotFound` (key
     genuinely free — safe to let the residual Set+Remove pair apply,
     it's a no-op or harmless) OR the current durable owner is a record
     THIS tx has itself staged a write for (the same touched-tolerance
     test `released_and_touched` already implements) — a mismatch means
     an unrelated tx durably owns the key now and this tx's stale
     Set+Remove pair must be REJECTED (abort with `UniqueViolation`,
     matching every other durable-conflict path in this function) rather
     than silently applied.
   - **(b)** Retract the whole Set+Remove pair for that key from
     `tx.index_write_set` as a unit when the durable check fails,
     mirroring this file's existing `stale_remove_indices`/
     `retract_stale_provenance_ops` retraction pattern, instead of
     aborting the whole tx. Consider: is silently dropping the pair
     (data row keeps the old value, unique posting untouched) actually
     correct here, or does it leave `tx.write_set`'s data-level mutation
     (which DOES still apply — `R`'s own delete/insert history) out of
     sync with the index? Trace through what `tx.write_set` (data ops)
     vs `tx.index_write_set` (index ops) each independently do at commit
     to answer this — they're applied by different phases
     (`apply_data_phase` vs Phase 5c) and retracting only the index half
     could leave a data row alive with no matching posting, a NEW
     divergence.
   State your reasoning for the chosen shape in the final summary — per
   this session's established standard, this decision matters more than
   the line-level diff. If (a) turns out correct, it likely needs to sit
   inside the SAME `if let Some(tbl) = repo.table_by_token(...)` block
   that already runs the durable check for the `live`/`g.owner` branches,
   restructured so the `released` case doesn't bypass it.
4. Whichever shape you choose, a genuine intra-tx-only scenario with NO
   concurrent interference (a plain net-release with the key durably
   free both before and after) must still commit cleanly — don't
   introduce a false-positive abort for the common, uncontended case.

## Tests

1. The reproduction — must FAIL (or otherwise demonstrably corrupt `B`'s
   posting) on current `master`, PASS/correctly-reject after your fix.
2. A regression test: a same-tx net-release with NO concurrent
   interference (key durably free throughout) still commits successfully
   — no false-positive abort.
3. Mutation-test your own fix (this session's established discipline):
   temporarily revert the fix (restore the blind `continue`), confirm
   the reproduction test fails/misbehaves again, restore, confirm the
   full gate is green.

## Gate

```
cargo fmt -p shamir-engine -p shamir-index -p shamir-tx -- --check
cargo clippy -p shamir-engine -p shamir-index -p shamir-tx --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-index -p shamir-tx --full
```

All three must pass clean. Report the real pass/fail counts, the fix
shape you chose and why, and confirmation of the mutation test in your
final summary.
