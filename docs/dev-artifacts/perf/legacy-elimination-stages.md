בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Приземлённый staged-план: элиминация legacy text encoding (МАКСИМАЛЬНЫЙ scope)

Продолжает InnerValue-кампанию (`innervalue-floor.md`) для второй оси цели #61:
**ноль legacy text encoding**. Scope (решение пользователя, финальное): **полностью удалить
библиотеку `serde_json` из всех 12 крейтов** и починить код; тесты — на
**query-builders + MessagePack** (ноль `mpack!`-литералов). Ломает v1-клиентов
(принято). Никакого «пола» — `serde_json` уходит из workspace целиком.

## Масштаб (замер)
12 крейтов с зависимостью, ~2400 call-sites, ~113 `mpack!`. Линчпин:
`query-builder` (477) сам эмитит legacy `Value`; `query-types` (419) —
wire-DTO. Удаление = **переархитектура wire legacy text encoding→MessagePack/QueryValue**.

## Порядок (по графу зависимостей — острие первым)
1. **Фундамент:** `QueryValue` как динамический value-тип вместо
   legacy `Value`; wire-DTO (`query-types`) + builder → emit
   `QueryValue`/msgpack; убрать `legacy.rs`-codec из `types`.
2. **Потребители:** engine → db → server → client (call-sites отваливаются по
   мере смены wire).
3. **Мелочь:** funclib/tx/wasm-host/connect.
4. **Тесты:** legacy literals → builders + msgpack.
5. **Cargo.toml:** `serde_json` вон из каждого крейта (критерий завершения).

Старый «детвиннинг + пол» ниже (J-I..J-IV) — теперь ПОДЭТАПЫ фундамента/потребителей,
а не финал. `legacy-floor.md` НЕ будет — пол = пустой.

Дисциплина — та же, что у InnerValue:
- **анти-формально**: метрика — построение legacy `Value` на горячем, не grep;
- один кластер/этап, ≤6–8 файлов, одна семантика;
- агент пишет код **и сам гонит гейт** (fmt/clippy/test.sh/@e2e); оркестратор ревью+коммит;
- byte-identity golden на каждом persist/wire-шаге;
- **v1-break за чекпоинтом** — отдельное явное «go» перед Движением J-III.

---

## 0. Карта legacy-следа (production, ~600 узлов)

| Кластер | ~Узлов | Класс | Движение |
|---|---|---|---|
| read-pipeline: `exec.rs`, `select_projection.rs project()`, `order.rs`, `hashable_legacy.rs`, `write_helpers.rs`, `resolve.rs` | ~90 | **canonical-key + apply_select legacy-twin** (живой!) | J-I |
| changefeed: db `changelog.rs`, server `subscriptions/{payload,bridge}.rs` | ~30 | **legacy-wire payload клиентам** | J-II |
| v1-wire DTO: query-types `query_record.rs`, `batch_op.rs`, `inserted_record.rs` | ~140 | **v1-протокол** (`QueryRecord::Legacy`) | J-III (break) |
| builder: query-builder `batch.rs`, `wire/mod.rs`, … | ~38 | **producer legacy-форм** | J-III (break) |
| client SDK: `shamir-client`, client-ts, client-node | ~11 | **msgpack-миграция клиента** | J-III (break) |
| codec: types `legacy.rs`, `value.rs` | ~103 | **библиотека кодека** | J-IV (пол) |
| control-plane: db `system_store.rs`, `access_control.rs`, `db_gateway.rs`, `helpers.rs` | ~84 | **admin/ACL/system** | J-IV (решение: пол vs миграция) |
| wasm-host `meta.rs`, funclib, tx, access | ~14 | **boundary/config** | J-IV (пол) |

---

## Движение J-I — детвиннинг read-пути (БЕЗОПАСНО, без v1-break)

`#60` увёл read-RESULT на `QueryValue`, но внутри пайплайна legacy text encoding ещё держит
**canonical-key** и **apply_select-twin**. Это устранимо QueryValue-нативно.

- **J1 — apply_select legacy-twin.** `select_projection.rs::project()` (legacy) +
  `exec.rs::apply_select`/`proj.project` call-site. Подтвердить, что production
  emits через `project_value`/`apply_select_value(_bytes)` (QueryValue), а
  legacy-`project` — bench/example-twin → удалить мёртвый twin ИЛИ задокументировать
  если ещё на live-control-пути. Размер: S–M, аудит.
