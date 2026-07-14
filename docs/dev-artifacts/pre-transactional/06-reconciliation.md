# Этап 5. Reconciliation с background и Migration

**Срок:** 4 дня. **Зависит от:** Этап 0-4.

Цель — устранить интерференцию transactional layer с background
tasks и migration coordinator. Никто не должен «чинить» in-flight
transactions, никто не должен sample inconsistent state.

## 5.1. MemBuffer flusher

**Контекст.** MemBuffer — write-back cache. `dirty` накапливает
ключи между write и actual flush в base store.

**Риск.** Если tx writes идут через MemBuffer → их видят
non-tx-aware readers, нарушение isolation.

**Решение.** Tx writes минуют MemBuffer:

```rust
// MvccStore writes идут напрямую в self.main (Arc<dyn Store>),
// который может быть либо raw backend, либо MemBuffer-wrapped.
// Tx commits должны bypass MemBuffer layer.

impl MvccStore {
    pub fn new(main: Arc<dyn Store>, history: Arc<dyn Store>, gate: Arc<RepoTxGate>) -> Self;

    /// Internal: возвращает base store, обходя MemBuffer wrapper если
    /// он есть. Detected через downcast.
    fn bypass_membuffer(store: &Arc<dyn Store>) -> Arc<dyn Store> {
        if let Some(mb) = store.as_any().downcast_ref::<MemBufferStore>() {
            return Arc::clone(mb.inner());
        }
        Arc::clone(store)
    }
}
```

**Cost.** Tx commits теряют write-back caching → каждый commit
делает sync write в backend. Это OK: commit is the durability point
— это **зачем** WAL и flush. Non-tx writes продолжают идти через
MemBuffer как сейчас, no regression.

**Альтернатива.** Если bypass окажется проблемой (например, для
бэкендов где flush — дорогая операция), MemBuffer может стать
**tx-aware**: dirty entries помечаются `tx_id`, flush only committed
ones. Это сложнее. Пока выбираем bypass.

**Acceptance.**
- Tx commit пишет в base store, не задерживает в MemBuffer.
- Non-tx writes используют MemBuffer как сейчас (через `Store::set`).
- Bench: non-tx write throughput не регрессирует.

## 5.2. Migration coordinator

**Контекст.** `MigrationCoordinator::run_snapshot` копирует data_store
байты с src на dst. `drain_shadow_log` применяет shadow log entries.
Cutover делает swap.

**Риск.** Если migration работает на repo, на котором есть open tx
— получается inconsistent dst:
- Snapshot copy может зацепить **uncommitted** writes (если они
  попали в main).
- Shadow log не tx-aware.

**Решение.** Два варианта.

### Вариант A — Migration waits for idle (выбираем)

```rust
impl MigrationCoordinator {
    pub async fn wait_for_idle_tx(
        &self,
        repo: &RepoInstance,
        timeout: Duration,
    ) -> DbResult<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if repo.tx_gate().active_snapshots_empty() {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err(DbError::MigrationStalled);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
```

В `BatchOp::CommitMigration`:
```rust
coord.wait_for_idle_tx(&src_repo, Duration::from_secs(30)).await?;
coord.final_drain_and_commit().await?;
```

Это значит: cutover **блокируется** новыми tx на src на короткий
период. Существующие tx на src дорабатывают, новых не пускаем.

Для **новых tx во время migration**: `RepoTxGate::open_snapshot()`
возвращает ошибку `migration_in_progress` если есть active migration
для этого repo. Это означает короткий блок (секунды) — для production
acceptable.

### Вариант B — Migration сама становится tx

Сложнее: migration entire = одна big tx. Дорого (long-running tx
wedges GC), не выбираем.

**Acceptance.**
- Migration test: открываем tx → запускаем migration → migration
  ждёт. tx commit → migration cutover проходит.
- Concurrent: tx ставится во время cutover → возвращает ошибку
  `migration_in_progress`.

## 5.3. Audit log

**Контекст.** `audit_chain` лежит в **отдельном** repo (системном).
Cross-repo atomicity вне scope.

**Решение.** Pragmatic deferred-append:

