בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Declarative Schema Validators — Overview

> Сделано в сторону красоты, удобства и совершенства — ради Всевышнего, ради Его
> Радости, ради Его Славы.

## Предметная область

S.H.A.M.I.R. уже умеет два вида валидаторов записей — глобальные переиспользуемые
объекты-КОД в `system/validators`, гейтятся `FunctionNamespace`, привязываются к таблицам
явным bind:

- **WASM** — недоверенный/портируемый КОД в песочнице.
- **Native** — доверенное Rust-замыкание, полная скорость, эфемерно (re-register на boot).

Этот документ вводит **третий вид — Declarative**: задаётся не кодом, а **данными** —
массивом правил полей, как столбцы с типами в реляционной СУБД. И — **строго по-таблично**:
это не глобальный объект, а **СХЕМА ОДНОЙ ТАБЛИЦЫ**, её shape:

```
[
  { "path": ["email"],            "type": "string", "max": 255, "required": true },
  { "path": ["age"],              "type": "int",    "min": 0, "max": 150 },
  { "path": ["address", "zip"],   "type": "string", "len": 5 },
  { "path": ["profile","active"], "type": "bool",   "required": true }
]
```

## Уточнённая трилогия: схема таблицы vs переиспользуемый код

| Вид | Природа | Хранение | Право | Привязка |
|---|---|---|---|---|
| **Declarative** | СХЕМА одной таблицы (данные) | в каталоговой записи **таблицы** | власть над **таблицей** | авто-binding (`ValidatorBinding`) |
| **WASM / Native** | переиспользуемый КОД | глобально (`system/validators`) | `FunctionNamespace` | явный bind к N таблицам |

Все три исполняются единой плоскостью (`run_validators_loop`), все три могут действовать на
одной таблице вперемешку — по `priority`.

## Сквозные решения (приняты)

0. **Валидатор — узкая роль `RecordValidator` с by-name входом; интернирование скрыто (Phase 0,
   `08-…`).** Паритет протащил валидаторы через общий `ShamirFunction`/`Params` (owned,
   msgpack для wasm) — отсюда полный де-интерн записи и протечка интерна. Убираем: валидатор —
   узкий контракт `validate(new: &dyn RecordFields, old, ctx) -> Validation`, поля ПО ИМЕНИ.
   **native/declarative** реализуют его напрямую (лениво, без де-интерна); **wasm** уже by-name
   в госте — его адаптер `WasmRecordValidator` локализует неустранимую msgpack-сериализацию у
   границы ABI (платит только wasm). **Тот же инвариант — для ФУНКЦИЙ:** их API
   (`FnBatch`/`FnCtx`/`Params`) уже строго по именам; `RecordFields` — общий by-name примитив
   навигации записи для функций И валидаторов. **Интерн-id нигде не пересекает границу
   пользовательского кода** (как «билдер слеп к интернированию» — теперь сквозь весь стек).
1. **Строго по-таблично (вариант A).** Declarative-схема хранится в **каталоговой записи
   таблицы** (`save_table_meta` — системная инфо-строка таблицы), не в `system/validators`.
   Создаётся/меняется **властью над таблицей** (`create_table` → `Action::Create`; alter →
   `Action::Write`). Durable из коробки, без re-register.
2. **Интернирование — клиент пакует, билдер слеп.** Билдеры оперируют ПЛОСКИМИ именами;
   упаковка в интернированные `u64`-id — клиентский слой (`interner_cache`). Интернер
   **per-repo** (не per-table). На провод и в каталог идут интернированные id; на
   compile-on-open путь де-интернируется id→name **один раз** для by-name правил (`04-…`).
3. **Именованные ограничения** (`{max:10, required:true}`); embedded-Rust fluent без
   бойлерплейта (`01-…`).
4. **Грань код-vs-данные — структурная.** Declarative = свойство таблицы (table-власть);
   wasm/native = объект namespace. Нового grant-измерения не нужно (`05-…`).

## Карта слоёв

| Док | Слой | Суть |
|---|---|---|
| `08-validator-field-interface.md` | **Phase 0** | by-name `RecordFields` поверх записи; миграция native/wasm |
| `01-engine-schema-validator.md` | Движок | `SchemaValidator`, теги→`Value`, синтет. id + auto-binding |
| `02-ddl-and-builder.md` | DDL | table-scoped: schema в `create_table` / `set_table_schema` |
| `03-storage-catalogue.md` | Хранение | схема в записи таблицы; compile-on-open в facade; ALTER/DROP |
| `04-interning.md` | Интернирование | per-repo; клиент пакует id; де-интерн на open |
| `05-permissions.md` | Права | власть над таблицей; структурная грань |
| `06-client-rust-js-ts.md` | Клиент | кросс-язычные table-scoped билдеры |
| `07-testing.md` | Тесты | unit + rust-e2e + ts/js-e2e матрица |
| `09-builtin-checks.md` | Проверки | каталог: типы/функции/`foreign_key`/`unique` (фаза A/B/C) |

## Фазировка

- **Phase 0 — валидатор как узкая роль (`08-…`).** Трейты `RecordFields` (by-name; скаляры
  borrow, контейнеры owned) + `RecordValidator`; реестр → `Arc<dyn RecordValidator>`; миграция
  `NativeValidatorAdapter` (прямо) и wasm (через `WasmRecordValidator`-адаптер). Precursor —
  declarative строится поверх него.
- **Phase A — ядро по-табличной вертикали.** table-scoped DDL-op + builder + `SchemaValidator`
  (поверх `RecordFields`) + хранение в каталоге таблицы + compile-on-open (facade) + синтет.
  `RecordId` + auto-`ValidatorBinding` + authz=table + теги (string/int/f64/dec/bool/bin) +
  именованные ограничения. Полный тест-срез.
  Встроенные ЧИСТЫЕ проверки (numeric/string/collection/`array_of`/`one_of`/…) — `09-…`.
- **Phase B — функции.** Scalar-bridge (правило = ссылка на зарегистрированный скаляр,
  built-in funclib ИЛИ user) + `format` (email/url/uuid/date) + кросс-полевые (`compare`).
- **Phase C — реляционные (`09-…`), АСПИРАЦИОННА.** `foreign_key` и `unique` требуют НОВЫХ
  непостроенных примитивов: **tx-scoped read-only снапшот для валидатора** (НЕ `DbGateway` —
  тот autocommit + ре-ентрант-дедлок) + индекс-интеграция. Несётся отдельной санкцией, **не
  блокирует 0/A/B**.
- **Phase D+ — referential actions** (cascade/restrict при удалении ссылаемой строки) —
  отдельно, за санкцией.
