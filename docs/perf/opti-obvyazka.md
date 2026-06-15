בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# /opti «обвязка» — снять универсальный per-record налог commit-пайплайна

**Стратегия (от пользователя):** мемори-движок тормозит → тормоза в **обвязке**
(commit-пайплайн), не в хранилище (DashMap = нс). Обвязка **общая для всех
backend'ов** (они платят её поверх своего I/O). Значит чинить её на мемори-движке
(где она единственная стоимость, без диск-шума) → **поднимаем пол для ВСЕХ**.

**Метрика:** `bulk_insert` in-memory (`crates/shamir-db/benches/engine_perf.rs`),
ns/строка. Базлайн на старте кампании: **~83µs/строка (current_thread) / ~90µs
(multi-thread бенч) = ~11K rec/s**. FLOOR (сырой `InMemoryStore.set`) = **2508
ns/строка** → пайплайн = **×33** над дном. Цель — приблизиться к дну.

**Протокол измерения (на каждый цикл):**
`CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-db --bench
engine_perf -- bulk_insert` (QUICK). Before (текущий HEAD) → реализация → After.
Привести ns/строка before/after + Δ%. Гейт: `fmt --check` + `clippy --all-targets
-D warnings` + `./scripts/test.sh @oracle` (+ батч/insert корректность). Коммит по
циклу с числами.

---

## Атрибуция (профиль, эфемерная инструментация, медиана, ns/строка)

| Фаза | ns/строка | % | Природа |
|---|---:|---:|---|
| WriteResult→QueryResult (+ build в write_exec) | ~24000 | ~28% | `return_flagged` строит ответ по строкам **дважды**, бенч выбрасывает |
| substitute_params + deep-eq `values==op.values` + owned_op.clone | ~15000 | ~18% | O(N) глубокое сравнение 1000 записей + 2 клона |
| JSON→InnerValue + интернинг полей | ~11000 | ~13% | layered-interner пересобирается на batch |
| wal_ops_from_tx (клон тел в WalOpV2 + re-parse RecordId) | ~8500 | ~10% | ещё одна копия всех тел |
| msgpack-encode тел (stage) | ~6000 | 7% | |
| overlay scc::TreeIndex + cell publish (per-row) | ~5600 | 7% | |
| прочее (authorize, clone resolver/admin, prelock) | ~10000 | 12% | |
| WAL-encode serde (870) + group-append (37) + counter/index (137) | ~1000 | ~1% | **durability-путь дёшев** |

**Нить:** тела клонируются/пересоздаются **4–5×** end-to-end. Durability-машинерия
(WAL/overlay/materialize) — НЕ горло (~12µs). Горло — **дублирующая работа** в
query/batch-слое.

Hot-spots (file:line):
- `crates/shamir-engine/src/query/batch/query_runner.rs` — `BatchOp::Insert` arm:
  `substitute_params`+deep-eq, `owned_op.clone()`, `write_result_to_query_result`.
- `crates/shamir-engine/src/table/write_exec.rs` — `execute_insert_tx` (resolve + result-map + интернер).
- `crates/shamir-engine/src/tx/commit.rs` — `wal_ops_from_tx` (per-body clone, re-parse RecordId).
- `crates/shamir-engine/src/tx/materialize.rs` — Phase 5a overlay/cell publish.

---

## Циклы (по убыванию value, низкорисковые первыми — это удаление мусора, не смена семантики)

| # | Множитель | Где | Потенциал |
|---|---|---|---|
| **C1** | Убрать **двойную сборку ответа** (return_flagged/result строит payload дважды) — уважая, что каждая return-mode реально отдаёт; не менять наблюдаемый вывод | query_runner + write_exec | **−20…24µs (~25%)** |
| **C2** | Убить **deep-eq `values==op.values`** → дешёвый флаг «есть ли `$param`»; снять 2 клона (owned_op.clone, substitute clone) на no-param пути | query_runner | **−13µs (~15%)** |
| **C3** | **Zero-copy WAL-ops** — строить `WalOpV2` из staging по `Bytes`-ручкам, не re-parse RecordId | commit::wal_ops_from_tx | **−4…6µs** |
| **C4** | **Амортизировать interner-load** на batch (1000 строк делят имена полей) | write_exec | **−4…6µs** |
| **C5** | Батч overlay/cell одним проходом | materialize 5a | **−2…3µs** |

C1 и C2 — один hot-spot (`query_runner.rs` BatchOp::Insert), делаются вместе.
Топ-3 стопкой → ~−40µs/строка → **~2–2.5× (11K → ~25K rec/s)**; FLOOR показывает
ещё **×10** запаса за этим.

**Инвариант корректности:** каждая return-mode (none / return_flagged /
return_result) отдаёт **байт-идентичный** наблюдаемый вывод до и после. @oracle +
батч/insert тесты зелёные на каждом цикле. Durability/SSI не трогаем.
