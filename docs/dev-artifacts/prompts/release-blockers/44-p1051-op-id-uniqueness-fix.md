# Brief — #1051 (CRITICAL): fix DDL `op_id` uniqueness, then complete #1048 with a discriminating test

## Context

S.H.A.M.I.R. Database. `@oh`'s adversarial review of #1048 (P1-2 sub-slice B,
commit `f40ab4fd`) found a CRITICAL bug that predates #1048 but that #1048's
entire client-facing contract is built on top of. Full review:
`docs/dev-artifacts/research/2026-08-09-1048-review.md` (finding 1). **Read
that review in full before starting** — this brief only summarizes it and
adds the fix scope; the review has the reproduction, the exact byte-math,
and the full finding set (2 MEDIUM issues you must also fix, both scoped
below).

## The bug, verified independently (do not re-derive, just confirm)

`RecordId::system(name)` (`crates/shamir-types/src/types/record_id.rs:95`)
copies **at most 12 bytes** of `name` into `bytes[4..16]`:

```rust
pub fn system(name: &str) -> Self {
    let mut bytes = [0u8; 16];
    let name_bytes = name.as_bytes();
    let len_to_copy = std::cmp::min(name_bytes.len(), 12);
    bytes[4..4 + len_to_copy].copy_from_slice(&name_bytes[..len_to_copy]);
    Self(bytes)
}
```

`"ddl_drop_index_"` is 15 characters, `"ddl_rename_index_"` is 17 — the
constant prefix ALONE already consumes the entire 12-byte budget, so
whatever index name is appended (`crates/shamir-db/src/shamir_db/execute/
admin_table_index.rs:696`, `:824`) never enters the id at all. Every DROP
INDEX op on a table mints the identical `op_id`; every RENAME INDEX op
mints the identical (different) one.

It compounds one level further: `ddl_op_log::op_status_key`
(`crates/shamir-engine/src/table/ddl_op_log.rs:32-35`) re-wraps the
already-16-byte `op_id` through **another** `RecordId::system(format!
("ddl_op:{op_id}"))` — `op_id`'s `Display` is base58 of all 16 bytes
(~22 chars), `"ddl_op:"` is 7 chars, leaving only 5 chars of that base58
text inside the 12-byte budget, and every `RecordId::system(...)` result
base58-encodes with a leading `"1111"` (the 4-byte zero system prefix) — so
the surviving storage-key prefix is `"ddl_op:11112"` for **every** op_id
that reaches this function. DROP and RENAME status records collide on the
SAME physical key even when their (already-colliding) op_ids differ from
each other.

