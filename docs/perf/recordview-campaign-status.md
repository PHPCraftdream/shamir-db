בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# RecordView-миграция — общая картина и состояние

Живой обзор кампании. Дополняет:
`record-view-migration.md` (исходный спек), `recordview-full-migration-map.md`
(read-path), `wave2-w2ab-index-plan.md` / `wave2-w2cd-write-cutover-plan.md`
(write-path), `stage5-wire-plan.md` (клиент-тир), `wave2-autonomy-decisions.md`
(лог автономных решений).

---

## 1. Суть

Убрать `InnerValue`-дерево как **промежуточную станцию** с горячих путей СУБД и
читать поля **линзой (`RecordView`) прямо по storage-байтам** (id-ключевой
msgpack, ключи = `InternerKey` как `bin`). Дерево материализовалось лишь чтобы
тут же закодироваться/раскодироваться и быть выброшенным — самый жирный per-row
аллок. Линза смотрит сквозь байты, не строя дерево.

**Север-звезда:** дерево и интернинг — прочь с горячих путей; работа линзой по
storage-байтам; name↔id целиком под капотом коннектора (API именованный);
интернер — редкий server-авторитет id (на `touch`, не per-insert).

---

## 2. Состояние — СДЕЛАНО (всё на master, гейты зелёные)

**Дерево убрано с ОБОИХ горячих путей — чтение и запись.**

### Чтение (Stage 0→4)
| Шаг | Коммит | Суть |
|---|---|---|
| Stage 0 | (в `1109f60`) | измерили GO: линза ×48-72 дешевле декода дерева |
| Stage 1+2+I | `1109f60` | линза `RecordView` (id-ключевая) + шов `RecordRef` + интернер per-repo |
| Stage 3 #1 | `5615479` | unique-key scan на шов |
| C0 | `6dbca1a` | рост `RecordRef` (present_kind/str/any_seq/materialize/for_each_field) |
| C1+C3 | `b8c9bbb` | filter presence/string атомы (Exists/IsNull/Like/Regex/Fts) |
| C2 | `208ec2c` | Compare/Between/In/InSet (scalar_ref_cmp) |
| C4 | `f7a4265` | Contains-семейство (materialize_at) |
| C5/C5b | `b2c3b2a`/`9cab824` | projection (per-field + is_all generic) |
| C6 | `3ff0f5f` | computed/`$fn` (IndexExpr::eval + resolve_filter_value generic) |
| C-sig | `b002787` | `FilterNode::matches(&impl RecordRef)` |
| **Stage 4** | `a153c5d` | **cutover**: скан отдаёт `RecordView`/`Bytes`, дерево не строится (`RecordCow`) |

### Интернер per-repo
`1109f60` (переезд на RepoInstance) + `36d6acb` (фикс cross-repo миграции —
`replicate_interner_from` на repo-уровне).

### Клиент-тир (registered-numeric)
| Шаг | Коммит | Суть |
|---|---|---|
| Stage 5d | `7f1cd4d` | `interner.dump`/`touch` wire-операции |
| Stage 5 minimal | `6695a2c` | клиентский field-map кеш (scc+THasher, §9.4-safe) + builder resolve + pre-touch |
| ambient-delta | `a35188e` | клиент шлёт epoch — сервер дослыает дельту (под капотом, backward-compat) |

### Запись (Wave 2 эпик)
| Шаг | Коммит | Суть |
|---|---|---|
| W1 | `d5abbc7` | validator round-trip убран (insert кормит QueryValue напрямую) |
| W2a-sorted | `e2abab5` | sorted-индекс на RecordRef (scalar_at+sort_codec, byte-identical) |
| W2a-hash | `7e60866` | legacy/unique индекс-ключи (materialize_at→with_values, byte-identical, **discriminant-крукс**) |
| W2b | `ba53050` | `IndexBackend`→`&dyn RecordRef` (FTS/vector/functional) |
| W2d-encoder | `ccb8ac4` | прямой `QueryValue→id-msgpack` энкодер (byte-identity 12/12) |
| **W2c+W2d** | `3f2f40a` | **cutover**: `execute_insert_tx` без дерева; staging на Bytes; удалены `StagedRow::Live`/`inner_values`/`rewrite_set_inner` |

