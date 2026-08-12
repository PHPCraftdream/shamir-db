# #1097 CRITICAL — `RemovePosting` has no owner, can cancel a different record's live in-tx unique claim

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

`crates/shamir-tx/src/index_write_op.rs`'s `IndexWriteOp::RemovePosting { key, provenance }`
carries no record-id/owner field. `crates/shamir-engine/src/tx/pre_commit.rs`'s
Step 1 walk (function `pre_commit_prelock`, around line 647-695) replays every
`IndexFamily::Unique` op in `tx.index_write_set` in staging order, tracking
`live: TFxMap<(u64, bytes::Bytes), RecordId>` (current owner per key) so it
can both detect genuine intra-tx collisions and compute the final owner per
key. On a `RemovePosting` for key `k` it currently does `live.remove(&k)`
**unconditionally** — it has no way to check whether the record whose plan
emitted this removal is actually the record `live` currently attributes
ownership of `k` to.

### Confirmed reproduction (found by an `@oh` review of #1096, then reverted — scratch code, not committed)

```
tx0: INSERT R{email:"y"}, INSERT D{email:"q"}; COMMIT
tx2: BEGIN                            // snapshot still sees R owning "y"
tx1: DELETE R; COMMIT                 // "y" is now durably FREE
tx2: UPDATE D SET email="y"           // durable "y" free -> passes; live["y"] = D
tx2: DELETE R                         // STALE plan -> RemovePosting("y") clears live["y"]
tx2: INSERT C{email:"y"}              // durable "y" still free -> passes; live["y"] = C
tx2: COMMIT                           // -> Ok(()), should have aborted
```

`tx2`'s own `DELETE R` was planned when `tx2`'s snapshot still believed `R`
owned `"y"` (`R` was already durably deleted by `tx1` before `tx2` even
issued the `DELETE`, under `Snapshot` isolation's documented
last-writer-wins semantics — no write-write conflict detection). That
`RemovePosting("y")` is emitted with no owner attached, so Step 1's walk
blindly clears `live["y"]` even though `D` (not `R`) is the record that
*actually* claimed `"y"` earlier in this same walk. `C`'s later `INSERT`
then claims `"y"` into an empty slot with no collision detected.

Result: `D`'s materialized row has `email: "y"` (the UPDATE's data write
lands unconditionally), but the unique index posting for `"y"` ends up
pointing at `C` — a live index/data divergence, and effectively two records
sharing one unique value with no error ever raised.

Order-dependent: swapping the UPDATE and the stale DELETE makes Step 1's
existing SetPosting-vs-SetPosting collision check catch it (two SetPostings
for the same key with different owners, no intervening *valid* release) —
which is why none of `#1039`/`#1074`/`#1096`'s existing tests hit this.

This bug is **pre-existing since `#1074`** (which introduced the live/
released walk) — it is NOT introduced by `#1096` (that task fixed a
different scenario: release-then-reclaim of a *durable*, prior-committed-tx
unique value; this bug is about an in-tx claim being cancelled by a stale
same-tx plan). Out of `#1096`'s scope; tracked separately as task `#1097`.

## The fix

Give `IndexWriteOp::RemovePosting` an `owner: Option<[u8; 16]>` field — the
16-byte `RecordId` bytes of the record this removal was planned for, when
known. `None` for every construction site where the removal is not for the
`IndexFamily::Unique` family (regular hash / sorted / index2 postings never
participate in Step 1's `live`/`released` logic — that logic filters to
`provenance.family == IndexFamily::Unique` only, so their `owner` value is
never read; `None` is the correct value for all of them).

Step 1's `RemovePosting` arm in `pre_commit.rs` then only clears `live[k]`
when the op's declared owner matches (or is absent and the key has no
current live owner) — a mismatch means this is a *stale* removal (planned
against a record that no longer holds the key per this tx's own walk so
far) and must be a no-op: the key's real current claim must survive.

### 1. `crates/shamir-tx/src/index_write_op.rs`

Add the field to the `RemovePosting` variant:

