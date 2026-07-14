בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# CRIT-4 — write-skew коммитится под Serializable в lock-free пути (#438)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #438 —
> CRITICAL находка панельного ревью (`docs/dev-artifacts/audits/2026-07-06-concurrency-engine.md`
> §A1, помечена автором аудита как "самая технически рискованная находка,
> нужна тройная проверка"). Область: `crates/shamir-engine/src/tx/commit.rs`
> (`commit_tx_lockfree`), возможно `crates/shamir-tx/src/repo_tx_gate.rs`.

## Дефект (подтверждён личным чтением кода оркестратором)

Легаси-путь (`commit_tx_inner_legacy_async`, `commit.rs:496-623`) держит
ВСЮ секцию validate→publish под `gate.commit_lock()` (`tokio::sync::Mutex`)
— это гарантирует first-committer-wins и полную сериализацию.

P2c-путь (`commit_tx_lockfree`, `commit.rs:633-...`) убрал этот
`commit_mutex` ради параллелизма непересекающихся таблиц. Комментарий на
`commit.rs:475-480` утверждает: "same-table committers serialize at uwl
acquisition" — но `uwl_guards` берутся ТОЛЬКО для таблиц с
`unique_guards` (`pre_commit.rs:226-235`). Serializable-транзакция БЕЗ
уникальных ограничений не сериализуется НИГДЕ в этом пути.

### Сценарий провала (write-skew, из аудита, подтверждён)

Обе транзакции A, B — Serializable, snapshot=10, x=y=v10:
- A читает x (`read_set[x]=10`), пишет y. B читает y, пишет x
  (write-sets дизъюнктны → `claim_write_set` не конфликтует, uwl нет).
- A: `pre_commit_locked_validate`: `version_of(x)=10` → OK; предикатов
  нет → OK; claim y → win.
- B: то же для y/x → OK (A ещё не сделал публикацию).
- A: WAL, footprint, publish y=11.
- B: WAL, footprint, publish x=12.
- **Результат:** обе закоммичены; A читала y@10, но B перезаписал
  прочитанное A. Цикл rw-антизависимостей → нарушение Serializable.

Тест `ssi_write_skew_one_aborts` (`acceptance_tests.rs:472`) строго
последовательный (A полностью коммитит до старта B) и эту дыру не ловит.

## Задача

### Фикс — точечный, НЕ архитектурный передел

Serializable-транзакции обязаны сериализовать validate→publish окно;
Snapshot-транзакции (которым SSI-проверки не нужны) должны сохранить
полный lock-free параллелизм — не регрессируй производительность
Snapshot-пути.

Предлагаемый подход (проверь и оспори, если увидишь контраргумент):
в `commit_tx_lockfree` (`commit.rs:633`), если
`tx.isolation == shamir_tx::IsolationLevel::Serializable`, взять
`gate.commit_lock().await` ПЕРЕД `pre_commit_locked_validate` и держать
guard живым до окончания `materialize` (публикации), затем drop —
зеркалируя ровно то окно, которое легаси-путь уже держит под этим же
mutex'ом. Для `IsolationLevel::Snapshot` — никакого лока, путь остаётся
как есть.

Псевдо-диф (адаптируй под реальную сигнатуру — переменные типов см.
файл):
```rust
async fn commit_tx_lockfree(...) -> Result<TxOutcome, TxError> {
    use crate::tx::materialize::materialize;
    use crate::tx::pre_commit::pre_commit_locked_validate;

    // CRIT-4 (#438): Serializable-txs must serialize validate→publish —
    // uwl_guards alone (unique-constraint tables only) don't cover the
    // general read-write-antidependency (write-skew) case. Snapshot-txs
    // don't need SSI validation and keep full lock-free parallelism.
    let _serializable_guard = if tx.isolation == shamir_tx::IsolationLevel::Serializable {
        Some(gate.commit_lock().await)
    } else {
        None
    };

    let validated = match pre_commit_locked_validate(...).await { ... };
    ...
    let post_publish = materialize(...).await;
    // _serializable_guard drops here (end of fn scope, or explicit drop
    // right after materialize completes) — releasing the serialization
    // window only after publish is durable/visible.
    ...
}
```

**Важно — избегай deadlock / self-block:** `commit_tx_lockfree` не
вызывается на AsyncIndex-пути (тот уходит в
`commit_tx_inner_legacy_async`, который САМ берёт `commit_lock()`) —
убедись, что нет пути, где `commit_tx_lockfree` вызывается уже ПОД
удерживаемым `commit_lock` (иначе `tokio::sync::Mutex::lock().await`
на том же мьютексе задедлочится — это НЕ реентерабельный лок). Грепни
все call site'ы `commit_tx_lockfree` и `commit_lock()` чтобы это
доказать в докладе.

**Обязательно проверь `active_serializable_count`**
(`repo_tx_gate.rs:94,222-256,397`) — похоже, это уже существующий
счётчик активных Serializable-транзакций для какой-то другой цели
(возможно, non-tx write path пропускает лишнюю работу, когда счётчик=0
— см. комментарий `repo_tx_gate.rs:146`). Пойми его текущее назначение,
убедись, что твой фикс с ним не конфликтует, и упомяни в докладе, нужно
ли его использовать вместо/вместе с новым `commit_lock`.

### НЕ трогай

- `commit_tx_inner_legacy_async` (AsyncIndex-путь) — уже сериализован,
  не относится к дефекту.
- Snapshot-путь производительность — фикс обязан быть **нулевым**
  оверхедом для Snapshot-транзакций (просто `if` branch, no lock taken).
- uwl_guards / unique-constraint механизм — не трогай, он остаётся как
  доп. защита для unique-таблиц (независимо корректен).

## Тесты

1. **Regression на write-skew**: две Serializable-транзакции,
   дизъюнктные write-sets (A читает x пишет y, B читает y пишет x),
   интерливинг КАК В СЦЕНАРИИ ВЫШЕ (обе валидируются ДО того как любая
   опубликует — используй ручные barrier'ы/каналы чтобы гарантировать
   порядок валидация-валидация-публикация-публикация, а не полагайся на
   таймингы). До фикса: обе коммитятся (баг). После фикса: одна должна
   либо заблокироваться на commit_lock и увидеть уже опубликованную
   версию competitора при повторной валидации → abort по SSI-конфликту,
   ЛИБО фактически сериализоваться так, что цикл антизависимостей не
   формируется. Assert: НЕ обе транзакции успешно закоммитились с
   версиями, нарушающими Serializable (т.е. либо одна вернула
   `TxError`/abort, либо докажи любым другим способом отсутствие
   write-skew).
2. **Snapshot performance smoke**: существующие Snapshot-уровневые
   lock-free тесты (грепни `commit_tx_lockfree`/`lockfree` в
   `crates/shamir-engine/src/tx/tests/`) остаются зелёными и НЕ
   регрессируют — если есть перф-бенч для параллельных disjoint-table
   Snapshot коммитов, прогони его до/после и подтверди отсутствие
   деградации в докладе (не обязательно формальный /opti цикл, просто
   sanity-check).
3. Существующий `ssi_write_skew_one_aborts` и всё в
   `acceptance_tests.rs`/`tx/tests/` остаются зелёными.

## Гейт

- `./scripts/test.sh @oracle --full` (shamir-tx) +
  `./scripts/test.sh @engine --full` (shamir-engine, commit path) 1×,
  целевой новый regression-тест 10-15× повторно (это concurrency-фикс —
  единичный зелёный прогон ничего не доказывает);
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo fmt -p shamir-engine -p shamir-tx -- --check`.

## Дисциплина

Тесты ТОЛЬКО через ./scripts/test.sh. Хирургично: `commit.rs` +
возможно `repo_tx_gate.rs` (если `active_serializable_count`
действительно нужно тронуть — обоснуй) + один новый regression-тест.
НЕ трогай A2 (`publish_cell` монотонность) — отдельная задача (#443
HIGH-cluster), не пересекается с этим фиксом кроме общего файла.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

Serializable-транзакции в lock-free пути сериализуют validate→publish
окно (через `commit_lock` или эквивалент), устраняя write-skew.
Snapshot-путь остаётся полностью lock-free (нулевой оверхед). Regression-
тест доказывает: сценарий из аудита (A2/A1-style write-skew) больше НЕ
проходит обе транзакции успешно. Никакого deadlock (докажи отсутствие
двойного захвата commit_lock на одном пути). Существующие тесты зелёные.
Финал доклада: точный diff, вывод тестов (включая 10-15× повторов
regression-теста), вывод гейта, явное подтверждение/опровержение
отсутствия deadlock и отсутствия перф-регрессии Snapshot-пути.