**Инварианты, что держим железно:** storage-формат НЕ менялся → recovery
**byte-identical** (crash_recovery 18/18, byte-identity-тесты на каждом
индекс-шаге, @oracle 1228, @e2e 543, encoder 12/12). ~25 коммитов, каждый
verified + byte-identical.

---

## 3. Архитектура (ключевые факты)

- **Storage = id-ключевой msgpack** (`InnerValue::to_bytes`=rmp_serde; ключи
  `InternerKey::serialize`→`bin`). Линза читает ровно это. Клиентский wire —
  строково-ключевой (сервер интернит на лету через `query_value_to_storage_bytes`).
- **`RecordRef` — шов**: трейт (`scalar_at`/`str_at`/`present_kind_at`/`any_seq_elem`/
  `materialize_at`/`for_each_field`), реализован ОБОИМИ — `InnerValue` (дерево) и
  `RecordView` (линза). Статическая диспетчеризация на чтении; `&dyn RecordRef` для
  index2-backend'ов.
- **byte-identity — священна**: индекс-ключи и storage-байты персистятся; каждый
  шаг доказан байт-в-байт (через `materialize_at`+неизменённый `with_values` для
  hash-ключей — `ScalarRef` хешировать нельзя, другой discriminant).
- **Интернер per-repo**: один на БД, минтит id на `touch` (редко); recovery
  восстанавливает один интернер; `interner_delta` repo-scoped.
- **Dec/Big-инвариант**: insert-`QueryValue` никогда не даёт Dec/Big/Set → линза
  (декодит Dec/Big как Str) согласна с деревом для индекс-ключей.

---

## 4. Ближайшие задачи

| # | Задача | Статус / решение |
|---|---|---|
| **#43** | clippy hasher-airtight | 🟡 в работе: type-ban SipHash + миграция ~140 HashMap/HashSet→Fx-алиасы. Капасити — advisory (686 сайтов = churn ради малого; dylint на потом) |
| #51 | W3: non-tx `execute_insert` + `update_tx`/`delete_tx` ещё строят дерево | ⏸ follow-up, низкий приоритет (горячий insert уже tree-free) |
| #52 | id-ключевой провод (клиент шлёт id) | ⏸ bench-first. Честный NO-GO без бенча: Wave 2 уже убрал дерево, §9.4-валидация съедает encode-skip → выигрыш лишь от обхода serde_json, спекулятивный |
| Stage 6 | positional + shape-dictionary | пропуск — только при measured-need |

---

## 5. Выигрыш (измерено / структурно)

- **Чтение:** поле линзой ×48-72 дешевле полного декода дерева (Stage 0, opt-0);
  на селективном фильтре не-матчащиеся строки **вообще не декодятся в дерево**.
- **Запись (insert):** дерево не строится — прямой энкодер `QueryValue→msgpack`
  (доказанно байт-в-байт), линза для index/validators.
- **Клиент:** field-map кеш + ambient-delta — узнаёт id под капотом, без отдельных
  dump-round-trip'ов; API остаётся именованным.

---

## 6. Где мы на дуге

**Сердце кампании — бьётся.** Главное (×-кратное ускорение чтения + tree-free
вставка + registered-numeric транспорт) **сделано и доказано**. Остаток: одна
гигиена-политика (#43, где красиво — узко: хешер-airtight, капасити-advisory),
один долизывающий рефактор (#51 — дерево с редких write-путей), одна спекулятивная
оптимизация под бенч (#52 — скорее отпустить).

**Метод (весь путь):** measure-first → GO/NO-GO (честный NO-GO валиден);
design-pass `@aoh` перед крупным/рискованным; byte-identity-тест на каждом
персист-шаге; коммит между этапами; узкий self-check у исполнителя + авторитетный
гейт у оркестратора; crush — основной исполнитель, агенты — fallback.
