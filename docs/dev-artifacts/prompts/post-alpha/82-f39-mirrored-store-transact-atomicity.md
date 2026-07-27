# Brief for F-39 (#847, P0) — `MirroredStore::transact` must give its durable subset real atomicity

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

A readonly review (`docs/dev-artifacts/research/2026-07-27-new-wave-readonly-review.md`,
finding P0-5) found that `MirroredStore` (`crates/shamir-storage/src/storage_mirrored.rs`,
F-33 Step 1) does not override `transact` — it inherits the `Store` trait's
default loop impl (`crates/shamir-storage/src/types.rs:194-206`), which is
explicitly documented as "per-op atomic only", NOT atomic across the whole
batch. But index code (sorted-index rename, legacy index rebuild) calls
`info_store.transact(ops)` specifically to get all-or-nothing semantics —
e.g. writing a new posting and deleting the old one in one batch, relying
on crash/failure atomicity. On a hybrid repo, `info_store` is a
`MirroredStore`, so that guarantee currently disappears.

**Important correction to the review's own suggested fix — verify this
yourself before writing any code, don't take the review's phrasing at face
value:** the review's "Что сделать" list says *"primary batch должен
применяться через атомарный `InMemoryStore::transact`"* — **this assumes
`InMemoryStore` already has a genuinely atomic `transact` override.
Grep `crates/shamir-storage/src/storage_in_memory.rs` for `fn transact` —
there is no hit.** `InMemoryStore` also relies on the SAME non-atomic trait
default. So there is no existing atomic primary-side primitive to delegate
to. What `MirroredStore`'s mirror backend (`FjallStore`,
`crates/shamir-storage/src/storage_fjall.rs:485-501`) DOES have is a
genuinely atomic `transact` via fjall's `OwnedWriteBatch` — confirm this
yourself too. This distinction matters for scoping the fix correctly (see
below) — do not attempt to add a new atomic multi-key transact to
`InMemoryStore` itself as part of this task (a much bigger, separately
risky change touching a foundational, widely-used primitive with ~hundreds
of call sites across the workspace) — that is explicitly OUT of scope
here.

## Why this specific gap matters (and why it didn't before hybrid mode)

For a PLAIN (non-hybrid) durable repo, `info_store` is a `FjallStore`
directly, whose `transact` is already genuinely atomic — no gap. For a
plain `InMemoryRepo` (the old, fully-ephemeral default), `info_store`'s
`transact` was ALSO never atomic, but that never mattered: an in-memory
repo's entire point is that nothing survives a crash/restart anyway, so a
partially-applied in-memory batch on error was always a tolerable,
by-design characteristic (a crash wipes the whole thing regardless). The
gap becomes a REAL problem specifically for `MirroredStore` (hybrid mode)
because its mirror half genuinely persists and is expected to survive a
restart — a mid-batch failure there can leave an inconsistent, *durable*,
observable-after-restart state (e.g. a stale index definition pointing at
a posting that was supposed to be atomically replaced).

## What to build

### 1. `MirroredStore::transact` — split by classifier, delegate the durable half to the mirror's real atomicity

```rust
async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
    if ops.is_empty() { return Ok(()); }

    let (ephemeral_ops, durable_ops): (Vec<KvOp>, Vec<KvOp>) = ops
        .into_iter()
        .partition(|op| !(self.classify)(op.key()));  // check KvOp's actual
                                                          // accessor name for
                                                          // its key — read
                                                          // types.rs's KvOp
                                                          // definition first

    // Apply the ephemeral subset to `primary` first (see ordering
    // rationale below) via the same per-op loop the trait default already
    // uses for a plain InMemoryStore — no atomicity regression vs today's
    // behavior for the ephemeral half; InMemoryStore had none to begin with.
    for op in ephemeral_ops {
        match op {
            KvOp::Set(k, v) => { let _ = self.primary.set(k, v).await?; }
            KvOp::Remove(k) => { let _ = self.primary.remove(k).await?; }
        }
    }

    // Apply the durable subset atomically via the mirror's REAL transact
    // (FjallStore's OwnedWriteBatch — all-or-nothing).
    self.mirror.transact(durable_ops).await
}
```

