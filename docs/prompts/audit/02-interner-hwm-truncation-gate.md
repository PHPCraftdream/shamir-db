בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# CRIT-2 — WAL F6b truncation не гейтится на interner HWM (#436)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #436 —
> CRITICAL находка панельного ревью
> (`docs/audits/2026-07-06-durability-storage-wal-tx.md` §1.2). Область:
> `crates/shamir-engine/src/tx/drainer.rs`,
> `crates/shamir-engine/src/table/interner_manager.rs`.

## Дефект (подтверждён моим личным чтением кода)

`drain_step` (`drainer.rs`, ~строки 441-483) для КАЖДОЙ entry:
1. `gate.mark_durable(*v)` (~454) — **безусловно** продвигает
   `durable_watermark` для версии `v`.
2. ПОТОМ проверяет A5-гейт `interner_delta_safe_to_truncate(repo,
   delta_max_id)` (~459) — если interner ещё не персистировал id из
   delta этой entry, гейт возвращает `Ok(false)` и **только** снятие
   per-entry inflight-МАРКЕРА (`wal.commit(entry.txn_id)`) откладывается.

**НО** F6b (~строка 528, `if wal.has_truncatable(durable) { ...
wal.truncate_below(durable) ... }`) использует `durable =
gate.durable_watermark()` — ТУ ЖЕ переменную, которая уже была
безусловно продвинута шагом 1, **НЕ дожидаясь** A5-гейта конкретной
entry. То есть: A5-гейт защищает только "можно ли снять WAL-маркер
конкретной entry", но НЕ защищает "можно ли физически удалить sealed
WAL-сегмент, содержащий эту entry" — а именно это делает F6b.

### Сценарий провала (из аудита, подтверждён чтением кода)

tx минтит поле `"foo"` (id=42), body записей в history закодирован этим
id; drain_step обрабатывает эту entry: `mark_durable(v)` продвигает
`durable_watermark` до `v` (или выше) БЕЗ ожидания, пока interner
персистирует id=42 (A5-гейт для МАРКЕРА мог сказать `Ok(false)` и
оставить маркер, но `durable` уже продвинут!). Далее в ТОМ ЖЕ или
следующем drain-pass F6b видит `wal.has_truncatable(durable)` — раз
`durable` уже включает версию `v`, а сегмент, содержащий её WAL-entry
(с interner_delta id=42), пересекает границу сегмента — **сегмент
truncate'ится**, стирая единственную запись о том, что id=42 == "foo".
Crash до чекпоинта интернера (`InternerManager::persist`, который сам
пишет через MemBuffer-буферизацию без принудительного fsync ДО
продвижения `last_persisted_len` — см. ниже) → рестарт: интернер не
знает id=42 → записи с этим id не декодируются, а следующий минт
**переиспользует id=42 под другое имя** → тихая порча данных (RTFM:
поля читаются под чужими именами).

## Задача

### 1. Развязать `durable_watermark` (для read-visibility) от "safe-to-truncate" (для F6b)

Не трогай семантику `mark_durable`/`durable_watermark` — она используется
читателями для видимости (`get_current` и т.п.), должна продолжать
продвигаться сразу после записи в history (иначе регрессия видимости).

Вместо этого введи ОТДЕЛЬНЫЙ "interner-safe truncation ceiling" — версию,
до которой ВСЕ interner-дельты уже персистированы. Механизм:
- В `drain_step`, для каждой entry, ПОСЛЕ вызова
  `interner_delta_safe_to_truncate(repo, delta_max_id)`:
  - если `Ok(true)` (безопасно) — эта entry МОЖЕТ участвовать в потолке
    truncation;
  - если `Ok(false)` или `Err` — эта entry (и все более новые версии)
    НЕ могут быть truncate'ены, пока interner не догонит.
