Task: #532 — re-key `MvccStore`'s in-memory coordination maps from `Bytes`
to `RecordKey` (`KeyBytes`). Follow-up from task G1/#525's `@fl` review:
some overlay/reservation maps stayed `Bytes`-keyed after the #503
`RecordKey` alias cutover, so an already-alloc-free `KeyBytes` value gets
converted BACK to `Bytes` (a real heap allocation) on every commit/scan
op via `.into()`.

## Context (confirmed by reading current code — re-confirm line numbers,
## they may have shifted since #525/#527/#528 touched nearby code)

The `Bytes`-keyed structures:
- `crates/shamir-tx/src/mvcc_store/mod.rs:130` —
  `cells: SccHashMap<Bytes, RecordCell, THasher>`
- `crates/shamir-tx/src/mvcc_store/mod.rs:135` —
  `locks: SccHashMap<Bytes, Arc<KeyLockInner>, THasher>`
- `crates/shamir-tx/src/versioned_overlay.rs:27` —
  `type OverlayKey = (Bytes, u64);` used by
  `VersionedOverlay`'s `tree: TreeIndex<OverlayKey, Bytes>`

Public API taking `key: Bytes` that would need to become
`key: RecordKey` for the re-key to actually eliminate the round-trip
allocation (confirmed current signatures):
- `crates/shamir-tx/src/mvcc_store/mod.rs`: `set_versioned` (~686),
  `set_versioned_many` (~773, takes `Vec<(Bytes, Bytes)>`),
  `set_versioned_many_append_only` (~885), `get_current` (~1042)
- `crates/shamir-tx/src/mvcc_store/mvcc_history.rs`: `seed_version` (~251),
  plus `publish_cell`/`try_reserve`/`finalize_reservation`/
  `release_reservation` — re-grep for their current signatures, the line
  numbers in this brief may be stale.
- `crates/shamir-tx/src/cell_reservation_guard.rs:69` —
  `CellReservationGuard::add(&mut self, key: Bytes)`
- `crates/shamir-tx/src/tx_context.rs`: `record_read` (~458),
  `record_read_shared` (~485)
- shamir-engine's `acquire_pessimistic_{read,write}_lock` (re-grep,
  location may have shifted)

**Note:** `get_current` already has a zero-alloc sibling
`get_current_bytes(&self, key: &[u8])` (mentioned in earlier campaign
context) — confirm this exists and whether it's ALREADY `RecordKey`-
compatible via `&[u8]` (a borrow works regardless of the underlying
map's key type via `Borrow<[u8]>`, which `KeyBytes` already implements
per task #491/#503) — if so, some call sites may already be getting the
zero-alloc benefit and don't need touching; focus this task on the
methods that only have a `Bytes`-taking form.

## The change (mechanical, no logic change — same discipline as #503)

1. Change `cells`, `locks`, `OverlayKey`'s `Bytes` component to
   `RecordKey`. `RecordKey` is `KeyBytes` (already `SccHashMap`/
   `TreeIndex`-compatible — it implements `Eq`/`Ord`/`Hash` correctly
   over the byte slice, per task #503's landing).
2. Change every public method above from `key: Bytes` to
   `key: RecordKey`. This is a genuine, deliberate PUBLIC API break
   within `shamir-tx` — confirmed acceptable since this is an internal
   engine crate, not a client-facing wire type (recheck this assumption
   against `shamir-tx`'s actual consumers before proceeding: is
   `shamir-tx` used ONLY by `shamir-engine`, or does anything external
   depend on its public API directly? If ANYTHING outside this
   workspace's own crates depends on `shamir-tx`'s public surface,
   STOP and flag it — this task assumes it's purely internal).
3. Update every CALLER across `shamir-engine`/`shamir-tx` to pass
   `RecordKey` directly instead of converting from `RecordKey` to
   `Bytes` first — this is the whole point: callers that already HAVE a
   `RecordKey` (e.g. from `RecordKey::from_slice(rid.as_bytes())` per
   task G1/#525) should now pass it straight through with ZERO
   conversion, not convert-then-reconvert.
4. Where a caller genuinely only has `Bytes` (e.g. a value arriving from
   a generic KV path with no `RecordId` structure), convert ONCE at that
   boundary (`RecordKey::from(bytes)`) — this is unavoidable and fine,
   the goal is eliminating REDUNDANT round-trips, not the occasional
   single necessary conversion.
5. Never derive `Eq`/`Ord`/`Hash`/`PartialEq` on anything wrapping
   `RecordKey`/`KeyBytes` internals — this rule from #503 still applies;
   `OverlayKey = (RecordKey, u64)`'s tuple `Ord`/`Eq` derive is fine
   (Rust's tuple derive delegates to each field's own `Ord`/`Eq`, which
   for `RecordKey` is already the correct hand-written impl — this does
   NOT violate the rule, since we're not deriving over `KeyBytes`'s
   internal `Repr` enum, just composing already-correct impls).

## Scope-down guidance

If investigation reveals the caller-side blast radius is much larger
than expected (e.g. dozens of call sites across shamir-engine doing
non-trivial logic around these keys, not just simple pass-through), or
if `shamir-tx`'s public API turns out to have external consumers this
brief didn't anticipate, STOP and document the specific blocker + a
narrower follow-up rather than forcing a risky, large mechanical change.

## TDD

1. Existing `shamir-tx` and `shamir-engine` test suites must stay green
   — this is a pure representation change, no behavior change intended.
2. If any NEW conversion boundary is introduced (step 4 above), confirm
   there's no silent byte-representation change (KeyBytes's `From<Bytes>`
   and `Into<Bytes>` are already byte-identical, verified in #503 — this
   should be a non-issue, but re-confirm if you touch anything new).

## Performance verification (this is a PERF task)

Bench before/after using `tx_pipeline` / `tx_overhead` (check
`crates/shamir-tx/benches/` for the exact bench names) — this is
specifically the commit/scan hot path the round-trip allocation sat on.
Report honest numbers; if the effect is below the noise floor (as G1/
#525's benches were for a similar single-allocation change), say so
rather than fabricating a signal — the value of this change may be
allocation-count hygiene more than a measurable wall-clock win, same as
#525's honest conclusion.

## Test scope

```
./scripts/test.sh -p shamir-tx -p shamir-engine
```

## Verification (lighter per-task gate, agreed this session)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-tx -p shamir-engine
```
Do NOT run the full fmt/clippy/test --full gate — that's FINAL-GATE's job.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

```
[Implementation] Status: fixed / scoped-down-with-followup
  > Which structures/methods were re-keyed (confirm shamir-tx is
    internal-only before proceeding, per step 2 above)
  > Caller-side changes: confirm redundant round-trips eliminated,
    not just moved
  > Bench: before/after, honest numbers
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-tx -p shamir-engine: pass/fail
```
