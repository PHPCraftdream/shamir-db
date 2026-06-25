בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# G.1 (B2) — Rust `FieldBuilder::one_of()`

## Цель
Паритет Rust-билдера с TS: value-enum констрейнт на поле схемы. Сейчас
`ConstraintsDto.one_of` есть на wire, но сеттера в Rust-билдере нет.

## Заземление (file:line)
- `crates/shamir-query-builder/src/ddl/schema.rs` — `impl FieldBuilder` (:39),
  цепочка constraint-сеттеров; образец `array_of` (:165), `compare` (:189).
  Хранит `self.constraints: ConstraintsDto`.
- Wire-поле уже есть: `ConstraintsDto.one_of: Option<Vec<QueryValue>>`
  (`crates/shamir-query-types/src/admin/types/schema_ops.rs:67`), с
  `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- `QueryValue` — из `shamir_types::types::value::QueryValue`. Крейт
  `shamir-types` уже в `Cargo.toml` (:19).
- TS-образец: `crates/shamir-client-ts/src/core/builders/ddl.ts:681` `oneOf()`.

## Срез (минимальный)
Добавить в `impl FieldBuilder` (после `array_of`, до Phase-B секции) метод:

```rust
/// Enum constraint: the field value must be one of these.
///
/// Mirrors the TS builder's `oneOf(values)`.
pub fn one_of<I, V>(mut self, values: I) -> Self
where
    I: IntoIterator<Item = V>,
    V: Into<QueryValue>,
{
    self.constraints.one_of = Some(values.into_iter().map(Into::into).collect());
    self
}
```

- Импорт `QueryValue` поднять в шапку файла (use-блок уже есть; добавь
  `use shamir_types::types::value::QueryValue;`). НЕ внутри функции.
- Проверь, что `impl Into<QueryValue>` существует для `&str`/`String`/`i64`
  (как в существующих тестах через `mpack!`). Если прямого `Into` для `&str`
  нет — приёмлемо принимать `impl IntoIterator<Item = QueryValue>` и в тесте
  передавать `vec![mpack!("active"), mpack!("archived")]` (см. образец
  `functional_args` в тестах). Выбери ту сигнатуру, что компилится чисто и
  эргономична; задокументируй выбор в doc-комментарии.

## Тест (wire round-trip unit)
Добавить в `crates/shamir-query-builder/src/ddl/tests/schema_ddl_tests.rs`
(там уже есть `roundtrip` helper и `mpack!`). Через `add_schema_rule` →
BatchOp → round-trip:

```rust
#[test]
fn field_one_of_wire() {
    let op = ddl::add_schema_rule("users")
        .rule(field(["status"]).string().one_of([mpack!("active"), mpack!("archived")]))
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["rule"]["one_of"], mpack!(["active", "archived"]));
}

/// `one_of` absent in wire when not set.
#[test]
fn field_one_of_absent_when_none() {
    let op = ddl::add_schema_rule("users")
        .rule(field(["status"]).string())
        .build();
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let j: shamir_types::types::value::QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert!(j["rule"].get("one_of").is_none(), "one_of must be absent when unset");
}
```
- `field` импортируется как `use crate::ddl::field;` или `use crate::ddl::schema::field;`
  — свериться с тем, как уже импортирован `ddl` в файле (`use crate::ddl;`),
  можно звать `ddl::field([...])` если реэкспортнут; иначе добавь корректный
  импорт в шапку теста.
- Точную форму сигнатуры (`Into<QueryValue>` vs `QueryValue`) согласуй между
  методом и тестом, чтобы компилилось.

## Гейт (запусти и убедись, что зелено)
- `cargo fmt -p shamir-query-builder -- --check`
- `cargo clippy -p shamir-query-builder --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-query-builder -- one_of`  (затем без фильтра весь крейт-lib)
  — тесты ТОЛЬКО через `./scripts/test.sh`, raw `cargo test` заблокирован.

## Дисциплина (ОБЯЗАТЕЛЬНО)
- ⛔ НЕ используй agent/sub-agent инструмент — он падает context-canceled.
  Читай файлы напрямую (view/grep/edit).
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
  или любую git-команду, мутирующую рабочее дерево/индекс. Только редактируй
  файлы; коммитит оркестратор.
- `use` только в шапке файла. Один файл = один export. Surgical changes —
  не трогай несвязанный код/комментарии.
- НЕ поднимай версии. НЕ коммить сам.

## Коммит (делает оркестратор после zero-trust verify)
`feat(query-builder): G.1 B2 — FieldBuilder::one_of`
