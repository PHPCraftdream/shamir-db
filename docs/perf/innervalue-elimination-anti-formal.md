בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Устранение InnerValue: формальность vs реальность, и долг конверсий

**Нормативная заметка (#61).** Записана после созерцания, в котором обнаружился
риск формального устранения. Дополняет `innervalue-elimination-plan.md`.

---

## 1. Ловушка: формальное устранение

`InnerValue = Value<InternerKey>` (id-ключевой) и `QueryValue = Value<String>`
(имя-ключевой) — **один generic-enum**. Поэтому существует соблазн «убрать
InnerValue», просто **переименовав тип в сигнатуре** и насадив конвертации
`inner_value_to_query_value` / `query_value_to_inner` на границах.

Это **хуже, чем ничего**: цель кампании — производительность (материализация
id-ключевого дерева была главным тормозом). Если мы убираем *имя типа*, но
добавляем конвертации (каждая = аллокация нового `Value` + возможный обход
интернера), мы **замедляем горячий путь** ради красивого grep-числа.

**Grep `InnerValue` — обманчивая метрика.** Она падает, даже когда конверсий
становится БОЛЬШЕ.

---

## 2. Правильная метрика

> **Число конвертаций `inner↔query` / `inner↔bytes` / `tree-decode` на ГОРЯЧИХ
> путях — и оно должно идти к нулю.**

Аудит (часть гейта):
```
grep -rnE "inner_value_to_query_value|query_value_to_inner|InnerValue::from_bytes|inner_to_json_value" \
  crates --include=*.rs | grep -vE "/tests/|_tests\.rs|/bench"
```
Каждое вхождение на per-op/per-row пути — подозреваемый. На холодных границах
(recovery-remap, write-mint, bare-scalar fallback) — допустимо и задокументировано.

---

## 3. Классификация этапов

**A. Подлинное устранение** (линза *заменяет* декод → конверсий МЕНЬШЕ):
- S3 (`get_many_bytes`→`RecordView`), S2 (changefeed lens), S9 (index lens-hash),
  M1 (doctor lens), M3 / byte-merge (write без дерева). ✅ Так и надо.

**B. Долг-создающие В ИЗОЛЯЦИИ** (type-flip без миграции окружения):
- **funclib (#75)**: ABI переведён на `QueryValue`, но окружение
  (filter-eval / compare / HAVING / computed-write) осталось InnerValue-native.
  funclib стал **островом QueryValue в InnerValue-море**, и на его берегах
  появились round-trip-конверсии. См. §4.

**C. Долг-гасящие** (миграция окружения → конверсии исчезают):
- **C6 (#80)** filter-eval/compare → QueryValue/lens, **S4 (#76)** agg-feed из
  линзы, **S8 (#77)** сужение `materialize_at`→RecordValue. Это НЕ довесок — это
  погашение долга, созданного B.

---

## 4. Конкретный долг funclib (на момент написания)

Round-trip `inner→query→funclib→query→inner`, которых раньше НЕ было:

| Сайт | Что | Гасит |
|---|---|---|
| `query/filter/resolve.rs:137,141` | `$fn` в фильтре: arg `InnerValue`→`QueryValue`, результат обратно | C6 (#80) |
| `table/write_helpers.rs:152,159` | computed-поле `$fn`: то же | C6/S8 |
| `query/read/aggregate.rs:508` | HAVING: `query_value_to_inner` | S4 (#76) |
| `aggregate.rs` agg-feed (fn_slots) | `&InnerValue`→`QueryValue` перед `accumulate` | S4 (#76) |

**Почему так:** funclib-аргумент берётся из `record.materialize_at`→`InnerValue`,
а сравнение/HAVING потребляют `InnerValue`. funclib (`QueryValue`) посередине →
конвертация на входе и выходе.

**Когда исчезнет:** когда `materialize_at` отдаёт `QueryValue`/scalar-ref (C3/S8),
а сравнение идёт по `QueryValue`/линзе (C6). Тогда `$fn`-аргумент строится из
линзы сразу в `QueryValue`, funclib его ест, результат сравнивается как
`QueryValue`. **Оба round-trip исчезают; конечное состояние — без InnerValue И
без лишних конвертаций.**

---

## 5. «Долина» — допустима, но только сквозная

Type-flip посреди миграции может ВРЕМЕННО поднять число конверсий (funclib —
ровно это). Это приемлемо **только как сквозной путь** к конверсий-free концу.
Бросить кампанию в долине = зафиксировать формальное устранение с замедлением.

**Правило дисциплины:**
1. Каждый этап обязан **net-reduce** конверсии на своём пути (или явно объявить
   себя долг-создающим B с указанием этапа-гасителя C).
2. **Нельзя засчитать #61 выполненным**, пока на горячих путях остаются
   `inner↔query` / `inner↔bytes` мосты. Проверять §2-аудитом, не grep-числом.
3. funclib (#75) **НЕ закрыт по духу**, пока C6 (#80) + S4 (#76) не погасят его
   боундари. Тип-имя убрано — долг открыт.

---

## 6. Допустимый холодный пол (НЕ долг)

Эти `InnerValue`/конверсии оправданы и остаются (документируются в S10 #79):
- tx-recovery `id_remap` (ремап id→id по природе на id-представлении),
- bare-scalar fallback (линза только map-root; legacy-скаляры),
- write-mint `query_value_to_inner_with`→storage-bytes (граница рождения id-байтов),
- index Dec/Big/контейнер-лист через один transient `materialize_at` (RecordRef
  отдаёт InnerValue; потребляется сразу, не персистится).

Холодная граница ≠ горячий долг. Путать их — тоже формальность, но наоборот.
