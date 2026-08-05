# Brief — #997: crash-recovery for compound regular/unique RENAME INDEX

Task: #997 in the session TaskList. Split out of #985 during the 2026-08-05
task revision because it is a **correctness / crash-safety gap**, not the
wire-protocol design question #985 now covers. Read this brief in full — the
exact pattern to mirror is already in this repo, three times over.

## The gap — verified by direct read of the current code

`TableManager::rename_index`
(`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`, ~line 1171)
handles four index families. Two of them are already crash-safe, two are not:

| family | mechanism | crash-safe? |
|---|---|---|
| sorted | `SortedIndexManager::rename_index_sorted` — durable "Renaming" tombstone → RCU definition swap → settle-loop rekey → tombstone clear; resumed by `recover_in_progress_renames` | ✅ #962 |
| index2 | `rename_entry` + `save_index2_metadata`; postings keyed by `u32 index_id`, no physical move | ✅ #961 |
| **regular** (hash, `is_unique=0`) | ~line 1219: `create_index(new_name, paths)` **then** `index_manager.drop_index(old_id)` | ❌ **none** |
| **unique** (hash, `is_unique=1`) | ~line 1257: `drop_unique_index(old_id)` **then** `create_unique_index_body(new_name, paths)` | ❌ **none** |

Both compound paths only produce an *honest error message* (#967, the
`.map_err(...)` blocks at ~1237 and ~1299) when the second step fails. There
is **no durable tombstone and no restart recovery**. A process crash (not just
an `Err`) between the two steps leaves:

- **regular**: BOTH `old_name` and `new_name` indexes existing, permanently.
- **unique**: NEITHER existing — the uniqueness guarantee the user asked for
  is silently gone after restart, with no signal at all.

The unique case is the severe one: the index is *derived* data, so "both exist"
(regular) is wasteful but not incorrect, whereas "neither exists" (unique)
silently drops a **constraint**.

## The pattern to mirror — read these three first

Do NOT invent a new mechanism. This repo has landed the durable-tombstone +
restart-recovery pattern three times; study all three before writing code:

1. **`#959`** — base_index DROP: `IndexManager`'s `system:idx_drop` tombstone
   (`crates/shamir-index/src/base_index/index_manager.rs` ~lines 397-433,
   523-553). This is the closest structural match, because `IndexManager` is
   the same struct you'll extend and it already owns an
   `info_store: Arc<dyn Store>` (line 66).
2. **`#962`** — sorted RENAME:
   `crates/shamir-index/src/base_index/sorted_index_manager.rs`
   (`renaming_sorted` field ~195-224, `save_/load_/add_to_/clear_from_renaming_sorted`
   ~1095-1190, `recover_in_progress_renames` ~1274, `rename_index_sorted`
   ~1319). This is the closest *semantic* match — same operation, different
   family — and its doc comment already contains the crash-state matrix format
   to imitate.
3. **`#988`** — index2 DROP: `crates/shamir-index/src/persistence.rs`
   (`meta_key_indexes_drop` ~294-310 and the four free functions) plus
   `TableManager::recover_index2_drops` and the
   `drop_index2_post_sweep_hook` test hook. Read this specifically for (a) how
   a recovery entry point is wired into `TableManager::create` / `new`, and
   (b) the pause-hook crash-simulation test technique in
   `crates/shamir-engine/src/table/tests/p03b_index2_drop_durability_tests.rs`.

## Critical difference from sorted — do NOT copy sorted's tombstone payload

`rename_index_sorted`'s tombstone stores only `old_id → new_id`, because a
sorted rename is a **rekey**: the definition and its postings both still exist,
so recovery just re-runs the idempotent settle loop.

The regular/unique paths are **drop + rebuild** (the hash key mixes
`name_interned` into h1/h2, so a raw rekey is impossible — see the doc comment
at ~1151). In particular, the **unique** path drops the old definition FIRST,
so by the time a crash can happen the old `IndexDefinition` — and therefore its
`paths` — is already **gone from both memory and disk**.

⇒ The tombstone payload for this task MUST carry enough to rebuild from
nothing: at minimum `old_name_id`, `new_name_id`, the family (regular vs
unique), and the index's `paths` (the interned `Vec<IndexInfoItem>`, or the
resolved string paths — pick one and justify it; note `resolve_index_paths`
at ~1364 already does the interned→string direction, and `build_index_definition`
at ~1128 does the reverse).

Verify this claim yourself by reading `drop_unique_index` before designing the
payload — if you find the old definition *is* still recoverable after the drop,
say so and adjust, but do not assume it.

## Required crash-state matrix — must be explicit and documented

Write the recovery function's doc comment as an explicit table, in the same
style as `recover_in_progress_renames` / `recover_index2_drops`. At minimum
these rows (derive the full set yourself from the actual code order):

**regular** (create-new → drop-old):
- tombstone present, `new` absent → the crash hit before/during `create_index`
  registered the new definition. Recovery: ?