- **J2 — canonical-key детвин (distinct).** `hashable_legacy.rs` (`HashableLegacy`) +
  `exec.rs::apply_distinct_qv` (строит `HashableLegacy(legacy::from(qv))` как ключ
  дедупа). Ввести `HashableQueryValue` (Hash+Eq над `QueryValue` напрямую, те же
  канонические биты) → дедуп без legacy `Value`. Удалить legacy-`apply_distinct`
  (twin) + `hashable_legacy.rs`. Критерий: distinct byte-identity. Размер: M.
- **J3 — canonical-key детвин (group-by/aggregate).** `aggregate.rs:170`
  (`inner_to_legacy_value` для group-key). Перевести group-key на QueryValue-нативный
  канонический ключ (зеркало J2). Критерий: group_by parity. Размер: M.
- **J4 — read-path остаток.** `order.rs` (31 — order-by канонизация?),
  `write_helpers.rs`, `resolve.rs` legacy-узлы → QueryValue или задокументировать
  §floor. Аудит. Размер: M.

## Движение J-II — changefeed payload

- **J5 — changefeed → решение payload-формата.** db `changelog.rs`
  (`inner_to_legacy_value` lazily) + server `subscriptions/{payload,bridge}.rs`
  (`to_legacy_value`). Это **wire-payload подписчикам**. Агрессивно: перевести на
  msgpack-payload (как read-result pass-through) + миграция подписчик-клиента.
  Альтернатива: оставить legacy-payload как control-plane-якорь. **Решение на
  чекпоинте** (затрагивает клиентов). Размер: M–L.

## Движение J-III — схлопывание v1-wire (АГРЕССИВНО — ЛОМАЕТ v1, ЧЕКПОИНТ)

⚠️ **СТОП-чекпоинт перед J6.** Здесь ломается v1-протокол: нужен bump версии,
депрекация `QueryRecord::Legacy`, миграция клиентского SDK. Outward-facing и
трудно-обратимо — отдельное явное «go».

- **J6 — протокол на msgpack-only.** query-types `query_record.rs`
  (`QueryRecord::Legacy` → удалить/депрекировать), `batch_op.rs`,
  `inserted_record.rs` → msgpack-only wire. Bump версии протокола (v2→v3).
  Размер: L.
- **J7 — builder msgpack-only.** query-builder: убрать `to_legacy_value`/`ToWire`
  legacy-выход, оставить msgpack-builder. Размер: M–L.
- **J8 — клиентский SDK.** `shamir-client` / client-ts / client-node → msgpack-only
  send/recv. Размер: L (вне default workspace для node).

## Движение J-IV — кодек-конфайнмент + запечатывание

- **J9 — control-plane решение.** db `system_store.rs`/`access_control.rs`/
  `db_gateway.rs` — admin/ACL/system на legacy text encoding. Решение: **пол** (admin-legacy
  приемлем — человекочитаемый control-plane) ИЛИ миграция. По умолчанию — **пол**.
- **J10 — codec-пол.** types `legacy.rs`/`value.rs` — что осталось после J-I..J-III
  = библиотека (serde-derive, config, wasm-host meta, control-plane). Документировать.
- **J-close — `docs/dev-artifacts/perf/legacy-floor.md`** + честный end-state + закрыть legacy-ось #61.

---

## Граф / чекпоинты

```
J1 → J2 → J3 → J4        (read-path, безопасно, параллельно по файлам с гейтом)
        │
        ▼
   J5 (changefeed — решение payload)  ── ЧЕКПОИНТ (затрагивает подписчиков)
        │
        ▼
 ⚠️ СТОП-ЧЕКПОИНТ ⚠️  (v1-break — явное «go»)
        │
   J6 → J7 → J8   (wire / builder / client — msgpack-only, bump протокола)
        │
        ▼
   J9 → J10 → J-close   (control-plane решение, codec-пол, документирование)
```

## Анти-паттерны
- ❌ снести legacy `Value` там, где он = control-plane/config/serde-derive (это пол).
- ❌ ломать v1-wire без bump протокола + чекпоинта.
- ❌ 2+ кластера в этапе; агент гоняет git.
- ✅ один кластер, byte-identity golden, гейт у оркестратора, v1-break за «go».