```rust
// В executor commit phase
async fn commit(&self, tx: &mut TxContext, ...) -> DbResult<()> {
    // ... phases 1-7 ...

    // Phase 8 (best-effort, non-atomic with main commit):
    let audit_entry = AuditEntry {
        tx_id: tx.tx_id,
        outcome: "committed",
        ops_summary: tx.summarize(),
        ts: now_ns(),
    };
    self.shamir.audit_log().append(audit_entry).await?;

    Ok(())
}
```

На abort:
```rust
async fn rollback(&self, tx: &mut TxContext) {
    // ... tx context drops ...

    let audit_entry = AuditEntry { outcome: "aborted", reason, ... };
    self.shamir.audit_log().append(audit_entry).await?;
}
```

**Trade-off.** Если crash между Phase 7 (publish) и Phase 8 (audit
append) — tx applied, но audit не записан. Acceptable: audit
существует для observability/compliance, не correctness. Можно
дополнить отдельным background job который сверяет recent commits
vs audit, добавляет missing entries (eventual consistency).

**Acceptance.**
- Committed tx → audit_chain содержит entry с outcome=committed.
- Aborted tx → entry с outcome=aborted + reason.
- Crash mid Phase 7-8 → tx видна, audit missing (документировано).

## 5.4. Auto-verify watchdog

**Контекст.** `TableManager` имеет background watchdog который
periodically делает `verify` pass (sample N writes, check
consistency). Работает с storage напрямую.

**Риск.** Watchdog может sample tx-uncommitted writes → report'нуть
inconsistency, дёрнуть recovery, сломать всё.

**Решение.** Watchdog работает только на **committed** snapshot
через MvccStore:

```rust
impl TableManager {
    async fn auto_verify_tick(&self) {
        let snapshot = self.repo.tx_gate().last_committed();
        // verify pass uses mvcc.get_at(_, snapshot) → видит только
        // committed data. Не trip on in-flight tx.
        ...
    }
}
```

**Acceptance.**
- Watchdog tick во время open tx → no false positive.
- Watchdog tick после commit → видит свежие записи.

## 5.5. Backup / restore

**Контекст.** Backup сейчас копирует storage byte-by-byte (предполагаю
— не верифицировал). Restore — обратный процесс.

**Риск.** Backup во время open tx может зацепить inconsistent state.

**Решение.** Backup ждёт `idle_tx` так же как migration:

```rust
async fn backup_repo(&self, repo: &RepoInstance) -> DbResult<BackupArchive> {
    repo.tx_gate().wait_for_idle(Duration::from_secs(60)).await?;
    // снимаем backup на consistent point. Новые tx будут открываться
    // на новых snapshot — backup отражает state на момент start.
    ...
}
```

Альтернатива: backup делает copy at `last_committed_version`. Это
снимает требование idle, но требует поддержки snapshot reads в backup
flow. Сложнее, делаем простой вариант.

**Acceptance.**
- Backup → restore → state восстановлен.
- Backup во время active tx → ждёт idle.

## 5.6. Доктор (existing TableManager auto-recovery)

Существующий recovery on open (Doctor) работает по WAL markers. Он
уже tx-aware если правильно поправить — recovery будет:

1. Открыть per-table WAL → forward-fix as before (для V1 entries).
2. Открыть repo-level WAL → forward-fix V2 entries (apply ops через
   MvccStore).
3. Перестроить version_cache из last_committed_version.

**Acceptance.**
- Симуляция crash с mixed V1 + V2 entries в WAL — recovery
  правильно обрабатывает оба.

## Порядок работы

1. MemBuffer bypass for MvccStore writes (1 день).
2. `wait_for_idle_tx` + migration integration (1 день).
3. Audit log deferred-append (0.5 дня).
4. Auto-verify watchdog снимок-aware (0.5 дня).
5. Backup wait_for_idle (0.5 дня).
6. Doctor V1+V2 unified recovery (0.5 дня).

**Не делаем здесь:**
- Не пишем GC (Этап 6).
- Не делаем concurrent multi-connection harness (Этап 7).
