בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-06-30 [sefer-alloc-rollout]

## Session summary

Длинная сессия, продолжение чекпоинта `2026-06-30-numa-phase-1.md`.
Прошла в четыре крупных блока, master ушёл на 18 коммитов вперёд origin'а:

**Блок 1 — NUMA-кампания финишировала.** `/babygoal` запущен с целью
«реализуй все задачи по numa между задачами делай коммиты». Через
sequential sh-агентов закрыты N1 (LinuxTopology через `/sys` +
`sched_setaffinity`), N2 (QEMU CI smoke-harness), N3 (`IndexInfo`
ArcSwap→NodeReplicated), N4 (`SortedIndexManager` ArcSwap→NodeReplicated).
5 коммитов в shamir-db. Workspace lib gate 3985/3985 PASS, нулевая
регрессия downstream. Goal автоочистился.

**Блок 2 — sefer-alloc rollout с поимкой bug'а.** /opti на тему
«подключить sefer-alloc v0.2.0 (global allocator)». Сначала зацепили
crates.io 0.2.0 с production feature + `LargeCacheConfig` (2 GiB
budget, 512 MiB headroom, 500ms decay, 25% rate, Lazy mode) в
shamir-server bin'е. При полном бенч-rollout'е поймали детерминированный
**OOM crash** на `duplex_throughput/duplex_cap32/32` (32 concurrent
TCP + tokio multi-thread + cross-thread free): sefer возвращал null на
640 байт. Запротоколировали в `docs/perf/sefer-alloc-0.2.0-oom-bug-2026-06-30.md`.
User закоммитил локальный fix → переключил `path = "D:/dev/rust/sefer-alloc"`,
cargo подтянул **0.2.1**, crash полностью исчез.

**Блок 3 — расширение покрытия sefer-alloc + stack-overflow fix.**
Добавлен `#[global_allocator]` во все 5 shamir-db бенчей. На первом
прогоне engine_perf — stack overflow в `bench_set_existing_with_index`.
Изолировано: воспроизводится **без** sefer-alloc → корневая причина
в bench profile `opt-level=0` (огромные async state-machine + ~1 MiB
Windows main thread stack). Workaround'ы: `block_on_setup` helper
(worker thread с 8 MiB stack) для seeded() call sites + поднятие
`profile.bench` opt-level 0→1. С обоими — 79+ вариантов engine_perf
проходят без crash'а.

**Блок 4 — удаление sled + feature-switch allocator'а + A/B sefer vs
mimalloc.** Через sh-агента полностью удалён sled backend (25 файлов,
+102/−1864 LOC, тесты переключены на FjallRepo). fjall остался
единственным disk-backed backend'ом. Введён cargo-feature switch для
аллокатора в бенчах (`bench-sefer` / `bench-sefer-tuned` / `bench-mimalloc`
/ system default) — `benches/bench_allocator.rs` shared include во всех
5 shamir-db бенчах. Прогнан A/B при opt-1 и opt-3: **sefer-alloc 17-22×
быстрее mimalloc** на alloc-heavy `--test` workload (103 setup'а +
1 итерация каждого). Прошлый каваэт «mimalloc лидирует +22-32% на
db_handler_rps» был на opt-0, нерелевантен.

**Инфраструктура:** установлен **sccache 0.16.0** как rustc-wrapper в
`D:/dev/rust/.cargo/config.toml` — shared compilation cache между всеми
проектами в `D:/dev/rust`. Server на 127.0.0.1:4226. Создан
`scripts/ts` — perl-based timestamp-prefix wrapper для логов (формат
`[HH:MM:SS +Δs] line`), снимает Windows pipe-buffering trap для
длительных bench / test прогонов.

**Inspected/read в этой сессии:** design doc NUMA, `IndexInfo`,
`SortedIndexManager`, `LargeCacheConfig` docs.rs, `engine_perf.rs`
(многократно), `Cargo.toml` (workspace + shamir-db + shamir-server),
`crates/shamir-numa/*`, sefer-alloc upstream docs.

**Active timer:** нет.

## Active goal

none

## TaskList

### in_progress
(пусто)

### pending
- #355 future: прогнать полный captrack PGO cycle на shamir-db
- #361 bench-infra fix: ACL drift в shamir-server/shamir-db бенчах

