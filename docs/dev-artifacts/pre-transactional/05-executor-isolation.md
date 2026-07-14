# Этап 4. Executor + изоляция (SI / SSI) + cross-repo guard

**Срок:** 4-5 дней. **Зависит от:** Этап 0-3.

Цель — подключить всю созданную machinery к batch executor так,
чтобы `transactional: true` в `BatchRequest` стало содержательным.
Реализовать два уровня изоляции (Snapshot — default, Serializable —
опционально).

## 4.1. Executor integration

Текущий batch executor (см. `crates/shamir-engine/src/query/batch/`)
прогоняет queries последовательно, каждый напрямую дёргает
`TableManager::execute_*`. Меняется на:

```rust
async fn execute_batch(&self, req: BatchRequest) -> BatchResponse {
    // Cross-repo guard (4.4): tx must target single repo
    if req.transactional && distinct_repos(&req.queries).len() > 1 {
        return BatchResponse::error("tx_cross_repo_not_supported");
    }

    if !req.transactional {
        return self.execute_non_tx(req).await;     // как сейчас
    }

    // Tx path
    let repo = single_repo(&req.queries);
    let gate = repo.tx_gate();
    let snapshot_guard = gate.open_snapshot().await;
    let snapshot = snapshot_guard.version();
    let tx_id = gate.repo_wal().fresh_txn_id();
    let isolation = parse_isolation(&req).unwrap_or(IsolationLevel::Snapshot);

    let mut tx = TxContext::new(tx_id, repo.id(), snapshot, isolation);

    let results: Result<Vec<QueryResult>, DbError> = async {
        for query in req.queries {
            self.execute_query(&query, Some(&mut tx)).await?;
        }
        self.commit(&mut tx, &repo, &gate).await?;
        Ok(...)
    }.await;

    match results {
        Ok(rs) => BatchResponse { transaction: Some(TxInfo { id: tx_id, status: "committed" }), ..  },
        Err(e) => BatchResponse { transaction: Some(TxInfo { id: tx_id, status: "aborted", reason: e.to_string() }), .. },
    }
    // tx drop → RAII rollback если не закоммитили
    // snapshot_guard drop → удаляется из active_snapshots
}
```

## 4.2. Commit phase

Один **критический участок** под `gate.commit_lock`:

```rust
async fn commit(&self, tx: &mut TxContext, repo: &RepoInstance, gate: &RepoTxGate) -> DbResult<()> {
    let lock = gate.commit_lock().await;

    // Phase 1: interner overlay merge → may remap ids in write_set
    let id_remap = commit_interner_overlay(repo.interner(), &tx.interner_overlay).await?;
    tx.apply_id_remap(&id_remap);

    // Phase 2 (SSI only): read-set validation
    if tx.isolation == IsolationLevel::Serializable {
        gate.validate_read_set(&tx.read_set, tx.snapshot_version)?;
        // если abort — drop lock, return tx_conflict
    }

    // Phase 3: assign new version
    let new_version = gate.assign_next_version();

    // Phase 4: write repo WAL entry (BEFORE physical writes)
    let entry = WalEntryV2::from_tx_context(tx, new_version);
    repo.wal().begin(entry).await?;

    // Phase 5: physical writes
    //   5a. Data: foreach (table, staging) in tx.write_set → mvcc.set_versioned(... new_version)
    //   5b. Indexes: apply_index_ops(tx.index_write_set, info_store, ...)
    //   5c. HNSW: foreach (table, staged) → hnsw_adapter.commit_staged(tx.tx_id)
    //   5d. Counters: foreach (table, delta) → counter.add(delta)

    // Phase 6: publish — atomic publish-committed
    gate.publish_committed(new_version).await?;

    // Phase 7: WAL cleanup (commit marker)
    repo.wal().commit(tx.tx_id).await?;

    drop(lock);
    Ok(())
}
```

**Порядок фаз важен.** Если crash:
- Между Phase 4 и 7 → recovery видит entry в WAL → forward-fix.
- После Phase 7 → entry удалена, всё видно через main store.

