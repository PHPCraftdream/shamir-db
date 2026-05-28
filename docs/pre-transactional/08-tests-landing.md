# Этап 7. Tests + wire format + docs

**Срок:** 5-6 дней. **Зависит от:** Этап 0-6.

Цель — доказать через тесты что транзакции реально работают под
concurrent load, не только в happy path. Завершить wire format,
обновить документацию.

## 7.1. Multi-connection harness

**Текущее состояние.** E2E orchestrator поднимает **один** сервер
на весь прогон. Все тесты подключаются одним клиентом (один session).

**Нужно.** Helper в `tests/e2e/setup/` для spawn N клиентов с
**отдельными** session против того же сервера. Это значит:

```js
// tests/e2e/setup/multi-client.js
export async function spawnClients(n, opts) {
    const clients = [];
    for (let i = 0; i < n; i++) {
        const c = await ShamirClient.connect({
            host: opts.host,
            port: opts.port,
            tls: opts.tls,
            // отдельный session — handshake creates new session_id
        });
        clients.push(c);
    }
    return clients;
}

export async function cleanup(clients) {
    for (const c of clients) await c.close();
}
```

Каждый client имеет свой TLS-канал, свой session_id, свой
HMAC-state. Запросы независимы.

## 7.2. Конкурирующие сценарии — 10 штук

В `tests/e2e/tests/12-transactions-concurrent.test.js`:

### 1. Lost update (SI baseline)
```js
const [c1, c2] = await spawnClients(2);
// c1: tx { read X=10, sleep 50ms, write X=20 }
// c2: tx { read X=10, sleep 100ms, write X=30 }
await Promise.all([c1.execute(...), c2.execute(...)]);
// Expected (SI+LWW): final X = 30 (last writer wins)
assertEq(await c1.execute(read X), 30);
```

### 2. Lost update detected (SSI)
```js
// isolation: "serializable" на обеих
// Одна commits OK, другая возвращает tx_conflict
```

### 3. Phantom protection
```js
// c1: tx begin → SELECT count where age>=18 → N
// c2: INSERT new user (outside tx)
// c1: SELECT count where age>=18 → N (snapshot держит)
// c1: commit
// c1 (вне tx): SELECT → N+1
```

### 4. Write skew (doctor on-call)
```js
// SI: оба сегодня видят 2, оба себя выключают → 0 врачей on_call (lost)
// SSI: один абортится, retry видит 1 врача → не выключается → 1 врач on_call
```

### 5. Counter race
```js
// 100 parallel JS tx'ов: read counter, write counter+1
// SI+LWW: final < 100 (lost updates)
// SSI: aborts + retry → eventually final = 100
```

### 6. Read-after-write inside tx
```js
// c1: tx { write X=20, read X → 20 (видит свой write) }
// c2 (вне tx): read X → старое
// c1 commits
// c2: read X → 20
```

### 7. Snapshot isolation under parallel writes
```js
// c1: tx begin → snapshot
// c2: many independent writes
// c1: continues reading → видит snapshot
// c1 commit → reads outside reflect новое state
```

### 8. Crash mid-commit recovery (`13-transactions-recovery.test.js`)
```js
// c1.execute(tx) → kill server мid commit
// restart server
// либо ВСЕ writes из tx видны, либо НИ ОДНОГО (atomicity через
// repo WAL marker)
```

### 9. GC под активной tx
```js
// c1 begins long-running read-heavy tx
// c2 делает 10000 writes (большая history)
// GC должен НЕ удалить версии нужные c1
// c1 продолжает читать корректно
// c1 commit → GC чистит history
```

### 10. Cross-repo guard
```js
// tx batch с queries на 2 repo → error tx_cross_repo_not_supported
```

### 11. Cross-table within one repo
```js
// tx batch с queries на 2 tables в одном repo → atomicity сохраняется
// write в t1 + write в t2 + ошибка в 3-м query → ни t1, ни t2 не изменились
```

### 12. Migration during open tx
```js
// open tx на repo
// start_migration на тот же repo
// migration ждёт → tx commits → migration cutover проходит
```

## 7.3. Rust-уровень unit tests

`crates/shamir-engine/tests/transactions/`:

