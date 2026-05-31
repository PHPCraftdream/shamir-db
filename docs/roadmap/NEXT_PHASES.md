בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Next Phases — Overview & Index

**Status:** living index, revision **2026-05-29**. This is the single
entry point to the post–Phase-A roadmap. It states where we stand, then
points at three deep-dive design docs — one per forward phase — and
recommends an order.

> The three sibling docs are normative *designs*, not commitments. Each
> is grounded in real `path:line` citations and labels every not-yet-built
> type/method as **PROPOSED**.

---

## Где мы стоим — по-русски

**Phase A закрыта и production-grade.** Однобатчевые транзакции работают
end-to-end: Snapshot Isolation + Serializable Snapshot Isolation, crash
recovery через WAL V2 (WAL-as-commit-point: версия публикуется как только
WAL-запись durable, материализация проекций — eager, при сбое откладывается
recovery и НИКОГДА не превращает закоммиченную tx в abort). Все
CRIT/HIGH/MED-векторы трёх audit-волн закрыты.

Здоровье кодовой базы на день ревизии:

| Метрика | Значение |
|---|---|
| Крейтов в workspace | **13** |
| Тест-функций в `src` | **~1561** |
| Pre-commit gate | `fmt` + `clippy --workspace --all-targets -D warnings` + `--workspace --lib` — **зелёный** |
| Маркеров незавершённости в lib-коде (`todo!`/`unimplemented!`/`FIXME`) | **0** |

Снимок состояния и честный список закрытых/открытых пунктов —
[`../pre-transactional/REVIEW.md`](../pre-transactional/REVIEW.md) §11.
Дизайн транзакционного слоя — [`TRANSACTIONS.md`](./TRANSACTIONS.md) +
[`TRANSACTIONS_IMPL.md`](./TRANSACTIONS_IMPL.md).

---

## Архитектурный лейтмотив (нить всех фаз)

