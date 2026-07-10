# Перегруппировка оставшихся задач аудит-кампании (2026-07-10)

Согласовано с пользователем: смежные/похожие задачи объединяются в группы —
один бриф, один агент-проход, один коммит (или короткая серия) на группу.

## Методика (обновлённая, согласована в этой сессии)

- **Per-task гейт**: `cargo check` + scoped `./scripts/test.sh -p <затронутые крейты>`,
  коммит сразу. Быстро, и регресс атрибутируется к конкретной задаче.
- **Финальный пакет** (одна завершающая задача в конце серии):
  `cargo fmt --all -- --check` + `cargo clippy --workspace --all-targets -- -D warnings` +
  `./scripts/test.sh --full` — и фикс всего найденного одним проходом.
- Пайплайн остаётся прежним: бриф в `docs/prompts/audit/` (коммит до запуска) →
  `@oh` (реализация) → независимая верификация оркестратором → `@fl` (ревью) →
  фиксы оркестратором напрямую → коммит.

## Группы

### G1 — KeyBytes миграция, шаги 3+4 (бывшие #504 + #505)
Один проход по плану `docs/design/record-key-128-migration-plan.md` §4 (3)+(4):
- alloc-free конструкторы на горячих путях: `rid.to_bytes()` →
  `RecordKey::from_slice(rid.as_bytes())` (table_manager_crud, drainer,
  tx/recovery, read_temporal, read_index_scan, table, streaming, staging).
- sweep in-memory backend / cold-путей на residual `Bytes::copy_from_slice`
  (+ interner_manager / record_counter / meta).
Бенч до/после обязателен (engine_perf / storage_*_pump / posting_cache_hit).
Блокируется завершением #503 (alias cutover).

### G2 — keyset-пагинация, оба ревью-финдинга #496 (бывшие #517 + #518)
Одна область (`read_index_scan.rs` / `sorted_index_manager.rs`):
- tie-breaker по record-id при равных ORDER BY значениях;
- короткая страница `lookup_range_first_k` на stale-записях индекса.

### G3 — security-residual из ревью #495 (бывшие #513 + #514)
Одна тематика:
- утечка слота subscription-cap, когда bridge-задача выходит сама;
- SSRF-гард: DNS-rebind TOCTOU + octal/short IP формы.

### G4 — тестовые флейки/падения (бывшие #509 + #511 + #521)
Однотипная работа «расследовать → починить или задокументировать»:
- oversample_higher_yields_at_least_as_many (флейк, #494);
- trusted_pure_scalar_backs_functional_index (падение, #495);
- argon2id_concurrency_cap_bounds_parallel_calls (флейк под нагрузкой, #498).

## Остаются одиночными

- #503 — in-progress (alias cutover, у @oh).
- #506 — optional/measure-first INLINE_CAP (блокирован G1).
- #507 — CLEANUP typos read_exec.rs (тривиальная, можно приклеить к финальному пакету).
- #512 — SECURITY design (channel-binding) — исследовательская.
- #515 — MemBuffer merge-overlay scan.
- #516 — SQ8 fused rescore + weighted-SIMD.
- #519 — CLIENT typed errors (требует бамп napi-rs 3.x → нужно явное
  разрешение пользователя на бамп версии; до этого — отложена).
- #520 — CLIENT Rust roundtrip timeout.
- #522 — TEST усиление reactivated_segment_sheds_stale_sidecar.
- #523 — PERF fjall-бенч (prerequisite).
- #524 — PERF worker-loop batching (блокирован #523).

## Новая задача-финализатор

**FINAL-GATE**: в самом конце серии — полный fmt/clippy/test --full по
workspace + фикс всего найденного. Все остальные задачи должны быть
завершены до неё.

## Порядок исполнения (примерный)

#503 → G1 → G2 → G3 → #515 → #516 → #520 → #522 → #523 → #524 → G4 →
#506(по бенчам) → #507 → #512 → (#519 после решения по napi-rs) → FINAL-GATE.