## 4.3. Snapshot Isolation (default)

- Reads видят snapshot (через MvccStore).
- Writes идут в `write_set`.
- Commit без read-set validation → last-writer-wins. Lost update
  возможен под contention (T1 и T2 обе пишут X; T2 commits last —
  её версия видна).
- Достаточно для большинства use cases.

```rust
// нет дополнительной логики commit — Phase 2 skipped
```

## 4.4. Serializable Snapshot Isolation (опционально)

Wire-format: `"isolation": "serializable"` в `BatchRequest`.

Read-set tracking — каждый read в tx запоминает `(table, key,
version_seen)`:

```rust
impl TableManager {
    pub async fn read_one(&self, rid, tx: Option<&mut TxContext>) -> DbResult<Bytes> {
        // ... read из write_set / mvcc ...

        if let Some(tx) = tx {
            if tx.isolation == IsolationLevel::Serializable {
                let v = self.mvcc.version_of(&rid.to_bytes()).await;
                tx.read_set.insert((self.name.clone(), rid.to_bytes()), v);
            }
        }

        ...
    }
}
```

Validation в commit phase 2:

```rust
impl RepoTxGate {
    pub fn validate_read_set(
        &self,
        read_set: &HashMap<(TableName, Bytes), u64>,
        snapshot: u64,
    ) -> Result<(), TxConflict> {
        for ((table, key), version_seen) in read_set {
            let current = self.mvcc(table).current_version(key);
            if current > *version_seen && current > snapshot {
                return Err(TxConflict { key: key.clone() });
            }
        }
        Ok(())
    }
}
```

**Client retry это ответственность клиента** — server возвращает
`code: "tx_conflict"`, дальше за SDK.

## 4.5. Cross-repo guard

`BatchRequest` в tx mode должен targeting один repo. Иначе
2PC между repos потребовался бы — out of scope.

```rust
fn distinct_repos(queries: &[Query]) -> HashSet<String> {
    queries.iter().filter_map(|q| q.target_repo()).collect()
}
```

Если `> 1` → ошибка `tx_cross_repo_not_supported`.

## 4.6. Wire-format расширение

```text
// BatchRequest
{
    "id": 1,
    "transactional": true,
    "isolation": "snapshot" | "serializable",   // optional, default "snapshot"
    "queries": { ... }
}

// BatchResponse
{
    "id": 1,
    "results": { ... },
    "transaction": {
        "tx_id": 42,
        "status": "committed" | "aborted",
        "reason": "tx_conflict" | "validation_failed" | null,
        "snapshot_version": 100,
        "commit_version": 105
    }
}
```

## 4.7. Cross-table batch

Внутри одного repo одна tx может затрагивать N таблиц. Это значит:
- `TxContext.write_set` индексирован по table.
- `TxContext.staged_hnsw_inserts` — то же.
- Commit phase 5 проходит по всем таблицам.
- WAL entry содержит ops по всем таблицам.

Этого достаточно для cross-table atomicity внутри repo.

## Acceptance

- Happy path: 5 writes + 1 read в одной tx, commit, outside observer
  читает — все 5 видны.
- Abort path: 2 writes + ошибка в 3-м query → outside observer не
  видит ни одного write.
- Read-after-write inside tx — свои writes видны.
- SI lost update: 2 параллельные tx пишут X — обе commit, последняя
  побеждает. Не ошибка, документированное поведение.
- SSI conflict: 2 параллельные tx с `isolation: "serializable"`
  читают X, обе пишут X — одна committed, другая `tx_conflict`.
- Cross-repo guard: tx batch с queries на 2 repo → `tx_cross_repo_not_supported`.
- Cross-table internal: tx batch с queries на 2 table внутри repo →
  atomicity сохраняется.

## Порядок работы (atomized sub-stages)

### Landed

