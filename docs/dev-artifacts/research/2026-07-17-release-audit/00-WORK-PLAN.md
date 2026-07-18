# План работ по итогам Release-Readiness Audit (2026-07-17)

Декомпозиция находок из 10 отчётов (`00-SUMMARY.md`) на этапы и задачи.
Каждый этап — отдельная задача в TaskList (см. соответствие в конце).
Порядок отражает приоритет: correctness/deadlock до релиза; funclib/honesty
до тега v0.10; docs/CI параллельно; performance — post-blocker.

Дисциплина для каждой задачи: prompt-first бриф в
`docs/dev-artifacts/prompts/<area>/`, коммит брифа ДО агента, zero-trust
верификация оркестратором (diff + `./scripts/test.sh` + fmt + clippy), коммит
проверенного кода перед следующей задачей.

---

## Этап 0 — ✅ Сделано

**ACL bypass fix** (отчёты 01/02). Commit `6d33fe9e`. Рекурсивный
`collect_required_access`, оба входа + WASM-gateway, 16 тестов, gate зелёный.

---

## Этап 1 — Correctness-блокеры (data-corruption, молча)

Источник: отчёт 04 (logical bugs) + отчёт 05 #1. Порядок фикса — по возрастанию
риска/размера, ровно как в §«Suggested fix order» отчёта 04.

1. **FK on-update dedup** — `fk_on_update.rs:179-180`. Дедупить только
   список имён полей, сохранять все `OnUpdateRef` для probe. One-line scoping.
   Тесты: два child-table, один child два FK, RESTRICT-вариант.
2. **`$contains_all` дубликаты** — `filter_node.rs:601-624` +
   `compile.rs:286-305`. Считать distinct set-members, не raw hits.
3. **Diamond cascade false cycle** — `fk_actions.rs:148-159, 380-391`. Per-path
   visited (снимать при возврате ветки) + row-level dedup мутаций.
