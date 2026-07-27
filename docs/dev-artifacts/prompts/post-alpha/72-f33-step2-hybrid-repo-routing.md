# Brief for F-33 Step 2 (#836, P1) — `BoxRepo::Hybrid` / `BoxRepoFactory::Hybrid` routing

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-33 Step 1 (#835, landed, commit `606eb6f3`) built `MirroredStore`
(`crates/shamir-storage/src/storage_mirrored.rs`) — an in-memory-primary
`Store` that conditionally mirrors durable-config writes to a backing
store. This step wires it into `shamir-engine`'s repo abstraction as a
new, additive `hybrid` backend choice.

**Read `crates/shamir-engine/src/repo/repo_types.rs` in FULL first** — it
is short (~330 lines) and this task adds to it directly, matching its
EXISTING patterns exactly (the file already has 4 `BoxRepo`/`BoxRepoFactory`
variants: `InMemory`, `Fjall`, `MemBuffer`, `Cached` — study how
`MemBuffer`/`Cached` compose an `inner: BoxRepo`/`BoxRepoFactory` to see
this file's established composition style before adding a 5th variant).

## What to build

### 1. New types

```rust
pub struct HybridRepoComposite {
    mem: Arc<InMemoryRepo>,
    disk: Arc<FjallRepo>,           // #[cfg(feature = "fjall")]
    stores: scc::HashMap<String, Arc<tokio::sync::OnceCell<Arc<dyn Store>>>, THasher>,
}

pub struct HybridRepoFactory {
    pub info_path: PathBuf,
}
```

(`#[cfg(feature = "fjall")]`-gate `HybridRepoComposite`/`HybridRepoFactory`
and the new `BoxRepo::Hybrid`/`BoxRepoFactory::Hybrid` variants exactly
like the existing `Fjall` variant is gated — hybrid mode is meaningless
without the fjall feature, since the durable mirror IS fjall.)

Add `BoxRepo::Hybrid(Arc<HybridRepoComposite>)` and
`BoxRepoFactory::Hybrid(HybridRepoFactory)` following the exact enum-arm
style already used for `Fjall`.

### 2. `store_get` routing — by STORE NAME, not by key content

`HybridRepoComposite`'s `store_get(name)` (called from `BoxRepo::Hybrid`'s
match arm, mirroring how `InMemory`/`Fjall` delegate directly) must
MEMOIZE per name (so `MirroredStore`'s hydration, which streams the
entire mirror, happens ONCE per store name, not on every `store_get`
call — verify this matters by checking `InMemoryRepo::store_get`'s own
existing memoization via its `stores: DashMap<String, Arc<InMemoryStore>>`
field, `crates/shamir-storage/src/storage_in_memory.rs` — your hybrid
composite needs the equivalent behavior). Since hydration is `async`, use
the SAME "clone the `Arc<OnceCell>` out from under the map guard, then
`.await` the init OUTSIDE the guard" pattern already established and
documented at `crates/shamir-engine/src/repo/repo_instance.rs` ~line
306-326 (`RepoInstance::get_table`) — read that method's doc comment for
the exact deadlock rationale (DashMap/`scc::HashMap` shards use a
synchronous lock; holding a shard guard across a long `.await` risks
worker-thread starvation under oversubscription) and mirror the same
shape here, not a novel one.

Routing table (route by the store NAME prefix — these are the ONLY 6
names any production caller ever requests, confirmed by an exhaustive
grep of `store_get(` call sites during this campaign's design phase; if
you find a 7th name this brief doesn't cover, treat it per the
"anything else" fallback below rather than guessing):

| Name pattern | Backing | Why |
|---|---|---|
| `__history__<table>` | `mem` (plain `InMemoryStore`, no mirroring) | **This is where the actual row data lives** for every table (MVCC tables always route data through the version log, never `__data__` — verify this against `repo_instance.rs`'s own doc comment near where `MvccStore` is attached, ~line 396-418, and its regression test asserting `__data__` has zero entries for MVCC tables). Ephemeral by design — this is the "data doesn't survive" half of the hybrid contract. |
| `__data__<table>` | `mem` | Dead keyspace for any MVCC table (see above) — route to plain in-memory for consistency/simplicity; no correctness weight either way since nothing ever reads/writes here for a real table. |
| `__info__<table>` | `MirroredStore::new(disk_store, is_durable_table_config).await?` where `disk_store` is `self.disk.store_get(name).await?` | Mixed keyspace (config + data-derived state) — Step 1's classifier is EXACTLY the boundary. |
| `__interner__` | `MirroredStore::new(disk_store, |_| true).await?` (an ALLOW-ALL classifier — every key in this store is durable config) | **Critical coupling, do not get this wrong**: index definitions in `__info__` store INTERNED `u64` field ids, not field-name strings. If `__interner__` does not persist alongside `__info__`'s index definitions, a reopened hybrid table's fresh interner reassigns those same ids to whatever field happens to be touched first — silent index corruption (an index on `email` silently becomes an index on some other field). `__interner__` must ALWAYS persist whenever `__info__`'s config does; using an allow-all classifier for this ONE store name (rather than trying to reuse `is_durable_table_config`, which is scoped to the system-record shape and would incorrectly reject the interner's own key shapes) is the correct fix. |
| `__tx__` | `mem` | MVCC/WAL recovery markers (`LastCommittedVersion`, `NextTxId`, `ReplicationBookmark`) are derived from the (ephemeral) committed transaction history — persisting them without the history they describe would let a reopened table seed counters from a version space with no corresponding rows, or a replication follower skip re-applying events whose effects were actually lost. (Step 1's classifier already independently excludes these SAME tags from `__info__`'s allowlist for the identical reason — this row just confirms `__tx__` as a WHOLE store gets the same treatment, since these markers live in `__tx__`, not `__info__` — verify this store/key placement against `repo_instance.rs`'s actual `store_get("__tx__")` call sites, ~line 574/635/703, before finalizing.) |
| `__changelog__` | `mem` | The changefeed journal describes ephemeral row data — same rationale as `__tx__`. |
| any OTHER name | `mem` + `log::warn!("hybrid repo: unrecognized store name {name:?}, defaulting to ephemeral")` + `debug_assert!(false, "...")` | Fail-safe: never persist unknown state by accident. The `debug_assert` turns an unrecognized name into a loud CI failure (debug builds) rather than a silent, unreviewed persistence decision; production (release) builds just log and continue ephemeral, which is the safe direction per this whole design's allowlist philosophy. |

### 3. Other `Repo`/`RepoFactory` trait pieces

- `store_delete(name)`: delete from BOTH `mem` and `disk` (for the
  `__info__`/`__interner__` names — deleting from `mem` alone would leave
  a stale durable copy that resurrects on the next hydration); return the
  logical OR of both results. For `mem`-only names, `disk.store_delete`
  is presumably a no-op/not-found — check whether that's safe to call
  unconditionally (simpler code) or whether it needs to be skipped for
  names never written to disk (marginal difference either way — pick
  whichever is the smaller, clearer diff).
- `stores_list()`: deduplicated union of both tiers' lists.
- `HybridRepoFactory::create()` (the `RepoFactory` impl): build `mem` via
  `InMemoryRepo::new()`, build `disk` via `FjallRepo::new(info_path)`
  DIRECTLY — NOT wrapped in `MemBuffer` (unlike `BoxRepoFactory::fjall`'s
  default-wrapped convenience constructor) — config writes are rare DDL
  events; they should be durable promptly, not sit in a buffered flush
  window. Check `FjallRepoFactory::create`'s existing shape
  (`repo_types.rs` ~line 131-146) for the `spawn_blocking` pattern to
  mirror for the disk side's construction.
- `BoxRepoFactory::hybrid(info_path: impl Into<PathBuf>) -> Self`
  constructor, matching the naming/shape of `fjall`/`fjall_raw`.
- `BoxRepoFactory::backing_dir()`: add a `Hybrid` arm returning `None`
  (NOT `Some(info_path)`). Read this method's own doc comment first (it
  states its sole consumer decides whether to use a file-backed WAL group
  or the in-memory KV-marker WAL) — a hybrid repo's actual DATA is
  ephemeral, so a file-backed WAL would durably record inflight write
  markers that, on the next open, replay into a freshly-EMPTY
  `__history__` — resurrecting a torn fragment of a dataset that's
  supposed to be gone. Returning `None` here correctly selects the
  in-memory WAL path, consistent with the ephemeral-data half of this
  design.
- `Clone for BoxRepoFactory`: add a `Hybrid` arm,
  `BoxRepoFactory::hybrid(f.info_path.clone())`, matching the existing
  arms' style exactly.

## Tests

**MANDATORY, test-then-fix in the same commit**, in
`crates/shamir-engine/src/repo/tests/` (extend the existing test module
structure — check what file(s) already test `BoxRepo`/`BoxRepoFactory`
routing, if any, and follow that convention; otherwise a new
`hybrid_repo_tests.rs`):

1. For each of the 6 known store names, assert `store_get` returns a
   store backed by the INTENDED tier — the simplest reliable way to
   assert this is probably: write a value via the returned store, then
   check whether it's ALSO visible by going through the `disk`
   (`FjallRepo`) tier directly for `__info__`/`__interner__` names (should
   be visible, for a classified key) vs. the other 4 names (should be
   invisible in `disk` — confirms they never reached the mirror at all).
2. `backing_dir()` on a `BoxRepoFactory::Hybrid` is `None`.
3. `store_delete` on an `__info__` name removes the entry from BOTH tiers
   (write a classified key, delete the store, confirm a freshly
   `store_get`'d instance under the same name doesn't see it).
4. The "unrecognized name" fallback: call `store_get` with a made-up name
   outside the 6 known ones and confirm it still returns a working
   (ephemeral) store rather than erroring (debug-mode `debug_assert`
   firing during a `#[should_panic]`-style test, or simply run this
   specific test in a way that tolerates the assert if your test harness
   runs in debug profile — check this crate's existing convention for
   testing a `debug_assert!`-guarded fallback path, if any exists,
   otherwise use judgment).

## Constraints

- Do NOT modify `MirroredStore` itself (#835, already landed) — only
  consume it.
- Do NOT wire this into `shamir-db`'s DDL surface yet
  (`extract_storage_type`/`factory_from_meta`/`handle_create_repo`) —
  that is Step 4 (#838), blocked behind Step 3 (#837).
- Do NOT touch `TableManager::create`'s open sequence — Step 3 (#837)
  MEASURES whether it already works correctly against a hybrid repo
  built directly via `BoxRepoFactory::hybrid(...)`, without any DDL
  surface; only fix `TableManager` if that measurement finds a real gap.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- hybrid
./scripts/test.sh -p shamir-engine -- repo
```