| Sub-stage | Commit | Summary |
|---|---|---|
| **4.A** | `7a16400` | `BatchRequest.isolation: Option<String>` + expanded `TransactionInfo { tx_id, status, reason, snapshot_version, commit_version }` + helpers |
| **4.B** | `83fee0e` | `TxContext::apply_id_remap` + `StagingStore::rewrite_set_bytes` — overlay-id → base-id rewrite in staged record bytes |
| **4.C** | `4059c81` | Cross-repo guard: `distinct_repos` helper + `BatchError::CrossRepoNotSupported` + executor check |
| **4.D.1** | `7f00341` | `RepoInstance::tx_gate() / repo_wal()` lazy OnceCell accessors with recovery-marker seeding |
| **4.D.2** | `a20b926` | `commit_tx` 7-phase scaffold (phases 3/4/6/7 wired, phases 1/2/5 TODO stubs) |
| **4.D.3** | `afe9f81` | `TableResolver::resolve_repo` + `RepoInstance::begin_tx / commit_tx` facade |
| **4.D.4** | `8131f2c` | Phase 5 actual physical writes: `base.transact(staging.drain())` per table |
| **4.D.5** | `6985f04` | SSI skeleton: `MvccStore::version_of`, `TxContext::validate_read_set(provider)`, Phase 2 wired with stub provider `\|_, _\| 0` |
| **4.D.6.a** | `cb92331` | `TableManager::insert_tx` + `table_token()` + `TxContext::ensure_table_staging` |
| **4.D.6.b** | `2a2dad8` | `update_tx` / `delete_tx` / `set_tx` + `StagedMutation` extract |
| **4.D.6.c.1** | `1e859de` | `execute_insert_tx` parallel wrapper |
| **4.D.6.c.2** | `b1854e2` | `execute_update_tx` / `execute_delete_tx` / `execute_set_tx` |
| **4.D.6.c.3** | `a4aa6c4` | `QueryRunner<'a>` struct refactor + tx-aware dispatch |
| **4.D.6.d** | `2316d7b` | `execute_batch` tx mode wiring + SI happy-path E2E test |
| **4.D.6.e** | `f4e65e9` | `VersionProvider` trait + SSI provider injection hook |
| **4.G.1** | `81c99fc` | `repo_token(name)` deterministic ID + `RepoInstance::name()` — closes compromise 1 |
| **4.G.2** | `ba36a13` | `StagingStore::snapshot_ops` + WAL entry contains Put/Delete data ops — closes critical crash-safety gap |
| **4.G.3** | `f422a9b` | `commit_tx` Phase 1 wired safely with empty overlay (no-op until Stage 5 LayeredInterner integration) |
| **4.G.4** | `803a4c2` | `RepoInstance::per_table_mvcc` + `RepoVersionProvider` auto-attached for Serializable — closes compromise 5 |
| **4.G.5/6/7** | (this commit) | `idx_id` invariant doc + `tx_pipeline.rs` bench suite + known limitations section |

## Known Production Limitations (post-Stage 4)

Несмотря на closed compromises in Stage 4.G, два residual gaps
известны и закрываются в Stage 5+:

### 1. tx writes do not bump MvccStore versions

Commit phase 5 applies write_set via `base.transact(drain)` directly
on the data_store, bypassing `MvccStore::set_versioned`. Consequence:
`MvccStore::version_of(key)` returns `0` for keys last written by
tx-mode mutations.

Impact:
- SSI conflict detection (Stage 4.D.5 + 4.G.4) cannot fire on
  tx-written keys — version stays at 0 forever from the mvcc map's
  perspective.
- History store stays empty for tx writes — no snapshot reads on
  old versions of tx-written rows.

Fix path (Stage 5+): route Phase 5 writes through
`MvccStore::set_versioned` instead of `base.transact`. Requires
careful interaction with history archival under active snapshots.

### 2. Recovery code for V2 WAL entries not implemented

`WalEntryV2` now contains all tx ops (data Put/Delete, index ops,
counter delta, interner overlay) — self-contained per 4.G.2. But
recovery code that reads these entries on repo open and replays
them does not exist yet.

Impact:
- Crash mid-commit_tx (between Phase 4 begin and Phase 7 commit)
  leaves an inflight WAL V2 entry that nobody applies on next open.
- Tx writes WILL be lost on such a crash even though the entry is
  durable.

