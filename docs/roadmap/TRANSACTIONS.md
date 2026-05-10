# Transactions — Design

Status: **planned, not yet implemented**. The `BatchRequest.transactional`
flag exists in the wire schema but the executor currently ignores it and
always returns `transaction: None`. This document is the contour for
turning that hollow promise into reality.

---

## Краткий ответ на главный вопрос

> *Как это работает на уровне записей, при том что параллельные потоки
> могут изменить часть наших данных пока транзакция в полёте? Нужны
> локи?*

**Локов на уровне записей мы не добавляем.** Их даёт сам storage
backend через MVCC — мы просто соблюдаем его правила.

### В трёх образах

**1. Снимок как стена.** Когда транзакция начинается, storage engine
«фотографирует» состояние БД на этот тик. Все мои чтения резолвятся
ИЗ снимка — даже если параллельный поток в этот момент пишет, его
запись идёт в **новую версию**, а моя видит **старую**. Данные не
«отменяются» — они *перекрываются* новой версией; старая остаётся
жить ровно столько, сколько нужно открытым читателям.

**2. Свой рабочий стол.** Мои writes не идут сразу в общую таблицу —
они копятся в приватном буфере (working set). Никто их не видит. На
`commit` буфер публикуется как новая версия. На `rollback` —
выбрасывается, никто и не узнал.

**3. Очередь у писателей.** redb (наш базовый MVCC бэкенд) —
single-writer per file. Если моя tx открыта, другой writer ждёт. Не
блокируется навсегда — просто сериализуется. Читатели в это время
гуляют по своим снимкам без блокировок.

### Конкретно на временной шкале

```
T1 (моя):     begin──read balance=1000──compute──write 800──commit
                  ↑                                          ↑
                  снимок v100                                публикует v101

T2 (другая):                       begin──read balance=?──...
                                     ↑
                                     если началась ПОСЛЕ commit'а T1 → видит 800
                                     если ДО → ждёт пока T1 закончит
```

Три гарантии:
- T1 никогда не видит T2 в полёте — её мир заморожен на v100.
- T2 не начинается посреди T1 — redb queue для писателей.
- Чистый читатель на v100 продолжает видеть v100 даже после commit'а
  v101 — старая версия живёт пока кто-то её держит.

### Почему не локи

Локи на отдельных записях — это:
- **сложно** (lock manager, deadlock detection, escalation, stale-lock
  cleanup)
- **медленно** (lock acquire = atomic op + потенциальное ожидание)
- **не нужно с MVCC** — конфликт разрешается на уровне «слота
  писателя» (одного на repo), а не каждой строки

Trade-off: при transactional batch на repo держится exclusive writer
slot. Это **грубее** чем построчная блокировка, но **радикально
проще**. Для embedded БД с разумной нагрузкой — правильный выбор.

### Что делать, если хочется параллелизма writers

Это и есть смысл design choice. Ответ — **разделять данные по
repo'sам**:

- `users` repo (низкая нагрузка на запись) — отдельный redb-файл
- `audit_log` repo — отдельный redb-файл
- `hot_orders` repo — отдельный redb-файл

Каждый repo = свой writer slot. Распределение `(table → repo)`
становится не косметикой, а **throughput-настройкой**.

Если нужен реально N-writer per logical table — это уже не embedded
БД, это PostgreSQL/FoundationDB. ShamirDB оптимизирует под «embedded,
простая семантика», не пытается быть и тем и другим одновременно.

---

Дальше идёт формальный design contract на английском.

## Why two phases

The value cleanly splits into two levels of ambition:

- **Phase A — single-batch transactions.** All operations inside one
  `client.execute(db, batch)` call run atomically. No server-side state
  between calls. Drop-in for the existing API.
- **Phase B — interactive (multi-call) transactions.** A client starts a
  transaction, sends a sequence of independent batches, then commits or
  aborts. Requires session-scoped server state and abort-on-disconnect
  plumbing.

Phase A is enough for ~80% of transactional needs (atomic multi-record
write, read-after-write within the same operation). Phase B is needed
only when a human is in the loop or when client logic spans multiple
network round-trips against the same consistent snapshot.

Phase A first. Phase B when there is concrete demand.

---

## How isolation actually works (the question this answers)

> *"How does this work at the record level when other threads can
> simultaneously modify part of our data? Do we need locks for
> transactions?"*

Short answer: **we don't add row-level locks.** We rely on what the
storage backend already gives us — **MVCC** for the engines that
support it (redb, persy, canopy), and we forbid transactional batches
on engines that don't (sled, fjall).

### MVCC in plain words

MVCC = Multi-Version Concurrency Control. The storage engine never
overwrites data in place — every write produces a new version, the old
versions stay for readers that already started.

When a transaction begins it captures a **snapshot** — a logical "the
database as it was at this exact tick". From then on:

1. **All reads inside the tx come from that snapshot.** Even if another
   process commits a new value while my tx is open, I keep seeing the
   old value. The "data" hasn't been invalidated — it has been
   *superseded* by a newer version, but my snapshot still resolves my
   reads correctly. The old version stays alive as long as any open
   reader needs it.
