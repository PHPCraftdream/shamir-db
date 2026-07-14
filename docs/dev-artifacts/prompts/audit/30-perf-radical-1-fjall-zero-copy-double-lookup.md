Task: HIGH performance — `FjallStore` (`crates/shamir-storage/src/storage_fjall.rs`)
does a full `memcpy` on every read even though fjall 3.x can hand back
refcounted, zero-copy bytes (audit finding 1.1), and does a redundant
`contains_key` LSM point-lookup before every `insert`/`set`/`remove`
mutation, doubling the cost of every point-write (audit finding 1.2),
`docs/dev-artifacts/audits/2026-07-06-perf-radical-o-notation.md`.

## Where — 1.1 (zero-copy reads)

- `crates/shamir-storage/Cargo.toml:32` (confirm current line):
  `fjall = { version = "3.0.1", optional = true }` — the `bytes`
  feature is NOT enabled. fjall 3.x's `Slice` type, when the `bytes`
  feature is on, becomes interchangeable with `bytes::Bytes` via a
  zero-copy conversion (`Bytes::from(slice)`, no memcpy — both are
  refcounted byte buffers).
- `crates/shamir-storage/src/storage_fjall.rs`: every read path
  currently does `Bytes::copy_from_slice(&slice)` (a full memcpy) at:
  - `:163` (`get`)
  - `:293` (`get_many`)
  - `:353` (`iter_stream` — BOTH key and value)
  - `:212-215` and `:428-431` (range/prefix scans)
  (Confirm current line numbers — this file may have shifted since
  2026-07-06.)

## Where — 1.2 (double LSM lookup on write)

- `crates/shamir-storage/src/storage_fjall.rs`:
  - `:114-123` (`insert`): does a `contains_key` check BEFORE the
    actual `insert` — a full LSM point-lookup (memtable → bloom filter
    → on-disk levels) that is REDUNDANT for `insert` specifically,
    because the key being inserted is always a fresh 128-bit random
    `RecordId` (collision probability ~2⁻¹²⁸ — the existing code
    comment near this check already acknowledges this via a TOCTOU
    discussion, but does not address the COST of the check).
  - `:142-148` (`set`): same `contains_key`-before-mutate pattern, used
    to report whether the key `existed` — but only some callers
    actually need that flag.
  - `:307-313` (`remove`): same pattern.

## Fix — 1.1 (zero-copy)

1. Enable the `bytes` feature on the `fjall` dependency in
   `crates/shamir-storage/Cargo.toml`.
2. Replace every `Bytes::copy_from_slice(&slice)` read-path call
   listed above with the zero-copy conversion (`Bytes::from(slice)` or
   whatever the fjall 3.x API actually exposes once the `bytes`
   feature is on — check fjall's docs/source for the exact conversion
   method name, since "zero-copy conversion" might be a `From`/`Into`
   impl, a `.into()` call, or a dedicated method — verify against the
   actual crate API rather than assuming the exact syntax).
3. Confirm this doesn't change the OBSERVABLE type/API of any
   `FjallStore` method — the return type should still be `bytes::Bytes`
   throughout (just constructed without a copy internally). No public
   API/signature change expected.

## Fix — 1.2 (remove double lookup)

1. **`insert`**: remove the `contains_key` check ENTIRELY — per the
   audit, it's provably pointless for the `insert` case (a fresh
   random `RecordId`). Confirm by reading the surrounding code and any
   existing TOCTOU-safety comment that this check truly serves no
   purpose beyond the (already-negligible) collision case, and that
   removing it doesn't silently change `insert`'s contract (e.g., does
   any caller rely on `insert` erroring/behaving differently if the
   key ALREADY existed? If `insert` is genuinely "always a fresh key"
   by construction everywhere it's called, removing the check is safe;
   if there's a real caller that relies on insert-vs-key-exists
   semantics, do NOT remove it there — investigate and report).
2. **`set`/`remove`**: per the audit's fix sketch, offer a variant that
   does NOT report the `existed` flag (since most callers don't need
   it and the engine, sitting on top of MVCC, usually already knows
   existence from its own bookkeeping) — OR extract the `existed` info
   from fjall's own return value if `insert`/`remove` on the underlying
   fjall API already returns the prior value/existence as a side effect
   of the single write operation (check fjall's actual
   `Partition`/`Keyspace` insert/remove API — some KV stores' `insert`
   returns `Option<old_value>` for free, which would let you derive
   `existed` from the SAME operation instead of a separate lookup).
   Investigate which approach fjall's actual API supports before
   deciding; report which you chose.
