בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Слой: Phase 0 — by-name доступ к полям (`RecordFields`) + валидатор как узкая роль (`RecordValidator`)

> Precursor-рефактор. Затрагивает native/wasm валидаторы И функции прошлых задач. До Phase A.

## Сквозной инвариант (функции И валидаторы)

> **Ни один кусок пользовательского кода (функция, валидатор, скаляр) нигде не видит интерн-id.
> Всё — по именам полей.** Интернирование — внутренняя деталь движка/клиента.

Заземлено: function-facing API уже строго by-name — `FnBatch::put/get(key: &str)`,
`FnCtx`-globals `set/get(key)`, `keys() -> Vec<String>` (`context.rs:50-240`); `Params`
string-keyed; wasm host_db/host_batch интерн-id гостю не отдают. Автор функции уже не видит
интернирования. `RecordFields` (ниже) — **универсальный by-name примитив навигации записи для
функций И валидаторов**, закрывающий и навигацию (не только I/O).

## Корень проблемы

Паритет-кампания протащила валидаторы через ОБЩИЙ контракт функции
`ShamirFunction::call(&FnCtx, &FnBatch, &Params) -> QueryValue` (`contract.rs:24`), упаковывая
запись в `Params` = `TMap<String, QueryValue>` (owned; для wasm **сериализуется в msgpack** в
линейную память гостя — `wasm_function.rs:302`). Отсюда протечка: запись вынуждена быть
owned+сериализуемой ради wasm, native/declarative платят ту же цену, интернирование вылезает.

Валидатор — **не функция общего вида**, его форма у́же: `(запись, старая, ctx) → Validation`.
Дадим ему свой узкий контракт по форме записи.

## Два узких трейта

```rust
// Доступ к полям ПО ИМЕНИ — универсальный вход. Интернер скрыт от автора.
pub trait RecordFields {
    fn scalar(&self, path: &[&str]) -> Option<ScalarRef<'_>>;   // BORROW — тег/длина (основной кейс)
    fn str(&self, path: &[&str]) -> Option<&str>;               // BORROW
    fn present(&self, path: &[&str]) -> Option<Kind>;           // под present_kind_at; Kind — ГРУБАЯ категория
    fn materialize(&self, path: &[&str]) -> Option<InnerValue>; // OWNED — контейнеры (List/Map/Set), редко
}
// Kind = { Null, Scalar, Container, NonComparable } (kind.rs) — НЕ скалярный подтип.
// ВНИМАНИЕ: Dec/Big — msgpack EXT; на lens/stored-пути (ViewFields, materialize_at) они
// схлопываются в Bin (record_value.rs:145, messagepack.rs:256) → `Dec`/`Big` различимы ТОЛЬКО
// на OwnedFields (входящий `new` ещё несёт QueryValue::Dec до storage-кодирования). См. 01.

// Валидатор — узкая роль. НЕ ShamirFunction.
// РЕАЛИЗОВАНО (Phase 0, коммит 5b1955b): метод АСИНХРОННЫЙ (#[async_trait] —
// wasm требует async-вызов гостя), а `new` — Option (DELETE не имеет новой
// записи; присутствует только `old`).
#[async_trait]
pub trait RecordValidator: Send + Sync {
    async fn validate(
        &self,
        new: Option<&dyn RecordFields>,
        old: Option<&dyn RecordFields>,
        ctx: &ValidatorCtx<'_>,
    ) -> Validation;
}
```

`scalar`/`str` — **borrow** (под реальные `RecordRef::scalar_at -> ScalarRef<'_>` /
`str_at -> &str`, `record_ref.rs:39,54`, zero-copy для скалярных правил). `materialize` —
**owned** (под `RecordRef::materialize_at -> Option<InnerValue>`, `record_ref.rs:95`, для
контейнеров). Это снимает MAJOR ревью (borrow vs owned).

`ValidatorCtx` — узкий контекст: `actor` + **сужённый** доступ к интернеру. РЕАЛИЗОВАНО (Phase 0):
поле `interner` приватно, наружу торчит ЕДИНСТВЕННАЯ capability — `field_name(id) -> Option<Arc<str>>`
(де-интерн id→name для текста ошибки); declarative/user-валидаторы НЕ могут итерировать ключи,
интернировать новые имена или иначе достать полный `Interner`. **db-handle** (tx-scoped read-only
снапшот для реляционных проверок Phase C — FK; `09-…`) пока НЕ часть структуры — добавится в Phase C.
Не `FnCtx` (тот `Clone`, без лайфтайма).