**Important — this is NOT a bug in `RecordId::system` itself.** The
codebase already documents, in two places, that `RecordId::system`'s
12-byte truncation is an intentional, load-bearing property for its
EXISTING legitimate callers — short, hand-picked, FIXED constant strings
where a human has already verified no collision (see the `"idx_drop"` /
`"uidx_drop"` naming story at `crates/shamir-index/src/base_index/
index_manager.rs:619-622`, and the `"idx_ren"` / `"uidx_ren"` story at
`:806`ish — grep `RecordId::system(name)` truncates \`name\` to 12 bytes\`
in that file for both comments). **Do not change `RecordId::system`'s
behavior** — every one of its other ~15 call sites in this codebase
(`grep -rn "RecordId::system"` across `crates/`) relies on the current
truncation semantics and changing it would be a much larger, riskier
change than this task needs. The bug is that `#1015`/`#1025`
(`admin_table_index.rs`'s `handle_drop_index`/`handle_rename_index`)
misapplied this "short fixed constant" primitive to construct a
per-**variable**-name key, which is exactly the pattern the two existing
in-repo warning comments say not to do.

## What to implement

### 1. Mint op_id with real entropy, not a truncated deterministic string

At both dispatch sites in `admin_table_index.rs` (`handle_drop_index`
line ~696, `handle_rename_index` line ~824), replace
`RecordId::system(&format!("ddl_drop_index_{name}"))` /
`RecordId::system(&format!("ddl_rename_index_{...}"))` with
`RecordId::new()` (genuinely unique: microsecond timestamp + random tail,
see `record_id.rs:24-52`). This is what the RENAME family's own
`HashRenameTombstone.op_id` field ALREADY receives and threads correctly
— you are extending an already-correct pattern to the two DROP families
that don't have it yet, not inventing a new mechanism.

### 2. Persist the real op_id in the DROP tombstones (they currently have no field for it)

Unlike `HashRenameTombstone` (a proper struct with an `op_id: Option
<String>` field), the two DROP tombstones are bare, unstructured:
- Hash DROP (regular + unique): `Vec<u64>` of `name_interned` ids,
  serialized directly under `system:idx_drop` / `system:uidx_drop`
  (`crates/shamir-index/src/base_index/index_manager.rs`, `save_dropping_
  regular`/`save_dropping_unique` near line 629/643, confirm exact names
  via `grep -n "fn save_dropping\|fn load_dropping\|dropping_regular\b"`).
- index2 DROP: `Vec<u32>` of descriptor ids, serialized under a system key
  (`crates/shamir-index/src/persistence.rs:291-345`ish, `add_to_dropping_
  index2`/`load_dropping_index2`).

Both need their tombstone shape widened to carry an op_id per entry —
mirror `HashRenameTombstone`'s pattern: change the bare `Vec<u64>`/
`Vec<u32>` to `Vec<(u64, Option<String>)>` / `Vec<(u32, Option<String>)>`
(or a small named struct per entry if that reads more cleanly — your call,
but keep it consistent with the existing `HashRenameTombstone` naming
style) where the second element is `op_id.map(|id| id.to_string())`,
`None` when no op_id was minted (paths that don't originate from the typed
DDL dispatch, if any — check whether `drop_index`/`drop_unique_index`/
`drop_index2` have any caller besides the dispatch handler; if the ONLY
caller is the dispatch handler, `op_id` can be `Option<RecordId>`
threaded as a real parameter rather than defaulting to `None`, matching
how `rename_index` already takes `op_id: Option<RecordId>` since #1048).
**Backward compatibility**: an old-format tombstone (bare `Vec<u64>`/
`Vec<u32>`, pre-this-fix) must still deserialize — either version the
encoding (try new shape, fall back to old shape treating every entry's
op_id as `None`) or confirm via `grep` that no shipped release has ever
persisted the old shape (this workspace is pre-1.0 alpha with "no
supported in-place upgrade path between alphas" per `CHANGELOG.md` — if
that policy applies here, a straight format change without a fallback
decoder is acceptable; use your judgement, but STATE which you chose and
why in your summary).

Thread `op_id: Option<RecordId>` as a new parameter through
`TableManager::drop_index`/`drop_unique_index`/`drop_index2`
(`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`), exactly
as round 1 of #1048 already did for `rename_index` — audit every caller
(tests included) and update them, same discipline as that prior round.

### 3. Recovery reads the REAL op_id from the tombstone — delete the deterministic-regeneration mechanism

`write_hash_drop_recovery_status` and `write_index2_drop_recovery_status`
(`table_manager_index_mgmt.rs:1091` / `:1249`, both added by #1048)
currently REGENERATE the op_id deterministically via
`RecordId::system(&format!("ddl_drop_index_{name}"))` — the same broken
formula. Once the tombstone carries the real op_id (step 2), these
functions should instead READ it directly off the tombstone entries they
already iterate, and skip the write when an entry's op_id is `None`
(pre-fix tombstone or a caller that didn't supply one) — this is simpler
than what's there today, not more complex; delete the "regenerate
deterministically" logic entirely once the tombstone carries the value.

`RENAME`'s existing recovery (`recover_hash_renames`) already does this
correctly (reads `tombstone.op_id`, parses via `RecordId::from_str`) —
use it as the template.

### 4. Fix `op_status_key`'s own re-truncation

`ddl_op_log::op_status_key` (`ddl_op_log.rs:32-35`) must derive the
storage key from the op_id's **raw 16 bytes** directly, not from
`RecordId::system` of its base58 text. `RecordId` already exposes
`to_bytes()` (used elsewhere in this same file/module) — build the
`RecordKey` by concatenating a short fixed byte prefix (distinguishing
"this is a ddl-op-status key" from any other key in `info_store`'s
shared namespace) with the op_id's raw bytes, without ever routing
through `RecordId::system`'s lossy string path. Check `RecordKey`'s
actual type (`shamir_storage::types::RecordKey`) and how other modules
in this crate build non-`RecordId::system`-derived keys, if any, for the
idiomatic way to do this in this codebase — do not invent a new key
namespace convention if a matching one already exists.

### 5. Fix the two MEDIUM findings from the same review while you're already in this code

Both in `TableManager::create` / `table_manager_index_mgmt.rs`:

- **Ordering (review finding 3)**: index2's `write_index2_drop_recovery_
  status` call currently runs BEFORE `mgr.recover_index2_drops()`
  (`table_manager.rs:649-668`) — a failure partway through the real
  recovery would leave a durable `SucceededViaCrashRecovery` claim for a
  drop that didn't actually finish. Move the status write to run AFTER
  `recover_index2_drops()` completes successfully, matching the hash
  family's already-correct ordering (write happens after `IndexManager::
  new()`, which performs its own recovery internally).
- **Missing-descriptor gap (review finding 4)**: `write_index2_drop_
  recovery_status`'s `else` branch (`table_manager_index_mgmt.rs:1291-
  1300`) silently skips the status write when an id isn't found in
  `persisted_index2_descriptors`, with a comment blaming "crash before
  the descriptor was persisted (during CREATE, not DROP)" — but per the
  function's OWN documented crash-state matrix (`:1188-1191`, row 3:
  "after persist, before clear"), a crash between `save_index2_metadata`
  removing the descriptor and `clear_from_dropping_index2` clearing the
  tombstone leaves EXACTLY this state during a DROP, not a CREATE. Once
  the tombstone itself carries the op_id (step 2), this whole
  name-resolution-via-descriptors mechanism becomes unnecessary for THIS
  purpose — the op_id no longer needs the descriptor to be resolved, only
  read off the tombstone. Confirm this closes the gap; if any residual
  case still can't resolve, fix the comment to state the real cause.

### 6. Rewrite #1048's four recovery tests' central assertion to be discriminating

`crates/shamir-engine/src/table/tests/p1048_hash_drop_durability_tests.rs`
(`:141`, `:254`), `p1048_index2_drop_durability_tests.rs` (`:201`),
`p997_hash_rename_durability_tests.rs` (`:1066`) — all currently assert
`status.op_id == op_id`, which per review finding 2 is tautological (both
sides were computed from the same name-independent constant before this
fix; after this fix they'll be genuinely unique, but the test still
doesn't PROVE addressability unless it tries to fail). Add, to each test
(or as new sibling tests if that's cleaner — your call), a second DDL
operation on the SAME table (a second index to drop, or a second rename)
that is allowed to complete normally (not parked/crashed), mint its own
op_id, and assert that polling the FIRST operation's op_id returns the
first operation's status/kind — not the second's. This is the
discriminating check the review says doesn't exist today and would fail
pre-fix.

## Tests

- The rewritten discriminating assertions above (mandatory — this is the
  actual proof the bug is fixed).
- A focused new test: two DROP INDEX ops (or one DROP + one RENAME) on
  one table, assert their minted op_ids are NOT equal, and assert
  `ddl_op_log::op_status_key` produces different storage keys for them.
  This is the most direct regression test for finding 1 itself, at the
  lowest level — don't rely solely on the higher-level e2e tests to catch
  a future regression here.
- Update or add a backward-compat test for whatever decision you made in
  step 2 about old-format tombstones (either "old shape still decodes,
  op_id treated as None" or "no back-compat needed, alpha policy" — test
  the former if you chose it; state clearly in your summary if you chose
  the latter and skipped a test for it).

## Constraints

- Follow `CLAUDE.md`: `Result<T, E>` conventions, tests in `tests/`
  directories, imports at top of file, one-file-one-primary-export.
- **Do not modify `RecordId::system`'s truncation behavior** — see the
  "Important" note above. This task fixes the misuse, not the primitive.
- This changes tombstone WIRE shapes for two families and several
  `TableManager` method signatures (`drop_index`, `drop_unique_index`,
  `drop_index2` gain an `op_id` parameter) — audit every caller
  (production AND test) and update them; don't leave a compile break.
- Gate: `cargo fmt -p shamir-types -p shamir-index -p shamir-engine -p
  shamir-db -p shamir-query-types -- --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `./scripts/test.sh -p shamir-types -p
  shamir-index -p shamir-engine -p shamir-db -p shamir-query-types
  --full`. Use the wrapper, never raw `cargo test`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.
⛔ Do not create scratch files at the repo root.

## Definition of done

- [ ] `admin_table_index.rs`'s two DDL dispatch sites mint `op_id` via
      `RecordId::new()`, not `RecordId::system(&format!(...))`.
- [ ] Hash DROP (regular + unique) and index2 DROP tombstones carry a real
      op_id per entry, threaded from dispatch through `TableManager::
      drop_index`/`drop_unique_index`/`drop_index2`.
- [ ] `write_hash_drop_recovery_status`/`write_index2_drop_recovery_
      status` read the op_id off the tombstone instead of regenerating it
      deterministically.
- [ ] `ddl_op_log::op_status_key` derives its storage key from the op_id's
      raw bytes, not from a second `RecordId::system` pass over its
      base58 text.
- [ ] index2's status write moved to run after `recover_index2_drops()`
      completes (review finding 3 fixed).
- [ ] index2's missing-descriptor status-write gap resolved (review
      finding 4 fixed) or the comment corrected to state the real
      residual cause if any case still can't resolve.
- [ ] All four #1048 recovery tests' `status.op_id == op_id` assertion is
      now genuinely discriminating (two ops on one table, cross-check).
- [ ] A new focused low-level test proves two DDL ops on one table mint
      distinct op_ids and distinct op-status storage keys.
- [ ] fmt/clippy/test gates green, real output reported (paste the actual
      nextest summary line, not a paraphrase).
