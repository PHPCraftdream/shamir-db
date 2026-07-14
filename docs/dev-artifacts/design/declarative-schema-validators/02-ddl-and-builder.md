בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Слой: DDL — table-scoped + интроспекция + модель обновления

## Предметная область

Declarative-схема — **свойство таблицы**, её DDL **табличная** (НЕ глобальная
`create_validator` — та для wasm/native). Табличные ops — `crates/shamir-query-types/src/admin/
types/`, билдер — `crates/shamir-query-builder/src/ddl/`. **Правило: всегда билдер, не raw
`serde_json`.** Пути — плоскими именами; интернирование клиентский слой (`04-…`).

## A. Интроспекция — «получить список валидаторов, имена и тексты хуков»

Две поверхности (code-валидаторы глобальны; declarative — свойство таблицы):

### Глобальные code-валидаторы (wasm/native)
```rust
ListValidatorsOp { list_validators: true }
  → [ { name, kind: "wasm"|"native", lang, source: Option<String>, wasm_hash, bound_in:[tables] } ]
GetValidatorOp { get_validator: name }
  → полная запись (для редактирования), включая source
```
`source` — **текст хука (Rust-код)** для wasm, созданных из `source`. Честно: wasm из готовых
байтов → `source=None` (есть `wasm_hash`); **native → `source=None`** (замыкание вкомпилировано
в хост-бинарь, текста нет — только имя/kind). Это и есть «названия и тексты хуков, где они
есть».

### Declarative-схема таблицы
```rust
GetTableSchemaOp { get_table_schema: table, repo }
  → { schema: [ {path:[names], type, constraints…} ], schema_version }   // пути ДЕ-интернированы в имена
```
Схема читается из каталоговой записи таблицы (`03-…`), не из `list_validators`.

## B. Модель обновления — продумано

### Declarative-схема (массив правил) — четыре примитива:
| Намерение | Операция | Семантика |
|---|---|---|
| **всем списком** | `SetTableSchemaOp{ repo, table, schema:[…], expected_version? }` | полная **замена** (overwrite, НЕ merge); декларативно «вот вся схема» |
| **очистить** | `SetTableSchemaOp{ schema: [] }` | убрать все правила |
| **добавить ещё** | `AddSchemaRuleOp{ repo, table, rule }` | upsert по `path` (есть путь → заменить правило, нет → добавить) |
| **удалить 1** | `RemoveSchemaRuleOp{ repo, table, path }` | удалить правило для пути |

**Какая лучше?** — оба класса нужны, по сценарию:
- **Whole-replace (`set_table_schema`) — основная**: декларативна, идемпотентна, подходит для
  «схема как данные»/миграций. Минус — потерянное обновление при read-modify-write двух клиентов
  → защищаем `expected_version` (монотонный `schema_version` в записи таблицы). Проверка version
  **атомарна под per-table lock** (`save_table_meta` пишет всю Map, не CAS — `03-…`); mismatch →
  `version_conflict`. Это «optimistic concurrency».
- **Surgical (`add`/`remove` по path) — для безопасных инкрементальных правок** без
  read-modify-write гонки (две правки разных полей не клобберят друг друга).

Все четыре → переписывают `schema` в записи таблицы + RCU-replace артефакта + bump
`schema_version` (`03-…`).

### Code-валидаторы (глобальные) — существующий жизненный цикл:
add one = `CreateValidatorOp`; delete one = `DropValidatorOp`; update one =
`CreateValidatorOp{replace:true}` / `RenameValidatorOp`; «clear all» намеренно НЕТ (код дропается
по-объектно, осознанно); привязка к таблицам — `Bind/UnbindValidatorOp`.

### Набор валидаторов НА таблице (bindings):
list = `GetTableSchemaOp` (declarative) + binding'и таблицы (code, с `kind`/`priority`); add =
`bind`; remove 1 = `unbind`; clear (code) = unbind все (declarative-схема убирается `clear`-ом
выше).

## Форма правил (DTO) и билдер

```rust
pub struct FieldRuleDto {
    pub path: Vec<String>,                   // плоские ИМЕНА (интернируются клиентом, 04-…)
    pub r#type: String,                      // "string"|"int"|"f64"|"dec"|"bool"|"bin"|… (01-… → Value)
    #[serde(flatten)] pub constraints: ConstraintsDto,  // max/min/len/required/nullable/unsigned
}
```

```rust
create_table("users").in_repo("main").schema([
    field(["email"]).string().max(255).required(),
    field(["age"]).int().min(0).max(150),         // .int().min(0) = «u64-намерение»
]);
set_table_schema("main","users").rules([ … ]).expected_version(3);   // whole-replace + optimistic
add_schema_rule("main","users").rule(field(["nickname"]).string().max(64));
remove_schema_rule("main","users").path(["nickname"]);
get_table_schema("main","users");                                   // интроспекция
list_validators();                                                  // глобальные code, с source
```

## План реализации

1. `query-types`: `CreateTableOp += schema`; `SetTableSchemaOp{…,expected_version?}`,
   `AddSchemaRuleOp`, `RemoveSchemaRuleOp`, `GetTableSchemaOp`; расширить `ListValidatorsOp`/
   ввести `GetValidatorOp` (возврат `source`/`kind`/`bound_in`); DTO `FieldRuleDto`/
   `ConstraintsDto`.
2. Сервер: исполнить ops → запись/чтение `schema` в каталоговой записи таблицы (`03-…`);
   `expected_version` mismatch → `version_conflict`; authz — `ResourcePath::table` (`05-…`).
3. Билдер: `create_table().schema()`, `set_table_schema().rules().expected_version()`,
   `add_schema_rule()`, `remove_schema_rule()`, `get_table_schema()`, `list_validators()`,
   `field(...)` fluent.
4. Валидация: нераспознанный тег → `unknown_type`; конфликт `len`+`min/max` → `invalid_input`.

## Тесты

**Unit** (`query-builder/.../ddl/tests/`): каждый op → корректный wire-op (serde round-trip);
`field(...)` покрывает теги/ограничения; `expected_version` сериализуется; nested path как
`["a","b"]`.

**Rust e2e / ts-js e2e** — `07-…`: whole-replace / add / remove / clear / get_table_schema;
`list_validators` возвращает source для wasm-из-source и `None` для native; `version_conflict`
при устаревшем `expected_version`.
