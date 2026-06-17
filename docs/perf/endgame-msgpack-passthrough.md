בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Endgame — MessagePack pass-through (server = lens-only over id-msgpack)

Терминальное состояние RecordView/интернер-кампании. Дополняет
`recordview-campaign-status.md` (что сделано) и `r5-deintern-plan.md`.

---

## 1. Принцип

**Сервер — линза над id-ключевым MessagePack, и больше ничего по данным.**
Интернинг (имя→id) и де-интернинг (id→имя) живут на **клиенте**; словарь
зеркалится с сервера (`interner.dump` + ambient-delta), новые поля минтятся
через `touch` (по одному / массивом). Storage-байты = wire-байты:

- **запись:** как получили id-msgpack → **провалидировали линзой** → записали
  **verbatim** (никакого re-encode);
- **чтение:** как считали с диска → отдали **verbatim** (`SELECT *`) либо
  линза-проекция в id-msgpack-подмножество; клиент де-интернит ответ словарём.

Сервер остаётся авторитетом **только** для: аллокации id (`touch`), валидации,
индексации, фильтрации, recovery. По данным на горячем пути — **ноль
intern/de-intern/дерева на операцию**.

---

## 2. Текущее состояние vs терминал

| Слой | Сделано | Разрыв |
|---|---|---|
| Storage | id-msgpack на диске ✓ | — |
| Чтение filter/scan | линза ✓ | — |
| Чтение результат | R5: де-интернинг прямо с линзы ✓ | сервер **всё ещё де-интернит** в имя-ключевое → отдать id-msgpack, де-интернинг **на клиента** |
| Запись insert (hot) | прямой энкодер, без дерева ✓ | wire **имя-ключевой**, сервер **интернит per-write** → клиент шлёт id-msgpack, сервер пишет verbatim |
| Клиент-интернер | кеш + touch + dump + ambient-delta ✓ | клиент **не интернит на отправке** / **не де-интернит ответ** |
| JSON-приём записей | `json_to_inner` принимает | удалить (костыль) |

---

## 3. Стадии до терминала

- **S-write** (= бывш. #52, поднят bench-gated→**GO**; драйвер архитектурный, не
  перф): id-ключевой write-wire. Клиент шлёт id-msgpack (интернит кешем +
  батч-pre-touch новых полей); сервер: линза-валидация + индекс-ключи с линзы +
  запись байт **verbatim**. Серверный per-write интернинг исчезает.
- **S-read** (новая): id-ключевой read-wire. `SELECT *` = буквальный pass-through
  storage-байт; проекция = линза→id-msgpack-подмножество; клиент де-интернит.
  **R5 переезжает на клиента**; серверные `inner_to_json` /
  `inner_value_to_query_value` уходят с горячего read-пути.
- **S-client** (достройка Stage 5): intern-on-send + de-intern-on-recv +
  батч-pre-touch. Кеш/ambient-delta уже есть.
- **S-json (ЯВНАЯ ЦЕЛЬ — full JSON elimination)**: убрать JSON из data-плоскости
  **полностью** и удалить все связанные типы/кодеки. Масштаб: `serde_json`/
  `json::Value` — **742 вхождения / 120 файлов**, почти все крейты. Разбивка:
  - **GO (внутреннее):** доминирующий потребитель — весь **read-result pipeline**
    (`query/read/*`: project/order/aggregate/distinct строят `json::Value`) →
    перевести на `QueryValue`/id-msgpack; wire-DTO (`query-types` несут json::Value)
    → id-msgpack; удалить `json_to_inner`/`inner_to_json*`/`QueryValue↔json` мост
    (`types/value.rs`, `codecs/interned/json.rs`).
  - **Не-стены:** funclib `json.rs` = 3 сайта (работает на InnerValue, не serde_json);
    wasm `host_call.rs` = 0 serde_json.
  - **Граница (решение):** FFI napi (`shamir-client-node`) + TS (`shamir-client-ts`)
    — формат внешнего API; вне дефолт-workspace; full-removal там = внешние клиенты
    только msgpack (продуктовое решение).
  - **Контрол-плоскость** (`shamir-db` admin/system) — под-развилка: типизировать/
    msgpack vs оставить JSON только в admin.
  Это отдельная под-кампания по крейтам (фундамент: types → query-types → engine‖…).
- **W3** (исслед. @aoh): update/delete тоже tree-free — вписывается как «запись
  полностью без дерева».

**Итог:** горячий путь сервера = чистая линза над msgpack.

**КОНЕЧНАЯ ЦЕЛЬ (расширена по запросу): ноль `InnerValue` + ноль JSON ВЕЗДЕ.**
Три холодных якоря `InnerValue` — это уже не «оставить», а **финальные цели
устранения**:
1. **recovery/doctor codec** (`to_bytes`/`from_bytes`) — заменить decode-таргет
   починки на линзу + стриминговый re-encode (или работу байтами).
2. **funclib** `fn(&[InnerValue])` — обобщить по `Value<Key>` / перевести на
   `QueryValue` (скаляры без ключей → в осн. type-churn; ~12 категорий).
3. **index-hash leaf** (`materialize_at`→`with_values`, `Value<InternerKey>::Hash`
   discriminant) — САМЫЙ ТРУДНЫЙ: discriminant-стабильный хеш с линзы/`ScalarRef`,
   либо index-format version-bump + **rebuild-миграция** (слом персист-byte-identity).
   Возможен отдельный sub-проект; если миграция не окупается — «везде кроме
   index-hash» как честно принятый предел.

JSON «везде» = S-json (мёртвый кодек) + переписать read-result pipeline
(`json::Value`→`QueryValue`/id-msgpack, ядро S-read) + решить control-plane +
граница FFI. См. задачу-umbrella «🎯 КОНЕЧНАЯ ЦЕЛЬ».

---

## 4. Честные оговорки (где «verbatim» не буквален)

1. **Validate-not-trust.** «Как получили — так пишем» = провалидировали линзой,
   потом пишем; не «слепо доверяем». Линза **untrusted-input-safe**
   (bounds-checked, не паникует) — спроектирована ровно под это. + проверка
   валидности id (в диапазоне интернера).
2. **$fn / computed / default-поля на записи** — сервер досчитывает значения
   (funclib/дерево). Остаётся серверным исключением, не pass-through.
3. **Проекция/агрегация** — `SELECT *` pass-through; подмножество/GROUP
   BY/computed = линза-обработка, но на выходе **id-msgpack** (не де-интернинг).
4. **Scope удаления JSON** — убираем JSON для **payload'ов записей**;
   control/query-plane и debug — отдельная развилка (оставить vs полный
   msgpack-протокол).
5. **Breaking change** — id-msgpack wire + удаление JSON ломают старых клиентов →
   version-bump протокола / жёсткий cutover.

---

## 5. Метод

Каждая стадия: design-pass (@aoh/@aom) → GO/NO-GO → реализация crush/агентами →
byte-identity + recovery-safe на каждом персист/wire-шаге → коммит между
этапами → авторитетный гейт + zero-trust верификация оркестратором.