2. **My writes go into a private working set,** not into the shared
   table. Other transactions don't see them.
3. **At commit** the working set is published as a new version. From
   that moment on, new transactions starting up see *my* version.
4. **At abort** the working set is discarded and nobody else ever knew
   it existed.

So at the record level there are no locks — locks are at the
**writer-slot level** of the whole repo. Readers and writers never
block each other. Two writers serialize.

### Concrete example — single-writer MVCC (redb)

```
time →

T1 (mine):     begin─────read user.balance=1000──compute──write 800──commit
                  ↑                                                 ↑
                  └─ snapshot v100                                  └─ publishes v101

T2 (other):                          begin─────read user.balance=?──...
                                       ↑
                                       └─ if T2 begins after T1 commits → sees 800
                                          if T2 begins before T1 commits → must wait
                                          (redb is single-writer per file)
```

Three things are guaranteed here:

- T1 never sees T2's in-flight writes. T1's view is frozen at v100.
- T2 doesn't start *until* T1 finishes. redb serializes writers.
  This is coarser than row-locking but radically simpler — no deadlock
  detection, no lock escalation, no stale-lock cleanup.
- A pure reader started at v100 keeps seeing v100 for as long as it
  needs, even after T1 commits v101. Reads never block writers either.

### Multi-writer scenarios (not us, for context)

Postgres / MySQL allow N concurrent writers and detect conflicts at
commit time:

```
T1: read X=10, write X=20, commit  → succeeds
T2: read X=10, write X=30, commit  → CONFLICT, abort + retry
```

This needs read-set tracking, write-set diffing, and a lock manager.
ShamirDB doesn't go there — redb's single-writer model is enough for
our throughput targets and orders of magnitude simpler.

### "What if my reads and writes interleave with someone else's?"

Inside a tx, they can't. The snapshot is a wall. Writers serialize.

