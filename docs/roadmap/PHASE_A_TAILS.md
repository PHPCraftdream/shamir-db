בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase A — Honest Tails (hardening before Phase B/C)

**Status:** _planned polish, NOT blockers._ Phase A (single-batch SI/SSI
ACID + WAL crash-recovery) is **functionally DONE** and shipped green:
`fmt` + `clippy --workspace --all-targets -D warnings` + `--workspace
--lib` (1561 test fns across 13 crates). The three items below are
**confidence / robustness tails** — they raise the *evidence* behind an
already-correct foundation, they do not add a missing feature. None gate
Phase B (interactive tx) or Phase C (full serializable). Read this doc as
"the last 5% of Phase A polish", planned honestly so the next engineer
doesn't re-litigate what's already settled.

Архитектурный лейтмотив, который держим во всех трёх секциях:

> **Truth in one place** — версионированный MVCC-store (`main` + history).
> Всё производное (index postings в `info`, HNSW-граф) — это
> **recoverable, lock-free overlay**. **WAL/recovery — гарант
> материализации**: durable WAL-entry в Phase 4 _и есть_ commit, а
> `main`/`info`/HNSW — его eager-applied проекции, которые сходятся через
> `recover_inflight_v2` на следующем open.

Этот лейтмотив — не лозунг: он буквально закодирован в
`crates/shamir-engine/src/tx/commit.rs:8-41` (doc на
`MaterializationState`) и `crates/shamir-engine/src/tx/recovery.rs:1-7`.
Он же — главный аргумент секции MED-A ниже.

---

## 0. Три хвоста и вердикт в одну строку

| # | Хвост | Вердикт (кратко) | Леверидж |
|---|---|---|---|
| **MED-A** | Cross-table **физическая** атомарность Phase 5 | **Оставить logical-WAL — это ПРАВИЛЬНЫЙ backend-agnostic дизайн.** Не строить multi-store transact-примитив (он протёк бы backend identity). Дёшево улучшить: один документированный инвариант + один honest multi-table reopen-тест на реальном диске. | низкий код / высокий ROI на уверенность |
| **CI** | Ширина CI-гейта | Базовый delta уже **закрыт** (IV.1 `55adef0` — `--test '*'` job). Остаётся: явно зафиксировать `--all-targets`-семантику в `clippy`-job (она уже там) и оформить miri/coverage как PROPOSALS, не per-PR. | низкий |
| **Property/fuzz** | Property + fuzz для SSI и version-codec | Ценно, но **требует новых dev-deps** (`proptest`, `arbitrary`, опц. `cargo-fuzz`). **САНКЦИЯ МЕЙНТЕЙНЕРА ОБЯЗАТЕЛЬНА** — правило проекта запрещает добавлять зависимости без явной просьбы. До санкции — только spec, ноль `Cargo.toml`. | высокий (на качество Phase C) |

Рекомендованный порядок — в §5 (Prioritization). Spoiler: **property/fuzz
первым** (как только дадут санкцию на deps), потому что он напрямую
укрепляет SSI-логику, на которой стоит весь Phase C.

---

## 1. MED-A — cross-table physical atomicity

### Мотивация (по-русски)

