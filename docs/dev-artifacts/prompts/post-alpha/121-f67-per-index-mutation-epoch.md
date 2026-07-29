# Brief for F-67 (#893, P1) — per-index mutation epoch instead of manager-wide high-water

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. An independent readonly review of
snapshot `e145b1d3` (`docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md`,
section P1-4) flagged that `SortedIndexManager::last_mutation_version()`
(`crates/shamir-index/src/legacy/sorted_index_manager.rs:140-213`) is a
single manager-wide `AtomicU64` high-water mark, but the AsOf cursor
index-seek fast path (`read_as_of_keyset_seek`,
`crates/shamir-engine/src/table/read_asof_seek.rs`) always plans against
ONE specific sorted index (identified by `index_name: u64`, its
`name_interned` id). Consequence: mutating any UNRELATED sorted index on
the same table bumps the shared high-water and disables the seek fast
path for every cursor on that table, not just cursors reading the index
that actually changed — a correctness-preserving but needlessly broad
performance cliff.

**This is a SCOPE-NARROWING task, not a correctness fix.** F-58 (#884,
already landed, commit `15b5a729`) closed a genuine TOCTOU race in this
same mechanism by adding a POST-scan re-check of
`last_mutation_version() > pinned_version` (see
`read_asof_seek.rs:259-272`'s comment block and its module doc,
`read_asof_seek.rs:1-57`, for the full ordering proof — read it first,
it explains exactly how the entry gate + post-scan re-check pair
together). F-58 kept the manager-wide high-water gate mechanism AS-IS —
it did not replace it with a seqlock/epoch design — so this task's
premise still holds exactly as described. Read F-58's landed diff
(`git show 15b5a729`) before starting, both for the mechanism you're
narrowing and as a style/reasoning precedent (the same monotonic-counter
proof pattern you'll need to redo per-index).

### Where the gate is checked today (entry point)

`crates/shamir-engine/src/table/read_temporal.rs:97`:
```rust
if self.sorted_indexes().last_mutation_version() <= version {
    if let Some(result) = self
        .read_as_of_keyset_seek(
            query,
            ctx,
            ...
```
`try_plan_keyset_seek` (called just above, `read_temporal.rs:95`) already
determines WHICH index the seek will use — confirm exactly what it
returns and how `index_name`/the index identity threads from there into
this gate check and into `read_as_of_keyset_seek`'s own `index_name: u64`
parameter (`read_asof_seek.rs:157`).

### Where the gate is bumped today (apply-time sites — three of them)

1. **Non-tx direct path** — `sorted_index_manager.rs`'s
   `on_record_created`/`on_record_updated`/`on_records_created_batch`/
   `on_record_deleted` (lines 573-657) each call
   `self.note_mutation_at_version(version)` unconditionally at entry,
   BEFORE planning/applying that specific call's ops. Each of these
   methods already has full type information about which record/fields
   changed — the planners they call internally (`plan_record_created`,
   `plan_record_updated`, `plan_record_deleted`, all in the same file,
   ~line 405-540) iterate `for def in &defs` and know `def.name_interned`
   for every op they build.
2. **Tx-commit path** — `crates/shamir-engine/src/tx/commit_phases.rs`'s
   `apply_index_batch` (~line 567-604) calls
   `tbl.sorted_indexes().note_mutation_at_version(commit_version)`
   unconditionally at the end, regardless of whether `ops: &[IndexWriteOp]`
   contains any entries at all, let alone which index(es) they target.
   `IndexWriteOp::{SetPosting, RemovePosting}` (defined near
   `sorted_index_manager.rs`'s planner section) carry only `{ key, value
   }` — the `name_interned` id is NOT a separate field, it's encoded as
   a big-endian-prefix inside `key` (see `build_entry_key`/`entry_prefix`,
   ~line 1216-1235). You will need either (a) a way to decode which
   index(es) `ops` touched from the key prefixes, or (b) a shape change
   so the per-op index identity is available without decoding raw bytes
   (e.g. threading it through as a separate field, or changing this
   apply site to receive per-index op groups instead of one flat
   `Vec<IndexWriteOp>`). Investigate both `apply_index_batch`'s own
   call site (does its caller already have per-index-grouped ops
   anywhere upstream, before they get flattened into one `Vec`?) and
   `IndexWriteOp`'s definition before deciding.

## What to do

1. **Read the full context first**: `sorted_index_manager.rs:94-213`
   (the `generation`/`last_mutation_version` field docs — `generation`
   is a useful sibling pattern, a manager-wide monotonic counter that
   this task will NOT touch, only `last_mutation_version`), `:405-540`
   (the three planners), `:573-657` (the four apply-time bump call
   sites), `commit_phases.rs:555-604` (`apply_index_batch`),
   `read_asof_seek.rs` in full (module doc + the gate check + the
   post-scan re-check you must ALSO narrow to per-index), and
   `read_temporal.rs`'s entry-gate call site.
2. **Design the per-index epoch structure.** Replace the single
   `last_mutation_version: Arc<AtomicU64>` field with a per-index
   mapping (keyed by `name_interned: u64`) from index id → its own
   monotonic high-water `AtomicU64`. Per this repo's NORMATIVE
   concurrency invariants (`CLAUDE.md` pillar 5's drop-in checklist,
   "Shared registry, key-value" row): use `scc::HashMap<u64, AtomicU64,
   THasher>` (or equivalent lock-free structure) — do NOT reach for
   `std::sync::Mutex`/`RwLock`/`DashMap` unless you can justify why the
   checklist's default doesn't fit here. An index that has never been
   mutated should read as epoch `0` (same default-empty semantics the
   old manager-wide counter had at construction).
3. **Update the two read sites** (`read_temporal.rs`'s entry gate,
   `read_asof_seek.rs`'s post-scan re-check) to look up ONLY the epoch
   for the specific `index_name` the seek is planned against, not the
   whole-manager value. Keep both checks' Acquire/Release semantics —
   re-derive (don't assume) that the monotonic-counter proof from F-58's
   module doc still holds when the counter is per-index instead of
   manager-wide (the argument should transfer directly: any mutation the
   scan could have observed for THIS index still bumps THIS index's
   counter, same as before).
4. **Update the apply-time bump sites** to bump only the epoch(s) for
   the index(es) actually touched by that call's ops, not every index
   unconditionally. For the non-tx direct path
   (`on_record_created`/`on_record_updated`/`on_record_deleted`/
   `on_records_created_batch`), the cleanest point is likely inside (or
   right after) each planner's `for def in &defs` loop, where
   `def.name_interned` is already in scope — bump only for defs that
   actually produced an op (i.e. `extract_and_encode` returned `Some`),
   not every registered index. For `commit_phases.rs::apply_index_batch`,
   resolve per-op index identity per your step-1 investigation and bump
   only the epochs for indexes with at least one op in `ops`.
5. **Add a test** proving the scope-narrowing: mutating sorted index B
   does NOT disable the AsOf seek fast path for a cursor planned against
   index A, while mutating index A still correctly disables (and the
   full-scan fallback still produces the correct page — reuse the
   negative-case pattern from F-53b Step 1's spike / existing
   `f53b_asof_seek_tests.rs` tests as a style reference). Also re-verify
   (not just re-run) that the EXISTING F-58 TOCTOU-closing test
   (`f58_post_check_catches_mid_scan_delete` in
   `f53b_asof_seek_tests.rs`) still passes and still genuinely exercises
   a same-index race (it should, since it mutates the SAME index the
   seek reads from) — if your per-index refactor accidentally makes
   that scenario look like a cross-index case, that's a red flag to
   investigate before proceeding.
6. Confirm no `scc::*::len()` call was introduced anywhere in the new
   code (banned by `clippy.toml`'s `disallowed-methods` — see
   `CLAUDE.md` pillar 3). Emptiness/lookup checks on the new per-index
   map should use `scc::HashMap`'s O(1)-ish entry-presence primitives
   (`get`/`read`/`contains`), never `.len()`/`iter().count()`.

## What NOT to do

- Do NOT touch F-55/F-56/F-57/F-58/F-59/F-60/F-61/F-62/F-63/F-65/F-66
  (other already-landed or in-flight tasks from the same review).
- Do NOT touch `generation: Arc<AtomicU64>`
  (`sorted_index_manager.rs:94-109` / `register`/`drop_index`'s bump
  calls) — that is a SEPARATE mechanism (F-50, DDL re-derivation gate),
  not in scope here, even though it lives in the same struct.
- Do NOT change the AsOf seek's actual visibility/correctness logic
  (the `version_of`/`get_at` classifier, the `concurrent_modified`
  defence-in-depth check) — this task narrows WHICH counter gates the
  fast path, not the classifier itself.
- Do NOT swap to `tokio::sync::Mutex`/`std::sync::Mutex` for the new
  per-index structure — this is exactly the kind of shared registry
  pillar 5 exists for.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-index -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- TDD: write the new scope-narrowing test first, confirm it fails
  against the current manager-wide behavior (temporarily point it at
  today's code, or reason clearly about why it must fail), then
  implement, confirm green.
- A bench or counter-based demonstration that the fast path is
  genuinely retained where it should be (per the task's own strategy
  note) is a nice-to-have if it fits cleanly with existing bench
  infrastructure (`crates/shamir-engine/benches/`) — do not force it if
  it doesn't fit within reasonable scope; a passing scope-narrowing test
  is the minimum bar.
- Clean up any scratch/debug files created in the repo root before
  finishing.

## Verification the orchestrator will run

```
cargo fmt -p shamir-index -p shamir-engine -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index -p shamir-engine --full
```

Plus a personal re-read of the full diff and a red-then-green
reproduction of the new cross-index scope-narrowing test.

When done, give your final summary as plain text: the per-index epoch
structure you chose (and why), exactly how you resolved the
`IndexWriteOp`-key-prefix-decoding question at the `apply_index_batch`
site, the diff shape across all touched files, which tests were added,
and confirmation fmt/clippy/tests are clean.