- `version_codec.rs` — `encode_version_key` round-trip + sort order
  preservation (key `a::v=5` < `a::v=10` < `b::v=1`).
- `mvcc_store.rs` — `get_at` under busy history (5 versions per key,
  random snapshot queries).
- `tx_gate.rs` — concurrent `assign_next_version` (no duplicates),
  recovery marker correctness, `active_snapshots` lifecycle.
- `gc.rs` — `gc_below` keeps latest-per-key, respects min_alive,
  doesn't pollute concurrent reads.
- `interner_overlay.rs` — `commit_interner_overlay` id remap under
  concurrent merge, idempotent on retry.
- `hnsw_staging.rs` — `commit_staged` / `rollback_staged` / in-tx
  search with merge.
- `repo_wal.rs` — V2 entry round-trip, recovery после симулированных
  crashes в разные moments.

Все запускаются в обычном `cargo test` sweep на in-memory backend
— быстрые.

## 7.4. Wire format finalization

`crates/shamir-query-types/src/batch/types.rs`:

```rust
#[derive(Serialize, Deserialize)]
pub struct BatchRequest {
    pub id: u64,
    pub queries: BTreeMap<String, Query>,
    #[serde(default)]
    pub transactional: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<IsolationLevelWire>,  // "snapshot" | "serializable"
}

#[derive(Serialize, Deserialize)]
pub struct BatchResponse {
    pub id: u64,
    pub results: BTreeMap<String, QueryResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction: Option<TxInfo>,
}

#[derive(Serialize, Deserialize)]
pub struct TxInfo {
    pub tx_id: u64,
    pub status: String,                  // "committed" | "aborted"
    pub reason: Option<String>,           // "tx_conflict" | etc.
    pub snapshot_version: u64,
    pub commit_version: Option<u64>,     // None если aborted
}
```

SDK update:
- **Rust client** — `BatchBuilder::transactional()` + `isolation()`.
- **Node client** — `client.execute(db, { transactional: true, isolation: "serializable", queries })`.

## 7.5. Docs

- `docs/roadmap/TRANSACTIONS.md` — пометить как **implemented**,
  ссылаться на этот pre-transactional folder.
- `docs/roadmap/TRANSACTIONS_IMPL.md` — пометить как **superseded
  by**: pre-transactional/{01-08}.
- `docs/LOGIC_FLOW.md` — обновить read/write pipeline диаграммы,
  добавить tx path.
- `README.md` корневой — capability list: «Transactions: Phase A
  (single-batch SI/SSI on any backend)».
- `AGENTS.md` — добавить раздел "Working with transactions" в
  Discipline rules.

## 7.6. Phase A landing checklist

Перед закрытием Phase A:

- [ ] 12 e2e scenarios — все зелёные на 3+ последовательных прогонах.
- [x] 7 Rust unit-test suites — зелёные. (1452+ tests, 0 failures)
- [x] Bench `tx_pipeline.rs` — non-tx single insert ~3.9µs stable.
- [x] Bench `tx_pipeline.rs` — tx commit throughput measured
  (single-record tx ~10µs, 100-record batch ~618µs on in-memory).
- [ ] Migration test: tx + migration интероперабельны.
- [ ] Backup test: backup во время tx ждёт idle.
- [x] Crash recovery test: 7.1.e end-to-end crash simulation.
- [x] Docs обновлены. (REVIEW.md, TRANSACTIONS.md, TRANSACTIONS_IMPL.md)
- [ ] Capability list в README обновлён.
- [ ] CHANGELOG.md (если есть) — entry "Phase A transactions".

## Порядок работы

1. Multi-client harness (0.5 дня).
2. 12 e2e scenarios (2 дня).
3. Rust unit tests (1 день).
4. Wire format + SDK updates (0.5 дня).
5. Crash recovery scenarios — тут самое страшное, идём осторожно
   (1 день).
6. Docs sweep (0.5 дня).
7. Phase A landing checklist run (0.5 дня).

## После Phase A

**Phase B (interactive multi-call transactions).** Отдельный sprint:
session-scoped server state, lease management, disconnect-on-abort,
`tx_id` echo в RequestEnvelope. Не браться до того как Phase A
выезжен в production хотя бы 2 недели.
