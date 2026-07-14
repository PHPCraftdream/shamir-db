בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# G.2 (B4) — Rust `Insert::row_idmsgpack()`

## Цель
Дать билдеру точку входа в id-keyed msgpack write-путь (v2-оптимизация),
сейчас недостижимую: `Insert::build()` хардкодит `records_idmsgpack: Vec::new()`.

## Заземление (file:line)
- `crates/shamir-query-builder/src/write/insert.rs` — `struct Insert` (:8) с
  полями `table_ref`, `values: Vec<QueryValue>`, `select`; `row()` (:42) пушит
  в `values`; `rows()` (:48); `build()` (:67) собирает `InsertOp` и **хардкодит
  `records_idmsgpack: Vec::new()`** (:71).
- Wire-поле: `InsertOp.records_idmsgpack: Vec<ByteBuf>`
  (`crates/shamir-query-types/src/write/types.rs:93`), `ByteBuf =
  serde_bytes::ByteBuf`, `#[serde(default, skip_serializing_if = "Vec::is_empty")]`.
  Семантика (из doc-комментария :85-96): каждый элемент — id-keyed storage
  msgpack ОДНОЙ записи; `values` и `records_idmsgpack` могут сосуществовать в
  одном op (разные записи), посемантически взаимоисключающи per-record.
- Реэкспорты write-модуля: `crates/shamir-query-types/src/write/mod.rs:11`
  (`ByteBuf` там пока НЕ реэкспортнут — добавить).
- Тест-образец билдера: `crates/shamir-query-builder/src/write/tests/write_tests.rs`
  (helper `assert_dto_wire`, импорты `InsertOp`, `mpack`, `rmp_serde`).
- End-to-end исполнение этого поля УЖЕ покрыто на уровне движка:
  `crates/shamir-engine/src/table/tests/s_write_server_tests.rs` (execute_insert_tx
  branch, indexed insert via records_idmsgpack, read-back). НЕ дублировать —
  задача чисто билдерная.

## Срез (3 правки)

### 1. Реэкспорт ByteBuf (query-types)
В `crates/shamir-query-types/src/write/mod.rs` добавить в re-export блок:
```rust
pub use serde_bytes::ByteBuf;
```
(ByteBuf — часть публичной поверхности `InsertOp.records_idmsgpack`, реэкспорт
законен; избегает нового dep у query-builder.)

### 2. Билдер-метод (query-builder, insert.rs)
- В шапку: `use shamir_query_types::write::{ByteBuf, InsertOp, InsertSelect};`
  (добавить `ByteBuf` к существующему импорту).
- Новое поле в `struct Insert`: `records_idmsgpack: Vec<ByteBuf>,`.
  Инициализировать `Vec::new()` во ВСЕХ конструкторах (`into`, `with_repo`).
- Метод (после `rows()`, до `returning_fields()`):
```rust
/// Append one record already encoded as id-keyed storage msgpack.
///
/// This is the pass-through write path for fully-literal, client-interned
/// records (v2 write optimization): `bytes` is one record's id-keyed
/// storage msgpack (what `query_value_to_storage_bytes` emits). Coexists
/// with `row()`/`rows()` — `values` and idmsgpack records are inserted in
/// the same op. Records with `$fn`/computed markers must use `row()`.
pub fn row_idmsgpack(mut self, bytes: impl Into<ByteBuf>) -> Self {
    self.records_idmsgpack.push(bytes.into());
    self
}
```
  (`ByteBuf: From<Vec<u8>>`, так что `impl Into<ByteBuf>` принимает `Vec<u8>`.)
- В `build()` заменить `records_idmsgpack: Vec::new()` на
  `records_idmsgpack: self.records_idmsgpack`.

## Тест (wire round-trip unit, write_tests.rs)
Добавить новой секцией. Проверить: (а) непустой `records_idmsgpack` после
`build()` и его выживание через msgpack round-trip как `bin`; (б) поле
отсутствует в wire когда не задано; (в) сосуществование `row()` + `row_idmsgpack()`.

```rust
/// `row_idmsgpack` carries id-keyed bytes through build() and the wire,
/// coexisting with literal `row()` records.
#[test]
fn insert_row_idmsgpack_wire() {
    let raw: Vec<u8> = vec![0x82, 0x01, 0xa5, 0x61, 0x6c, 0x69, 0x63, 0x65];
    let op = insert("users")
        .row(mpack!({ "id": 1 }))
        .row_idmsgpack(raw.clone())
        .build();
    assert_eq!(op.values.len(), 1);
    assert_eq!(op.records_idmsgpack.len(), 1);
    assert_eq!(op.records_idmsgpack[0].as_ref(), raw.as_slice());

    // Serializes as msgpack bin (0xc4), not seq-of-u8; round-trips equal.
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    assert!(bytes.contains(&0xc4), "idmsgpack must serialize as bin8: {bytes:x?}");
    let back: InsertOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(op, back);
}

/// Absent records_idmsgpack omitted from wire (skip_serializing_if).
#[test]
fn insert_no_idmsgpack_absent_in_wire() {
    let op = insert("users").row(mpack!({ "id": 1 })).build();
    assert!(op.records_idmsgpack.is_empty());
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let j: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert!(j.get("records_idmsgpack").is_none(), "must be absent when empty");
}
```
- Свериться с импортами в шапке write_tests.rs: `insert`/`InsertOp`/`mpack`/
  `QueryValue` — что-то уже есть (`use crate::write::*;`, `use shamir_types::mpack;`).
  Добавить недостающее в ШАПКУ (не в тело).

## Гейт (запусти, добейся зелёного)
- `cargo fmt -p shamir-query-builder -p shamir-query-types -- --check`
- `cargo clippy -p shamir-query-builder -p shamir-query-types --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-query-builder -p shamir-query-types -- idmsgpack`
  (затем оба крейта целиком без фильтра). Тесты ТОЛЬКО через `./scripts/test.sh`.

## Дисциплина (ОБЯЗАТЕЛЬНО)
- ⛔ НЕ используй agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER git reset/checkout/clean/stash/restore/rm или любую мутирующую git-команду.
  Только редактируй файлы. НЕ коммить — коммитит оркестратор.
- НЕ поднимай версии и НЕ добавляй новых внешних crate-зависимостей (реэкспорт
  serde_bytes идёт через УЖЕ существующий dep query-types). `use` только в шапке.
  Один файл = один export. Surgical changes — не трогай несвязанный код.
- Заверши финальным текстом: изменённые файлы + вывод гейта.

## Коммит (оркестратор, после zero-trust verify)
`feat(query-builder): G.2 B4 — Insert::row_idmsgpack`