Fix path (Stage 7): write V2 recovery loop in `RepoInstance::open`
or equivalent that lists inflight V2 entries, applies their ops,
removes the marker.

### 3. `idx_id: 0` placeholder in WAL IndexPut/IndexDel

See `WalOpV2::IndexPut` doc comment — recovery decodes `idx_id` from
the posting key prefix (existing invariant). Either keeps as-is or
threads through `IndexWriteOp` in Stage 5 — decision at recovery
implementation time.

### 4. Repo-level interner not yet present

`tx.repo_id` and WAL entry `repo_id_interned` use `repo_token(name)`
(DefaultHasher) instead of a real interned ID. Same for
`table_id_interned` via `table_token`. Stage 5 reconciliation swaps
to real interner — only the value source changes, struct shapes
remain stable.

### 5. `tx.interner_overlay` not populated

LayeredInterner integration with TableManager / executor not landed —
overlay stays empty in all production flows. `commit_tx` Phase 1
runs `apply_id_remap` with empty remap (no-op safe wire). Stage 5
populates overlay through tx-aware interning paths.

### Remaining

**4.D.6 — Write pipeline tx-aware (5 sub-stages)**

The largest remaining piece: mutation methods must route through
`TxContext` when `tx.is_some()`. Decomposed into:

| Sub-stage | Scope | Notes |
|---|---|---|
| **4.D.6.a** | `TableManager::insert_tx(value, tx)` | Stage data in `tx.write_set[table_hash]`, accumulate index ops via existing `plan_insert`, bump `counter_deltas`, stage HNSW. `table_id_interned` = `fxhash(self.name)` placeholder (Stage 5 replaces with repo-level interner). Non-tx ⇒ current `insert`. |
| **4.D.6.b** | `update_tx`, `delete_tx`, `set_tx` | Same pattern as insert_tx for remaining mutation types. |
| **4.D.6.c** | `execute_query_tx` parallel dispatch path | New function alongside `execute_query`. Routes each `BatchOp` through `_tx` methods. Wire into `execute_batch` when `request.transactional`. |
| **4.D.6.d** | SI happy-path integration test | Full end-to-end: `repo.begin_tx(SI)` → 3 inserts via `insert_tx` → `commit_tx` → outside observer reads via `table.get` → sees all 3. Also test read-after-write inside tx (via `read_one_tx`). |
| **4.D.6.e** | SSI real version provider | Build `HashMap<u64, Arc<MvccStore>>` per-table map during commit. Plug into Phase 2 `validate_read_set` instead of stub `\|_, _\| 0`. Test: T1 reads X, T2 writes X + commit, T1 tries commit → `SsiConflict`. |

Key design decisions for 4.D.6:

### D7. `table_token()` accessor for table identity

**Problem.** `TxContext.write_set: HashMap<u64, StagingStore>` is keyed
by interned table id. But repo-level interner is Stage 5. TableManager
knows only `name: String`.

**Solution.** `TableManager::table_token() -> u64` accessor. Stage 4
implementation = `fxhash(self.name)`. Stage 5 swap to
`LayeredInterner.touch(self.name).id()` — callsites stable.

`TxContext` gains `table_tokens: HashMap<u64, String>` (token → name)
and a helper `ensure_table_staging(token, name, base) -> &mut StagingStore`
that populates both maps simultaneously.

```rust
impl TxContext {
    pub fn ensure_table_staging(
        &mut self, token: u64, name: &str, base: Arc<dyn Store>,
    ) -> &mut StagingStore {
        self.table_tokens.entry(token).or_insert_with(|| name.to_string());
        self.write_set.entry(token).or_insert_with(|| StagingStore::new(base))
    }
}
```

### D8. `StagedMutation` value object for mutation types

**Problem.** 4 mutation types × tx-aware logic = ~600 lines of
copy-paste (stage data, plan index ops, bump counter, stage HNSW).

**Solution.** Extract `StagedMutation { data_op, index_ops, counter_delta }`
struct + `TableManager::stage_mutation(rid, m, tx)` helper. Each
`*_tx` method becomes ~20 lines: compute effect → stage.