## Каждый вид потребляет один by-name вход НАТИВНО

```
                 вход: &dyn RecordFields (by-name, поверх интернированной записи)
                                        │
   native-замыкание           declarative SchemaValidator          wasm-валидатор
   impl RecordValidator       impl RecordValidator                 WasmRecordValidator (адаптер)
   scalar()/str() — лениво    правила by-name — лениво              materialize→QueryValue→msgpack→гость
   де-интерна нет             де-интерна нет                       ← де-интерн ЗДЕСЬ, внутри адаптера
```

**Цена де-интерна+сериализации локализована в wasm-адаптер** — у границы ABI, где она
внутренне необходима. Платит **только wasm**. Native/declarative — by-name, без аллокации на
скалярных правилах. Автор (Rust-замыкание ИЛИ wasm-гость через guest SDK `IntoFieldPath`)
всегда пишет по имени; интернирование скрыто везде.

> **Важно про wasm:** wasm-валидаторы **уже** by-name в госте (guest SDK), хост де-интернирует
> за них — менять автору нечего. Неустранимо лишь то, что для wasm запись материализуется в
> `QueryValue`+msgpack (граница ABI) — теперь это инкапсулировано в `WasmRecordValidator`.

## Два backing'а `RecordFields`

- `ViewFields<'a>{ view: &RecordView, interner: &Interner }` — DELETE сейчас, INSERT/UPDATE
  (цель): `scalar(["a","b"])` → `interner.get_ind("a")…` → `RecordView::scalar_at(&[id…])`.
  Резолв имя→id **лениво, точечно**, без полного де-интерна.
- `OwnedFields<'a>{ qv: &QueryValue }` — транзитный fallback, где запись уже `QueryValue`
  (текущий INSERT/UPDATE `resolved_values`): прямой строковый лукап. Поведение идентично.

## Реестр и цикл

Валидаторный реестр держит **`Arc<dyn RecordValidator>`** (не `ShamirFunction`).
`run_validators_loop` строит `RecordFields` (ViewFields для DELETE; OwnedFields транзитно для
INSERT/UPDATE) и зовёт `validator.validate(new, old, ctx)` — без `Params`, без полного
де-интерна для native/declarative. wasm оборачивается в `WasmRecordValidator` при регистрации.

## План реализации

1. Трейты `RecordFields` (+ `ViewFields`/`OwnedFields`) и `RecordValidator` + `ValidatorCtx`
   (новый модуль в `shamir-engine`).
2. Реестр валидаторов: `Arc<dyn ShamirFunction>` → `Arc<dyn RecordValidator>`.
3. `run_validators_loop`: строить `RecordFields`, звать `.validate(...)`. DELETE → `ViewFields`
   (убрать `to_query_value`); INSERT/UPDATE → `OwnedFields` (TODO: `ViewFields`, когда
   write-path отдаст `RecordView`).
4. **Миграция прошлой задачи:** `NativeValidatorAdapter` → `impl RecordValidator` (by-name);
   `register_native(|fields, old, ctx| -> Validation)`. wasm → `WasmRecordValidator` (адаптер
   материализует запись внутри себя и зовёт гостя; гость не меняется). Адаптер сам собирает
   `FnCtx` (`with_actor` из `ValidatorCtx.actor` + engine/limits) и воспроизводит packing
   `record`/`old_record` в `Params` (`table_manager_validators.rs:245-255`) — guest-ABI неизменен.
5. declarative `SchemaValidator` → `impl RecordValidator` (`01-…`).
6. **Функции:** где функция навигирует ЗАПИСЬ (в `where`/`set`/keygen или запись-параметр) —
   отдавать тот же `&dyn RecordFields` (by-name, лениво поверх `RecordView`), не полный
   де-интерн. I/O-поверхность функций (`FnBatch`/`FnCtx`/`Params`) уже by-name — не трогаем.
   Скаляры в фильтрах получают уже извлечённые значения (компилятор достаёт поле по интерн-пути,
   скрыто от автора) — уже чисто.

## Тесты

**Unit:** `ViewFields.scalar(["a","b"])` by-name через интернер ↔ `scalar_at`; `materialize`
для контейнеров; `OwnedFields` строковый лукап; отсутствует → `None`.

**Rust e2e (миграция):** `native_parity_e2e` валидаторы переписаны на `RecordValidator`
by-name — те же accept/reject (регресс-гард); DELETE без полного `to_query_value`; wasm-путь
через `WasmRecordValidator` — те же результаты; `@server --full` зелёный.
