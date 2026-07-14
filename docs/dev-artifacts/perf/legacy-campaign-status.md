בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Legacy-text-elimination кампания — общая картина (живой статус)

Удаление `serde_json` из всего проекта (вторая ось цели #61). Дополняет
`legacy-elimination-stages.md` (этапы) и `innervalue-floor.md` (первая ось).

---

## 1. Фундамент (построено)

**Кампания #61 (InnerValue) — ЗАКРЫТА.** Production `InnerValue` ~1004 → ~654,
сведён к 4 задокументированным категориям (`innervalue-floor.md`); горячие пути
линз-нативны (`RecordView`/`ScalarRef`); index V1→V2 авто-мигрирует на открытии
(S9b). Запушено.

**Кампания legacy-text-elimination (агрессивный scope: снести `serde_json` целиком) — в активной
фазе.**

---

## 2. Сделано в legacy-text-elimination (локально)

| Этап | Суть |
|---|---|
| **J1** | мёртвые legacy read-twins прочь (apply_select/_to_bytes/project/distinct) |
| **tx + connect** | off legacy-text-encoding lib → rmp_serde / toml (leaf-крейты) |
| **`mpack!`** | msgpack-builder данных для тестов (39 тестов; `shamir-types`) |
| **md-sweep** | ~27 доков legacy text→MessagePack (campaign/spec/история сохранены) |
| **J2/J3/J4** | engine-internal: `HashableQueryValue` (distinct), group-key, order/computed off legacy-text-encoding lib — e2e 567/567 |
| **funcargs** | первый CORE-кластер; wire-совместимость QueryValue↔msgpack доказана |

---

## 3. Ключевые открытия (сняли главные риски)

1. **Провод УЖЕ MessagePack.** `serde_json::Value` — in-memory динамический
   тип, десериализуемый ИЗ msgpack. `QueryValue` имеет custom plain-msgpack
   `Serialize`/`Deserialize` (Int→i64, Str/Dec/Big→str, List→seq, Map→map),
   **байт-идентичный** `serde_json::Value`. → **CORE = внутренний type-swap
   `serde_json::Value`→`QueryValue`, НЕ слом протокола.** v1-bump не нужен.
2. **Интернер-синк-протокол уже построен** на Rust (Stage 5-wire): per-repo
   `request.interner_epochs` → `attach_interner_delta` → `response.interner_delta`
   (batch-piggyback); `InternerDump` (full); `entries_after(epoch)` (delta);
   `InternerTouch` (минт). Rust-клиент это использует; TS/napi — нет (это и есть
   будущая работа #100, а не «изобрести»).
3. **Инвариант:** id append-only, НИКАКОЙ переборки интернера (epoch-delta
   всегда безопасен; клиентские кэши не инвалидируются).

---

## 4. Ближайшие этапы (legacy-core, по dep-порядку)

1. 🔄 `core-wire-field-swaps` (profile/id/value → QueryValue) — **в работе**.
2. **`QueryRecord::Legacy` → `Direct(QueryValue)`** — центровой, отдельным
   фокусным этапом (variant + From-impls + as_legacy/get/Index → QueryValue;
   db admin_result, client-parsers, server, engine read-fallback).
3. **builder** — ToWire `to_legacy_value`, write-builders (row/set/key/value),
   `Batch::id` → QueryValue.
4. **db** — SystemStore `load_*`/`save_*`, `access_tree`, FacadeDbGateway
   filter-round-trips → typed / QueryValue.
5. **types codec** — `legacy.rs`/`LegacyCodec`/`RecordRef::to_legacy_value`/
   `ResourceMeta`/`From<serde_json::Value>`-impls — **последним** (после ухода
   всех потребителей).
6. **leaf-финал** — wasm-host `FunctionMeta`, client-parsers, **napi
   `client-node`** (граница JS↔Rust → msgpack `Buffer`).
7. **Снос `serde_json`** из каждого Cargo.toml (критерий готовности крейта) +
   **финальный sweep комментариев** в коде (legacy→msgpack — отложен, чтобы не
   переписывать дважды).
8. **legacy-close** — ноль `serde_json` в workspace, честный итог в доке.

---

## 5. Последующие (north-star+, после legacy-text-elimination)

- **#100 — TS/napi client-side interner.** Подключить клиенты к УЖЕ
  существующему epoch-delta протоколу (`interner_epochs`/`interner_delta`/
  `InternerDump`/`InternerTouch` + локальный `FieldMap`-кэш с de-intern на
  чтении) → разгрузка сервера (горячая трансляция name↔id уходит на клиент),
  полный msgpack pass-through + e2e TS-тесты. Сервер остаётся авторитетом id
  (минт+персист); клиент берёт трансляцию.
- **Бэклог:** #82 (DX value-API над QueryValue для процедур), #55 (X-remap),
  #41 (Stage 6 positional), #72, #83.

---

## 6. Метод (работает)

Workflow'ы (ash-агенты) фанятся/пайплайнятся → каждый кластер сам гонит гейт
(`fmt` + `clippy --workspace --all-targets` + `test.sh`) → **@e2e как сеть
валидации провода** (TS↔Rust round-trip) → оркестратор zero-trust ревью +
коммит. Workspace зелёный между этапами: связанное ядро снимается
координированными **атомарными кластерами по полю/варианту**, а не по кускам
(иначе «красный» build у всех параллельных агентов).

**Дисциплина гейта:** `cargo fmt --all -- --check` ПЕРВОЙ проверкой; никогда не
писать «green», не прогнав все четыре.
