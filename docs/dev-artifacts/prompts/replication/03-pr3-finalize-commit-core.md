בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# PR3 — выделить финализационное ядро коммита (/opti-дисциплина)

> Контекст: `docs/dev-artifacts/research/REPLICATION-PRE-REFACTOR-2026-06-30.md` §Б PR3,
> `docs/dev-artifacts/roadmap/REPLICATION.md` §4.1. Fowler preparatory-refactoring:
> «make the change easy, then make the easy change».

## Цель

Сейчас финализация коммита (нижняя половина: применение данных →
публикация версии → SSI-футпринт → emit changefeed) дублируется в ТРЁХ
путях:
- `crates/shamir-engine/src/tx/commit.rs::commit_tx_inner_legacy_async`
  (~491, держит глобальный `commit_guard`, спавнит фоновый materialize-tail,
  emit changefeed ПЕРЕД спавном);
- `crates/shamir-engine/src/tx/commit.rs::commit_tx_lockfree` (~597, без
  глобального мьютекса);
- `crates/shamir-engine/src/tx/group_commit.rs::run_single_tx` (~439,
  materialize+post_publish_cleanup синхронно, emit changefeed ПОСЛЕ).

`apply_replicated` (ядро R1) станет ЧЕТВЁРТОЙ копией той же
последовательности. Задача — выделить общую нижнюю половину в переиспользуемую
`async fn finalize_commit(...)` (имя/сигнатуру подобрать по месту), чтобы
R1 вызвал её же с версией из реплицированного события, а не копировал.

## ⚠️ Это HOT-PATH — строгая /opti-дисциплина (см. .claude/skills/opti)

Цель по перформансу — **ноль регрессии** (чистое выделение функции должно
инлайниться обратно). Просадку НЕ коммитить.

### Обязательный порядок

1. **Baseline-бенч ДО любых правок.** Бенч: `tx_pipeline` в крейте
   `shamir-engine`. Команда (release, quick-mode, изолированный target):
   ```
   CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo bench -p shamir-engine --bench tx_pipeline
   ```
   Также сними `tx_concurrent` (многопоточный путь — group_commit) той же
   командой с `--bench tx_concurrent`. Запиши mean/throughput ТЕКСТОМ.
2. **Рефакторинг.** Выдели общее ядро. Сохрани СЕМАНТИКУ каждого пути в
   точности — особенно:
   - разный порядок emit changefeed относительно materialize/спавна tail;
   - какой путь держит `commit_guard` и когда его дропает;
   - legacy_async спавнит `tokio::spawn` фоновый tail и возвращает
     `BackgroundCommitHandle`; run_single_tx/lockfree — синхронно, background: None.
   Если пути НЕ имеют чистого общего шва (семантика расходится так, что
   «ядро» получается с 5 булевыми флагами и разветвлениями) — НЕ насилуй
   абстракцию. Останови рефакторинг, верни файлы в исходное состояние
   (через Edit, НЕ через git), и в финальном сообщении опиши где именно шов
   грязный и какой минимальный кусок (напр. только `emit_changefeed_event`
   + `record_commit_writes` + `version_guard.commit`) реально общий. Частичное
   выделение маленького честного ядра лучше, чем большая дырявая абстракция.
3. **Тесты.** `./scripts/test.sh @oracle` (shamir-tx + shamir-engine) —
   зелёный. Если что-то падает — чини до продолжения.
4. **Post-бенч ПОСЛЕ.** Те же две команды. Сравни явно: было X, стало Y.
   Регрессия > ~2-3% (вне шума) недопустима → разбирайся или откатывай.
5. Финальное сообщение: baseline/after цифры обоих бенчей, что выделено,
   вердикт (zero-regression / откат / частичное ядро + причина).

### Кэш-изоляция (КРИТИЧНО)

Бенчи ТОЛЬКО с `CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench`. Тесты
(`./scripts/test.sh`) пишут в обычный `target/` — не смешивай, иначе full
rebuild между baseline и after съест время. Гейт (fmt/clippy) — ОДИН раз в
конце цикла, не между baseline и after.

## Гейт перед завершением

- `./scripts/test.sh @oracle` зелёный.
- `cargo fmt -p shamir-engine -p shamir-tx -- --check` чистый.
- `cargo clippy -p shamir-engine --all-targets -- -D warnings` чистый.
- Оба бенча без регрессии (или обоснованный откат).

## Discipline

- Хирургические правки: только commit.rs / group_commit.rs (+ возможный
  новый sibling-файл `finalize.rs` под нижнюю половину, если оправдано —
  но предпочти расширение существующего модуля; mod.rs только re-export).
- Не трогай верхние половины путей (SSI-валидация, WAL, prelock, локи).
- Не меняй публичные API коммита без необходимости.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits. Если нужно откатить рефакторинг — делай
это правками через Edit, восстанавливая исходный код вручную.