### recently completed (last 10)
- #287 NUMA Фаза 1: skeleton крейта shamir-numa
- #356 /opti: подключить sefer-alloc v0.2.0 (production + LargeCacheConfig)
- #357 NUMA N1: LinuxTopology + detect() Linux branch + libc dep + cfg-gated тесты
- #358 NUMA N3: миграция IndexInfo Arc<ArcSwap<Vec<IndexDefinition>>> → NodeReplicated
- #359 NUMA N2: QEMU CI harness — scripts/ci-qemu-numa-test.sh + numa.yml tier3 wiring
- #360 NUMA N4: миграция SortedIndexManager Arc<ArcSwap<Vec<SortedIndexDefinition>>> → NodeReplicated
- #362 bench-infra fix: stack overflow в engine_perf::set_existing_with_index
- #363 Удалить sled storage backend полностью

## Decisions

- **sefer-alloc 0.2.1 (local path) принят стратегически.** Native-Rust
  safety > FFI mimalloc, плюс на release-build alloc-heavy workload
  выигрывает 17-22× в --test mode. Reject: остаться на mimalloc —
  жертвуем safety без явного perf-win'а в release.
- **profile.bench opt-level 0 → 1.** opt-0 генерирует огромные async
  state-machine'ы которые переполняют Windows main thread stack
  (~1 MiB). opt-1 устраняет, build-time ~2× vs opt-0 (всё ещё ~2.5×
  faster than opt-3). Reject: спасать стэк через `block_on_setup`
  worker thread у каждого callsite — слишком инвазивно (хелпер всё
  равно оставлен defensively для seeded() путей).
- **sled удалён полностью.** Pre-existing Windows file-lock hang
  в `bulk_insert_sled/1000` блокировал engine_perf full run. fjall
  superseded sled (тот же LSM API, активно поддерживается). Reject:
  оставить sled "на всякий случай" — устаревший крейт, никто не
  использует на проде.
- **sccache как глобальный wrapper для D:/dev/rust.** При 47 GB
  target-каталоге на один проект окупается сразу. Caveat: incremental
  + sccache несовместимы, hit rate упадёт. Принято — incremental
  оставлен, sccache принесёт пользу на ad-hoc / cross-project builds.
- **Allocator feature-switch вместо global static.** `#[global_allocator]`
  compile-time — нельзя переключить рантаймом. Cargo features +
  shared `include!("bench_allocator.rs")` дают 4 пресета (sefer /
  sefer-tuned / mimalloc / system) c ~10-сек incremental rebuild.

## Open questions

- **Обновить устаревший perf-каваэт?** В `crates/shamir-server/Cargo.toml`
  и `docs/perf/sefer-alloc-rollout-2026-06-30.md` фигурирует фраза
  «mimalloc лидирует +22-32% на db_handler_rps» — была измерена на
  opt-level=0 (debug), нерелевантна. Пользователь предложил поправить
  комментарий + перепись пунктов в .md.
- **Что делать с opt-1 mimalloc 17× slowdown vs sefer-alloc?** В чём
  именно: TLS lookup overhead? Page reclamation per Drop? Windows
  HeapAlloc'а fast-path? Можно перепрогнать отдельные варианты с
  perf profiler'ом для верификации.
- **Incremental + sccache несовместимы.** Решить — отключить
  incremental в bench profile (sccache даст больше) или оставить как
  есть (incremental для интенсивных /opti циклов).
- **Sefer-alloc `bench-sefer-tuned` пресет — ещё не прогнан.**
  Feature заведена в Cargo.toml, switch работает, но A/B vs
  defaults sefer ещё не сделан.

## Repo state

```
(working tree clean)
```

```
b8af2c87 bench(shamir-db): feature-switch allocator (sefer / sefer-tuned / mimalloc / system)
e0c6ef81 chore(scripts): + scripts/ts — timestamp-prefix wrapper для логов
4196f0a5 refactor(storage): удалить sled backend, fjall — единственный disk-backed
c0bfaaa0 deps(bench): sefer-alloc 0.2.1 во всех бенчах + opt-level 0→1 stack-overflow fix
29a58c37 deps(server): mimalloc → sefer-alloc 0.2.0 (native-Rust global allocator)
```

master на **18 коммитов впереди origin** — push не делал, жду явной просьбы.

Untracked dirs: `D:/dev/rust/.cargo-target-bench/` (внешняя bench-cache
директория, не в дереве). sccache server на 127.0.0.1:4226 — живёт
вне репозитория.
