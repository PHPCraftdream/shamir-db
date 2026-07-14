Task: PERF-RADICAL-STRUCTURAL step 1 (of the sequence in
`docs/dev-artifacts/design/record-key-128-migration-plan.md` — READ THAT DESIGN DOC
FIRST, in full, before writing any code; it contains file:line citations
for the current state and the exact rationale for every decision below).

This is STEP 1 ONLY of a 4-5 step sequence. Step 1 adds a new type with
**ZERO call-site changes** anywhere else in the workspace — do not touch
`crates/shamir-storage/src/types.rs`'s `RecordKey` alias, do not touch
any other crate. This step must compile, test, and merge completely
independently of the later cutover steps (which are separate follow-up
tasks, not your job here).

## Goal

Add `KeyBytes` — a representation-transparent, small-buffer-optimized
byte-string type — to `crates/shamir-storage`, fully tested, completely
unused by any other code yet.

## Design (from the plan doc, §3 — implement exactly this shape)

New file `crates/shamir-storage/src/key_bytes.rs` (one primary export
per file, per this repo's convention):

```rust
/// Small-string-optimized key: inline for len <= INLINE_CAP, heap `Bytes` beyond.
/// Representation is UNOBSERVABLE: Eq/Ord/Hash/serde are defined over the byte
/// slice only. Total size 32 bytes = same as `bytes::Bytes` on x86-64.
pub struct KeyBytes(Repr);

const INLINE_CAP: usize = 30; // covers 16B RecordId, 21B WAL, 25B unique-index, 9B prefix

enum Repr {
    Inline { len: u8, buf: [u8; INLINE_CAP] },
    Heap(Bytes),
}
```

Required trait/API surface (implement all of these):
- `Deref<Target = [u8]>`, `AsRef<[u8]>`, `Borrow<[u8]>`.
- `Clone`, `Debug`.
- `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash` — **MUST delegate to
  `self.as_slice()` (or equivalent), NEVER derived over the `Repr` enum
  directly.** This is the single most important correctness constraint
  in this task — see Landmine 1 below.
- `PartialEq<[u8]>` and `PartialEq<Bytes>` conveniences (slice-delegating).
- `From<Bytes>`, `From<Vec<u8>>`, `From<&'static [u8]>` — inline-copy
  when the input fits in `INLINE_CAP`, otherwise store as `Heap`.
- `impl From<KeyBytes> for Bytes` (heap variant: zero-copy move; inline
  variant: one copy — this is a cold/boundary conversion, not a hot path).
- `KeyBytes::from_slice(&[u8]) -> Self` — the workhorse constructor,
  inline when it fits, else heap-copies into a `Bytes`.
- `Serialize`/`Deserialize` (serde) — MUST encode as a plain byte-blob,
  byte-for-byte IDENTICAL to how `serde_bytes`/`Bytes` currently encodes
  the same byte sequence for BOTH `bincode` and `rmp-serde`. This is
  mandatory — see Landmine 3 below. Do NOT let serde derive over the
  `Repr` enum (that would add a variant tag byte and break wire/disk
  compatibility for a LATER step, even though this step doesn't touch
  any real wire path yet — the type must be ready).
- `size_of::<KeyBytes>() == 32` (assert this in a test — same size as
  `bytes::Bytes` on this target).

## Mandatory TDD — write these tests FIRST, they define correctness

In `crates/shamir-storage/src/key_bytes.rs`'s own `tests/` submodule (per
this repo's test-organization convention — one `tests/` dir, `tests/mod.rs`
manifest, topic-split files, wired via `#[cfg(test)] mod tests;` in the
parent):

1. **Property test — Eq/Ord agreement with `Bytes`.** For a range of
   random byte strings of length 0..64 (include boundary lengths 0, 1,
   `INLINE_CAP-1`, `INLINE_CAP`, `INLINE_CAP+1`, 64), construct both a
   `KeyBytes` and a `Bytes` from the same bytes, and confirm: (a)
   `KeyBytes::as_ref() == Bytes::as_ref()` for the same input: (b) two
   `KeyBytes` built from byte strings that would sort differently must
   sort the same way `Bytes`/`[u8]` would (lexicographic byte order) —
   test with pairs that specifically probe cross-length comparisons
   (e.g. a 5-byte key vs a 6-byte key where the 5-byte key is a strict
   prefix — must compare as `Less`, matching `[u8]`'s `Ord`, NOT numeric
   comparison of any kind).