4. **Dec-aware comparison layer** (04 #4/#5/#6 одной когерентной задачей):
   Dec/Big-арм в `compare_values`/`scalar_ref_cmp_qv`; Dec в аккумуляторах
   aggregate (sum/avg/min/max); numeric `QvSortKey::Dec` для ORDER BY; Dec в
   `$expr` numeric coercion.
5. **UPSERT `created_at` overwrite** (05 #1) — `write_exec.rs:863-870`. Перенести
   `apply_transforms`/перепроверку после lookup INSERT-vs-MERGE.

Верификация: `./scripts/test.sh @engine --full`, регресс-тесты на каждый баг.

---

## Этап 2 — Concurrency deadlock hazards (класс #589, расширяет #671)

Источник: отчёт 06. Порядок — §«Recommended fix order».

1. **H1+H2** (HIGH): `active_snapshots.entry_async`→`entry_sync`
   (repo_tx_gate.rs:415); `locks.entry_async`→`entry_sync` (mvcc_locks.rs:54).
   Рецепт #589 один-в-один.
2. **H3** (MED-HIGH): вектор-индекс — 5 карт (`deleted`, `vectors_u8`,
   `vectors`, `rid_to_internal`, `compaction_deleted_rids`) в
   `hnsw_adapter.rs`/`vector_backend.rs`, механический `_async`→`_sync` sweep.
3. **H4+H5** (MED): `per_table_mvcc`, `token_names` `read_async`→`read_sync`,
   `iter_async`→`iter_sync`.
4. **H6** (LOW, гигиена): `layered_interner` `touch`→`touch_sync`; чистка
   стале-комментария `mvcc_history.rs:460`.

Верификация: contention-repro (worker_threads=1..2, loop под нагрузкой),
`./scripts/test.sh @oracle @tx --full`. НЕ поднимать таймауты.

---

## Этап 3 — Honesty fixes (молчаливое → явная ошибка / warn-лог)

Источник: отчёт 05 (silent gaps) + хвост отчёта 04 (MED/LOW).

1. Отвергать nested-path `default`/`auto_now`/computed-default на DDL-time
   (симметрично `unique`, 05 #2).
2. Отвергать `Call` в `transactional:true`/interactive-tx с
   `call_in_tx_not_supported` (05 #3, прецедент `nested_tx_not_supported`).
3. warn-логи: fail-open computed default (05 #4), Null Call-параметры (05 #5).
4. Хвост 04: #7 coercing set-probes, #8 thread ScalarResolver в SELECT/when/
   bind/over, #9 count_distinct/mode над Set/Map, #10 self-ref FK, #11 FK
   Int↔F64 coercion, #12 `$expr mod` checked_rem, #13 checked i64, #14/#15
   Big compare/cast.

---

## Этап 4 — v0.10 funclib top-up

Источник: отчёт 10, Part A §3.

**P0 (до тега):**
1. Null-функции: `coalesce`/`if_null`/`nullif`/`is_null` (новая папка `null/`).
2. Wire-параметры `percentile`/`string_agg` (`args` в `SelectItem::AggregateFn`)
   + honor-or-reject `distinct`.
3. `datetime/format(ts, pattern)` + `parse(s, pattern)` (chrono).
4. `uuid_v4()` (+ опц. `random`/`random_bytes`) как `pure:false`.
5. Фикс `arrays/sort` через cross-type `compare()` + `sort_desc`.
6. `parse_json`/`to_json`.

**P1 (тот же поезд, если есть ёмкость):** календарь (`add_months`/`add_years`/
`diff_days`/`quarter`/`iso_week`); array set-ops (`reverse`/`concat`/`union`/
`intersect`/`difference`); `object/set_path`/`remove_path`/`deep_merge`;
`strings/capitalize`/`format`/`text/camel`/`snake`; `min_by`/`max_by`.

---

## Этап 5 — Compliance & Ops

Источник: отчёт 03.

1. Audit-покрытие: мост `AuditSink`→`AuditChainWriter`, append на DDL/ACL/admin
   арках (P1). До тех пор — поправить доки.
2. Фикс `07-operations.md` про backup (stop-and-copy, не live); долгосрочно —
   quiesced snapshot + restore с auto-revoke tickets.
3. Erasure-остатки: расширить `data-protection.md` §2 (index-снапшоты, реплики,
   FTS-postings); «purge each replica independently».
4. Мёртвый `audit.retention_days` — реализовать sweep или убрать knob+claim.
5. SBOM-артефакт (`cargo cyclonedx`/`cargo-about`) в supply-chain workflow.
6. Latent plaintext secrets → `SecretString`: `CreateScramUser.password`,
   `VectorBackendRef::External.api_key_secret`.

---

## Этап 6 — Documentation accuracy

Источник: отчёт 09.

1. `08-interconnect.md`: «❌ Нет кода» → рабочий статус (репликация, changefeed,
   подписки).
2. `README.md`/`CONTRIBUTING.md`/`CLAUDE.md` Pre-commit gate: заменить сырые
   `cargo test` на `./scripts/test.sh`/`cargo t`/`cargo tl`.
3. `03-storage.md` + doc-комменты: `redb`→`fjall`.
4. `07-operations.md`: убрать фантомный `allow_public_metrics` (или реализовать).
5. CLAUDE.md: убрать неверное «`shamir_bench_utils` … gone».
6. funclib docs (`05-functions.md`): ~10 фантомных имён → реальные
   (`strings/lower`, `cast/to_string`, `datetime/format_rfc3339`, `crypto/sha256`).
7. Мелочь: `skipped` в TS-типах, порт 13760, стале-комменты «not wired» (05 хвост).

---

## Этап 7 — Test / CI robustness

Источник: отчёт 08.

1. Запинить `cargo-nextest@0.9.137` в CI (guard-coupling риск).
2. `shell: bash` на Windows `test`-джобе (паритет с integration).
3. `[[profile.ci.overrides]]` для SCRAM/WASM (сейчас dev-box killи текут в CI).
4. Файл marker-combination тестов: top-level `$expr`, `$expr`/`$cond`+`$ref`
   pinning, `SetOp.key` marker, глубокая вложенность.
5. Contention/stress-лейн (nightly): `@oracle` + mvcc_store_tests с высокой
   параллельностью.
6. Обернуть unbounded spin-wait'ы в `tokio::time::timeout` (шаблон —
   `quantized_graph_tests.rs:1630`).
7. Пин `cargo-cooldown`; чистка `fix643_test.log`.

---

## Этап 8 — Performance (post-blocker, не гейт релиза)

Источник: отчёт 07.

1. **F1/F2**: value-IR для `resolve_filter_query` (pre-intern `FieldRef`,
   pre-resolve record-independent операнды) — un-generalised #643.
2. **F3/F7**: считать score внутри `read_async`/`iter_sync` closure в HNSW
   (устранить per-candidate `Vec` клоны).
3. **F4**: SQ8 cosine — hoist query-norm (VR-7 опция 2) + SIMD-кернелы для
   `approx_l2_sq`/`dequant_norm_sq`.
4. **F5**: ForEach — прямая `QueryResult`→`QueryValue` конверсия (без serde
   round-trip) + hoisted compiled-body cache.
5. Мелочь: F6/F8/F9/F10/F11 по мере касания соответствующего кода.

---

## Соответствие задачам TaskList

- #671 → Этап 2 (расширен находками отчёта 06; H1-H6).
- Этап 1 → «Correctness-блокеры: FK/Dec/UPSERT/$contains_all»
- Этап 3 → «Honesty fixes + logical MED/LOW»
- Этап 4 → «funclib top-up v0.10 (P0/P1)»
- Этап 5 → «Compliance & Ops»
- Этап 6 → «Documentation accuracy»
- Этап 7 → «Test / CI robustness»
- Этап 8 → «Performance (post-blocker)»