- Посчитай `truncation_ceiling` как МИНИМУМ версии первой "небезопасной"
  entry минус 1 (или `durable_watermark`, если все entries в этом окне
  безопасны) — то есть truncation никогда не пересекает первую entry,
  чей interner-delta ещё не персистирован.
- F6b (~528) должен использовать `wal.has_truncatable(truncation_ceiling)`
  и `wal.truncate_below(truncation_ceiling)` — НЕ `durable` напрямую.

Учти: drain_step обрабатывает entries пачкой в цикле — тебе нужно
отслеживать "первый провал A5-гейта в этом проходе" и капать
`truncation_ceiling` на этой границе, даже если `durable_watermark`
продвинулся дальше по более новым (безопасным) entries. Обоснуй в
докладе точный механизм (например: булева переменная
`interner_gate_tripped` + `min_unsafe_version`, посчитанная в цикле
до вызова F6b-блока).

### 2. `InternerManager::persist()` — fsync ДО продвижения `last_persisted_len`

`interner_manager.rs::persist` (~строки 300-320): пишет chunk через
`self.info_store.set(...)` (MemBuffer-dirty, RAM) и **сразу** продвигает
`last_persisted_len.store(new_high, Release)` — то есть `hwm`
(`persisted_high_water()`) заявляет "durably persisted" когда на самом
деле данные ещё только в RAM-буфере (MemBuffer 500ms + fjall-журнал без
fsync).

Фикс: добавь `self.info_store.flush().await?` (или эквивалент —
грепни, как `flush_buffers`/`flush_all_history` делают явный fsync/drain
для других сторов, повтори паттерн) МЕЖДУ `info_store.set(...)` и
`last_persisted_len.store(...)`. Если у `info_store`
(`shamir_storage`-типа) нет прямого `flush()`-метода на инстансе — найди
правильный API (возможно `drain_once`/`persist(SyncAll)` на
info_store's backend) и подключи корректно, не выдумывай несуществующий
метод.

## Тесты

1. **A5-гейт → truncation ceiling regression**: сконструируй сценарий
   (аналогично существующим drainer-тестам, если есть — грепни
   `drainer_tests.rs`/аналог) где: entry с interner-delta, A5-гейт
   возвращает `Ok(false)` (искусственно — mock/test-hook, если нужно,
   заведи `#[cfg(test)]` static для форсирования `interner_delta_safe_to_
   truncate` в `Ok(false)`, мирроря паттерн `FAIL_HISTORY_SEED_TX_ID` из
   CRIT-1), НО `durable_watermark` продвинулся дальше (более новая entry
   без interner-delty уже задренирована). Assert: F6b НЕ truncate'ит
   сегмент, содержащий небезопасную entry, несмотря на то что
   `durable_watermark` её уже покрывает.
2. **persist() flush regression**: assert, что после `persist()`
   `persisted_high_water()` действительно отражает данные, ФИЗИЧЕСКИ
   сброшенные (не полагайся только на happy-path — если возможно,
   промоделируй сбой между set и flush, аналогично паттерну CRIT-1).
3. Существующие drainer/interner тесты не должны сломаться.

## Гейт

- `./scripts/test.sh @engine --full` 1×, целевые новые тесты 5-10× повторно;
- `cargo clippy -p shamir-engine --all-targets -- -D warnings`;
- `cargo fmt -p shamir-engine -- --check`.

## Дисциплина

Тесты ТОЛЬКО через ./scripts/test.sh. Хирургично: drainer.rs +
interner_manager.rs + их тесты. НЕ трогай mark_durable/durable_watermark
семантику для читателей — только truncation-путь. stray-логи отметь, не
удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

F6b truncation больше не может пересечь границу первой entry с
непоперсистированным interner-delta, даже если `durable_watermark` уже
продвинут дальше. `InternerManager::persist()` делает реальный flush
ДО продвижения hwm. Regression-тесты доказывают оба инварианта. Гейт
зелёный. Финал: точный механизм truncation_ceiling, diff-места, вывод
тестов, вывод гейта.
