# Brief 81 — #1096: `insert_tx`'s stage-time unique check must become tx-aware of the transaction's own prior releases

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background — already investigated, confirmed against the code

`#1074` (commit `87c978e2`) fixed `crates/shamir-engine/src/tx/pre_commit.rs`'s
Step 1/Step 2 unique-guard walk so that a key released (via DELETE or an
UPDATE that moves the value off the key) and reclaimed by a DIFFERENT
record LATER IN THE SAME TRANSACTION commits successfully, instead of a
false `UniqueViolation`. That fix is correct for what it covers, but a
follow-up review (2026-08-11) found it does NOT close the general case.
Read `pre_commit.rs`'s Step 1/Step 2 doc comment in full first — it has
the complete derivation this brief builds on; do not re-derive it from
scratch.

**Confirmed reproduction** (verified by direct testing during the
investigation, not just theorized):

```
tx1: INSERT A {email:"x"}; COMMIT          // durable: info_store["x"] = A
tx2: DELETE A; INSERT B {email:"x"}        // -> Err(DuplicateKey) at the
                                               INSERT B call itself
```

The UPDATE-off variant fails identically:
```
tx1: INSERT A {email:"x"}; COMMIT
tx2: UPDATE A SET email="z"; INSERT B {email:"x"}   // -> DuplicateKey
```

**Root cause**: `crates/shamir-engine/src/table/table_manager_tx_ops.rs`'s
`insert_tx` (line ~417) calls
`self.index_manager.validate_unique_for_create(value).await?` — a
STAGE-TIME check that reads ONLY durable storage
(`crates/shamir-index/src/base_index/index_manager_unique.rs::validate_unique_for_create`
→ `check_unique_key`). By the time `INSERT B` is staged, `tx.index_write_set`
ALREADY contains the `RemovePosting` from the earlier `DELETE A` call
(staged sequentially, same tx, before `INSERT B` runs) — but this check
never looks at `tx.index_write_set` at all. Since `A` is still durable
(the DELETE hasn't reached storage — nothing mutates durable state until
commit), the check finds `A` and rejects `INSERT B` immediately, BEFORE
the operation is ever staged into `tx.index_write_set` — so `pre_commit.rs`'s
Step 1/Step 2 walk (correct for the case it covers) never even runs; the
tx never gets that far.

**Severity**: fail-CLOSED, NOT a security/data-integrity issue (no
duplicate can ever actually be created) — this rejects a LEGITIMATE
operation, a functional regression, not a data-corruption risk.

## The fix — full design already worked out, implement exactly this shape

This reuses `pre_commit.rs`'s ALREADY-PROVEN "live/released" walk pattern
(Step 1), scoped to run at STAGE time instead of commit time, against only
the specific keys the value being inserted would claim.

### 1. New method on `IndexManager`

In `crates/shamir-index/src/base_index/index_manager_unique.rs`, alongside
the existing `validate_unique_for_create`/`validate_unique_for_create_with_defs`:

```rust
/// Like `validate_unique_for_create`, but a durable conflict is NOT an
/// error if the conflicting index_key is present in `released_in_tx` —
/// the caller (insert_tx) has already determined, by walking its own
/// tx.index_write_set, that this key was legitimately vacated earlier
/// in the SAME transaction (a release-then-reclaim pattern invisible to
/// a durable-only check).
pub async fn validate_unique_for_create_with_released(
    &self,
    value: &(impl RecordRef + ?Sized),
    released_in_tx: &shamir_collections::TFxSet<Vec<u8>>,
) -> DbResult<()> {
    if !self.has_unique_indexes() {
        return Ok(());
    }
    let defs: Vec<IndexDefinition> = self.indexes_unique.iter().collect();
    for def in &defs {
        if let Some(irk) =
            build_index_key_from_record(true, def.name_interned, value, &def.paths)
        {
            let index_key = irk.to_bytes();
            if let Some(existing_id) = self.check_unique_key(&index_key).await? {
                if released_in_tx.contains(index_key.as_ref()) {
                    continue; // released earlier in this same tx — safe to reclaim
                }
                return Err(shamir_storage::error::DbError::DuplicateKey(format!(
                    "Unique index '{}' violated: value already exists for record {:?}",
                    def.name_interned, existing_id
                )));
            }
        }
    }
    Ok(())
}
```