Транзакция в рамках одного repo может трогать N таблиц. Контракт
ShamirDB — all-or-nothing видимость (`docs/pre-transactional/REVIEW.md:31`,
"Cross-table internal — tx within one repo may touch N tables; all-or-
nothing visibility"). Вопрос хвоста MED-A: эта атомарность сегодня
**логическая** (один WAL-marker + replay на recovery), а не **физическая**
(один fsync, накрывающий все таблицы разом). Стоит ли строить multi-store
transact-примитив?

### Current state (grounded in code)

Phase 5a материализует данные **по таблице**. Оркестратор `materialize`
крутит per-table цикл —
`crates/shamir-engine/src/tx/commit.rs:799-825` (Phase 5a loop над
`collect_data_batches`), — каждая итерация зовёт `apply_data_batch`
(`crates/shamir-engine/src/tx/commit.rs:1031-1065`), который роутит либо
через `MvccStore::apply_committed_ops`
(`commit.rs:1062`), либо fallback `base.transact(ops)`
(`commit.rs:1063`). То есть N таблиц → N независимых физических
`transact`/`apply_committed_ops`. Точно так же Phase 5c (index) идёт
per-`table_token` — `commit.rs:880-899`.

Атомарность держится на **WAL-as-commit-point** контракте, который уже
landed (commit `a333a91`, "WAL is the commit point — Phase 5 is
idempotent materialization, no abort after Phase 4"). Конкретно:

- Phase 4 `wal.begin` — единственная durable точка коммита
  (`crates/shamir-engine/src/tx/commit.rs:732-746`). После неё **нет
  abort'а — только материализация** (`commit.rs:416-435`).
- `materialize` **никогда не возвращает Err**: отказ одной проекции
  логируется, ставит `ok = false`, и WAL-marker остаётся inflight
  (`commit.rs:975-997`), а версия всё равно публикуется через
  `publish_committed` (`commit.rs:925-928`). Исход — `Ok(TxOutcome {
  materialization: Deferred })`.
- `recover_inflight_v2` — гарант сходимости: сортирует inflight-entries по
  `commit_version` (`crates/shamir-engine/src/tx/recovery.rs:235-247`),
  replay'ит каждую (Put/IndexPut — last-write-wins; Delete/IndexDel —
  ignore-NotFound, `recovery.rs:68-72`, `147-150`; CounterDelta —
  намеренно SKIPPED, `recovery.rs:74-100`), и пере-персистит floor
  (`recovery.rs:249-256`).

**Честность уже в коде** (не пряталось): doc на `MaterializationState`
(`commit.rs:23-58`) прямым текстом признаёт, что multi-table deferral
**ПАРТИАЛЬНЫЙ** — таблица A может материализоваться inline, B — нет, и
опубликованная версия тогда **cross-table-inconsistent** до следующего
`recover_v2_inflight`. Это «restart-bounded eventually consistent, NOT
immediately consistent». Контракт `materialize` повторяет это в
`commit.rs:777-787`. Тест на партиал —
`multi_table_partial_deferral_is_reconciled_by_recovery` (commit
`00f3841`).

### Существует ли multi-store transact-примитив? (НЕТ — подтверждено)

`Store::transact(&self, ops: Vec<KvOp>)` —
`crates/shamir-storage/src/types.rs:351`. Receiver — **`&self`**, т.е.
**один** `Store` = **одно** keyspace. Дефолтный impl не атомарный
(`types.rs:351-363`), disk-backends переопределяют для настоящей
атомарности **внутри одного store** (`types.rs:336-350` — atomicity
contract). Trait `Repo` (`types.rs:515-526`) даёт только
`store_get/store_delete/stores_list` — **никакого `transact_multi`**.
Вывод: примитива, атомарно накрывающего несколько stores, **нет**, и это
осознанно.

### Могут ли backends это вообще? (матрица — честно)

Ключевой факт: у redb-репо **все stores делят один `Arc<Database>`**, а
различаются `table_name` —
`crates/shamir-storage/src/storage_redb.rs:55-57` (`RedbStore { db,
table_name }`), `storage_redb.rs:84-97` (`store_get` создаёт
`RedbStore { db, table_name }` поверх общего `db`). Из этого вытекает
матрица:

| Backend | Может ли один физический tx накрыть несколько keyspaces? | Доказательство |
|---|---|---|
| **redb** | **Да в принципе** — `write_txn.open_table(name)` можно вызвать для нескольких таблиц в одном `write_txn` и закоммитить разом. Но текущий `transact` скоупится на ОДНУ `table_name`. | `storage_redb.rs:587-617` (один `table_name` на `write_txn`) |
| **persy** | **Да, и уже делает** — один `Tx` спан'ит `table_name` И `index_name` внутри одного store. | `storage_persy.rs:340-373` (один `db.begin()` над двумя keyspaces) |
| **nebari / canopy** | Транзакционные движки — потенциально да (общий handle). | `storage_nebari.rs:360`, `storage_canopy.rs:322` (per-store `transact`) |
| **fjall** | Per-store `transact`. | `storage_fjall.rs:237` |
| **sled** | **НЕТ через `Batch`** — `sled::Batch` применяется к ОДНОМУ `tree`; tree-scoped, спан нескольких trees атомарно через Batch невозможен. | `storage_sled.rs:428-446` |
| **in-memory** | Нет понятия транзакции (дефолтный per-op loop). | `storage_in_memory.rs:351` (наследует дефолт) |
| **MemBuffer / Cached** | Обёртки — `transact` проксирует во внутренний store, не накрывает несколько. | `storage_membuffer.rs:578`, `storage_cached.rs:294` |

### Честный вердикт: ОСТАВИТЬ logical-WAL. Аргумент строго

Соблазн — добавить `Repo::transact_multi(Vec<(store_name, Vec<KvOp>)>)`,
который redb/persy реализуют атомарно. **Отвергаем по четырём причинам,
каждая привязана к коду:**

1. **Это протекло бы backend identity — ровно то, что отвергает
   `TRANSACTIONS.md`.** Примитив был бы **атомарным только на redb / persy
   / nebari / canopy** и **деградировал бы до per-store loop на sled /
   in-memory** (sled физически не может — `storage_sled.rs:428-446`).
   Контракт `transact`, который движок видит, стал бы зависеть от того,
   _какой backend под ним_ — а весь смысл `Store`/`Repo`-абстракции в том,
   что движок их не различает. Дефолт-impl `transact_multi` всё равно был
   бы НЕ-атомарным, и `materialize` обязан был бы _всё равно_ держать
   fallback на logical-WAL для backend'ов без поддержки. То есть мы бы
   получили **две** кодовые дорожки вместо одной, а более слабая
   (logical-WAL) всё равно осталась бы обязательной.

2. **Logical-WAL уже даёт ровно ту атомарность, которую обещает
   контракт.** Обещание — не «один fsync», а «all visible together OR all
   re-applied on next open» (`REVIEW.md:443-445`). Это в точности то, что
   реализует пара `materialize` (никогда не abort после Phase 4,
   `commit.rs:416-435`) + `recover_inflight_v2` (идемпотентный replay,
   `recovery.rs:229-267`). WAL — **единственный унифицированный лог** над
   физически раздельными stores `main`/`info` (это прямо сказано в commit
   `a333a91`: "main/info are separate physical stores → cross-store
   atomicity is impossible by reorder; the WAL is the only unified log").

3. **Один fsync через multi-store transact НЕ устранил бы partial-
   visibility окно — он бы его сузил, не закрыл.** Даже атомарный
   redb-`write_txn`, накрывающий все per-table таблицы, оставил бы открытым
   зазор между `publish_committed` (in-memory, `commit.rs:925-928`) и
   физическим commit'ом, плюс HNSW-promote вынесен _за_ `commit_lock`
   намеренно (III.5, `commit.rs:442-460`) и НЕ участвует в WAL-replay
   (вектора не сериализуются как `IndexPut` — `commit.rs:487-497`). То
   есть «единый физический fsync для всей tx» — недостижимая цель в
   принципе, пока HNSW — derived rebuild-on-open проекция. Logical-WAL
   честно это моделирует; физический примитив создал бы _иллюзию_ полной
   атомарности, которой на самом деле нет.

4. **Стоимость интеграции непропорциональна.** `transact_multi` потребовал
   бы: расширения `Repo`-трейта (7+ backends), переписывания Phase 5a/5c
   из per-table цикла в «собрать все ops по всем stores → один вызов»,
   per-backend атомарных impl'ов с честным fallback, и нового набора
   тестов «атомарно на redb, logically на sled». Это **дни** работы ради
   сужения окна, которое recovery и так закрывает на следующем open — при
   нулевом выигрыше для sled/in-memory.

> **Вывод MED-A:** logical-WAL — не компромисс, а **корректный backend-
> agnostic ответ**. Физический multi-store transact-примитив строить
> **не нужно и вредно** (протечка backend identity, дублирование дорожек,
> ложная иллюзия атомарности). Закрываем MED-A как **WONTFIX-by-design** с
> двумя дешёвыми улучшениями ниже.

### Concrete proposed work (дёшево, без новых примитивов)

1. **Документированный инвариант (docs-only).** Поднять honest doc-comment
   из `commit.rs:23-58` / `commit.rs:777-787` в явный раздел этого файла
   и в `docs/pre-transactional/05-executor-isolation.md`
   ("Known Production Limitations") как _намеренное design-решение_, а не
   "follow-up". Формулировка-инвариант (PROPOSED wording):
   > _Cross-table atomicity in ShamirDB is **logical**, guaranteed by the
   > single per-tx WAL entry and idempotent recovery replay, NOT by a
   > single physical multi-store fsync. Backend-agnosticism forbids a
   > physical multi-store transact: it would be atomic only on
   > transactional backends (redb/persy/nebari/canopy) and silently
   > degrade on tree-scoped ones (sled), leaking backend identity into the
   > engine contract. The invariant is "all visible together OR all
   > re-applied on next open"._

2. **Один сильнее recovery-тест (test-only, без новых deps).** Сейчас
   партиал-deferral покрыт **in-process** через инъекции
   `FAIL_PHASE_5A_TX_ID` + `FAIL_PHASE_5A_TABLE_TOKEN`
   (`crates/shamir-engine/src/tx/commit.rs:201-208`,
   `commit.rs:1045-1055`) — и это хорошо. Дополнить **реальным
   redb-on-disk reopen**-сценарием поверх уже существующего subprocess-
   харнеса (`crates/shamir-engine/tests/crash_recovery.rs`, landed II.1
   `783a7bf`): tx пишет в **две** таблицы, краш на seam `phase5a` (после
   data таблицы A, до таблицы B), reopen → `recover_inflight_v2` →
   ассерт, что **обе** таблицы сошлись на одной `commit_version`. Это
   делает «restart-bounded eventual consistency» из doc'а
   **исполняемым доказательством на реальном fsync'е**, а не только
   in-process инъекцией. PROPOSED test name:
   `cross_table_partial_then_reopen_reconciles_both_tables` в
   `crash_recovery.rs`.

3. **(Опционально, отдельный chore)** В `apply_data_batch`
   (`commit.rs:1031`) на redb-стеке per-table `apply_committed_ops` уже
   эффективен (`Durability::None` per write → амортизированный fsync,
   `storage_redb.rs:594-597`). Если профиль когда-нибудь покажет N×fsync
   как горячку, корректный ответ — **батчить флаш**, а не атомарность:
   один `flush()` на общий redb-handle покрывает все pending-таблицы
   (это уже использует crash-seam, `commit.rs:141-149`). Это perf-, не
   correctness-задача — выносится в `PERF_OPPORTUNITIES.md`, не сюда.

### Effort estimate

| Под-задача | Оценка |
|---|---|
| Docs-only инвариант (этот файл + `05-executor-isolation.md`) | ~0.5 ч |
| Реальный redb-reopen cross-table тест в `crash_recovery.rs` | ~2 ч |
| **Итого MED-A** | **~2.5 ч** (ноль prod-кода, ноль новых deps) |

### Risks

- **Низкий.** Тест опирается на уже-существующий subprocess-харнес и
  крэш-seam'ы (`commit.rs:131-152`), которые уже зелёные. Главный риск —
  flakiness реального fsync'а на CI; митигируется тем же шаблоном
  изоляции, что и остальной харнес (`tempfile::TempDir`,
  абнормал-death assert уже кроссплатформенный — Win MSVC / Unix, см.
  `783a7bf`).
- **Анти-риск:** *не* строить примитив — это и есть снижение риска. Любой
  physical multi-store transact ввёл бы новую дорожку с собственным
  набором edge-cases поверх и так корректного logical-WAL.

---

## 2. CI breadth

### Мотивация (по-русски)

Pre-commit gate в `CLAUDE.md` требует `clippy --workspace --all-targets`
и прогон тестов. Вопрос хвоста: **enforce'ит ли CI ту же ширину на каждом
push**, или часть гейта живёт только в локальной дисциплине?

### Current state (grounded in config) — БОЛЬШАЯ ЧАСТЬ УЖЕ ЗАКРЫТА

`.github/workflows/ci.yml` сейчас содержит **четыре** job'а:

- `fmt` — `cargo fmt --all -- --check` (`.github/workflows/ci.yml:14-22`).
- `clippy` — **`cargo clippy --workspace --all-targets -- -D warnings`**
  (`.github/workflows/ci.yml:24-33`). **`--all-targets` уже здесь.**
- `test` — `cargo test --workspace --lib` (`.github/workflows/ci.yml:40-47`).
- `integration` — **`cargo test --workspace --test '*'`**
  (`.github/workflows/ci.yml:82-89`). Это закрыло IV.1 (commit
  `55adef0`, "gate integration test targets, not just --lib"): до него
  36 integration-файлов под `crates/*/tests/` только **компилировались**
  (под clippy), но **не запускались** на PR.

То есть **исходная formulировка хвоста ("CI does not enforce --all-targets
breadth nor an integration-test job") уже устарела**: оба гейта на месте.
Это нужно зафиксировать в `REVIEW.md` (см. §4 этого дока — `55adef0`
закрыл §11-пункт "CI — `--all-targets` clippy" в части интеграции).

### Что реально остаётся (delta — мелочь)

1. **Зафиксировать намеренный выбор `--test '*'` vs `--all-targets` в
   integration-job.** Это уже сделано как развёрнутый комментарий
   (`.github/workflows/ci.yml:58-81`) — объясняет, почему НЕ
   `--all-targets` в test-job'е: bench-таргеты имеют `criterion_main` →
   `--all-targets` _запустил_ бы бенчи (медленно, бессмысленно как
   correctness-гейт). **Действие: ничего менять не нужно**, только
   сослаться на это решение в `REVIEW.md`, чтобы следующий инженер не
   «починил» его обратно.

2. **(PROPOSED, НЕ per-PR) Scheduled/nightly job для тяжёлого.** JS-e2e
   под `tests/e2e/` намеренно НЕ в per-PR гейте (нужен `npm install` +
   release `shamir-server` + MSVC-only napi binding —
   `.github/workflows/ci.yml:76-81`). Предложение — отдельный
   `schedule:`-workflow (nightly), а не нагружать каждый push. Это
   **proposal**, не обязательство.

3. **(PROPOSED) `cargo test --doc`.** Сейчас doctests не гоняются явно
   (job `test` — `--lib`). Если doc-примеры станут исполняемыми, добавить
   `--doc`. Сейчас низкий приоритет.

4. **(PROPOSED, осторожно) miri / coverage.** miri не подходит «в лоб»:
   в дереве есть `unsafe set_len`, за которым идёт async I/O, который miri
   не может прогнать (зафиксировано в `55adef0`: "miri (the one unsafe
   set_len is followed by async I/O miri can't drive)"). Coverage
   (`cargo-llvm-cov`/`tarpaulin`) — приятно для метрики, но это **новая
   tool-зависимость в CI** и НЕ должна блокировать PR. Оба — **proposals**,
   явно НЕ per-PR, явно требуют отдельного решения.

### Concrete proposed work

| Задача | Тип | Действие |
|---|---|---|
| Отметить IV.1 как DONE в `REVIEW.md` §11 | docs | `55adef0` закрыл integration-часть |
| Закрепить «`--test '*'` by design» в `REVIEW.md` | docs | ссылка на `ci.yml:58-81` |
| Nightly JS-e2e workflow | PROPOSAL | отдельный `.github/workflows/nightly.yml` (не трогаем сейчас) |
| `--doc` job | PROPOSAL | низкий приоритет |
| miri / coverage | PROPOSAL | НЕ per-PR; требует отдельного решения и (для coverage) tool-deps |

### Effort estimate

- Docs-фиксация текущего состояния: **~0.5 ч**.
- Каждый PROPOSAL (если санкционируют): nightly ~1 ч, `--doc` ~0.3 ч,
  coverage ~2 ч (с настройкой). **Не входят в базовый хвост.**

### Risks

- **Очень низкий.** Базовый хвост — почти полностью docs (зафиксировать,
  что уже сделано). Реальный риск только у PROPOSAL'ов: nightly e2e может
  быть flaky (сеть/сборка), coverage добавляет CI-tool-dep (что само по
  себе подпадает под политику «без новых deps без санкции», даже если это
  CI-only).

---

## 3. Property / fuzz coverage  ⚠️ ТРЕБУЕТ САНКЦИИ НА DEV-DEPS

### Мотивация (по-русски)

Две самые тонкие, инвариант-несущие части tx-слоя — **SSI conflict
detection** и **version-codec** — покрыты сегодня **только example-based**
тестами. Example-тесты ловят регрессии на _конкретных_ входах; они не
ловят _интерливинги_, которых автор не придумал, и _adversarial_ входы
(codec). Property-тесты (генеративные инварианты) и fuzz-таргет (codec
round-trip + sort-order + документированный 0xFF-инвариант) подняли бы
уверенность с «проверено на примерах» до «проверено на тысячах
сгенерированных случаев». Это **самый высоколевериджный** хвост, потому
что SSI-логика — фундамент Phase C.

### ⚠️⚠️ DEPENDENCY SANCTION REQUIRED — громко

Правило проекта (`CLAUDE.md`, "Критические запреты"): **никогда не
поднимать/добавлять зависимости без явной просьбы мейнтейнера.** Property/
fuzz-работа **физически требует новых dev-deps**. Поэтому весь этот раздел
— **PROPOSAL**. До явной санкции: **ноль изменений в любом `Cargo.toml`,
ноль `fuzz/`-директорий, ноль `#[cfg(...)] proptest!`**. Только спека ниже.

**Точный список dev-deps на санкцию:**

| Crate | Тип | Куда | Зачем | Обязателен? |
|---|---|---|---|---|
| `proptest` | `[dev-dependencies]` | `crates/shamir-tx` | Генеративные property-тесты для `validate_read_set` (SSI) и round-trip version-codec. | **Да** — ядро предложения. |
| `arbitrary` | `[dev-dependencies]` (+ feature `derive`) | `crates/shamir-tx` | `#[derive(Arbitrary)]` для структурированных входов fuzz-таргета (key-байты + версии). | Нужен только если делаем cargo-fuzz таргет; для чистого `proptest` — нет. |
| `cargo-fuzz` (`libfuzzer-sys`) | отдельный `fuzz/`-crate (вне workspace `members`) | новый `crates/shamir-tx/fuzz/` или top-level `fuzz/` | Coverage-guided fuzzing version-codec. Требует nightly toolchain. | **Опционально** — `proptest` уже даёт 80% выгоды без nightly. |

**Рекомендация по объёму санкции:** минимально достаточно **только
`proptest`** (stable toolchain, ноль nightly, ноль отдельного crate'а).
`arbitrary` + `cargo-fuzz` — второй, опциональный шаг, если захочется
coverage-guided fuzzing именно codec'а. Мейнтейнер может санкционировать
**только `proptest`** и этого хватит для обоих целевых модулей.

> Существующее состояние deps подтверждено: ни `proptest`, ни
> `quickcheck`, ни `arbitrary`, ни `cargo-fuzz`/`libfuzzer` **нигде** в
> дереве нет (grep по всем `Cargo.toml` — пусто; `fuzz/`-директорий нет).
> `serial_test` уже dev-dep (используется в CI-комментарии,
> `.github/workflows/ci.yml:73-74`) — но это про сериализацию тестов, не
> про property/fuzz.

### Target #1 — SSI conflict detection (property tests)

**Что тестируем (grounded):** `TxContext::validate_read_set`
(`crates/shamir-tx/src/tx_context.rs:250-276`) и его парный writer
`record_read_shared` (`tx_context.rs:226-237`). Сейчас покрыто example-
тестами: `validate_read_set_passes_when_versions_unchanged`
(`tx_context.rs:472`), `validate_read_set_detects_advance`
(`tx_context.rs:492`), `validate_read_set_empty_passes`
(`tx_context.rs:511`), `validate_read_set_unknown_table_returns_conflict`
(`tx_context.rs:604`), `record_read_only_for_serializable`
(`tx_context.rs:379`). Хорошая база — но это _точечные_ входы.

**Инварианты-кандидаты на property (PROPOSED):**

1. **No false negative (soundness).** Для любого набора прочитанных ключей
   с записанными версиями `v_seen` и любого `version_provider`, где **хотя
   бы один** ключ имеет `current > v_seen`, `validate_read_set` ОБЯЗАН
   вернуть `Err`. Это прямой инвариант ветки `Some(current) if current >
   *version_seen` (`tx_context.rs:266-268`). Property гоняет тысячи
   случайных read-set'ов × provider'ов.
2. **No false positive (precision) на стабильных версиях.** Если для всех
   ключей `current <= v_seen` (и таблица известна, `Some(_)`),
   `validate_read_set` ОБЯЗАН вернуть `Ok` (ветка `Some(_) => {}`,
   `tx_context.rs:269`).
3. **Unknown table ⇒ conflict, всегда.** Любой `None` от provider'а на
   непустом read-set ⇒ `Err` (ветка `None => conflict`,
   `tx_context.rs:265`).
4. **First-read-wins инвариант.** Многократный `record_read_shared` одного
   ключа с возрастающими версиями оставляет **самую раннюю** (наименьшую)
   версию — load-bearing SSI-семантика (`tx_context.rs:216-237`).
   Property: для любой перестановки версий одного ключа итоговая записанная
   версия = min. Это ловит регрессию «last-write-wins маскирует конфликт»,
   которая прямо описана в doc'е (`tx_context.rs:221-225`).
5. **SI-инвариант.** Под `IsolationLevel::Snapshot` read-set **всегда
   пуст** после любого числа `record_read_shared` (ранний return,
   `tx_context.rs:227`) — `validate` тривиально `Ok`.

**Опционально — interleaving harness:** маленькая in-memory модель двух
конкурентных tx (commit-order × read/write-order перестановки) против
`MvccStore::version_of` (`crates/shamir-tx/src/mvcc_store.rs:239`) +
`apply_committed_ops` (`mvcc_store.rs:275`), проверяющая, что детектор
никогда не пропускает write-skew. Это _model-based_ property, более
амбициозное — второй шаг после базовых пяти.

### Target #2 — version-codec (fuzz / round-trip property)

**Что тестируем (grounded):** `encode_version_key` / `decode_version_key`
в `crates/shamir-tx/src/version_codec.rs:42-66`. Сейчас — 6 example-
тестов (`version_codec.rs:72-118`): `round_trip`, `empty_key_round_trip`,
`sort_order_matches_version`, `different_keys_dont_interleave`,
`short_input_decodes_to_none`, `missing_separator_decodes_to_none`.
Doc-comment **сам** обещает «verified by round-trip **property** tests
below» (`version_codec.rs:29-30`) — но по факту они example-based. Этот
gap — буквально написан в коде.

**Инварианты-кандидаты (PROPOSED):**

1. **Round-trip.** ∀ `key: Vec<u8>`, ∀ `v: u64`:
   `decode_version_key(&encode_version_key(&key, v)) == Some((&key, v))`
   — пока `key` не содержит хвост `0xFF + 8 байт` (см. инвариант ниже).
2. **Sort-order = numeric-order.** ∀ `key`, ∀ `v1 < v2`:
   `encode_version_key(key, v1) < encode_version_key(key, v2)`
   лексикографически (big-endian суффикс, `version_codec.rs:1-8`). Это
   load-bearing для range-scan'а истории в `MvccStore`.
3. **Prefix dominates suffix.** ∀ `k1 < k2` (как байт-строки, без хвоста-
   коллизии): любая версия `k1` сортируется до любой версии `k2`
   (`version_codec.rs:100-105` — обобщение `different_keys_dont_interleave`).
4. **⚠️ Adversarial — документированный 0xFF-инвариант (главная ценность
   fuzz'а).** Doc явно предупреждает (`version_codec.rs:18-27`): если
   пользовательский `key` ЗАКАНЧИВАЕТСЯ на `0xFF + 8 байт`, `decode`
   мис-парсит границу. Сейчас это «negligible chance» по аргументу про
   16 случайных байт `RecordId`. **Fuzz-таргет должен явно искать такие
   входы** — либо подтвердить, что round-trip ломается ровно на них (и
   тогда зафиксировать как explicit precondition + assert в вызывающем
   коде), либо доказать, что вызывающие пути их не порождают. Это
   единственный честный способ перевести «negligible» из аргумента в
   проверенный факт. Coverage-guided fuzzer (`cargo-fuzz`) найдёт границу
   мгновенно; `proptest` с генератором, нацеленным на хвост-`0xFF`, —
   тоже.

**Форма:** Target #2 реализуем **либо** как `proptest!` в
`version_codec.rs`-тестах (stable, минимальная санкция = только
`proptest`), **либо** как полноценный `cargo-fuzz`-таргет в отдельном
`fuzz/`-crate'е (nightly, нужен `arbitrary` + `libfuzzer-sys`).
**Рекомендация: начать с `proptest`** — покрывает инварианты 1-3 целиком
и инвариант 4 при таргетированном генераторе, без nightly и без отдельного
crate'а.

### Размещение тестов (по house-style)

Согласно `CLAUDE.md` ("Test organisation"): property-тесты ложатся в
`tests/`-директории соответствующих модулей, `mod.rs` — манифест. Для
`shamir-tx` это означает (PROPOSED, после санкции):
`crates/shamir-tx/src/tests/ssi_props.rs` и `.../codec_props.rs`, либо
расширение существующих inline-`#[cfg(test)] mod tests` в `tx_context.rs`
/ `version_codec.rs` (последнее противоречит правилу «никаких inline
`mod tests`», так что предпочтительна `tests/`-директория). cargo-fuzz-
таргет (если санкционируют) — отдельный `fuzz/`-crate, **исключённый из
workspace `members`** (как `shamir-client-node`), чтобы не тянуть
nightly-only зависимость в дефолтный `cargo test`.

### Effort estimate

| Под-задача | Оценка | Зависит от |
|---|---|---|
| Санкция на `proptest` (решение мейнтейнера) | — | **БЛОКЕР** |
| SSI property-тесты (инварианты 1-5) | ~2 ч | `proptest` |
| codec round-trip/sort property (инв. 1-3) | ~1 ч | `proptest` |
| codec adversarial 0xFF (инв. 4, proptest) | ~1 ч | `proptest` |
| Interleaving model harness (опц.) | ~3 ч | `proptest` |
| cargo-fuzz codec target (опц., nightly) | ~2 ч | `arbitrary` + `cargo-fuzz` санкция |
| **Итого (минимальный, только proptest)** | **~4 ч** | после санкции |

### Risks

- **Главный риск — НЕ начинать без санкции.** Любой `Cargo.toml`-эдит до
  явного разрешения нарушает правило проекта. Этот раздел остаётся spec'ом,
  пока мейнтейнер не скажет «да, добавляй `proptest`».
- **Proptest flakiness / determinism.** Property-падения должны быть
  воспроизводимы — фиксировать `PROPTEST_CASES` и сид в CI, коммитить
  `proptest-regressions/` (proptest сам его генерит). Низкий риск.
- **Адверсариал-находка по codec'у — это успех, не провал.** Если fuzz
  найдёт реальный мис-парс на хвосте-`0xFF`, это ровно та ценность, ради
  которой и затевалось: переведём «negligible» в explicit assert/
  precondition. Возможный side-effect — крошечный prod-патч в codec'е или
  его вызывающих (тогда — отдельный `fix:`-commit, по дисциплине).
- **cargo-fuzz тянет nightly.** Поэтому он опционален и изолирован в
  отдельный crate вне workspace — дефолтный stable-гейт не затрагивается.

---

## 4. Что уже CLOSED (verified via git log — НЕ перепланировать)

`docs/pre-transactional/REVIEW.md` §11 («Honest follow-ups still OPEN»)
был написан до серии последующих коммитов. **Проверено по `git log
--oneline`** — следующие §11-пункты **уже landed** и их **НЕ нужно**
перепланировать в этом доке:

| §11 item | Статус | Commit | Что закрыто |
|---|---|---|---|
| **I.1** executor `BatchOp::Read` tx-threading | ✅ DONE | `230f8b5` | "executor threads tx into reads — SSI works end-to-end". Read-set теперь популяется на реальных SELECT'ах. |
| **I.2** index-config catalogue replay on recovery | ✅ DONE | `20ee9ed` | "persist table catalogue (recovery data-replay) + table_by_token O(1) + durable DDL". |
| **II.1** real-crash subprocess harness | ✅ DONE | `783a7bf` | "real subprocess crash-recovery harness — atomicity at every phase". `process::abort()` на 7 seam'ах, reopen на реальном redb. |
| **III.1** `table_by_token` O(1) | ✅ DONE | `20ee9ed` | O(1) lookup на commit hot-path. |
| **III.2 + III.3** alloc-free `current_version` + version_cache eviction | ✅ DONE | `ba6fa0a` | "alloc-free current_version + version_cache eviction in GC". |
| **III.4** batched insert_many через MvccStore | ✅ DONE | `2cfb7f6` | вместе с I.4 read-your-own-writes. |
| **III.5** HNSW promote вне `commit_lock` | ✅ DONE | `64e148b` | "HNSW promote outside commit_lock"; код — `commit.rs:442-460`, `promote_vectors` `commit.rs:498-528`. |
| **IV.1** CI integration-gate | ✅ DONE | `55adef0` | "gate integration test targets, not just --lib"; см. §2 выше. |
| **IV.2 + IV.3** REVIEW refresh + drop dead `anyhow` dep | ✅ DONE | `ea0fb20` | "refresh REVIEW.md + delete dead TxError + drop anyhow dead dep". |
| **I.3 / MED-A (логическая часть)** WAL-as-commit-point | ✅ DONE | `a333a91` | "WAL is the commit point — Phase 5 is idempotent materialization, no abort after Phase 4". Это **фундамент** аргумента §1. |
| **C6** empty-tx fast-path + honest multi-table deferral contract | ✅ DONE | `00f3841` | fast-path `commit.rs:389-414`; честный partial-deferral doc `commit.rs:23-58`. |

**Также landed** рядом (контекст для §1/§3): I.4 read-your-own-writes
(`2cfb7f6`, `6cbd3f2`), `materialized`-флаг на `TransactionInfo`
(`34728ec`), D12 HNSW atomic rid-claim (`97e4d60`), idempotent recovery on
NotFound (`4f419bf`).

**Что из §11 действительно ОСТАЁТСЯ открытым** (и есть в этом доке):
- **MED-A** — _физическая_ атомарность как примитив → §1 (вердикт:
  WONTFIX-by-design + 2 дешёвых улучшения).
- **Property / fuzz** → §3 (требует санкции на deps).
- **CI `--all-targets`** → §2 (по факту уже закрыто `55adef0`; остаётся
  docs-фиксация + опц. proposals).
- **Perf — `table_by_token` на commit hot-path** → частично снято
  `20ee9ed` (O(1) lookup); остаток — per-commit resolution cache —
  это perf-задача, **выносится в `PERF_OPPORTUNITIES.md`**, не сюда.

> Действие по итогу §4: обновить `REVIEW.md` §11, пометив I.1/I.2/II.1/
> III.*/IV.* как CLOSED (со SHA), и переформулировав MED-A из «follow-up»
> в «by-design, см. `PHASE_A_TAILS.md`».

---

## 5. Prioritization — порядок и леверидж

Ранжирование по **(уверенность, которую добавляет) / (стоимость + риск)**:

1. **🥇 Property/fuzz (§3) — ПЕРВЫМ, как только дадут санкцию на
   `proptest`.** Самый высокий леверидж: напрямую укрепляет SSI-логику
   (`validate_read_set`) и version-codec — два инвариант-несущих модуля,
   на которых стоит **весь Phase C** (serializable). ~4 ч после санкции.
   **Блокер — только решение мейнтейнера по dev-dep.** Пока санкции нет —
   это первый «готов к старту» пункт, который физически нельзя начать, так
   что параллельно двигаем №2/№3.

2. **🥈 MED-A (§1) — вторым.** Ноль prod-кода, ноль deps, ~2.5 ч. Главная
   ценность — **зафиксировать правильность текущего дизайна** (docs-
   инвариант) и сделать «restart-bounded eventual consistency»
   исполняемым доказательством (реальный redb-reopen тест). Можно делать
   **прямо сейчас**, не дожидаясь санкций.

3. **🥉 CI (§2) — третьим (почти даром).** Базовый delta уже закрыт
   (`55adef0`); остаётся ~0.5 ч docs-фиксации + опциональные proposals
   (nightly/coverage), которые сами требуют отдельной санкции. Делается
   заодно с обновлением `REVIEW.md` из §4.

**Рекомендованная последовательность исполнения:**

```
(параллельно, без санкций)         MED-A docs+test  +  CI docs-фиксация  +  REVIEW.md §11 update
(после санкции на proptest)        SSI property → codec property → codec adversarial 0xFF
(опц., после доп. санкции)          cargo-fuzz codec target  +  interleaving model harness
```

Итог по времени: **без санкций ~3 ч** (MED-A + CI + REVIEW), **+~4 ч**
после санкции на `proptest`, **+~5 ч** на полностью опциональный
fuzz/model-слой.

---

## 6. Cross-references

Этот документ — часть планируемой серии Phase-доков в `docs/roadmap/`.
Companion-доки (planned siblings, ещё не созданы на момент написания):

- **`NEXT_PHASES.md`** — обзор всех будущих фаз (overview); этот файл —
  его «tails of Phase A» приложение.
- **`PHASE_B_INTERACTIVE_TX.md`** — интерактивные (multi-batch)
  транзакции. MED-A-инвариант (logical-WAL atomicity) — предпосылка для
  Phase B: расширение tx за пределы одного batch'а наследует тот же
  WAL-as-commit-point контракт.
- **`PHASE_C_SERIALIZABLE.md`** — полный serializable. **Property-тесты
  из §3 особенно укрепляют именно эту фазу** — `validate_read_set` и
  interleaving-инварианты — фундамент Phase C.

Существующие доки (на диске сейчас):

- `docs/roadmap/TRANSACTIONS_IMPL.md` — house-style и детальная история
  имплементации tx-слоя.
- `docs/roadmap/TRANSACTIONS.md` — дизайн tx и принцип backend-
  agnosticism, на который опирается вердикт MED-A (§1).
- `docs/roadmap/PRODUCTION_HARDENING_ROADMAP.md` — тон/структура (honest
  roadmap с оценками + «что НЕ делать»), которой следует этот файл.
- `docs/roadmap/PERF_OPPORTUNITIES.md` — куда выносятся perf-остатки
  (per-commit `table_by_token`-cache, batched-flush на redb-стеке).
- `docs/pre-transactional/REVIEW.md` — авторитетный §11-список follow-
  ups (см. §4: какие пункты уже CLOSED).
- `docs/pre-transactional/05-executor-isolation.md` — "Known Production
  Limitations"; сюда поднимается MED-A-инвариант как by-design решение.

---

_Статус: planned polish — Phase A функционально DONE; это confidence/
robustness tails, не блокеры. MED-A вердикт: **оставить logical-WAL
(by-design), примитив не строить** + 2 дешёвых улучшения. Property/fuzz:
**требует санкции мейнтейнера на dev-deps (`proptest`; опц. `arbitrary` +
`cargo-fuzz`)** — до санкции ноль изменений в `Cargo.toml`. CI: базовый
delta уже закрыт `55adef0`. Дата ревизии — **2026-05-29**._
