בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# InnerValue elimination — staged plan (#61)

Решение пользователя: **избавиться от id-ключевого `InnerValue` (`Value<InternerKey>`)
из production везде, где физически возможно** — не «принять как валюту». Этот док —
этапный, identity-gated, agent-sized план (design-пасс @aoh).

Дополняет `recordview-campaign-status.md` (общая картина) и
`endgame-msgpack-passthrough.md` (north-star wire).

---

## Суть

`InnerValue = Value<InternerKey>` (id-ключевой) и `QueryValue = Value<String>`
(имя-ключевой) — **один generic-enum `Value<K>`**. «Устранить InnerValue» =
устранить материализацию id-ключевого дерева; сам enum выживает как `QueryValue`.
Значение живёт либо в storage-байтах через линзу `RecordView` (`RecordRef`), либо
как имя-ключевой `QueryValue`. Удаляем **alias `InnerValue`** и его production-узлы,
кроме холодных границ (recovery-remap, write-mint, bare-scalar legacy, опц. index-leaf).

## Поле боя (production, не-тест)

5 крейтов: shamir-engine 360 · shamir-funclib 255 · shamir-types 202 ·
shamir-index 110 · shamir-server 40 · shamir-tx 21. (wasm-host/db/connect —
в production уже 0: 8/7/1 были comment/test-import.)

## Кластеры (production InnerValue → цель устранения)

| # | Кластер | Цель | Устранимо? |
|---|---|---|---|
| C1 | `Value<K>` enum + serde/Hash | **IRREDUCIBLE** — выживает как QueryValue; удаляем alias | — |
| C2 | id-ключевые кодеки (messagepack/legacy tools) | bytes/lens для чтения; delete deprecated tools.rs; `query_value_to_inner` (write-mint) остаётся | partial |
| C3 | `RecordRef` leaf API (`materialize_at`→InnerValue) | `RecordValue`/`ScalarRef` для скалярных листьев | partial |
| C4 | engine `get`/`get_many` валюта (bulk 360) | `get_bytes`/`get_many_bytes` + линза у callers | **ДА** |
| C5 | агрегаторы (`AggState::Min/Max{&InnerValue}`) | leaf-bytes/ScalarRef borrow; после C4 | ДА (lifetimes) |
| C6 | filter eval / resolve ($fn args) | следует за C7 (funclib ABI) | ДА, связан с C7 |
| C7 | **funclib ABI** `fn(&[InnerValue])->InnerValue` | alias → `QueryValue` (имя-ключевой) | ДА (type-churn) |
| C8 | **index hash-leaf + covering** (persisted) | **decision gate S0** | conditional |
| C9 | **server changefeed filter-eval** (новый кластер) | `RecordView`/bytes; кэш Bytes не дерево | ДА |
| C10 | tx recovery `id_remap` + `as_inner` | **irreducible** (мутирует id-ключи на recovery) | нет |
| C11 | bare-scalar fallback (write-exec/doctor/record_cow) | **irreducible-ish** (линза только map-root; legacy скаляры) | нет |

shamir-types 202: IRREDUCIBLE ядро = `Value<K>` enum+serde (~60, → QueryValue);
REMOVABLE = `inner_to_legacy*`, deprecated `legacy/tools.rs`, InnerValue-сигнатуры
`materialize_at`/`for_each_field`.

## Этапы (dependency-ordered, identity-gated)

| Этап | Кластер | Суть | Identity / golden | Сложность |
|---|---|---|---|---|
| **S0 (gate)** | — | **Решение пользователя**: index-anchor (a) frozen-hasher vs (b) format-rebuild. Без кода. | — | trivial |
| S1 | C2 | Delete deprecated `codecs/legacy/tools.rs` (tests-only) + re-exports | compile-only | trivial |
| S2 | C9 | server changefeed: кэш `Bytes`, filter-eval через `RecordView` | subscription-match parity | medium |
| S3 | C4 | `get_bytes`/`get_many_bytes`; read_index_scan/read_exec → линза | read result parity (recordview_cutover_parity) | hard |
| S4 | C5 | агрегаторы на leaf-bytes/ScalarRef (вход = bytes из S3) | agg byte-identity (agg_prune_byte_identity) | hard (lifetimes) |
| S5 | C7-1 | funclib `registry.rs`/`agg.rs`: alias InnerValue→QueryValue | compile gate | trivial |
| S6 | C7-2 | 12 funclib-категорий: механ. InnerValue→QueryValue (**parallel, disjoint**) | per-fn value-equality | medium (parallel) |
| S7 | C6+C7-3 | callers: resolve.rs args, aggregate feed, write_helpers, wasm-host argon2 | $fn/computed/agg parity | medium |
| S8 | C2/C3 | сузить `materialize_at`→RecordValue; удалить мёртвые inner_to_legacy* | no prod caller | medium |
| **S9** | C8 | **по S0**: (b) bump формата + rebuild-on-open + lens-native hash + covering→QueryValue; ИЛИ (a) discriminant-stable hasher + frozen golden + rustc-canary | **PERSISTED byte-identity** (a) или migration parity (b) | **hard (highest)** |
| S10 | C10/C11 | аудит+документировать холодный пол (recovery/mint/bare-scalar) | doc-пасс | trivial |

Параллельность: S1∥S2 (разные крейты); S5→S6 барьер, затем S6 веером по 12 файлам;
S3→S4 серийно (lifetimes); S9 серийно, gated на S0. **Persisted byte-identity
(серийно, high-care): только S9.**

## Честный конечный итог

Production `Value<InternerKey>` материализация **не ноль**, но id-ключевая валюта
устранена; alias выживает в малом названном холодном остатке:

1. **tx `id_remap`** (~16) — recovery-кодек ремапит InternerKey→InternerKey, по
   определению на id-представлении. **Irreducible.**
2. **bare-scalar fallback** (~10) — линза только map-root; legacy-скаляры нужен tree.
3. **`query_value_to_inner` write-mint** — граница рождения id-байтов (один энкодер).
4. **index-leaf — по S0**: путь (a) → один честный анчор; путь (b) → ноль.

Реалистичный итог: ~988 prod-уз → **~30-45** (recovery+bare-scalar+mint+опц.index),
всё холодное/граничное, каждый с инлайн-обоснованием. Горячий read-путь (C4/C5),
changefeed (C9), funclib (C7), read-кодеки (C2/C3) → **ноль id-ключевой
материализации**.

## Решение S0 — РЕШЕНО (2026-06-17)

Пользователь: **старые данные на диске можно смело удалять** → byte-identity
index-файлов между версиями НЕ ограничение. Выбран путь **(b), упрощённый**:
- C8/S9 — чистый рерайт: **lens-native хеш листа (без `InnerValue`)**,
  covering-проекция → имя-ключевая `Vec<(String, QueryValue)>`. **Index-leaf
  `InnerValue` → НОЛЬ.**
- При смене формата индекса — **rebuild-on-open из записей** (`doctor::repair`,
  записи сохраняются). Не нужен ни frozen-golden хешер (a), ни аккуратная
  байт-идентичная миграция.
- Конечный итог итога: index-якорь устранён → холодный пол ~26 узлов
  (tx-recovery `id_remap` + bare-scalar fallback + write-mint).
