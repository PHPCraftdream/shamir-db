# Этап 6. GC worker + telemetry + max-tx-lifetime cap

**Срок:** 3 дня. **Зависит от:** Этап 0-3.

Цель — гарантировать, что history store не растёт неограниченно,
что operators видят health состояние tx subsystem, и что один
stuck tx не блокирует всю систему.

## 6.1. `GcWorker` per repo

```rust
pub struct GcWorker {
    repo_gate: Arc<RepoTxGate>,
    history: Arc<dyn Store>,
    period: Duration,         // config, default 60s
    shutdown: Arc<AtomicBool>,
}

impl GcWorker {
    pub fn spawn(repo_gate, history, period) -> Self {
        let worker = ...;
        tokio::spawn(worker.run_loop());
        worker
    }

    async fn run_loop(&self) {
        loop {
            if self.shutdown.load(Acquire) { break; }
            tokio::time::sleep(self.period).await;

            let min_alive = self.repo_gate.min_alive();
            self.gc_below(min_alive).await;
        }
    }

    async fn gc_below(&self, min_alive: u64) -> DbResult<()> {
        // Scan history store по prefix
        // Для каждого key:version, если version < min_alive — delete.
        // Делать batch'ами через transact() (Этап 0.2).

        let mut stream = self.history.iter_stream(1024);
        let mut to_delete: Vec<KvOp> = Vec::new();

        while let Some(batch) = stream.next().await {
            for (key, _) in batch? {
                let (_, version) = decode_version_key(&key);
                if version < min_alive {
                    to_delete.push(KvOp::Remove(key));
                }
                if to_delete.len() >= 1000 {
                    self.history.transact(std::mem::take(&mut to_delete)).await?;
                }
            }
        }

        if !to_delete.is_empty() {
            self.history.transact(to_delete).await?;
        }
        Ok(())
    }
}
```

**`min_alive` calculation:**

```rust
impl RepoTxGate {
    pub fn min_alive(&self) -> u64 {
        // min(active_snapshots) — если пусто, то last_committed_version
        // (всё что ≤ него — можно чистить кроме последней per key).
        let mut min_snap = u64::MAX;
        self.active_snapshots.scan(|&v, _| {
            min_snap = min_snap.min(v);
        });
        if min_snap == u64::MAX {
            return self.last_committed_version.load(Acquire);
        }
        min_snap
    }
}
```

**Сохраняем последнюю версию per key.** GC не должен удалять самую
свежую committed версию даже если она < min_alive — иначе snapshot
read для несуществующих ключей сломается. Реализация:

```rust
// При GC: для каждого ключа keep наибольшую версию ниже min_alive
// (она нужна как fallback для future snapshot reads ≤ min_alive — да,
// они не могут существовать сейчас если min_alive = current snapshot,
// но для recovery от старого `__last_committed_version__` маркера).
//
// Проще: keep top-1 in history для каждого base key.
```

Это требует двойного scan history или sorted iteration с
grouped-by-key. Для in-memory история — простой scan. Для on-disk —
range scan with prefix grouping.

**Acceptance.**
- 10k tx commits → history растёт.
- min_alive=100 → GC удаляет всё < 100 кроме keep-latest.
- Concurrent open tx на snap=50 → GC не удаляет нужные ему версии.

## 6.2. Telemetry

Add Prometheus metrics:

```rust
// repo-scoped
metrics::register_gauge("shamir_tx_active_count", repo_id);
metrics::register_gauge("shamir_tx_max_age_ms", repo_id);
metrics::register_gauge("shamir_gc_lag_versions", repo_id);
metrics::register_counter("shamir_tx_committed_total", repo_id);
metrics::register_counter("shamir_tx_aborted_total", repo_id);
metrics::register_counter("shamir_tx_conflict_total", repo_id);
metrics::register_histogram("shamir_tx_duration_ms", repo_id);

// system-wide (cross-repo aggregations)
metrics::register_histogram("shamir_tx_versions_per_key");
```

**Where to bump:**
- `gate.open_snapshot` — `tx_active_count++`.
- `tx_drop / commit` — `tx_active_count--`, duration_ms observation.
- `tx commit ok` — `committed_total++`.
- `tx abort` — `aborted_total++`; `conflict_total++` если SSI conflict.
- `gc_below` — `gc_lag_versions = current_version - min_alive`.

**Тонкое место с `tx_max_age_ms`** — нужно отслеживать самый старый
open snapshot. Реализация: помимо `active_snapshots: HashMap<version,
()>`, хранить `active_snapshot_start_ns: HashMap<version, u64>`.
Обновляем в open_snapshot и open_snapshot_guard drop.

**Acceptance.**
- Метрики экспортируются через existing observability endpoint.
- Прометей-friendly формат через `/metrics`.
- Smoke test: после 100 tx counter `committed_total` = 100.

## 6.3. Max-tx-lifetime cap

**Проблема.** Один stuck client (например, Phase B interactive tx,
который не вернулся к commit/abort) держит `active_snapshots` ↦ GC
не может работать, history растёт неограниченно.

**Решение.** Config'urable максимум возраста tx. Background reaper:

```rust
pub struct TxReaper {
    repo_gate: Arc<RepoTxGate>,
    max_age: Duration,        // config, default 5 min
    period: Duration,         // check каждые 30s
}

impl TxReaper {
    async fn run_loop(&self) {
        loop {
            tokio::time::sleep(self.period).await;
            let now_ns = now_unix_ns();
            let max_age_ns = self.max_age.as_nanos() as u64;

            // Foreach active snapshot, check age:
            let to_force_abort: Vec<u64> = ...;

            for snap in to_force_abort {
                self.repo_gate.force_abort_snapshot(snap, "tx_max_age_exceeded").await;
            }
        }
    }
}
```

**Force abort effect.** `force_abort_snapshot` удаляет snapshot из
`active_snapshots`. Если потом владелец snapshot пытается commit →
gate возвращает ошибку. RAII rollback на drop как обычно.

Для Phase A (batch-only tx, не interactive) reaper редко срабатывает
— только если query внутри batch очень долго работает (например,
огромный full-scan). 5 min default — щедро.

Для Phase B (interactive) — reaper критичен.

**Acceptance.**
- Tx старше max_age force-aborted; reads после force-abort из этой tx
  возвращают `tx_aborted`.
- GC после force-abort может проceed.

## Порядок работы

1. `GcWorker::gc_below` + tests (1 день).
2. Keep-latest-version logic + tests (0.5 дня).
3. Telemetry metrics (0.5 дня).
4. `TxReaper` + force_abort (1 день).

**Не делаем здесь:**
- Не пишем e2e tests (Этап 7).
- Не делаем Phase B (interactive tx) — это отдельный sprint после
  всех 7 этапов.