Outside a tx (today's behavior, no `transactional: true`), each
operation commits on its own and there is no atomicity across them —
that's exactly the problem this whole design solves.

### What if I want finer concurrency than "one writer per repo"?

That's the trade-off you accept by choosing transactional mode. Two
mitigations:

- **Split your data across repos.** A repo per high-contention table
  family. Each repo gets its own redb file, its own writer slot.
- **Keep transactions short.** Read what you need, write, commit. Don't
  hold a tx open across user interaction (that's why Phase B is risky
  and waits for clear demand).

If you genuinely need multi-writer per repo, the answer is to switch
to a backend with that property (Postgres, FoundationDB) — but then
you're running a different database. ShamirDB optimises for "embedded,
embedded-grade, simple semantics."

---

## Phase A — what gets built

### Boundary rules

1. **One repo per transactional batch.** If `transactional: true` and
   ops touch more than one repo, return `not_supported` error
   ("transactional batch must target a single repo"). Two-phase commit
   across heterogeneous backends is out of scope.
2. **Backend must support tx.** `redb`, `persy`, `canopy`, `nebari`
   (with care), and `in_memory` (via Mutex). `sled` and `fjall` reject
   with `not_supported_by_backend`. We don't fake atomicity.
3. **Serial execution inside the tx.** The planner's parallel stages
   are flattened — a single WriteTxn handle is rarely `Send`-safe
   across tasks. Reads and writes in the batch run one after another.
   Trade-off: slower than parallel batches; correct.
4. **Read-after-write inside the tx.** A `from` query later in the
   batch sees the writes earlier in the batch. This is "free" with
   MVCC backends — the WriteTxn naturally exposes its own pending
   writes to its own reads.
5. **Drop = rollback.** If the executor returns early for any reason
   (panic, I/O error, validation), the WriteTxn is dropped without
   commit and storage rolls back. Rust RAII does the work; we just
   don't forget to NOT call `commit()` on the error path.

### Storage trait extension

```rust
// crates/shamir-storage/src/types.rs

#[async_trait]
pub trait Repo: Send + Sync {
    // existing
    async fn store_get(&self, name: &str) -> DbResult<Arc<dyn Store>>;
    async fn store_delete(&self, name: &str) -> DbResult<bool>;
    async fn stores_list(&self) -> DbResult<Vec<String>>;

    // NEW — opt-in. Default: not supported.
    async fn begin_write_tx(&self) -> DbResult<Box<dyn WriteTx>> {
        Err(DbError::NotSupported(
            "transactions not supported by this backend".into(),
        ))
    }
}

#[async_trait]
pub trait WriteTx: Send {
    /// Get a tx-scoped write handle for one of the stores in this repo.
    /// All writes through this handle are buffered until `commit()`.
    async fn store(&mut self, name: &str) -> DbResult<&mut dyn Store>;

    /// Publish the working set. Consumes self — caller can't reuse.
    async fn commit(self: Box<Self>) -> DbResult<()>;

    /// Discard the working set explicitly. Same effect as Drop.
    async fn rollback(self: Box<Self>) -> DbResult<()>;
}
```

Backends with native MVCC implement `begin_write_tx` over their own
WriteTxn primitive. The default impl rejects, so unsupported backends
fail loudly the moment a client asks for `transactional: true`.

### Engine + executor integration

```rust
// pseudocode — crates/shamir-engine/src/query/batch/executor.rs

let tx = if request.transactional {
    let repo = unique_repo_targeted_by(&request)?; // errors if multi-repo
    Some(repo.begin_write_tx().await?)
} else {
    None
};

let outcome = if tx.is_some() {
    run_stages_serially(stages, tx.as_mut()).await
} else {
    run_stages_in_parallel(stages).await
};

let (committed, txn_id) = match (outcome, tx) {
    (Ok(_), Some(t)) => {
        t.commit().await?;
        (true, Some(random_u64()))
    }
    (Err(e), Some(t)) => {
        // Drop is enough but explicit rollback gives backends a hook
        // for diagnostics.
        let _ = t.rollback().await;
        return Err(e);
    }
    (Ok(_), None) => (false, None),
    (Err(e), None) => return Err(e),
};

response.transaction = txn_id.map(|id| TransactionInfo { id, committed });
```

### Test coverage to add

In `tests/e2e/tests/11-transactions.test.js`:

- happy path: 5 writes + 1 read in one tx, commit, read back from
  outside the tx, see all writes
- abort path: 2 writes + 1 deliberately-failing op, verify table is
  unchanged afterward
- single-repo rule: tx batch touching two repos → `not_supported`
- read-after-write: write inside tx then read inside same tx → sees
  the new value
- backend rejection: open tx batch against a `sled` repo →
  `not_supported_by_backend`
- isolation: open long-ish tx, in parallel run a regular write from
  another connection, observe it doesn't appear inside the tx but does
  appear after commit

Plus Rust integration tests in `crates/shamir-engine/tests/` for the
storage-level WriteTx contract.

---

## Phase B — interactive transactions (later)

Sketch only — committed to no API yet.

```
client.beginTx()                           → { txId }
client.execute(db, batch, { txId })        → BatchResponse
client.commitTx(txId)                      → ok
client.abortTx(txId)                       → ok
```

Server-side state per session:

```rust
struct Session {
    // existing fields...
    active_tx: Option<ActiveTx>,
}

struct ActiveTx {
    tx_id: u64,
    repo_path: (String /* db */, String /* repo */),
    write_tx: Box<dyn WriteTx>,
    expires_at: Instant,         // server-side TTL — auto-abort if client vanishes
    last_activity_ns: u64,
}
```

Hard parts:
- **Lease management.** TTL needs to be short enough to free writer
  slots quickly, long enough to survive a slow client. 30 s feels
  right; configurable.
- **Disconnect → abort.** TCP close handler must drop `active_tx` so
  the writer slot frees immediately, not after TTL.
- **Protocol echoes tx_id** in every `RequestEnvelope` belonging to a
  tx, so the server can route. Otherwise the dispatcher would have to
  consult session state on every request.
- **One tx per session** is the simplest rule — nested or parallel tx
  in the same session is a YAGNI hole.
- **Heartbeat.** If client wants to hold a tx longer than TTL, it
  pings the tx. Adds a wire op.

This is real work and earns it own sprint. Phase A first.

---

## Risks and corners

- **Interner persistence and tx commit order.** TableManager keeps a
  string-interner that lazy-persists to the `__info__` store. If the
  interner persists *outside* the tx but the tx then aborts, we leak
  intern slots. Fix: interner persist must ride the same WriteTxn, OR
  must be deferred until after commit. Probably the latter — the
  interner is monotonic so leaking a slot is harmless except for size.
- **Index updates.** IndexManager updates secondary indexes on every
  write. Inside a tx, those updates must go through the tx handle too —
  otherwise an aborted tx leaves orphan index entries. The
  IndexManager code path needs an audit.
- **redb WriteTxn is exclusive per file.** During a long-running
  transactional batch, no other write can touch that repo. Document.
  Makes "split repos by access pattern" advice not just style — it's
  a throughput knob.
- **Audit log writes.** Every batch emits an audit entry. Should the
  audit entry ride the same tx? Probably yes for transactional
  batches — otherwise we can audit a batch that ended up rolled back.
  But the audit chain is in a different store entirely; making it
  cross-store consistent is exactly the multi-repo problem we banned.
  Pragmatic compromise: audit entry is appended *after* successful
  commit, with `outcome: aborted` for failed transactional batches.

---

## Order of work (when we pick this up)

1. Storage trait extension + `WriteTx` shape (1 h)
2. redb backend `begin_write_tx` impl (3-4 h, including index store glue)
3. persy + canopy impls (2 h each)
4. in-memory impl (Mutex-based, 1 h)
5. Sled/Fjall reject path + `not_supported` error code (30 min)
6. Executor integration — single-repo check, serial exec, commit/rollback (2-3 h)
7. Interner-persist deferral fix (1 h)
8. IndexManager tx-aware writes (audit + fix, 2-3 h)
9. Rust tests + e2e test file (2 h)
10. Docs: this file marked "implemented", LOGIC_FLOW updated, root
    README capability list updated (30 min)

Total: roughly 1.5-2 days of focused work. Phase A only.
