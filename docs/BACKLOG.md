בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Backlog (pending tasks)

Снимок открытых задач после закрытия InnerValue-кампании (#61) и кампании
удаления `serde_json` (`f19d593`). Полностью отсортирован по объёму/срочности.

---

## #83 — `scripts/test.sh`: Windows `.exe` fallback (tiny, инфра)

**Проблема.** В `scripts/test.sh:54` fallback-петля ищет `cargo-nextest`, но
**без расширения `.exe`**. На Windows у разработчика, у которого `nextest` не на
`PATH`, fallback-поиск не находит `cargo-nextest.exe` и тест-обвязка отваливается.
У меня сработало случайно (nextest на PATH), у ревьюера — нет.

**Фикс.** Тривиальный — цикл по расширениям:
```sh
for ext in "" ".exe"; do
  [ -x "$_dir/cargo-nextest$ext" ] && ...
done
```
Из ревью 17 июня п.5. Инфра, не код, ~5 минут.

---

## #72 — `INSERT…RETURNING` уважает `ResultEncoding::Id` (low, консистентность)

**Контекст.** Клиент при handshake может договориться получать записи в
**id-keyed** msgpack (компактнее: ключи — `u64` interner-id, а не строки-имена).
read-путь это уважает (`read_tx_with_encoding` → re-энкодит строки в `IdBytes`),
а **INSERT…RETURNING** — нет: всегда идёт через `write_result_to_query_result`,
который возвращает имя-ключевой `Inserted`/`Direct`, минуя negotiated encoding.

**Это не баг.** Клиент декодит `Inserted` нормально. Это **непоследовательность**:
read pass-through есть, write — нет. Чисто DX/перф-сжатость.

**Фикс.** Пробросить `result_encoding` в insert-write-путь (`query_runner`) и
re-энкодить RETURNING в `IdBytes` при `Encoding::Id`. Low.

---

## #82 — DX value-API над `QueryValue` для хранимых процедур (large, DX-проект)

**Контекст.** Anti-formal §5d. Когда автор пишет WASM/funclib-процедуру, движок
материализует запись в **owned-мутабельный `QueryValue`**, процедура мутирует и
возвращает обратно. Сейчас `QueryValue` — сырой enum, и работать с ним из
процедуры неудобно: `match v { QueryValue::Map(m) => match m.get("age") { … } }`
вместо привычного `rec["age"].as_int()?`. Lens-оптимизация (RecordView для
чтения) отдала чистый perf на горячих путях, но именно для **процедур** на
границе нужен удобный owned-API.

**Что нужно.**
- path-based `get`/`set` (`rec.get_path("user.name")`, `rec.set_path(...)`);
- билдеры для типичных конструкций (списки, мапы, числа);
- автоматическая коэрция типов (`as_int`/`as_str`/`as_bool` с разумными ошибками);
- эргономика на уровне «привычного JSON-объекта», но с тем же `QueryValue` под капотом.

Имя-ключевой `QueryValue` (rec["age"], не rec[42]) — для процедур DX-правильный.
Имена он уже хранит, дело только в API-обёртке. Backlog после #61 (DX-забота, не часть #61).

---

## #100 — TS/napi client-side interner (large, разгрузка сервера)

**Контекст.** Сейчас для TS-клиента **сервер** делает горячую трансляцию
`имя_поля↔id` на каждый запрос — заметная нагрузка. У Rust-клиента уже есть
client-side interner (FieldMap + epoch-delta-протокол). У TS — нет.

**Хорошая новость.** Wire-протокол для синхронизации интернера **уже построен**
на Rust (Stage 5-wire):
- `request.interner_epochs` (клиент сообщает, какие версии у него),
- `attach_interner_delta` (сервер прикрепляет дельту),
- `response.interner_delta` (отдаёт обратно),
- `InternerDump` (full snapshot), `entries_after(epoch)` (delta), `InternerTouch` (минт).
То есть **переизобретать ничего не нужно** — портировать существующий протокол.

**Что нужно сделать (TS-сторона).**
1. `FieldMap` name↔id кэш в `shamir-client-ts` + epoch-инвалидация.
2. `touch_fields` / `dump_repo` API на клиенте (минт / bulk-загрузка).
3. **intern-on-send**: при сборке запроса перевести имена-ключи в id-keyed msgpack;
   договориться с сервером о `ResultEncoding::Id` ответа.
4. **de-intern-on-recv**: при приёме id-keyed ответа развернуть в имена через
   локальный кэш; cache-miss → синхронный fetch `InternerDump`/delta.
5. e2e TS-тесты: id round-trip, минт нового поля, эпоха-инвалидация, cache-miss.

**Инвариант** (важно, чтобы клиент не сломался): сервер остаётся **авторитетом
id** (минтит и персистит). Клиент берёт только горячую трансляцию. id — append-only,
никакой переборки/реренамеров на стороне сервера.

**Эффект.** Сервер перестаёт делать имя↔id-трансляцию на каждый запрос; TS-клиент
становится **полным msgpack pass-through** (запрос и ответ идут компактным id-keyed
форматом). Это north-star+ задача (после JSON-core), большая и ценная.

---

## Снимок размеров

| # | тема | объём | приоритет |
|---|---|---|---|
| #83 | Windows `.exe` fallback в test.sh | tiny (~5 мин) | infra |
| #72 | INSERT…RETURNING + ResultEncoding::Id | small | low |
| #82 | DX value-API над QueryValue (процедуры) | large | medium |
| #100 | TS/napi client-side interner | large | high (разгрузка сервера) |
