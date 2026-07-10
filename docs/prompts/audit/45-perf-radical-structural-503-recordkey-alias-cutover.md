Task: `RecordKey` alias cutover from `Bytes` to `KeyBytes`. Task #503,
step 2 of the migration sequence in
`docs/design/record-key-128-migration-plan.md` (task #491, audit finding
3.1). Step 1 (the `KeyBytes` type itself, fully tested, zero call-site
changes) is DONE and committed — see `crates/shamir-storage/src/key_bytes.rs`.

## Read the plan doc FIRST

`docs/design/record-key-128-migration-plan.md` §3 and §4 item 2 describe
this exact step in detail — required trait surface, expected fix classes,
and 7 documented landmines with mitigations (§5). This brief summarizes the
relevant parts but the plan doc is the authoritative source; read it in
full before starting.

## The change

`crates/shamir-storage/src/types.rs:8` currently reads:

```rust
pub type RecordKey = Bytes;
```

Flip it to:

```rust
pub type RecordKey = KeyBytes;
```

(`KeyBytes` is defined in `crates/shamir-storage/src/key_bytes.rs`, already
`pub` from that crate — check current re-export path, likely needs adding
to `crates/shamir-storage/src/lib.rs`'s public exports if not already
there.)

This is a **mechanical, compile-driven cutover — NO logic changes**. The
diff should consist entirely of: (a) the one-line alias flip, (b) fixing
every resulting compile error by adjusting `Bytes`-specific call sites to
their `KeyBytes` equivalent, (c) nothing else. If you find yourself wanting
to change behavior anywhere, STOP and flag it to the orchestrator instead
of silently doing it — this step must be a pure representation swap.

## Expected fix classes (per plan doc §4 item 2, confirm against current code)

- Test literals: `Bytes::from_static(b"k1")` → `RecordKey::from(&b"k1"[..])`
  or `KeyBytes::from_slice(b"k1")` (check which conversions actually compile
  — `KeyBytes` has `From<Bytes>`, `From<Vec<u8>>`, `From<&'static [u8]>`,
  plus the `from_slice` workhorse constructor; use whichever fits each call
  site with the least churn).
- Explicit `Bytes` ↔ `RecordKey` boundary conversions in backends:
  `crates/shamir-storage/src/storage_fjall.rs`,
  `crates/shamir-storage/src/storage_in_memory.rs`,
  `crates/shamir-storage/src/storage_cached.rs`,
  `crates/shamir-storage/src/storage_membuffer.rs`. `KeyBytes` implements
  `Deref<Target=[u8]>`/`AsRef<[u8]>`/`Borrow<[u8]>`, so `&key[..]`-shaped
  call sites (fjall's raw-byte API) should keep compiling unchanged or with
  trivial adjustment.
- Stream item construction (`Vec<(RecordKey, Bytes)>` tuples).
- `crates/shamir-wal/src/active_key.rs` (`WalActiveKey` parse/construct),
  `crates/shamir-engine/src/migration/shadow_key.rs`, vector
  `crates/shamir-index/src/vector/snapshot.rs` string-shaped keys.
- Any `RecordKey`-vs-`Bytes` equality assertions in tests — `KeyBytes`
  already has `PartialEq<Bytes>` (slice-delegating) for this.

## Landmines to avoid re-introducing (plan doc §5 — read the full detail there)

1. **NEVER derive `PartialEq`/`Eq`/`Hash`/`Ord` on anything wrapping
   `KeyBytes`'s internals** — `KeyBytes` itself already hand-implements
   these correctly over `as_slice()` (do not touch `key_bytes.rs`'s trait
   impls). If a NEW wrapper type is needed anywhere during this cutover
   (it shouldn't be), the same rule applies.
2. **Do not introduce a `u128`-based key anywhere** — inline repr is
   `[u8; N]`, ordering is byte-slice `Ord`, which is what makes
   `RecordId`'s BE-timestamp-prefix ordering and `WalActiveKey`'s BE
   txn_id ordering keep working. This is already how `KeyBytes` is built;
   just don't add anything that reinterprets key bytes numerically.
3. **Serde format**: `KeyBytes` already serializes as a plain byte-blob
   identical to the `serde_bytes` encoding of `Bytes` (guarded by step 1's
   tests) — do not change this. If the cutover surfaces a NEW serde
   call site not covered by step 1's tests, verify it byte-for-byte against
   the equivalent `Bytes` encoding before assuming it's fine.
4. **fjall read path**: `fjall::Slice → Bytes → KeyBytes` — the inline
   copy is cheap for ≤23-byte keys (see `key_bytes.rs`'s doc comment for
   why `INLINE_CAP = 23`, not the plan doc's nominal 30 — an already-landed,
   documented deviation from measuring real `bytes::Bytes` size on this
   target); for longer keys it's `Heap(bytes)` zero-copy via `From<Bytes>`.

## Verification (MANDATORY — this touches the on-disk/on-wire key format path)

The plan doc explicitly calls out `crates/shamir-index/src/legacy/tests/index_manager_tests/byte_identity_tests.rs`
as THE guard proving on-disk keys are unchanged — it MUST stay green
unmodified (if it needs ANY change to pass, stop and report why; that
would indicate an actual format change, which is not in scope for this
step).

Because this is a workspace-wide cutover (not a single-crate change), run
the FULL test suite, not a scoped subset:

```
./scripts/test.sh --full
```

## Gate

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

If `fmt --check` fails, do NOT run `cargo fmt --all` (that reformats the
whole repo) — fix formatting only in the files you touched
(`cargo fmt -p <crate>` per touched crate, or by hand).

If clippy flags PRE-EXISTING lints in code you did not touch, do not fix
them here — note them in your final report instead.

## Performance note

This step is NOT expected to show a measurable perf win by itself (that's
step 3 — replacing `rid.to_bytes()` call sites with alloc-free
`KeyBytes::from_slice(rid.as_bytes())` constructors, a separate follow-up
task, blocked on this one). This step is purely the representation swap;
do not add or run new benches for it.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

```
[Implementation] Status: fixed / partially-fixed / blocked
  > Confirm: pub type RecordKey = KeyBytes; landed at types.rs:8
  > List every file touched and WHY (which fix class from the list above)
  > Confirm byte_identity_tests.rs required ZERO changes (or explain if not)
  > Confirm the diff contains no logic changes beyond mechanical type fixes
```
Full test/gate results (exact commands + pass/fail). List the full set of
changed files.