3. Check EVERY caller of the `existed`-reporting variants (`set`,
   `remove`) across the codebase to determine how many genuinely
   consume the `existed` flag vs. discard it — if the flag is discarded
   at most/all call sites, that strengthens the case for a
   flag-free fast path being the DEFAULT, with an explicit
   flag-reporting variant only where actually needed. Do not break any
   caller that genuinely needs `existed` — if in doubt, keep both an
   `_with_existed` variant (slow, does the check/derives it) and a
   fast default (no check), and migrate callers appropriately.

## Performance verification requirement (MANDATORY — this is a PERF task)

Per this repo's `/opti` methodology (see `CLAUDE.md`): a performance
task is NOT considered done without measured before/after numbers.

1. **Baseline benchmark BEFORE any change.** The audit notes
   (finding, section "fjall-бэкенд вообще не бенчится") that NO
   existing bench currently exercises `FjallStore` against a real
   tempdir-backed fjall instance — `membuffer_pump` and engine benches
   run in-memory, so findings 1.1/1.2 are currently invisible to any
   bench. **You must first ADD a `storage_fjall_pump` bench**
   (tempdir-backed, real fjall instance) covering:
   - point `get` (repeated reads of existing keys)
   - point `insert` (fresh random keys)
   - point `set` (existing keys, to exercise the `existed`-check path)
   - a small `scan_prefix`/`iter_stream` case
   Follow this repo's bench conventions: use
   `shamir_bench_utils::tune(...)` for QUICK-mode defaults (see
   `CLAUDE.md`'s bench section), and put it in whatever
   `crates/shamir-storage/benches/` (or workspace bench location) this
   repo's existing benches use as their pattern — check for an
   existing benches directory/Cargo.toml `[[bench]]` entries in
   `shamir-storage` first, and match that structure.
2. Run this NEW bench against the CURRENT (unfixed) code — this is
   your BASELINE. Record the numbers (ops/sec or ns/op for each
   variant).
3. Apply BOTH fixes (1.1 zero-copy, 1.2 remove-double-lookup).
4. Re-run the SAME bench — this is your AFTER. Record the numbers.
5. Report the baseline vs. after numbers explicitly, with the speedup
   ratio, per this repo's `/opti` commit-message convention (see
   `CLAUDE.md`'s `/opti` skill section for the exact format expected:
   "was X ms/op, stood Y ms/op -> Nx").
6. If EITHER fix shows no measurable improvement (or a regression),
   do NOT just ship it anyway — investigate why (e.g., is the
   `contains_key` check actually cheap in this fjall version due to a
   good bloom filter / warm cache in the bench scenario? Is the
   `bytes` feature conversion actually still doing a copy under the
   hood for some reason?) and report your findings honestly, including
   if a fix turns out NOT to deliver the audited benefit in practice.

## TDD/regression requirement

Since this changes runtime behavior (not just internal representation)
for the `existed`-flag paths:
1. Ensure existing `shamir-storage` tests covering `insert`/`set`/
   `remove`'s existing-key-detection semantics still pass — if you
   introduce a new flag-free fast-path variant alongside a slower
   flag-reporting one, both should have test coverage; if you simply
   remove the check from `insert` (per the "provably safe" reasoning
   above), confirm no existing test relies on `insert` behaving
   specially for an already-existing key.
2. Add tests for the zero-copy read path if there's any way to verify
   zero-copy-ness observably (e.g. `Arc::ptr_eq`-style reference
   counting between what fjall hands back and what `Bytes` ends up
   holding) — otherwise, rely on the existing correctness tests (values
   read back correctly) plus the bench numbers as the primary evidence
   of the fix's effect.

## Test scope command

```
./scripts/test.sh -p shamir-storage
./scripts/test.sh -p shamir-engine
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-storage -- --check
cargo clippy -p shamir-storage --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly (per this repo's `/opti` convention):
```
[Cycle: PERF-RADICAL-1]
  > Baseline:     <bench output for get/insert/set/scan, before>
  > Изменения:    fjall bytes feature (zero-copy) + removed
                   contains_key double-lookup on insert/set/remove
  > Тесты:        green / fixed N
  > After:        <bench output for get/insert/set/scan, after>
  > Δ:            <Nx for each op type>
```
- Confirm which callers of `set`/`remove`'s `existed` flag were found
  and how they were handled (kept, migrated to a fast-path variant,
  etc.).
- Confirm `insert`'s contains_key removal doesn't break any test/caller
  contract.
- Full test/gate results (exact commands + pass/fail).
