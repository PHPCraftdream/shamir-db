# Release-Readiness Audit — Сводка (2026-07-17)

Сводка по 10 read-only отчётам исследования всей кодовой базы S.H.A.M.I.R.
перед релизом v0.10. Каждый отчёт исследовал свою тему по всему проекту.
Исходные отчёты: `01-security-posture.md` … `10-release-readiness-v0.10.md`
в этой же папке. План работ и декомпозиция — `00-WORK-PLAN.md`.

---

## Главный итог

Проект **близок к готовности к v0.10** для early-adopter релиза: архитектура,
поверхность запросов, модель безопасности и операционная история — выше планки.
Ни одна находка не является архитектурным блокером. Но есть один уже закрытый
CRITICAL, кластер молчаливых correctness-багов и несколько deadlock-рисков,
которые надо закрыть до тега.

## Статус по темам

| # | Отчёт | Худшая находка | Статус |
|---|-------|----------------|--------|
| 01 | Security posture | CRITICAL: ForEach/Batch обходят per-table ACL | **✅ ИСПРАВЛЕНО** (commit `6d33fe9e`) |
| 02 | Permissions / ACL correctness | Тот же ACL bypass (независимо подтверждён) | **✅ ИСПРАВЛЕНО** |
| 03 | Compliance / data governance | Audit-лог покрывает только auth; доки врут про DDL/ACL | Открыто |
| 04 | Logical correctness bugs | 6 HIGH багов (FK, Dec-слой, $contains_all) | Открыто |
| 05 | Incomplete features / gaps | 5 молчаливо-неверных путей (UPSERT created_at и др.) | Открыто |
| 06 | Concurrency / deadlock hazards | H1/H2 HIGH + вектор-индекс (5 карт) — класс #589 | Открыто (расширяет #671) |
| 07 | Performance optimizations | resolve_filter_query IR per-row; HNSW клоны | Открыто (не блокер) |
| 08 | Test coverage / CI robustness | nextest не запинен в CI; пробелы marker-тестов | Открыто |
| 09 | Documentation accuracy | Стале «❌ Нет кода» про репликацию/подписки; битые cargo test инструкции | Открыто |
| 10 | Release-readiness v0.10 | funclib: нет coalesce/null-функций; datetime format/parse | Открыто |

---

## Что уже сделано

**CRITICAL ACL bypass (отчёты 01 F1 + 02 P1)** — `BatchOp::Batch`/`ForEach`
возвращали `None` из `required_access()`, поэтому per-table авторизация не
проверялась для операций внутри вложенного тела. Аутентифицированный
не-superuser мог читать/писать/удалять запрещённую таблицу, обернув операцию
в `ForEach`/`Batch`. Исправлено рекурсивным `collect_required_access`
(зеркалит `distinct_repos`/`collect_repos` из #660), закрывает оба входа
(`execute_as`/`tx_execute_as`) и WASM-gateway. Коммит `6d33fe9e`, 16 тестов,
gate зелёный.

---

## Кластеры оставшихся находок

### A. Correctness-блокеры (молча портят данные) — отчёты 04, 05
Высший приоритет до релиза, т.к. приводят к тихой потере/искажению данных:
- **FK on-update dedup** (04 #2): `dedup_by(parent_ref_field)` схлопывает все
  FK-ссылки кроме одной → молчаливые dangling references.
- **$contains_all дубликаты** (04 #1): fast-path считает дубликаты → ложный `true`.
- **Diamond cascade false cycle** (04 #3): легальный DAG отвергается как «cycle».
- **Dec-слой сравнения** (04 #4/#5/#6): Dec-колонки не фильтруются, sum=0,
  min/max возвращает первую строку, ORDER BY лексикографический — всё молча.
- **UPSERT перезаписывает `created_at`** (05 #1): AutoNowAdd с `is_insert=true`
  на merge-ветке затирает исходную метку.

### B. Concurrency deadlock hazards (класс #589) — отчёт 06
Расширяет существующую задачу #671:
- **H1** `active_snapshots.entry_async` (repo_tx_gate.rs:415) — HIGH, горячий ключ.
- **H2** `mvcc_locks.entry_async` (mvcc_locks.rs:54) — HIGH.
- **H3** вектор-индекс: 5 карт в hnsw_adapter.rs/vector_backend.rs смешивают
  `_async`/`_sync` — новое, MEDIUM-HIGH.
- **H4/H5** `per_table_mvcc`, `token_names` — MEDIUM, нужен DDL под нагрузкой.
- **H6** `layered_interner` — только гигиена (per-TxContext, не гонится сегодня).

### C. Honesty fixes (молчаливое → явная ошибка/лог) — отчёт 05
- Отвергать nested-path defaults/transforms на DDL-time (05 #2).
- Отвергать `Call` внутри транзакции (`call_in_tx_not_supported`, 05 #3).
- warn-логи для fail-open computed defaults (05 #4) и Null Call-параметров (05 #5).

### D. v0.10 funclib top-up — отчёт 10
P0 (до тега): `coalesce`/`if_null`/`nullif`/`is_null`; wire-параметры для
`percentile`/`string_agg` + honor `distinct`; `datetime/format`+`parse`;
`uuid_v4()`; фикс `arrays/sort` (cross-type); `parse_json`/`to_json`.
P1: календарная арифметика, array set-ops, object set_path, string helpers.

### E. Compliance & Ops — отчёт 03
Audit-покрытие DDL/ACL/admin (сейчас только auth); фикс доки бэкапа
(stop-and-copy, не live); undocumented erasure-остатки (index-снапшоты,
реплики); мёртвый `audit.retention_days`; SBOM-артефакт; latent plaintext
secrets → `SecretString`.

### F. Documentation accuracy — отчёт 09
`08-interconnect.md` «❌ Нет кода» → на самом деле работает (репликация,
changefeed, подписки); битые `cargo test` инструкции в README/CONTRIBUTING;
`redb`→`fjall`; фантомный `allow_public_metrics`; CLAUDE.md про
`shamir_bench_utils`; funclib docs drift (~10 фантомных имён).

### G. Test / CI robustness — отчёт 08
Запинить nextest в CI; `shell: bash` на Windows test-леге; `profile.ci`
overrides для SCRAM/WASM; файл marker-combination тестов ($expr, $cond+$ref,
SetOp.key); contention stress-лейн; обернуть spin-wait'ы в timeout.

### H. Performance (post-blocker) — отчёт 07
`resolve_filter_query` value-IR (un-generalised #643); устранение per-candidate
клонов в HNSW-поиске; SQ8 cosine norm hoist + SIMD; ForEach msgpack round-trip.

---

## Рекомендованный порядок

Correctness-блокеры (A) и deadlock-риски (B) — до релиза. Honesty (C) и
funclib P0 (D) — до тега v0.10. Docs (F) и CI (G) — параллельно, дёшево.
Compliance (E) — частично до релиза (audit-покрытие, backup-док), остальное
в v0.11. Performance (H) — post-blocker, не гейт релиза.