(The above is illustrative — check `KvOp`'s actual shape/accessors in
`crates/shamir-storage/src/types.rs` before writing the real
implementation; don't assume the exact method names above are correct.)

### 2. Ordering + failure/compensation story — define and document precisely

**Ephemeral-first, then durable** (matching the illustrative order above)
is the recommended ordering — state your reasoning in your summary,
including whether you agree or found a reason to do it differently:

- If the ephemeral loop fails partway: no durable write is even attempted
  (fail fast). `primary` may be left partially mutated — same
  pre-existing, tolerable characteristic a plain `InMemoryStore` already
  has (nothing durable was touched, so nothing inconsistent survives a
  restart; the live process's in-memory state is transiently
  inconsistent until the caller's own retry/compensation, exactly as
  today for a non-mirrored in-memory store).
- If the ephemeral loop succeeds but the durable `mirror.transact` then
  fails: `primary` is now AHEAD of `mirror` (fully applied vs. not applied
  at all — `FjallStore::transact`'s real atomicity means the durable side
  is either fully applied or not at all, never partial). The caller sees
  the durable error (propagated), so it correctly learns durability was
  NOT achieved. On the NEXT restart, hydration only replays what's
  actually in `mirror` (unchanged, i.e. the pre-transact durable state) —
  so the ephemeral-side changes from this failed transact are simply lost
  on restart, exactly matching hybrid mode's own "data/config not
  durably-written doesn't survive restart" design ethos. This is a SAFE,
  well-understood failure mode, not a new hazard — document it as such
  (no compensation/rollback of `primary` is needed; the live process
  keeps functioning correctly with its current, transiently-ahead
  in-memory state until the next restart, at which point the ephemeral
  divergence is silently reconciled back to the last durably-committed
  state).

Document this reasoning directly in the code (module doc + `transact`'s
own doc comment) — this is exactly the kind of "precisely document the
residual/tradeoff rather than hand-wave it" discipline this campaign has
followed for its other race-closure fixes (see F-36's generation-check
residual-window doc for the expected level of precision).

### 3. The reader-visible partial-ephemeral-batch window — investigate, don't over-promise

Separately from crash/restart durability, the review also raises a LIVE
concern: while the ephemeral loop is applying its ops one at a time, a
CONCURRENT reader (`get`/`iter_stream`/etc, which read `primary` directly
with no lock) can observe a partially-applied batch. Investigate whether
there's a reasonably cheap way to narrow this window (e.g. does
`InMemoryStore`'s underlying `scc::TreeIndex` offer any batched/atomic
multi-key primitive already unused by this codebase? Check its actual
API surface before assuming not). If you find a cheap improvement, apply
it. If, after genuinely checking, no cheap fix exists (this is the
expected outcome — `scc::TreeIndex` is a lock-free structure without
multi-key CAS), **do not attempt a bigger `InMemoryStore` redesign to
close this** — instead document the residual precisely (module doc +
`transact`'s doc comment): concurrent readers CAN observe a
partially-applied ephemeral batch mid-transact; this is a pre-existing
characteristic inherited from `InMemoryStore` having no cross-key
atomicity primitive, not something this fix introduces or worsens for the
ephemeral side specifically (it only fixes the DURABLE side's atomicity,
which is the part that actually matters for post-restart correctness).

## Tests — MANDATORY, in the same commit

Extend `crates/shamir-storage/src/tests/storage_mirrored_tests.rs` (read
it in full first — F-33 Step 1's existing test file, mirror its
established style):

1. **Durable-subset atomicity, happy path**: a `transact` with 2+
   classified (durable) ops applied together; confirm both land in
   `mirror` (query the mirror directly, matching `storage_mirrored_tests.rs`'s
   existing pattern for checking mirror state).
2. **Durable-subset atomicity, injected failure**: construct a `transact`
   batch whose durable subset would fail partway (you'll likely need a
   test-only failing/wrapping `Store` implementation around the mirror
   that errors on, say, the Nth internal operation — check whether such a
   fault-injection `Store` wrapper already exists anywhere in this crate's
   tests to reuse, or write a minimal one scoped to this test file).
   Confirm: NO partial durable state exists after the failure (either all
   of the durable ops landed or none did) — this is the actual atomicity
   proof, not just "an error was returned".
3. **Mixed ephemeral+durable batch**: a single `transact` call with BOTH
   ephemeral and durable ops; confirm ephemeral ops land in `primary` (not
   `mirror`) and durable ops land in BOTH.
4. **Ephemeral-then-durable-fails ordering proof**: force the durable half
   to fail (same injection technique as #2) while the ephemeral half
   would otherwise succeed; confirm `primary` DOES reflect the ephemeral
   ops (already applied) while `mirror` does NOT reflect the durable ops
   (correctly rolled back to nothing by fjall's own atomicity) — proving
   the documented ordering/failure story from section 2 above.
5. Whatever the section-3 investigation concludes (fixed or documented
   residual): add a test that HONESTLY demonstrates the actual current
   guarantee for concurrent-reader visibility during a `transact` call —
   do not write a test that asserts a stronger guarantee than what's
   actually implemented.

## Constraints

- Do NOT add a new atomic multi-key `transact` to `InMemoryStore` itself —
  out of scope, too large a blast radius for this task.
- Do NOT change `FjallStore`'s already-correct `transact` — it's already
  atomic; this task only needs to USE it correctly from `MirroredStore`.
- Also grep `crates/shamir-index/src` and
  `crates/shamir-engine/src/table/table_manager_index_mgmt.rs` for every
  call site of `info_store.transact(...)` / `.transact(ops)` on an
  `Arc<dyn Store>` that could be a hybrid repo's `__info__` store (legacy
  index rename, sorted index, vector index, per the review's own list) —
  confirm each one's atomicity ASSUMPTION is now correctly served by this
  fix (the keys each of these actually writes should be checked against
  `is_durable_table_config` — if a given rename/update touches ONLY
  durable-classified keys, this fix fully restores its atomicity
  expectation on a hybrid repo; if it touches a MIX or ONLY
  non-classified keys, say so explicitly in your summary, since that
  changes what this fix actually buys that specific call site).
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-storage -- --check` and
  `cargo clippy -p shamir-storage --all-targets -- -D warnings` must be
  clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-storage -- --check
cargo clippy -p shamir-storage --all-targets -- -D warnings
./scripts/test.sh -p shamir-storage -- mirrored
./scripts/test.sh -p shamir-storage --full
./scripts/test.sh -p shamir-engine -- hybrid
```
