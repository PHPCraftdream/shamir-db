# Brief for F-41 (#849, P1) — `MirroredStore` write-error divergence + hydration classifier re-filter

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

A readonly review (`docs/dev-artifacts/research/2026-07-27-new-wave-readonly-review.md`,
finding P1-2) found two related, but distinct, gaps in `MirroredStore`
(`crates/shamir-storage/src/storage_mirrored.rs`) — read the current file
in full first, especially `set`/`remove` (~line 254-276) and
`MirroredStore::new`'s hydration loop (~line 219-238). This is a separate
concern from F-39 (transact atomicity, already landed) — do not re-touch
F-39's `transact` override.

## Concern 1 (mandatory fix): write-error divergence between primary and mirror

`set`/`remove` currently mutate `primary` FIRST, then conditionally
mirror:

```rust
async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
    let created = self.primary.set(key.clone(), value.clone()).await?;
    if (self.classify)(&key) {
        self.mirror.set(key, value).await?;   // if this errors...
    }
    Ok(created)
}
```

If the mirror write errors, the caller sees `Err` (propagated), but
`primary` was ALREADY mutated in the line above — so the live process now
behaves as though the write succeeded (any subsequent read through this
same `MirroredStore` instance sees the new value), even though the API
told the caller the operation failed. On the next restart, hydration
replays only what's actually in `mirror` (the write never landed there),
so the observed state reverts to the pre-write value. Three different
observable states across one operation's lifetime: "API says failed",
"live process behaves as succeeded", "restart reverts to failed". This is
a real error-atomicity violation for DDL/config writes specifically (the
one class of write `MirroredStore` exists to make durable).

**Fix: for a CLASSIFIED (durable) key, write to `mirror` FIRST, and only
mutate `primary` if the mirror write succeeds.** This matches the
review's stated preference ("для config writes предпочтительно durable
mirror commit до публикации в primary") and is also the same directional
preference F-39 (already landed) used for its durable-subset ordering
question. For an UNCLASSIFIED (ephemeral) key, there is no mirror
involvement at all — write directly to `primary` as today, no ordering
change needed.

```rust
async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
    if (self.classify)(&key) {
        // Durable key: mirror commit FIRST. If this fails, primary is
        // NEVER touched — the caller's Err is now honest: nothing
        // happened, not "half happened".
        self.mirror.set(key.clone(), value.clone()).await?;
    }
    self.primary.set(key, value).await
}
```

(Illustrative — check `remove`'s exact shape too and apply the same
reordering; verify `InMemoryStore::set`/`remove`'s actual return-value
semantics — e.g. whether `set`'s returned `bool` means "was this a fresh
insert vs overwrite" — are preserved correctly by whichever concrete
reordering you write, don't just copy this sketch blindly.)

**Document the new ordering's own tradeoff precisely** (matching this
campaign's "residual window, precisely documented" discipline — see
F-36's generation-check doc or F-39's failure-ordering doc for the
expected rigor level): mirror-first means a mirror SUCCESS followed by a
primary failure would leave `mirror` ahead of `primary` — investigate
whether `InMemoryStore::set`/`remove` can actually fail in a way that
matters here (F-39's own investigation found they don't fail in practice)
and state your finding. If they genuinely cannot fail, say so and note
this ordering has no realistic failure-divergence window in the reverse
direction.

## Concern 2 (mandatory fix): hydration doesn't re-apply the classifier

`MirroredStore::new`'s hydration loop streams EVERY entry out of `mirror`
and writes it into `primary` unconditionally, trusting the invariant
"everything in the mirror was, by construction, something the classifier
accepted when it was written." This is fragile against:
- a classifier change across a version upgrade (a key durable under an
  OLD classifier version might not be durable under a NEW one, or
  vice versa),
- on-disk corruption or manual tampering.

**Fix**: during hydration, re-run `classify(&key)` for each entry
streamed from `mirror`. Decide (and document your reasoning) what to do
with a key that does NOT pass the CURRENT classifier:
- At minimum: log a diagnostic (`log::warn!` or similar) naming the key
  and the fact that it no longer matches the durable-config classifier —
  this is the review's explicit ask ("диагностировать rejected keys").
- Decide whether to still load it into `primary` (safe default — reads
  are unaffected either way, since reads always go through `primary`
  regardless of classification) or skip it. State your reasoning either
  way; there is no clearly-mandated "right" choice here per the review's
  own phrasing (it says "диагностировать", not necessarily "reject") —
  pick the option that best preserves data availability while still
  surfacing the drift for operator visibility, and justify it.

## Tests — MANDATORY, in the same commit

Extend `crates/shamir-storage/src/tests/storage_mirrored_tests.rs`:

1. **Mirror-first ordering, happy path**: a classified `set`/`remove`
   still lands in both `primary` and `mirror` exactly as before (no
   regression to the existing classify-and-mirror behavior).
2. **Mirror-write failure leaves primary untouched**: using a
   test-only failing mirror wrapper (check whether F-39's tests already
   added one you can reuse, e.g. `FailingTransactMirror` — you may need a
   sibling that fails `set`/`remove` specifically, not `transact`), set a
   classified key against a mirror that errors on `set`; assert the
   overall call returns `Err`, AND assert `primary` does NOT reflect the
   attempted write (the core proof — "API says failed" and "live state"
   now agree, unlike before this fix). Same for `remove`.
3. **Hydration classifier re-filter**: construct a mirror containing a
   key that would NOT pass the CURRENT classifier (simulate "classifier
   drift" by writing directly to the underlying mirror store, bypassing
   `MirroredStore`'s own classify-gated `set`), then construct a fresh
   `MirroredStore` over it and confirm your chosen behavior (diagnostic
   logged, and whichever load/skip decision you made) — assert whichever
   outcome you implemented, precisely.

## Constraints

- Do NOT touch F-39's `transact` override — separate concern, already
  landed, leave it alone.
- Do NOT change `MirroredStore`'s public API shape (constructor
  signature, `Store` trait methods) beyond internal reordering.
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
