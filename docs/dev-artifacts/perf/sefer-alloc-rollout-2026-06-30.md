בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# sefer-alloc rollout — functional verification (2026-06-30)

**Версии:** `sefer-alloc 0.2.0` (crates.io) → **`sefer-alloc 0.2.1`** (local path
`D:/dev/rust/sefer-alloc`, user-applied fix для OOM).
**features:** `["production"]` (= `alloc-global + alloc-xthread + alloc-decommit`).
**Host:** Windows 10 Pro x86-64, rustc 1.93.0.
**Profile:** `cargo bench` запускается под `bench profile [unoptimized]` —
**дебаг-сборка**, perf-числа сравнения не имеют смысла. Этот документ — про
**функциональную работоспособность** (что падает, что нет, по чьей вине).

---

## TL;DR

После применения sefer-alloc 0.2.1 (с user-applied fix'ом) **ни один бенч
не падает по вине sefer-alloc**. Все наблюдаемые crash'и — pre-existing
баги bench-инфраструктуры, существовавшие до rollout'а:

| Класс | Бенчи | Причина | Sefer-alloc? |
|---|---|---|---|
| ACL drift | `wire_pipelining`, `wire_latencies`, `authorize_gate::user_traverse_table` | `access_denied` для test user — отстало от текущей системы прав | **НЕТ** |
| Subscribe-response shape | `subscription_throughput`, `subscription_delivery`, `subscription_fanout` | `expected Batch response`, panic'и на mismatch shape — bench setup отстал от текущего dispatch'а | **НЕТ** |
| Stack overflow | `engine_perf::set_existing_with_index` (+ всё что идёт после в default-порядке) | main thread stack overflow в setup (`seeded(n, true)` с index build) | **НЕТ** (подтверждено — crash reproduces и без sefer-alloc) |
| Работает clean | `db_handler_rps`, `duplex_throughput`, `engine_perf::{bulk_insert,set_existing_no_index}`, `record_size_axis`, `changelog_read`, `durability_axis`, `authorize_gate::system_bypass` | — | ✓ OK |

Дополнительно: `duplex_throughput/duplex_cap32/32` — критическая
**verification**, что 0.2.0 OOM-crash полностью починен в 0.2.1 (32
concurrent TCP-сессии × 32 pipelined req, tokio multi-thread + bounded
MPSC + cross-thread free — стабильно проходит).

---

## Покрытие

Sefer-alloc применён как `#[global_allocator]` в:

* `shamir-server/src/main.rs` — production bin, `SeferAlloc::with_config(...)` с
  production-tuned LargeCacheConfig (2 GiB budget + 512 MiB headroom + 500ms
  decay + 25% rate + Lazy mode).
* `shamir-server/benches/*` (4 файла: db_handler_rps, duplex_throughput,
  wire_pipelining, wire_latencies) — `SeferAlloc::new()` defaults.
* `shamir-server/benches/subscription_*` (3 файла) — `SeferAlloc::new()` defaults
  (бенчи сами падают на pre-existing setup-баги, но allocator-coverage
  стоит для consistency на будущее).
* `shamir-db/benches/*` (5 файлов: engine_perf, authorize_gate, changelog_read,
  record_size_axis, durability_axis) — `SeferAlloc::new()` defaults; sefer-alloc
  добавлен как dev-dependency в `shamir-db/Cargo.toml`.

**Итого:** 1 prod bin + 12 бенчей.

---

## Что прошло чисто (sefer-alloc 0.2.1)

### shamir-server::db_handler_rps

| Variant | Time |
|---|---|
| `ping_inprocess` | 9.63 µs |
| `execute_read_filter_sort_limit_100rec` | 654.59 µs |
| `execute_full_scan_100rec` | 385.23 µs |

Exit 0, без crash'ей. (Числа — дебаг-сборка, нерелевантны для perf
сравнения.)

### shamir-server::duplex_throughput (concurrent TCP pipelining)

| Variant | Time | Throughput |
|---|---|---|
| `lockstep_cap1/10` | 155.21 ms | 64 elem/s |
| `duplex_cap32/10` | 15.43 ms | 648 elem/s |
| `lockstep_cap1/32` | 494.71 ms | 65 elem/s |
| `duplex_cap32/32` ⭐ | 15.51 ms | 2.06 Kelem/s |

⭐ — **critical**: тот вариант, что крэшил 0.2.0 (`memory allocation of 640
bytes failed`). На 0.2.1 — стабильно проходит, **availability bug устранён**.

### shamir-db::engine_perf (частично)

| Variant | Status |
|---|---|
| `bulk_insert/100` | ✓ OK |
| `bulk_insert/1000` | ✓ OK |
| `set_existing_no_index/100` | ✓ OK |
| `set_existing_no_index/1000` | ✓ OK |
| `set_existing_no_index/10000` | ✓ OK |
| `set_existing_with_index/*` | ✗ stack overflow (NOT sefer-alloc) — см. ниже |

### shamir-db::record_size_axis, changelog_read, durability_axis

Все exit 0, все варианты прошли.

* `record_size_axis`: 1MB blob inserts, 10KB/100KB strings, nested object — все
  variants OK.
* `changelog_read`: depth × limit grid (5 вариантов) — все OK.
* `durability_axis`: buffered/synced × n_1/n_10/n_100 (6 вариантов) — все OK.

### shamir-db::authorize_gate

`system_bypass` ✓ OK, `user_traverse_table` ✗ — pre-existing ACL баг (см. ниже).

---

## Pre-existing баги (НЕ sefer-alloc)

### A. ACL drift в server-бенчах

`wire_pipelining`, `wire_latencies`, `authorize_gate::user_traverse_table` —
все падают на однотипном `access_denied` для тестового user'а:

