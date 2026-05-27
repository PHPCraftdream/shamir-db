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

```jsonc
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

## Порядок работы

1. `parse_isolation` + wire-format на стороне executor (0.5 дня).
2. Cross-repo guard + tests (0.5 дня).
3. Commit phase 1-7 step-by-step с unit tests (2 дня).
4. SI integration test (0.5 дня).
5. SSI read-set tracking + validation (1 день).
6. SDK update (Rust client + Node SDK для wire response shape)
   (0.5 дня).

**Не делаем здесь:**
- Не пишем GC (Этап 6).
- Не делаем concurrent multi-connection harness (Этап 7).
- Не трогаем migration coordinator (Этап 5).
