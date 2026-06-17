בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# R5 — прямой де-интернинг результата чтения с линзы (RecordView → QueryValue/JSON)

Дополняет `recordview-campaign-status.md`. Это **последняя горячая
материализация `InnerValue` на чтении** — зеркальный близнец доказанного
write-энкодера (`query_value_to_storage_bytes`).

---

## 1. Тормоз (что убираем)

Result-encode `SELECT *` сейчас (`select_projection.rs:125` `project_value`,
`:77` `project`) на `is_all` делает:

```
record.for_each_field(|k,v| m.insert(k,v))  →  InnerValue::Map(m)  →  inner_value_to_query_value(&whole)
```

А `RecordView::for_each_field` (`record_ref.rs:321`):

```rust
for (key, _val) in self.fields() {        // walk #1: читает (key, RecordValue), val ВЫБРАСЫВАЕТСЯ
    if let Some(iv) = self.materialize_at(&[key]) {   // walk #2: value_bytes_at РЕ-СКАНИТ с начала + InnerValue::from_bytes
        f(k, iv);
    }
}
```

Итог на запись из N полей:
- **O(N²)** — `value_bytes_at` линейно сканит запись с начала на КАЖДОЕ из N полей;
- полный **build** промежуточного id-дерева `InnerValue::Map`;
- полный **повторный walk** дерева в `inner_value_to_query_value` для де-интернинга.

Три прохода + квадрат. Это и есть «внутренняя конвертация», которую на скане и
записи мы уже убрали, а тут она переехала в проекцию.

## 2. Решение (один O(N) проход)

Walk линзы `view.fields()` **один раз**, каждый `RecordValue` → `QueryValue`
напрямую, ключ id→имя резолвится инлайн через reverse-snapshot (как в
`inner_value_to_query_value_with_rev`). Никакого `InnerValue::Map`, никакого
повторного walk, никакого re-scan.

### Инвариант byte-identity (почему это безопасно)
Storage НЕ содержит `Dec/Big/Set` — энкодер сериализует их как `Str`/seq
(`value.rs:71-88`). Поэтому и текущий путь `InnerValue::from_bytes →
inner_value_to_query_value`, и линза дают РОВНО один и тот же набор вариантов
(`Str` там, где был Dec/Big; `List` там, где был Set). Прямой walker
`RecordValue → QueryValue` обязан совпасть **байт-в-байт** с текущим выводом.
Доказывается parity-батареей (см. §5).

Маппинг `RecordValue → QueryValue` (и `→ json::Value`):
| RecordValue | QueryValue | json::Value |
|---|---|---|
| Null/Bool(b)/Int(i)/F64(f) | те же | как `inner_to_json_value` (F64 non-finite → Str) |
| Str(Cow) | `Str(s.into_owned())` | `String(...)` |
| Bin(&[u8]) | `Bin(b.to_vec())` | массив байт-чисел (как `inner_to_json_value`) |
| Arr(RawSeq) | `List` (рекурсия по `RawSeq::iter`) | `Array` |
| Map(RecordView) | `Map` (рекурсия) | `Object` |

## 3. Дизайн (seam-consistent, как C0–C6)

**Новые функции** в `crates/shamir-types/src/codecs/interned/json.rs`
(дом де-интернинга):
- `record_view_to_query_value(view: &RecordView, interner: &Interner) -> Result<QueryValue, CodecError>`
  + приватный `_with_rev(view, rev: &[Option<UserKey>]) -> Result<QueryValue>` (рекурсивный).
- `record_view_to_json_value(view, interner) -> Result<json::Value, CodecError>`
  + `_with_rev`. Арм-в-арм зеркалит `inner_to_json_value_with_rev` (F64/Bin-семантика идентична).
- (бонус, если просто) `RecordViewRef<'a>: Serialize` — потоковый аналог
  `InternedRef`, walk байтов линзы → любой serde-формат; даёт прямой
  `record_view_to_json(interner, view) -> Vec<u8>` для байтового fast-path.

**Расширение шва** `RecordRef` (`record_ref.rs`) — чтобы проекция осталась
generic (`&impl RecordRef`):
- `fn to_query_value(&self, interner: &Interner) -> QueryValue;`
- `fn to_json_value(&self, interner: &Interner) -> serde_json::Value;`
- impl для `InnerValue`: делегирует существующим
  `inner_value_to_query_value` / `inner_to_json_value` (`.unwrap_or(Null)`).
- impl для `RecordView`: новые `record_view_to_*` walkers.

**Перепроводка** `select_projection.rs`:
- `project_value` is_all (`:125-132`): `record.to_query_value(interner)`
  вместо `for_each_field`→`InnerValue::Map`→де-интернинг.
- `project` is_all (`:78-85`): `record.to_json_value(interner)`.
- per-field и funcs ветки НЕ трогаем (они `materialize_at` единичные листы —
  малый выигрыш, вне scope; как index-key leaf).

## 4. Scope

IN: `record_view_to_query_value`/`record_view_to_json_value` (+`_with_rev`),
два метода `RecordRef`, перепроводка двух is_all-веток, parity-тесты.
OUT (отдельные таски): per-field/funcs ветки; `apply_select_to_bytes`
байтовый fast-path на `&[(RecordId, InnerValue)]` (если ещё живой — отдельно,
через `RecordViewRef: Serialize`); AGG/GROUP BY `RecordCow::Owned`; W3-merge.

## 5. Тесты (byte-identity, обязательны)

`crates/shamir-types/src/record_view/tests/deintern_parity_tests.rs` (через
`tests/mod.rs`): батарея значений (скаляры incl. Int/F64/Str/Bool/Null/Bin,
вложенные map, list, list-of-map, unicode-ключи, U64>i64::MAX edge, пустые
map/list). Для каждого: построить storage-байты, и проверить

```
record_view_to_query_value(&RecordView::new(&bytes), interner)
  == inner_value_to_query_value(&InnerValue::from_bytes(&bytes), interner)
```

и аналогично для `_json_value` (== `inner_to_json_value(&from_bytes)`).
То есть линза-путь ≡ дерево-путь, на каждом shape.

## 6. Гейт

- `cargo fmt -p shamir-types -p shamir-engine -- --check` → 0
- `cargo clippy -p shamir-types -p shamir-engine -- -D warnings` → 0 (узко; workspace — оркестратор)
- `./scripts/test.sh -p shamir-types -- deintern_parity` → 0 fail
- `./scripts/test.sh @engine -- projection` → 0 fail (проекция не сломана)

## 7. Выигрыш

`SELECT *`: O(N²)+три-прохода → один O(N) проход; минус промежуточное
id-дерево записи на строку. Прямой аналог write-энкодера; закрывает последнюю
горячую `InnerValue`-материализацию на чтении.
