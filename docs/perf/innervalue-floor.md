בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Неустранимый InnerValue-пол — честный итог #61 (S10)

Капстоун кампании. После `E1–E6` + `I1` + `S9b` production-`InnerValue`
сведён к **четырём неустранимым категориям**. Главное: **ноль id-ключевой
материализации дерева на ГОРЯЧИХ путях** (read-result / filter / aggregate /
streaming — все линз-нативные: `RecordView`-walk, `ScalarRef`-compare,
`apply_select_value_bytes`). То, что осталось — это **сам тип-библиотека**,
**персистированные byte-identity якоря**, **recovery**, и **owned-value
API-поверхность** там, где движок ВЛАДЕЕТ значением, а не СМОТРИТ на него.

Дополняет: `innervalue-elimination-stages.md` (микро-этапы),
`innervalue-elimination-anti-formal.md` (принцип §5b),
`innervalue-elimination-contemplation.md` (форма дуги).

---

## Динамика кампании

| Точка | Production `InnerValue` |
|---|---|
| Старт (до устранения) | ~1004 |
| После Wave 1 / S3 / S4 / C6 | ~692 |
| **После E1–E6 + I1 + S9b (сейчас)** | **~654** |

Счётчик не идёт в ноль — и **не должен** (см. «Почему не ноль» ниже). Метрика
анти-формального принципа — **конверсии/материализации на горячем**, а не grep.

---

## Карта пола (по категориям)

### Категория 1 — Библиотека типа (~155, `shamir-types`)

Сам enum `Value<Key>` + его кодеки + линза. `InnerValue = Value<InternerKey>`
— одна инстанциация zero-cost generic; снятие generic отвергнуто
(`innervalue-elimination-stages.md` §1: zero-cost, нужны обе ключевости).

| Файл | ~Узлов | Роль |
|---|---|---|
| `codecs/interned/messagepack.rs` | 59 | storage-кодек (msgpack ↔ InnerValue) |
| `record_view/record_ref.rs` | 42 | trait `RecordRef` + `materialize_at` escape-hatch + impl на InnerValue |
| `codecs/interned/legacy.rs` | 35 | legacy text codec (v1-inbound `QueryRecord::Legacy` + control-plane) |
| `record_view/scalar_ref.rs` | 9 | `ScalarRef` ↔ InnerValue мост |
| `types/value.rs`, `types/repo_record.rs` | 4 | сам enum + repo-record |

**Неустранимо по определению**: это дом типа. Кодек обслуживает якоря;
`materialize_at` — документированный escape-hatch трейта (контейнеры/Dec/Big).

### Категория 2 — Персистированные byte-identity якоря (~84, `shamir-index`)

Хеши index-постингов персистированы; их байт-раскладка = on-disk
совместимость. Менять = ломать формат (теперь чинится авто — S9b).

| Файл | ~Узлов | Роль |
|---|---|---|
| `legacy/index_keys.rs` | 16 | hash-схема постингов (S9, стабильные u8-теги) |
| `legacy/sorted_index_manager.rs` | 13 | sorted-index постинги |
| `legacy/index_manager(_unique).rs` | 15 | hash/unique постинги |
| `expr.rs` | 21 | functional-index eval — **owned computed value** (I1) |
| `functional_backend.rs` | 15 | FUNCTIONAL posting-hash, byte-identity (I1) |
| `vector/`, `fts_ranked` | 4 | vector/FTS бэкенды |

Lookup-путь принимает `&[InnerValue]`; write-путь хеширует с линзы, материализуя
InnerValue ТОЛЬКО для Dec/Big/контейнеров (`materialize_at`). V1→V2 миграция
автоматическая (**S9b** rebuild-on-open).

### Категория 3 — Recovery-якоря (~18, `shamir-tx`)

Crash-recovery переигрывает staged id-ключевые байты — линза неприменима
(на этапе recovery ещё нет имя-контекста).

| Файл | ~Узлов | Роль |
|---|---|---|
| `id_remap.rs` | 15 | overlay-id → base-id remap при commit/recovery (id-ключевой по природе) |
| `staging_store.rs` | 3 | staged Set/Remove байты ↔ значение |

### Категория 4 — Engine owned-value границы §5b (~290, engine + server/db/funclib)

Там, где движок **ВЛАДЕЕТ** значением, а не смотрит на него:

| Граница | Файлы | Почему owned |
|---|---|---|
| **Point-read API** | `table.rs`, `table_manager_crud.rs`, `table_manager_tx_ops.rs`, `table_manager_streaming.rs` (`get`/`get_many`/`read_one_tx` → `InnerValue`) | API возвращает владеемое значение; смена сигнатуры = ripple по всем callers (анти-паттерн) |
| **Write-mint** | `write_exec.rs`, `write_helpers.rs` | записи минтятся как InnerValue до storage-encode |
| **Aggregator cross-row** | `aggregate.rs` (`OwnedExtreme` Min/Max, S4) | владеемо, т.к. переживает между строками |
| **Filter/index адаптеры** | `resolve.rs` (`filter_value_to_inner` ← read_planner index-bound; `resolve_filter_value` ← legacy-twin) | документировано E5/E6 |
| **Bare-scalar fallback** | `doctor.rs`, `read_*` | decode InnerValue, когда `RecordView::new` падает на non-map записи |
| **funclib / changefeed** | `funclib`, `server/subscriptions/filter_eval.rs`, `db/changelog.rs` | owned-computed аргументы/результаты, changefeed-вход |

Бóльшая часть уже несёт инлайн-§5b-обоснования (E5: resolve.rs; I1:
expr/functional_backend; S9: index_keys).

---

## Чего здесь НЕТ — это и есть победа

**Ноль `InnerValue` на горячих путях результата/фильтра/агрегата/стрима:**
- read-result → `apply_select_value_bytes` / `RecordView`-проекция;
- filter → `resolve_filter_query` (QueryValue) + `ScalarRef`-compare;
- aggregate → `ScalarRef`-шаги + bytes-фид (S4);
- streaming → `RecordView`-линза (E3 убрал мёртвый InnerValue-стрим);
- temporal history → `apply_select_value_bytes` (E4).

Кампания увела id-ключевую материализацию дерева С КАЖДОГО горячего пути.
Остаток — библиотека типа + холодные якоря + owned-value API-поверхность.

---

## Почему не ноль и не de-generify

Анти-формальный принцип (`innervalue-elimination-anti-formal.md`): убрать ИМЯ
типа, ДОБАВИВ конверсии — это регрессия-театр. `Value<K>` —
zero-cost-монорфизация; нужен и для name-keyed (`QueryValue`), и для id-keyed
(`InnerValue`) якорей. `InnerValue` выживает как **честная owned-валюта на
оправданных границах**. **#61 закрывается этим задокументированным прибытием,
а не вынужденным нулём.**

בְּעֶזְרַת הַשֵּׁם — дуга завершена честно.
