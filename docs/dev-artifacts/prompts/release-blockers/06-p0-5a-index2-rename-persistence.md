# Brief — P0-5a: index2 RENAME INDEX is lost after restart

Task: #961 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-new-wave-readonly-review.md` §P0-5a, re-verified against the current source (after the #973 `legacy` → `base_index` rename, which did NOT touch `crates/shamir-index/src/registry.rs` — that file is index2-specific, separate from the renamed module) before filing this task. Tasks #957/#958/#959/#960/#972/#973 are already fixed and committed.

## Confirmed bug (read directly from current source)

`crates/shamir-index/src/registry.rs`:

- The registry's `by_id` map stores a tuple `(Arc<dyn IndexBackend>, gen: u64, state: IndexState)` per backend (~line 92, `insert`). The `state` slot is the AUTHORITATIVE, mutable lifecycle state — `all_descriptors()` (~line 241) explicitly overrides the cloned descriptor's `.state` field from this tuple slot, NOT from the backend's own immutable `descriptor()`, because (per that method's own doc) "the registry tuple is the single source of truth for a LIVE backend's current state."
- `rename_entry` (~line 261) does the OPPOSITE for the NAME: it updates ONLY the `by_name` map (a separate `name_interned -> id` lookup index), and does NOT touch the `by_id` tuple or the backend's own descriptor.
- `IndexBackend::descriptor(&self) -> &IndexDescriptor` (`crates/shamir-index/src/backend.rs` ~line 70) returns a reference to the backend's OWN immutable descriptor — there is no mutation method on the trait to update a backend's name in place.
- Consequence: `all_descriptors()` clones `backend.descriptor()` (still carrying the OLD name/`name_interned`) and only overrides `.state` — so `save_index2_metadata` (which calls `all_descriptors()` to build the persisted blob) writes the STALE name to disk. Live in-memory lookups by the NEW name work (because `by_name` was updated), but after a restart, `IndexManager`/`IndexRegistry`'s reload reads the persisted blob with the OLD name — the rename is silently reverted.

## Required fix

Mirror the `state` pattern already proven for exactly this kind of problem: make the registry tuple the authoritative source for the NAME too, not just `state`.

1. Extend the `by_id` tuple's authoritative-override fields to also carry the CURRENT `name` (`String`) and `name_interned` (`u64`) — i.e. the tuple becomes `(Arc<dyn IndexBackend>, gen, IndexState, name: String, name_interned: u64)` (or introduce a small struct instead of a raw tuple if that's cleaner given how many places already destructure it — your call, but check every `by_id`-touching method first: `insert`, `set_state`, `state_of`, `all_descriptors`, `backends_newer_than`, `get_by_id`, `remove_by_id`, and any others `grep`-discoverable — all destructure the tuple shape and must be updated consistently).
2. `insert` (~line 71): seed the new name/name_interned fields from `backend.descriptor()`'s ORIGINAL name at construction time (same as `state` is seeded from `d.state`).
3. `rename_entry`: after successfully updating `by_name`, ALSO update the `by_id` tuple's name/name_interned slot for this backend's id (an `update_async` on `by_id`, mirroring `set_state`'s pattern). If the `by_id` update fails for some reason after `by_name` succeeded, decide and document the recovery/rollback behavior — the two maps must not end up inconsistent (prefer: do the `by_id` update FIRST or make failure impossible/infallible given `scc::HashMap`'s actual API — check whether `update_async` on an existing key can genuinely fail before assuming you need rollback logic at all).
4. `all_descriptors()`: override BOTH `desc.name` and `desc.name_interned` from the tuple slot (not just `.state`), exactly the same way `.state` is already overridden.
5. Double-check any OTHER place that reads a backend's name for persistence or display purposes and bypasses this tuple (e.g. does `get_by_name`/`find_by_field_and_kind` or any admin/introspection code call `backend.descriptor().name` directly instead of going through the registry's authoritative slot? If so, those call sites will show the STALE name even in-memory after a rename that hasn't yet round-tripped through `all_descriptors`/persist — decide whether that's an acceptable narrow gap (in-memory dispatch already works correctly via `by_name`, this is purely a DISPLAY/introspection staleness) or whether it needs fixing too; document your decision).

## Required tests

Follow this crate's `tests/` layout (`crates/shamir-index/src/registry.rs`'s own test module if one exists, or wherever registry-level tests live — check `crates/shamir-index/src/lib.rs`'s module tree; also check `crates/shamir-engine`'s index2 rename dispatch tests for the RIGHT place to add an end-to-end reopen test).

- **Persisted rename survives reopen**: create an index2 backend (pick the simplest kind to set up — likely `functional` or `fts`, check existing test fixtures for the least-setup option), rename it, call `save_index2_metadata`, reconstruct a fresh registry/manager from the SAME persisted blob (simulating restart), and assert a lookup by the NEW name succeeds and by the OLD name fails — this is the core regression test the review asked for, run it for **FTS, functional, AND vector** index kinds (the review explicitly calls out all three — do not test only one kind and assume the others are covered, each kind's backend construction path may differ subtly).
- **Rename updates BOTH the string name and interned ID** in the persisted descriptor — assert on the deserialized `IndexDescriptor`'s `.name` AND `.name_interned` fields directly, not just an end-to-end lookup (a lookup-only test could pass by accident if only one of the two fields got fixed).
- **Live (non-restart) rename still works** — a regression test that the EXISTING in-memory rename behavior (which already worked via `by_name`) is unaffected by this change.

## Scope discipline

- Do NOT touch sorted-index RENAME (that's #962, blocked on this task in the chain but a SEPARATE code path — `SortedIndexManager::rename_definition`, a different file, different bug per the review's §P0-5b). Do NOT touch DROP INDEX further (that's #959/#972, already done).
- Run ONLY the centralized test entry point: `./scripts/test.sh -p shamir-index` (and `-p shamir-engine` if you touch anything there). Raw `cargo test` is blocked.
- `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings` must be clean before you declare done (this touches a shared registry type — verify nothing downstream broke).

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Do NOT run `git commit` or `git add` — the orchestrator verifies your diff and the test run, then commits. Only edit files and run read-only/build/test commands. Delete stray log files you create yourself; mention it if you leave any.

## What to report back

State the exact new tuple/struct shape you chose and why, confirm every `by_id`-touching call site was updated consistently, what each test proves (especially confirming FTS/functional/vector are ALL covered), and the exact `cargo fmt`/`cargo clippy`/`./scripts/test.sh` commands with real pass/fail counts and exit codes.
