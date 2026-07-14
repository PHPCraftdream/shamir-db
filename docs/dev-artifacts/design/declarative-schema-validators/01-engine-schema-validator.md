בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Слой: Движок — `SchemaValidator`

## Предметная область

Валидаторы — **узкая роль** `RecordValidator` (Phase 0, `08-…`):
`validate(new: &dyn RecordFields, old, ctx) -> Validation`, поля читаются **ПО ИМЕНИ** через
`RecordFields` (интернирование скрыто). Реестр держит `Arc<dyn RecordValidator>`,
`run_validators_loop` итерирует `ValidatorBinding{validator_id: RecordId, ops, priority}` и
резолвит `reg.get_by_id`. Возврат `Validation` кодируется `validation_to_query_value` /
декодируется `decode_validation_result` (`validator/encode.rs`, `decode.rs`).

Declarative-схема — `impl RecordValidator` (НЕ `ShamirFunction`). Хранится в записи таблицы
(`03-…`), регистрируется под персистентным `RecordId` (`schema_validator_id`, `03-…`) с
авто-`ValidatorBinding`.

## Цели

- Скомпилировать правила в `Arc<dyn RecordValidator>`, авто-bound к своей таблице.
- Навигация **по имени** через `RecordFields` (скаляры — borrow `scalar()`/`str()`, без
  аллокации; контейнеры — `materialize()`).
- Композируемость: declarative + bound wasm/native на одной таблице — по `priority`.
- Embedded без бойлерплейта.

## Форма правила — теги отображены на модель `Value`

`Value` = `Null, Bool, Int(i64), F64, Dec(Decimal), Big(BigInt), Str, Bin, List, Set, Map`.
Отдельного `U64`/`Bytes` НЕТ.

> **Ограничение `Dec`/`Big`:** в storage это msgpack EXT, и lens (`RecordValue::Bin`), и
> `materialize_at` схлопывают их в `Bin` (`record_value.rs:145`, `messagepack.rs:256`). Значит
> тег `Dec`/`Big` различим ТОЛЬКО когда `new` приходит как `QueryValue` (`OwnedFields` —
> текущий INSERT/UPDATE до storage-кодирования). На lens-пути (`ViewFields`, цель для
> `old`/будущего `new`) `Dec`/`Big` неотличимы от `Bin`. Решение: ограничить `TypeTag::Dec/Big`
> входящим-`new` (OwnedFields) ЛИБО валидировать их как `Bin`-форму. В матрице тестов (`07-…`)
> «различение Dec/Big» помечается этим ограничением.

```rust
pub struct FieldRule { pub path: Vec<String>, pub ty: TypeTag, pub constraints: Constraints }
pub enum TypeTag { String, Int, F64, Dec, Bool, Bin, List, Map, Set, Null, Any }
pub struct Constraints {
    pub required: bool, pub nullable: bool,
    pub min: Option<Num>, pub max: Option<Num>, pub len: Option<u64>,
    pub unsigned: bool,                 // «u64-намерение» = Int + unsigned (Int>=0), не отдельный тип
    pub one_of: Option<Vec<Value>>,     // enum / const (один элемент)
    // Phase B: pub scalar: Option<ScalarRef>;   Phase C: pub foreign_key/unique  (09-…)
}
```

`SchemaValidator { rules: Vec<FieldRule> } impl RecordValidator`:

```rust
fn validate(&self, new: &dyn RecordFields, _old, _ctx) -> Validation {
    let mut v = Validation::new();
    for rule in &self.rules {
        let p: Vec<&str> = rule.path.iter().map(String::as_str).collect();
        match new.present(&p) {                                   // Option<Kind> (грубая категория)
            None if rule.constraints.required => v.field_error(rule.path.clone(), "missing_required"),
            None => {}
            Some(Kind::Null) if rule.constraints.nullable => {}
            Some(Kind::Null) => v.field_error(rule.path.clone(), "null_not_allowed"),
            Some(_) => rule.check(new, &p, &mut v),                // тег: scalar()/str() (точный тип) + ограничения
        }
    }
    v
}
```

`Validation::field_error(Vec<String>, code)` — `ValidationError.field: Option<Vec<String>>`
(имена). Коды: `type_mismatch, too_long, too_short, out_of_range, missing_required,
null_not_allowed, wrong_length, not_in_enum`. Скалярные правила используют `scalar()`/`str()`
(borrow, без аллокации); контейнерные (`list`/`map`/`set` + длина) — `materialize()`.

## Регистрация, id и авто-binding

`run_validators_loop` зовёт ТОЛЬКО то, что есть в `validator_bindings` И резолвится
`get_by_id`. Поэтому declarative:

1. Компилируется в `Arc<dyn RecordValidator>`.
2. Регистрируется под **персистентным `RecordId`** = `schema_validator_id` из записи таблицы
   (генерится `RecordId::new()` при первой компиляции, хранится в каталоге, восстанавливается на
   reopen — `03-…`; НЕ детерминированный-из-имени: `RecordId::system` режет до 12 байт →
   коллизия). Имя в реестре — `"__schema__/<repo>/<table>"` (уникальность — `registry.rs:64`).
3. Авто-`ValidatorBinding{ ops: все write-ops, priority: 500 }` — **зарезервированный
   поддиапазон `[1..999]`** под системные авто-валидаторы (пользовательский — `[1000,9999]`,
   `validator_management.rs:442`), схема бежит ПЕРВОЙ; tie-break — по стабильному
   `validator_id`.

ALTER → RCU `replace_artifact` под тем же id. DROP таблицы → `remove_binding` + `reg.remove(id)`.

## Embedded-API без бойлерплейта

```rust
db.table("repo","users").set_schema([
    rule(["email"]).string().max_len(255).required(),
    rule(["age"]).int().min(0).max(150),          // .int().min(0) = «u64-намерение»
    rule(["address","zip"]).string().len(5),
])?;
```

`rule(path)` — `IntoFieldPath`; типовые методы → `TypeTag`, ограничители → `Constraints`. Без
ручных `FieldRule{…}`.

## План реализации

1. Типы `TypeTag`/`Constraints`/`FieldRule` (`crates/shamir-engine/src/validator/schema/`).
2. `SchemaValidator impl RecordValidator` поверх `RecordFields` (Phase 0).
3. `from_rules(...)` — парс каталоговой формы (`03-…`), путь id→name на этом шаге.
4. Регистрация под `schema_validator_id` + авто-binding `priority=500`; RCU-replace на ALTER.
5. Fluent rule-builder + `table(...).set_schema(...)`.

## Тесты

**Unit** (`validator/schema/tests/`): каждый тег vs `Value`-вариант (вкл. `Dec`/`Big`/`Bin`/
`Set`); `Int+unsigned`; ограничения (границы); `one_of`; вложенный путь by-name; required/
nullable; накопление ошибок; пустая схема. **e2e** — `07-…`.