2. **Hash consistency — inline vs heap of THE SAME bytes must hash
   identically.** Expose a test-only construction path that FORCES the
   heap variant even for a short (≤30-byte) input — e.g. a
   `#[cfg(test)] pub(crate) fn force_heap_for_test(bytes: &[u8]) -> KeyBytes`
   that directly builds `Repr::Heap(Bytes::copy_from_slice(bytes))`
   bypassing the normal inline-preferring constructor. Then: for the
   SAME short byte sequence, build one `KeyBytes` via the normal
   `from_slice` (which will pick `Inline`) and one via
   `force_heap_for_test` (which picks `Heap`) — assert they are `==`
   AND hash to the same value under `shamir_collections::THasher`
   (`FxHasher`) AND under the default `std::hash::DefaultHasher`. This
   is the exact class of bug this campaign already found and fixed once
   in task #489 (Value<Key>'s Hash/Eq NaN inconsistency) — do not skip
   this test, and do not derive Hash on the enum.
3. **Serde byte-identity vs `Bytes`.** For several representative byte
   strings (0, 15, 30, 31, 41 bytes — cover both inline and heap paths),
   confirm `bincode::serialize(&KeyBytes::from_slice(&b))` ==
   `bincode::serialize(&Bytes::copy_from_slice(&b))` byte-for-byte
   (adjust to whatever this workspace's actual bincode/serde_bytes
   wrapper convention is — grep `crates/shamir-wal` and
   `crates/shamir-storage` for how `Bytes`/generic KV keys are currently
   (de)serialized and match that EXACT encoding, since correctness here
   is judged against on-disk/on-wire compatibility for a later step, not
   against serde defaults). Also confirm `rmp_serde` encoding parity if
   that's used anywhere for these keys (grep before assuming). Round-trip:
   `deserialize(serialize(x)) == x` for both variants.
4. **Size assertion**: `assert_eq!(std::mem::size_of::<KeyBytes>(), 32)`.
5. **Boundary/edge cases**: empty key (`KeyBytes::from_slice(&[])`),
   exactly `INLINE_CAP` bytes (must be `Inline`), `INLINE_CAP + 1` bytes
   (must be `Heap`) — assert the internal representation choice directly
   via a `#[cfg(test)]` accessor if needed (e.g. `is_inline()` gated to
   test builds, or check via the `force_heap_for_test` pattern above to
   compare, not necessarily exposing internals in the public API).
6. **`Debug` doesn't panic and is reasonably readable** (basic smoke test).
7. **`From<Bytes>`, `From<Vec<u8>>`, `From<&'static [u8]>`,
   `From<KeyBytes> for Bytes`** — each has at least one round-trip test.

## What you must NOT do in this task

- Do NOT touch `crates/shamir-storage/src/types.rs`'s `pub type RecordKey
  = Bytes;` line — leave the alias exactly as-is. This type is added
  ALONGSIDE, unused by production code, in this step.
- Do NOT touch any other crate (shamir-index, shamir-engine, shamir-tx,
  shamir-wal, shamir-server, etc.) — this is a self-contained addition to
  shamir-storage only.
- Do NOT derive `PartialEq`/`Eq`/`Hash`/`Ord` on the `Repr` enum or on
  `KeyBytes` via `#[derive(...)]` — every one of these MUST be a
  hand-written impl that goes through the byte-slice view. A derived
  impl would compare/hash the `len`/tag/padding bytes too, which is
  exactly the representation-leaking bug this design exists to prevent.
- Do NOT add a `u128`-based representation anywhere — the design doc
  (§5.2) explicitly rules this out because `RecordId`'s big-endian
  timestamp prefix makes byte-order (not numeric order) the correct
  comparison; an inline `[u8; N]` buffer with slice-`Ord` sidesteps this
  automatically, a `u128` would not (unless very carefully handled, and
  the design doc's decision is to avoid that footgun entirely by not
  using `u128` at all).

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-storage -- --check
cargo clippy -p shamir-storage --all-targets -- -D warnings
./scripts/test.sh -p shamir-storage
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not fix
them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report:
```
[KeyBytes type] Status: implemented
  > File: crates/shamir-storage/src/key_bytes.rs
  > Test count + pass/fail for each of the 7 mandatory test categories above
  > Confirmation: zero changes to types.rs or any other crate (git diff --stat)
  > Confirmation: no derive(PartialEq/Eq/Hash/Ord) anywhere on Repr/KeyBytes
  > Any open questions about the exact current serde encoding convention
    for Bytes-keyed WAL/storage entries that you had to investigate/match
```
Full test/gate results (exact commands + pass/fail).
