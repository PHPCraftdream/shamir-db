# Brief for F-33 Step 1 (#835, P1) — `MirroredStore` + key classifier

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is Step 1 of a 6-step campaign (F-33, #826) implementing a
`hybrid` repo engine: table DATA stays fully ephemeral (in-memory) while
table CONFIGURATION (index definitions, validator bindings, buffer
config, the interner) survives a process restart. Full design rationale
is in a research report already produced this session (not yet a
committed doc — this brief carries everything you need; ask if something
is genuinely ambiguous rather than guessing on a load-bearing detail).

**Read `crates/shamir-storage/src/types.rs`'s `Store` trait in full
first** (~line 32-320) — this task implements it.

## What to build

New file `crates/shamir-storage/src/storage_mirrored.rs` (register it in
`crates/shamir-storage/src/lib.rs`'s module list, matching this crate's
existing convention for `storage_in_memory.rs`/`storage_fjall.rs`):

```rust
pub struct MirroredStore {
    primary: InMemoryStore,
    mirror: Arc<dyn Store>,
    classify: fn(&RecordKey) -> bool,
}
```

- **`MirroredStore::new(mirror: Arc<dyn Store>, classify: fn(&RecordKey) -> bool) -> DbResult<Self>`**
  (async — hydration needs I/O): construct an empty `InMemoryStore` as
  `primary`, then hydrate it by streaming EVERY entry out of `mirror` via
  `mirror.iter_stream(...)` and writing each into `primary` (bypassing
  the classifier — everything already in the mirror is, by construction,
  something that was classified as durable when it was written, so it
  belongs in the primary too). Config state is small (a handful of keys
  plus interner chunks) — a full streamed hydration at construction time
  is cheap and simple; do not over-engineer this into anything lazy.
- **Reads** — `get`, `get_many`, `iter_stream`, `scan_prefix_stream`,
  `iter_range_stream`, `iter_range_stream_reverse`: delegate to `primary`
  ONLY. Never touch `mirror` on the read path — this is what keeps
  `MirroredStore` zero-cost after open.
- **Writes** — `insert`, `set`, `remove`: write to `primary`
  UNCONDITIONALLY (matching `InMemoryStore`'s existing semantics exactly
  — return value shape, `NotFound` on missing-key `remove`, etc.), and
  ADDITIONALLY write to `mirror` IF `classify(&key)` returns `true`.
  `insert()` mints a random `RecordId`-derived key
  (check `InMemoryStore::insert`'s exact key-generation to mirror it) —
  this can NEVER be a system/config key by construction, so `insert`
  never needs to write to `mirror` at all (verify this claim by checking
  how `RecordKey`s from `insert` are shaped vs. the classifier's
  allowlist, which only matches system-record-shaped keys — state your
  confirmation in your summary).
  - For `set`/`remove`, only call through to `mirror` when
    `classify(&key)` is true — an unclassified write must NEVER touch
    disk.
- **`insert_many`/`set_many`/`remove_many`/`transact`**: investigate
  whether the `Store` trait's DEFAULT implementations (which loop calling
  `self.set`/`self.insert`/`self.remove` per item —
  check `types.rs` ~line 148-227) already produce CORRECT
  classify-and-conditionally-mirror behavior for free, simply because
  your own `set`/`insert`/`remove` overrides already do the
  classification internally. If so, do NOT write custom
  `set_many`/`remove_many`/`transact` overrides — rely on the default
  trait methods (smaller diff, less to get wrong). Only write a custom
  override if you find the default's per-item looping is genuinely
  wrong or unacceptably inefficient for this store's specific case
  (state your reasoning either way in your summary).
- **`flush()`**: flush `mirror` only (the primary is in-memory, nothing
  to flush there).
- **`raw_backend()`**: return `None` (this is not a cache-wrapper store
  in the sense that method's doc comment describes — check
  `types.rs`'s doc for `raw_backend`'s existing contract/callers before
  finalizing this, since getting it wrong could cause a caller to bypass
  this store's classification logic entirely).
- **`apply_buffer_config`**: forward to `mirror` only (the primary,
  being pure in-memory, has no buffer tuning to apply).

## The classifier

A **free function** (not a closure — `fn(&RecordKey) -> bool`, so it can
be stored as a plain function pointer and is trivially testable in
isolation), e.g. `pub fn is_durable_table_config(key: &RecordKey) -> bool`.

**Allowlist semantics** (a key defaults to EPHEMERAL unless explicitly
recognized — this direction matters: a forgotten-to-add future config key
just means "a setting doesn't survive restart" (a bug report), while a
forgotten-to-EXCLUDE key would mean "stale derived state silently
corrupts a reopened index" (much worse) — so the allowlist bias is
deliberate, not accidental):

```
is_durable(key) :=
      key.len() == 16
   && key[0..4] == [0, 0, 0, 0]      // system-record prefix
   && tag ∈ ALLOWLIST

where tag = key[4..16] with trailing NUL bytes trimmed
      (mirror RecordId::system's own encoding — check
       crates/shamir-types/src/types/record_id.rs's `system()`
       constructor and `SYSTEM_RECORD_PREFIX` for the exact byte
       layout to match precisely, do not guess the offsets)

ALLOWLIST = {
    "indexes", "indexes_unique", "sorted_indexes",
    "_m.idx", "_m.idx.lfv", "_m.val", "buffer_config",
    "internals", "_m.tbl", "_m.wal", "_m.mig",
}
∪ { tags with prefix "i.d" }   // interner chunks — check
                                 // crates/shamir-engine/src/table/
                                 // interner_manager.rs (~line 82) for
                                 // this exact tag/prefix convention

EXPLICITLY EXCLUDED (must return false): "count" (the RecordCounter's
persisted key — MetaKey::Count — this is the ONE 16-byte system key that
is DERIVED FROM DATA, not configuration; persisting it would make a
reopened hybrid table's row count lie about how many rows actually
exist post-restart).
```

Cross-check EVERY tag against `crates/shamir-engine/src/meta/namespace.rs`'s
`MetaKey` enum (or wherever the canonical list of system-record tags
lives in this codebase) — do not invent tag strings from this brief
alone; confirm each one's exact string against the source, and find any
`MetaKey` variants this brief's list might have missed.

Non-system keys (postings, vector snapshot chunks, vector delta log
entries) do NOT match the `len()==16 && prefix==[0,0,0,0]` shape at all
(verify each of these key shapes against their actual construction sites
— `crates/shamir-index/src/legacy/index_keys.rs`,
`crates/shamir-index/src/posting_layout.rs`,
`crates/shamir-index/src/legacy/sorted_index_manager.rs`,
`crates/shamir-index/src/vector/snapshot.rs` — to confirm each one falls
through the classifier's FIRST check (`len() == 16`) and is correctly
excluded without even reaching the tag comparison).

## Tests

**MANDATORY, test-then-fix in the same commit**, in a new
`crates/shamir-storage/src/tests/storage_mirrored_tests.rs`:

1. A classified key: `set` it via `MirroredStore` → confirm it's visible
   in the underlying `mirror` store directly (not just via the
   `MirroredStore` facade) → construct a FRESH `MirroredStore` over the
   SAME `mirror` → confirm the value hydrates back into the new
   instance's reads.
2. An unclassified key: `set` it → confirm it is ABSENT from the
   underlying `mirror` (check `mirror.get(key)` directly returns
   `NotFound`) → construct a fresh `MirroredStore` over the same mirror →
   confirm the value is GONE (it never persisted, so hydration can't
   bring it back).
3. If you kept `set_many`/`transact` as custom overrides (not relying on
   trait defaults): a mixed batch of classified + unclassified keys in
   one call → confirm the split is correct (classified subset in mirror,
   unclassified subset absent). If you relied on the trait's default
   loop-based impls instead, this is likely already covered by tests 1-2
   exercised through the batch API — add a batch-specific test only if
   it exercises something tests 1-2 don't.
4. `remove` of a classified key removes it from `mirror` too (a dropped
   index definition must not resurrect on the next hydration).
5. `scan_prefix_stream` after hydration returns the mirrored keys
   correctly (this exercises the interner-chunk scan shape used by
   `interner_manager.rs` — check that file for how it actually calls
   `scan_prefix_stream` and mirror a realistic case).
6. **Classifier exhaustiveness guard** (the most important test): iterate
   every `MetaKey` variant found in `namespace.rs` and assert the
   classifier's verdict matches this brief's intended allowlist for each
   one (especially: `Count` → `false`, everything else in the allowlist →
   `true`). Also assert representative NON-system keys (a posting key
   shape, a sorted-index entry key shape, a vector snapshot key shape) are
   all `false`. This test is what catches a FUTURE MetaKey addition that
   forgot to update the classifier — write it so a new variant genuinely
   fails this test if unclassified, rather than silently passing.

## Constraints

- Do NOT touch `InMemoryStore`, `FjallStore`, or the `Store` trait itself
  — this is a NEW, additive type composing them.
- Do NOT wire this into `shamir-engine`'s repo/`BoxRepo` types yet — that
  is Step 2 (#836), a separate task, blocked on this one landing first.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-storage -- --check` and
  `cargo clippy -p shamir-storage --all-targets -- -D warnings` must be
  clean.
- Follow workspace conventions: `use` at file top, one primary export per
  file, `THasher`/`scc`/`ArcSwap` per the concurrency ideology where
  applicable (this store has no concurrent-map state of its own beyond
  what `InMemoryStore` already provides, so this constraint is mostly
  N/A here — just don't introduce a new `std::sync::Mutex`/`RwLock`
  without justification).

## Verification the orchestrator will run

```
cargo fmt -p shamir-storage -- --check
cargo clippy -p shamir-storage --all-targets -- -D warnings
./scripts/test.sh -p shamir-storage -- mirrored
./scripts/test.sh -p shamir-storage --full
```