```rust
    /// Delete a posting by key from the index store.
    RemovePosting {
        key: Bytes,
        /// See [`SetPosting::provenance`].
        provenance: Provenance,
        /// R0-C (#1097): the 16-byte id of the record this removal was
        /// planned against, when known. Only populated by base_index
        /// UNIQUE planners (`index_manager_unique.rs`'s
        /// `plan_record_updated_unique` / `plan_record_deleted_unique`) —
        /// the only family `pre_commit.rs`'s Step 1 walk uses this for.
        /// `None` for every other family (regular hash / sorted / index2),
        /// which never populate or read it.
        owner: Option<[u8; 16]>,
    },
```

`set_provenance`'s match arm already uses `RemovePosting { provenance, .. }`
— unaffected, no change needed there.

### 2. The three REAL construction sites — `crates/shamir-index/src/base_index/index_manager_unique.rs`

These are the only three places in the whole codebase that ever construct a
`RemovePosting` with `provenance.family == IndexFamily::Unique` (confirmed
by grep — every other `RemovePosting` construction site uses a Regular /
Sorted / Index2 provenance). Set `owner: Some(*record_id.as_bytes())` at
each (the record id is already a parameter of the enclosing function in all
three cases — for `plan_record_deleted_unique`, the parameter is currently
named `_record_id` with a leading underscore because it was unused; rename
it to `record_id` and use it):

- Line ~953 (`plan_record_updated_unique`, `(Some(ok), None)` arm):
  ```rust
  (Some(ok), None) => {
      ops.push(IndexWriteOp::RemovePosting {
          key: ok.to_bytes(),
          provenance,
          owner: Some(*record_id.as_bytes()),
      });
  }
  ```