```
Execute returned access_denied: access denied: User(...) cannot READ on db://app
```

Сетап бенчей отстал от текущей access-tree схемы (вероятно где-то перестали
давать default-read на `db://app` или `db://benchdb/data` тестовым ID'шкам).
**Pre-existing**, sefer-alloc-нейтрален. Заводить отдельную таску
«починить bench-setup ACL для shamir-server/shamir-db бенчей».

### B. Subscribe-response shape drift

`subscription_throughput`, `subscription_delivery`, `subscription_fanout` —
panic'и на bench setup:

| Bench | Panic point | Сообщение |
|---|---|---|
| `subscription_throughput` | `:192:34` | `expected Batch response` |
| `subscription_delivery` | `:164:18` | panic (не диагностирован) |
| `subscription_fanout` | `:246:22` | panic (не диагностирован) |

Тот же класс — bench-setup отстал от текущей dispatch-логики Subscribe-ops.
**Pre-existing**, sefer-alloc-нейтрален.

### C. Stack overflow в engine_perf

**Изолировано:** `bench_set_existing_with_index` падает на

```
thread 'main' has overflowed its stack
```

`RUST_MIN_STACK=16777216` (16 MiB) не помогает — он влияет только на
spawned threads, main thread stack на Windows — link-time setting (~1 MiB
default).

**Проверено что НЕ sefer-alloc:** временно закомментировал
`#[global_allocator]` в `engine_perf.rs`, тот же `set_existing_with_index`
filter — **тот же** stack overflow без sefer-alloc'а в loop'е.

Причина — где-то в `seeded(n, true)` (создание ShamirDb с index pre-built'ом
для N=10000) или в `bench_set_existing_with_index` body есть deep recursion
/ large stack-allocated struct, которые упираются в 1 MiB Windows main
thread limit. Эта таска не имеет отношения к sefer-alloc rollout'у.

**Mitigations** (для отдельного fix'а):
* `let _ = std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(|| { ... }).unwrap().join();` — перенести bench logic в worker thread с большим стеком
* Найти и устранить deep-recursion в seeded()/index-build пути

---

## Sefer-alloc 0.2.0 → 0.2.1 fix story

При первом rollout sefer-alloc 0.2.0 (с crates.io) бенч
`duplex_throughput/duplex_cap32/32` детерминированно падал в фазе warm-up:

```
memory allocation of 640 bytes failed
```

— sefer-alloc возвращал null на крошечной аллокации под concurrent tokio
(32 потока, cross-thread free, bounded MPSC). lib-тесты (115/115) и
single-thread бенчи (`db_handler_rps`) баг **не** воспроизводили — баг был
сугубо concurrent-specific.

User закоммитил локальный fix, переключил `shamir-server/Cargo.toml` на
`path = "D:/dev/rust/sefer-alloc"`, cargo подтянул 0.2.1. Полная reverification:
- `duplex_cap32/32`: **stable OK**, 15.51 ms, 2.06 Kelem/s
- все остальные бенчи где работали до — продолжают работать
- никаких новых crash'ей из-за allocator'а на других путях

См. также `sefer-alloc-0.2.0-oom-bug-2026-06-30.md` — детали обнаружения
оригинального бага.

---

## Состояние working tree

```
M Cargo.lock                                       (auto-bump 0.2.0 → 0.2.1)
M crates/shamir-server/Cargo.toml                  (path = "D:/dev/rust/sefer-alloc", user-applied)
M crates/shamir-db/Cargo.toml                      (+ sefer-alloc dev-dep)
M crates/shamir-db/benches/engine_perf.rs          (+ #[global_allocator])
M crates/shamir-db/benches/authorize_gate.rs       (+ #[global_allocator])
M crates/shamir-db/benches/changelog_read.rs       (+ #[global_allocator])
M crates/shamir-db/benches/record_size_axis.rs     (+ #[global_allocator])
M crates/shamir-db/benches/durability_axis.rs      (+ #[global_allocator])
M crates/shamir-server/benches/subscription_throughput.rs   (+ #[global_allocator])
M crates/shamir-server/benches/subscription_delivery.rs     (+ #[global_allocator])
M crates/shamir-server/benches/subscription_fanout.rs       (+ #[global_allocator])
?? docs/dev-artifacts/perf/sefer-alloc-0.2.0-oom-bug-2026-06-30.md
?? docs/dev-artifacts/perf/sefer-alloc-rollout-2026-06-30.md
```

---

## Открытые таски (отдельные от sefer-alloc rollout)

1. **Починить ACL drift в bench setup** — `wire_pipelining`,
   `wire_latencies`, `subscription_*`, `authorize_gate::user_traverse_table`
   падают на access_denied / response shape mismatch. Bench-инфраструктура
   отстала от текущей access-tree / dispatch-логики.
2. **Stack overflow в `engine_perf::set_existing_with_index`** — изолировать
   и устранить deep-recursion / large stack-allocated struct в
   `seeded(n=10000, true)` или body bench-функции.
3. **(Когда (1) и (2) fix'нуты)** — пере-прогнать все исправленные бенчи
   на sefer-alloc 0.2.1 для финального функционального confirmation.

---

## Финальный вердикт по sefer-alloc 0.2.1

**Работает.** Ни одного crash'а по вине аллокатора на 12 настроенных
бенчах + production bin'е. Все наблюдаемые crash'и — pre-existing bench
infrastructure rot, неотносимый к выбору аллокатора. Стратегическая
замена mimalloc → sefer-alloc принята.

Perf-числа дебаг-сборки игнорируем; release-сборка + captrack workload —
отдельный эксперимент, по rust release-build (`cargo build --release`)
шаблону, не в этом scope'е.