Match `validate_unique_for_create_with_defs`'s exact existing structure/error
message format (grep it, right above where you're adding this) — the only
behavioral difference is the `released_in_tx.contains(...)` short-circuit.

### 2. New helper in `shamir-engine`

Add to `crates/shamir-engine/src/table/table_manager_tx_ops.rs` (or a
shared location if there's an obvious one — check for an existing
`tx_helpers`-style module first; if none, a private fn in this file is
fine):

```rust
/// #1096: which unique keys THIS transaction has released-and-not-reclaimed
/// as of the CURRENT staging point — the exact same `live`/`released`
/// walk `pre_commit.rs`'s Step 1 uses, scoped to just `table_token` and
/// producing a set instead of aborting on conflict.
fn released_unique_keys_in_tx(
    tx: &shamir_tx::TxContext,
    table_token: u64,
) -> shamir_collections::TFxSet<Vec<u8>> {
    let mut live: shamir_collections::TFxMap<Vec<u8>, shamir_types::types::record_id::RecordId> =
        Default::default();
    let mut released: shamir_collections::TFxSet<Vec<u8>> = Default::default();
    for (tt, op) in &tx.index_write_set {
        if *tt != table_token {
            continue;
        }
        match op {
            shamir_tx::IndexWriteOp::SetPosting {
                key,
                value,
                provenance,
            } if provenance.family == shamir_tx::IndexFamily::Unique => {
                live.insert(
                    key.to_vec(),
                    shamir_types::types::record_id::RecordId::try_from_bytes(value)
                        .unwrap_or_default(),
                );
                released.remove(key.as_ref());
            }
            shamir_tx::IndexWriteOp::RemovePosting { key, provenance }
                if provenance.family == shamir_tx::IndexFamily::Unique =>
            {
                live.remove(key.as_ref());
                released.insert(key.to_vec());
            }
            _ => {}
        }
    }
    released
}
```

Fix the exact import paths (`IndexWriteOp`, `IndexFamily`, `RecordId` — grep
`pre_commit.rs`'s own imports for the correct paths, this file may need
different `use` statements than what's sketched above; the SHAPE of the
logic is what matters, mirror `pre_commit.rs`'s Step 1 loop body exactly).

### 3. `insert_tx`'s call site

In `table_manager_tx_ops.rs`, replace the single line:

```rust
self.index_manager.validate_unique_for_create(value).await?;
```

with:

```rust
let released = released_unique_keys_in_tx(tx, self.table_token());
self.index_manager
    .validate_unique_for_create_with_released(value, &released)
    .await?;
```

### 4. `insert_tx_many`'s batch path — check first, fix if affected

`insert_tx_many` (same file, "1. Batch-validate unique indexes" section)
calls `self.index_manager.validate_unique_for_create(v).await?` in a
per-row loop. Determine whether this path is affected by the SAME class of
bug (a batch insert that releases a durable value via an EARLIER staged op
in the same tx, then re-claims it via a later row in the SAME batch) — if
so, apply the equivalent `_with_released` treatment there too. If NOT
reachable (e.g. `insert_tx_many` is only ever called with no prior ops
staged in the same tx), state that explicitly in your final report with
the reasoning, do not guess.

## Do not change unrelated behavior

The non-tx-aware `validate_unique_for_create`/`validate_unique_for_create_with_defs`
methods must remain UNCHANGED — they're still used by the non-tx `insert`
path, which has no `tx.index_write_set` to consult. Do not touch their
signatures or callers other than `insert_tx`/`insert_tx_many`.

## Required tests

1. **The two reproduction scenarios must now commit successfully**:
   - `tx1: INSERT A {email:"x"}; COMMIT` then `tx2: DELETE A; INSERT B {email:"x"}` — must succeed, `B` ends up owning the unique value.
   - `tx1: INSERT A {email:"x"}; COMMIT` then `tx2: UPDATE A SET email="z"; INSERT B {email:"x"}` — must succeed.
2. **A GENUINE double-claim must still reject.** Prove `released_unique_keys_in_tx` does NOT include a key that's merely mentioned in the tx's write set but is STILL live (owned by a different record as of the current staging point) — e.g. `tx: INSERT A {email:"x"}; INSERT B {email:"x"}` (no delete in between) must still fail with `DuplicateKey` at the `INSERT B` call.
3. **A key released then reclaimed by the SAME transaction's later insert, but where a THIRD concurrent transaction also holds a claim on it durably** (if constructible) — sanity-check this doesn't create a new race; if this scenario requires infrastructure you don't have (e.g. genuinely concurrent transactions), note it as an open question in your report rather than skipping silently.
4. **`insert_tx_many`'s batch path**, if step 4 above found it affected: an equivalent DELETE-then-reclaim-within-one-batch test.
5. Place new tests in `crates/shamir-engine/src/tx/tests/` (check the existing directory structure for the right file — likely alongside or near `base_index_tx_tests.rs`, which already has related unique-constraint tests) per this repo's test-organization convention (one `tests/` dir, split by topic, wired via the parent `mod.rs` — see `CLAUDE.md`'s "Test organisation" section).

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index --full
./scripts/test.sh -p shamir-engine --full
./scripts/test.sh -p shamir-tx --full
./scripts/test.sh -p shamir-db --full
```

Paste the actual final summary line from every command — literal output,
not a paraphrase. Report explicitly: (a) whether `insert_tx_many` needed
the same fix (with your reasoning either way), (b) confirmation that the
genuine-double-claim test (required test #2 above) actually fails BEFORE
your fix and passes AFTER it (prove the test is discriminating, not
vacuous), (c) the exact final summary line from every gate command. If
anything fails, fix it before reporting done. This touches unique-
constraint enforcement — a bug in the wrong direction (treating a still-
live key as released) would be a genuine uniqueness bypass, worse than the
bug being fixed; be exact, not approximate, about the `live`/`released`
walk's correctness.