> **Истина живёт в одном месте — в версионированном MVCC-store
> (`<key>::<version_be>`, поверх «тупого» KV-трейта `Store`/`Repo`,
> одинаково на любом backend'е). Всё производное — overlay над этой
> истиной: восстановимый и свободный от лока. WAL/recovery — гарант
> материализации.**

Каждая следующая фаза — выражение этого принципа:

* **Phase B** — интерактивная транзакция это просто **дольше живущий
  overlay** (`TxContext`), переживающий несколько round-trip'ов. Durability
  не меняется: пока нет `wal.begin` на commit — ничего не durable, краш
  посреди интерактивной tx это чистый abort.
* **Phase C** — predicate/range-locks это **in-memory conflict-detection
  state** поверх той же истины. Они НЕ durable и это правильно: локи живут
  только для in-flight tx, ни одна tx не переживает рестарт.
* **Phase A tails** — вердикт MED-A («оставить logical-WAL») это
  утверждение того же принципа: физический multi-store transact-примитив
  протёк бы идентичностью backend'а — ровно то, что
  [`TRANSACTIONS.md`](./TRANSACTIONS.md) отвергает. Истина одна, сведение —
  через WAL.

---

## Три форвард-фазы

### ① Phase B — Interactive (multi-call) transactions — ✅ DONE (2026-05-31)
📄 [`PHASE_B_INTERACTIVE_TX.md`](./PHASE_B_INTERACTIVE_TX.md)

`BEGIN → EXECUTE(handle, batch)* → COMMIT | ROLLBACK | timeout/disconnect`.
Живой `TxContext` переживает несколько запросов клиента (сегодня tx живёт
ровно один batch).

**Ключевые решения дока:**
- Новые top-level request-варианты `TxBegin/TxExecute/TxCommit/TxRollback`
  (а НЕ новые `BatchOp` — те table-keyed, а tx-глаголы session-lifecycle).
  Backward-compat по прецеденту serde-default'а `materialized`.
- Живая tx паркуется в server-side `scc::HashMap`-реестре по `tx_handle`,
  привязанном к `session_id`/`user_id` (не per-connection — dispatch
  пересекает `spawn_blocking` и видит только `&Session`). `SnapshotGuard`
  паркуется вместе с ctx (его Drop отпускает GC `min_alive`-пин).
- Executor переиспользует уже существующий `execute_plan_tx` (тянет
  `&mut TxContext` без commit'а) — новой машины исполнения не нужно.
- Честно названы натяжения: рост staging (нужен `tx_too_large`-cap),
  долгий read-set против version-GC, abort-on-disconnect, idle-timeout vs
  `DEFAULT_MAX_TX_LIFETIME`, и **запрет** держать unique-write-локи между
  round-trip'ами (Phase A уже откладывает их до commit-time re-validation —
  это и спасает). SI-first, потом SSI; phantom-защита — в Phase C.

### ② Phase C — Predicate/range locks → true serializability
📄 [`PHASE_C_SERIALIZABLE.md`](./PHASE_C_SERIALIZABLE.md)

Сегодняшний SSI валидирует **точечный** read-set (ловит write-skew по
прочитанным ключам), но слеп к **phantom**: диапазонное чтение
(`WHERE age > 30`) не видит конкурентно вставленную подходящую строку.

**Ключевые решения дока:**
- Модель лока: **index-range locks (SIREAD над интервалами ключей)** как
  основной механизм, **table-granularity** как безопасный fallback. Это
  единственная модель, ложащаяся на MVCC-above-dumb-KV (истинные predicate
  locks слишком дороги; next-key locking требует блокирующих row-локов,
  которых в `Store`-трейте нет намеренно).
- Точечный `read_set` дополняется параллельным **PROPOSED `predicate_set`**
  (`IndexRange{lo,hi}` точно / `TableScan` грубо), захватываемым на тех же
  `*_tx`-хуках, что уже пишут точечные чтения (напр. `lookup_range_tx`,
  сегодня игнорирующий `_tx`).
- На commit — **Phase 2-bis** в `pre_commit`: пересечение read-диапазонов с
  write-ключами конкурентно закоммиченных tx (footprint берётся даром из
  `tx.index_write_set`), всё под существующим `commit_lock`.
- Zero-overhead на Snapshot/non-tx пути; предикат-трекинг только при
  `Serializable`. Честно: грубые предикаты over-abort'ят.

### ③ Phase A tails — honest hardening
📄 [`PHASE_A_TAILS.md`](./PHASE_A_TAILS.md)

Три полировочных хвоста (не блокеры — фундамент функционально готов):

- **MED-A (cross-table физическая атомарность)** — **вердикт: оставить
  logical-WAL.** Док строго аргументирует из кода: `Store::transact`
  single-keyspace; redb/persy *могли бы* span-keyspaces, sled — нет
  (`sled::Batch` tree-scoped) ⇒ физический примитив протёк бы backend'ом и
  создал ложную иллюзию атомарности (HNSW-promote намеренно вне
  `commit_lock`). WAL-as-commit-point + идемпотентный `recover_inflight_v2`
  уже дают контракт «всё видно вместе ИЛИ всё переиграно на старте». Два
  дешёвых выигрыша: зафиксировать инвариант в доках + один redb-reopen
  cross-table тест на существующем subprocess-harness. ~2.5 ч, без новых
  примитивов.
- **CI breadth** — бо́льшая часть уже закрыта (`55adef0`); остаётся мелкая
  delta (зафиксировать `--all-targets` + integration-job на каждый push).
- **Property/fuzz** — ⚠️ **ТРЕБУЕТ САНКЦИИ НА DEV-DEPS.** `proptest`
  (обязателен, для property-тестов SSI-interleavings и round-trip/sort-order
  version-codec) и опционально `arbitrary`+`cargo-fuzz` (отдельный `fuzz/`
  крейт вне workspace). **Ничего не добавляется в `Cargo.toml` до явного
  разрешения сопровождающего.**

---

## Рекомендованный порядок

```
   ③ A-tails (дёшево, без deps)        ① Phase B (headline)        ② Phase C
   ─────────────────────────────  →   ──────────────────────  →   ──────────────
   MED-A docs-fix + redb x-table       BEGIN/EXECUTE/COMMIT/        index-range
   test (~2.5h, 0 deps).               ROLLBACK lifecycle,          SIREAD locks
   proptest SSI props — КАК            session-parked TxContext,    поверх B's
   ТОЛЬКО санкционируют dep.           SI-first → SSI.              read-set.
```

**Почему такой порядок:**
1. **A-tails сначала** — часть бесплатна (без deps) и страхует фундамент.
   property-тесты SSI особенно усиливают будущую работу Phase C — но ждут
   санкции на `proptest`.
2. **Phase B — следующий крупный шаг.** На ~80% опирается на готовое
   (MVCC/WAL/staging/SSI/`execute_plan_tx`), даёт самую востребованную
   возможность (read-modify-write через round-trip'ы), и это и есть
   открытая задача #17.
3. **Phase C — после B.** Phase B удлиняет окно, которое predicate-локи
   должны переживать; проектировать C, зная lifetime-модель B, — правильнее.

---

## Beyond (не binding — существующие доки)

За горизонтом трёх фаз — отдельные планы, уже лежащие в `docs/roadmap/`:

| Тема | Док |
|---|---|
| Production server (listeners, RBAC, ops) | [`PRODUCTION_SERVER_PLAN.md`](./PRODUCTION_SERVER_PLAN.md) |
| Production hardening | [`PRODUCTION_HARDENING_ROADMAP.md`](./PRODUCTION_HARDENING_ROADMAP.md) |
| Browser WASM client (Argon2id в Web Worker) | [`BROWSER_WASM_PLAN.md`](./BROWSER_WASM_PLAN.md) |
| Vectors / embeddings | [`EMBEDDINGS_AND_VECTORS.md`](./EMBEDDINGS_AND_VECTORS.md) |
| Full-text search | [`FULL_TEXT_SEARCH.md`](./FULL_TEXT_SEARCH.md) |
| Perf opportunities | [`PERF_OPPORTUNITIES.md`](./PERF_OPPORTUNITIES.md) |
| Auth v1.1+ / транспорты / PQ-identity | [`ROADMAP.md`](./ROADMAP.md) |

---

## Index — все ссылки в одном месте

- 📄 [`PHASE_B_INTERACTIVE_TX.md`](./PHASE_B_INTERACTIVE_TX.md) — interactive multi-call tx
- 📄 [`PHASE_C_SERIALIZABLE.md`](./PHASE_C_SERIALIZABLE.md) — predicate/range locks, phantom protection
- 📄 [`PHASE_A_TAILS.md`](./PHASE_A_TAILS.md) — MED-A verdict, CI breadth, property/fuzz (dep-sanction)
- 📄 [`TRANSACTIONS.md`](./TRANSACTIONS.md) / [`TRANSACTIONS_IMPL.md`](./TRANSACTIONS_IMPL.md) — Phase A design
- 📄 [`../pre-transactional/REVIEW.md`](../pre-transactional/REVIEW.md) — state snapshot, §11 honest open items

---

_Index revision 2026-05-29 — после волны C6/D12/multi-table-honesty (`00f3841`..`97e4d60`). Обновляется по мере landing'а фаз._