- tombstone present, both `old` and `new` present → crash between the two
  steps. Recovery: drop `old`, clear tombstone.
- tombstone present, `old` absent and `new` present → `drop_index` succeeded
  but the tombstone clear didn't. Recovery: clear tombstone (no-op otherwise).

**unique** (drop-old → create-new):
- tombstone present, both absent → the severe case. Recovery: rebuild `new`
  from the tombstone's stored paths, then clear.
- tombstone present, `old` present (drop didn't land durably) → decide and
  document: is re-running the whole rename correct, or is rolling forward from
  wherever we are correct? Justify.
- tombstone present, `new` present → clear tombstone.

**Interaction you must resolve explicitly** (state your reasoning in the doc
comment): #966 already added an open-time self-heal for indexes stuck in
`Building` state. A crash mid-`create_index` can leave the NEW index registered
but `Building`. Does #966's self-heal already cover that half, making the
tombstone's job only "drop the old one"? Read #966's self-heal block in
`TableManager::create` (the "F-50 Step 3b" area — `recover_index2_drops` is
called right after it) and say plainly which recovery owns which state. Do not
let the two mechanisms both try to own the same repair, and do not leave a
state neither owns.

**Unique-specific hazard**: a recovery-time rebuild of a unique index runs a
backfill that can legitimately find a DUPLICATE (writes may have landed while
the index didn't exist — exactly the window this bug opens). Decide and
document what happens then: recovery must NOT panic, and must NOT silently
leave the table looking healthy. Failing the open with a clear diagnostic, or
leaving the index absent + surfacing it via `doctor::verify()` (#966's
`IndexHealth`), are both defensible — pick one, justify it, and test it.

## Implementation placement

- Tombstone storage + save/load/add/clear: `IndexManager`
  (`crates/shamir-index/src/base_index/index_manager.rs`), mirroring the
  existing `system:idx_drop` code it already has. Pick a new meta key and
  **verify it does not collide** under `RecordId::system`'s 12-byte truncation
  against the existing keys — `persistence.rs` ~294-310 shows the exact
  byte-level collision analysis format to reproduce for your new key.
- The recovery entry point must live where a **backfill** is possible (it needs
  the record stream + interner), i.e. on `TableManager`, called from
  `TableManager::create` / `new` next to `recover_index2_drops`. Follow #988's
  wiring exactly.
- The tombstone must be written **before** the first mutating step in each path
  and cleared **after** the last one succeeds. For `unique`, note the existing
  write-barrier span (`begin_write_barrier(UNIQUE_INDEX_CREATE)` at ~1287) —
  decide whether the tombstone write goes inside or outside it and justify it
  (a durable store write while holding the barrier is a real cost; a tombstone
  written after the barrier is taken but before `drop_unique_index` is likely
  correct — reason it out, don't guess).

## Required tests

New file under `crates/shamir-engine/src/table/tests/` (wire it into
`tests/mod.rs`, which is a re-export-only manifest per this repo's convention).
Model it on `p03b_index2_drop_durability_tests.rs`.

Cover, at minimum:
- Every row of the crash-state matrix you documented, per family
  (regular + unique) — simulate the crash by dropping/rebuilding the
  `TableManager` at the relevant point, using a test-only pause hook if the
  crash point is mid-async (copy the `drop_index2_post_sweep_hook` /
  `drop_index2_pause_hook` shape — a `#[cfg(test)]` field on `TableManager`).
- Idempotence: a double restart after recovery is a no-op.
- The unique duplicate-during-recovery hazard described above.
- A normal (no-crash) rename of each family still works — regression smoke.

Each crash test must assert the ACTUAL end state (index present/absent under
each name, and postings resolvable via lookup), not just that recovery returned
`Ok`.

## Scope discipline

- Do NOT touch the sorted or index2 rename paths — they are already correct.
- Do NOT change the ORDER of the existing regular/unique steps unless recovery
  provably cannot work otherwise; if you must, call it out prominently and
  explain why, because those orders were chosen for concurrency reasons
  documented in the comments at ~1206-1218 and ~1247-1256 (audit A9, F-70/#897).
- Do NOT weaken or remove #967's enriched error messages — they stay; the
  tombstone is additive.
- Do NOT redesign `IndexWriteOp`, the write barrier, or anything outside the
  rename/recovery surface.

## Gate (MANDATORY)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-index --full
```

⚠️ Raw `cargo test` is BLOCKED by this repo's perimeter guard. Use
`./scripts/test.sh` (`-p <crate>`, `-- <substring>` for a narrow run).

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or
any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the test
run, then commits. Only edit/create files and run read-only / test / gate
commands.

## What to report back

- The tombstone key you chose + the byte-level collision analysis proving it is
  safe.
- The tombstone payload shape and why it carries what it carries.
- The full crash-state matrix as you implemented it, per family, and your
  explicit answer to the #966-self-heal ownership question.
- Your decision on the unique-duplicate-during-recovery hazard and why.
- The list of tests added and which matrix row each covers.
- Exact gate command output.