```rust
struct StagedMutation {
    data_op: KvOp,                // Set or Remove
    index_ops: Vec<IndexWriteOp>, // from plan_insert/update/delete
    counter_delta: i64,           // +1 insert, -1 delete, 0 update
}

impl TableManager {
    async fn stage_mutation(&self, m: StagedMutation, tx: &mut TxContext) -> DbResult<()> {
        let staging = tx.ensure_table_staging(
            self.table_token(), self.name(), self.table.data_store().clone(),
        );
        match m.data_op {
            KvOp::Set(k, v) => staging.set(k, v).await,
            KvOp::Remove(k) => staging.remove(k).await,
        }
        tx.index_write_set.extend(m.index_ops);
        tx.bump_counter(self.table_token(), m.counter_delta);
        Ok(())
    }
}
```

Total ~150 lines for all 4 mutation types instead of 600.

### D9. `QueryRunner` struct for tx-aware dispatch

**Problem.** `execute_query` is a free function without tx parameter.
Adding `Option<&mut TxContext>` touches dozens of callsites. Parallel
`execute_query_tx` function duplicates the entire dispatcher.

**Solution.** `QueryRunner<'a>` struct that encapsulates
`resolver + admin + tx: Option<&mut TxContext>`. The existing free
function becomes a thin wrapper `QueryRunner { tx: None }.run(op)`.
Tx mode: `QueryRunner { tx: Some(&mut ctx) }.run(op)`.

```rust
pub struct QueryRunner<'a> {
    pub resolver: &'a dyn TableResolver,
    pub admin: Option<&'a dyn AdminExecutor>,
    pub tx: Option<&'a mut TxContext>,
}

impl<'a> QueryRunner<'a> {
    pub async fn run(&mut self, op: &BatchOp, ...) -> Result<QueryResult, BatchError> {
        // dispatch — tx-aware methods when self.tx.is_some()
    }
}
```

Existing `execute_query(query, resolver, admin)` becomes:
```rust
QueryRunner { resolver, admin, tx: None }.run(&query.op, ...).await
```

No callsite changes. tx state encapsulated. Refactor, not duplication.

- **`execute_query_tx` as parallel path.** Don't modify existing
  `execute_query` signature — add a parallel function. Zero risk to
  non-tx flows. Convergence into one function (with
  `Option<&mut TxContext>`) deferred to Stage 5 reconciliation.

- **`read_one_tx` enhanced.** Currently forwards to `table.get`.
  Stage 4.D.6.d wires: (1) check `tx.write_set[table].get(rid)`
  first, (2) fall through to `mvcc.get_at(snapshot)` or `table.get`.

**4.E — SDK updates**

| Sub-stage | Scope |
|---|---|
| **4.E.1** | Rust client (`shamir-client`): update `BatchRequest` builder for `isolation` field + parse new `TransactionInfo` response shape |
| **4.E.2** | Node.js client (`shamir-client-node`): same wire-format updates in napi bindings |

Depends on: 4.D.6.c (executor wiring) — wire format is landed (4.A)
but useless until the server actually commits transactions.

**4.F — Acceptance integration tests**

| Test | Verifies |
|---|---|
| **SI happy path** | 5 writes + 1 read → commit → outside observer reads all 5 |
| **Abort path** | 2 writes + error on 3rd → outside observer sees none |
| **Read-after-write** | tx inserts X, same tx reads X → sees staged value |
| **SI lost update** | T1 writes X, T2 writes X → both commit → last writer wins |
| **SSI conflict** | T1 reads X, T2 writes X + commits, T1 tries commit → `tx_conflict` |
| **Cross-table internal** | tx batch writes to 2 tables in same repo → atomic (both or neither visible) |

Depends on: 4.D.6.d (SI integration) + 4.D.6.e (SSI provider).

**Не делаем здесь:**
- Не пишем GC (Этап 6).
- Не делаем concurrent multi-connection harness (Этап 7).
- Не трогаем migration coordinator (Этап 5).