- Line ~962 (`plan_record_updated_unique`, `(Some(ok), Some(nk))` arm, the
  `if old_bytes != new_bytes` branch's `RemovePosting` push):
  ```rust
  ops.push(IndexWriteOp::RemovePosting {
      key: old_bytes,
      provenance,
      owner: Some(*record_id.as_bytes()),
  });
  ```
- Line ~981-997 (`plan_record_deleted_unique`): rename the `_record_id`
  parameter to `record_id` and use it:
  ```rust
  pub async fn plan_record_deleted_unique(
      &self,
      record_id: &RecordId,
      old_value: &(impl RecordRef + ?Sized),
  ) -> DbResult<Vec<IndexWriteOp>> {
      if !self.has_unique_indexes() {
          return Ok(Vec::new());
      }
      let mut ops = Vec::new();
      for def in self.indexes_unique.iter() {
          if let Some(irk) =
              build_index_key_from_record(true, def.name_interned, old_value, &def.paths)
          {
              ops.push(IndexWriteOp::RemovePosting {
                  key: irk.to_bytes(),
                  provenance: unique_provenance(&def),
                  owner: Some(*record_id.as_bytes()),
              });
          }
      }
      Ok(ops)
  }
  ```

These two functions (`plan_record_updated_unique` / `plan_record_deleted_unique`)
are the ONLY callers used by both the original stage-time tx paths
(`table_manager_tx_ops.rs`'s `update_tx`/`update_tx_bytes`/`delete_tx`) AND
the P0-2 commit-time rederive path (`pre_commit.rs`'s
`rederive_base_index_ops_post_stage`, calling them at lines ~1555 and
~1628) — fixing the two functions covers every real call site
automatically. Do not touch those call sites themselves; they just forward
`record_id`/`rid` through unchanged.

### 3. Every OTHER construction site — add `owner: None`

These all construct with a NON-Unique provenance (Regular hash, Sorted, or
Index2 via `index2_provenance`/`regular_provenance`) — Step 1 never reads
their `owner`, so `None` is simply the mechanical fill for the new field.
Exact sites (confirmed by grep — do not go looking for more; this list is
exhaustive for non-test production code):

- `crates/shamir-index/src/base_index/index_manager.rs`:
  - ~line 2541 (`plan_record_updated`, `(Some(ok), None)` arm)
  - ~line 2551 (`plan_record_updated`, `(Some(ok), Some(nk))` arm)
  - ~line 2621 (`plan_record_deleted`)
- `crates/shamir-index/src/base_index/sorted_index_manager.rs`:
  - ~line 1783 (`ops.push(IndexWriteOp::RemovePosting { key, provenance });` → add `owner: None,`)
  - ~line 1828 (`plan_record_deleted`)
- `crates/shamir-index/src/functional_backend.rs`:
  - ~line 227 (`plan_update`)
  - ~line 254 (`plan_delete`)
- `crates/shamir-index/src/fts_ranked_backend.rs`:
  - ~line 203 (`plan_update`, disappeared-token loop)
  - ~line 245 (`plan_delete`)
- `crates/shamir-index/src/fts_backend.rs`:
  - ~line 165 (`plan_update`, disappeared-token loop)
  - ~line 192 (`plan_delete`)

For every one of these, add `owner: None,` as an extra field in the struct
literal (do NOT change any other field or logic in these functions — purely
additive).

### 4. Match-pattern sites that name all fields explicitly (need `..` or `owner: _`)

Struct patterns that already use `{ key, .. }` / `{ provenance, .. }` /
`{ .. }` need NO change (the new field is silently absorbed by `..`).
Exactly one production match site names both existing fields without `..`
and will fail to compile once `owner` is added:

- `crates/shamir-engine/src/table/table_manager_tx_ops.rs` line ~46, inside
  `released_unique_keys_in_tx`:
  ```rust
  IndexWriteOp::RemovePosting { key, provenance }
      if provenance.family == IndexFamily::Unique =>
  {
      released.insert(key.to_vec());
  }
  ```
  Change to `IndexWriteOp::RemovePosting { key, provenance, .. }` (this
  function only needs `key`/`provenance` — it doesn't need `owner`, it is
  purely the stage-time optimistic check `#1096` added, unaffected by this
  fix's Step 1 semantics change since it's a *different* function from
  `pre_commit.rs`'s Step 1).

Grep the whole workspace for `IndexWriteOp::RemovePosting` and
`RemovePosting {` after making the enum change and fix EVERY resulting
compile error — the list above is our best-effort enumeration from a grep
pass, but the compiler is the authority here, not this list. Known test
files that construct `RemovePosting` with named fields and will also need
`owner: None,` (or `owner: Default::default()`) added — fix these
mechanically the same way, they are not part of the correctness fix:
- `crates/shamir-index/src/tests/write_ops_tests.rs` (2 sites)
- `crates/shamir-tx/src/tests/repo_tx_gate_tests.rs` (1 site)

### 5. The actual correctness fix — `crates/shamir-engine/src/tx/pre_commit.rs`, Step 1's `RemovePosting` arm (~line 685-692)

Current code:

```rust
            IndexWriteOp::RemovePosting { key, provenance }
                if provenance.family == IndexFamily::Unique =>
            {
                let k = (*table_token, key.clone());
                live.remove(&k);
                released.insert(k.clone());
                ever_released.insert(k);
            }
```

Change to:

```rust
            IndexWriteOp::RemovePosting {
                key,
                provenance,
                owner,
            } if provenance.family == IndexFamily::Unique => {
                let k = (*table_token, key.clone());
                // #1097: only clear the live claim when this op's declared
                // owner actually matches the record `live` currently
                // attributes the key to (or there IS no current live owner
                // to protect, or the op didn't declare an owner at all —
                // the unconditional pre-#1097 behavior, kept as a safe
                // fallback for any future non-unique-family construction
                // site that might reach here). A MISMATCH means this
                // removal was planned against a record that no longer
                // holds the key per this tx's own walk so far (built
                // against a stale snapshot — see this file's #1097 doc
                // above) — treat it as a stale no-op: do not clear `live`,
                // do not mark the key released, so the key's real current
                // owner within this tx is preserved and Step 2 validates
                // against it correctly.
                let removing_owner = owner.map(RecordId);
                let current_owner = live.get(&k).copied();
                if current_owner.is_none() || removing_owner.is_none() || current_owner == removing_owner
                {
                    live.remove(&k);
                    released.insert(k.clone());
                    ever_released.insert(k);
                }
            }
```

Check `RecordId`'s exact shape before writing `RecordId(bytes)` —
`crates/shamir-types/src/types/record_id.rs` defines
`pub struct RecordId(pub [u8; 16]);` (public tuple field, directly
constructible) and derives `PartialEq, Eq, Clone, Copy`, so
`owner.map(RecordId)` and the `==` comparison both work directly; import
`shamir_types::types::record_id::RecordId` if not already imported in this
file (it already is — this file already uses `RecordId::try_from_bytes` a
few lines below in Step 2).

## Tests to add

New file `crates/shamir-engine/src/tx/tests/p1097_remove_posting_owner.rs`,
wired into `crates/shamir-engine/src/tx/tests/mod.rs` per this repo's
test-organization convention (`pub mod p1097_remove_posting_owner;`).

1. **The exact reproduction from this brief's Background section**,
   adapted to this codebase's tx/table test helpers (mirror the setup style
   of `crates/shamir-engine/src/tx/tests/p1096_tx_aware_unique_check.rs` —
   same crate, same test harness, a unique index on `email`):
   - `tx0`: INSERT `R{email:"y"}`, INSERT `D{email:"q"}`; COMMIT.
   - `tx2`: BEGIN (snapshot before the next line commits).
   - `tx1`: DELETE `R`; COMMIT.
   - `tx2`: UPDATE `D` SET `email="y"`.
   - `tx2`: DELETE `R` (stale plan — `R` already durably gone, but `tx2`'s
     snapshot still believes it owns `"y"`).
   - `tx2`: INSERT `C{email:"y"}`.
   - `tx2`: COMMIT.
   - Assert the commit is REJECTED (`Err(CommitError::UniqueViolation { .. })`
     or whatever the crate's actual commit-time error type/variant is —
     check `crate::tx::commit::TxError`/`CommitError`'s real shape, mirror
     the assertion style `p1096_tx_aware_unique_check.rs`'s
     `tx_genuine_double_claim_still_rejects` test uses for a Step-1-caught
     collision). Before the fix, this test must FAIL (the commit wrongly
     succeeds) — prove this by mentally tracing the pre-fix code path
     (do NOT temporarily revert the fix in the actual repo to "prove" it;
     the failure_scenario in the Background section already traces it, and
     a reviewer will re-verify by mutation testing).
   - After a successful abort, also assert `D`'s durable row still has
     `email: "q"` (unchanged) and no orphaned `C` record's unique claim
     — i.e. that the whole transaction rolled back cleanly, not just that
     an error was returned.

2. **A legitimate release-then-reclaim-within-the-same-plan-generation
   case that must still SUCCEED** (regression guard against an
   over-corrected fix that makes `owner` matching too strict): a single
   record `X` inserted with `email:"a"`, then in a LATER tx updated to
   `email:"b"` then updated again to `email:"a"` (moves off then back onto
   the same key, no other record involved) — must commit successfully.
   `plan_record_updated_unique`'s `RemovePosting` for `X`'s own old key
   during the first update carries `owner: Some(X)`, and at the point Step
   1 processes it `live[key]` is `Some(X)` (still `X`'s own posting from
   an earlier op or none), so the match succeeds and the release proceeds
   normally.

3. **A genuine two-different-records collision with NO stale plan
   involved** (positive control — must still reject, proving the fix
   didn't accidentally make Step 1 permissive): `tx: INSERT A{email:"x"};
   INSERT B{email:"x"}` (no DELETE/UPDATE between them at all) — must
   still hit the existing SetPosting-vs-SetPosting collision check
   (unaffected by this fix) and abort at commit time.

Run `mutation testing` on your own fix before considering it done: revert
just the `pre_commit.rs` Step 1 condition change (temporarily, in your own
working copy) back to unconditional `live.remove(&k)` and confirm test #1
above fails; restore the fix and confirm it passes again. Do this
BEFORE handing off — the orchestrator will re-verify independently, but
your own pass should not ship an accidentally-vacuous test.

## Gate

Before finishing:
```
cargo fmt -p shamir-tx -p shamir-index -p shamir-engine -- --check
cargo clippy -p shamir-tx -p shamir-index -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-tx -p shamir-index -p shamir-engine --full
```
All three must pass clean. Fix any pre-existing unrelated clippy warning
ONLY if it's in a line you touched as a mechanical side effect of the enum
field addition (e.g. a test file's struct literal) — do not go fix
unrelated pre-existing issues.

Do not touch anything not described above. This is a surgical, well-scoped
fix — no incidental refactors, no touching comments unrelated to this
change, no renaming beyond the one documented `_record_id` → `record_id`
rename.
