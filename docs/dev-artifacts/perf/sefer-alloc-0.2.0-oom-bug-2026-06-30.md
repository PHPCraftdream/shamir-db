בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# sefer-alloc 0.2.0 — `memory allocation of 640 bytes failed` под concurrent tokio-нагрузкой

> **UPDATE (2026-06-30, later):** **исправлено в 0.2.1** (local-path версия
> `D:/dev/rust/sefer-alloc`). Воспроизведение на тех же бенч-аргументах
> теперь стабильно проходит — см. `sefer-alloc-rollout-2026-06-30.md` за
> результирующими измерениями. Этот документ оставлен как запись того,
> как баг был пойман и охарактеризован.

**Дата:** 2026-06-30
**Версия:** `sefer-alloc 0.2.0`, features = `["production"]` (= `alloc-global + alloc-xthread + alloc-decommit`)
**Host:** Windows 10 Pro x86-64, rustc 1.93.0
**Workspace:** S.H.A.M.I.R. Database (private)
**Severity:** **availability** — аллокатор возвращает `null` на крошечной аллокации в нормальной concurrent-нагрузке, Rust runtime аварийно завершает процесс.

---

## TL;DR

При переключении глобального аллокатора с `mimalloc` на `sefer-alloc 0.2.0`
бенчмарк, моделирующий **32 параллельные TCP-сессии × 32 batched
request'а каждая** через tokio multi-thread runtime, детерминированно
падает в фазе measurement:

```
memory allocation of 640 bytes failed
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
error: bench failed
```

640 байт — это явно не реальный OOM на машине с 32 GiB RAM; это либо
sefer-alloc вернул null из своего fast-path, либо внутренний invariant
сломался. Bench с `mimalloc` в той же сборке проходит без проблем.

---

## Как поймали

### Сетап

В рамках стратегического переключения с FFI-аллокатора `mimalloc` (0.1.x)
на native-Rust `sefer-alloc` 0.2.0 был применён swap:

* `crates/shamir-server/Cargo.toml`:
  `mimalloc = "0.1"` → `sefer-alloc = { version = "0.2.0", features = ["production"] }`

* `crates/shamir-server/src/main.rs` — production-tuned `LargeCacheConfig`:

  ```rust
  const ALLOCATOR_CONFIG: sefer_alloc::LargeCacheConfig = sefer_alloc::LargeCacheConfig::new()
      .budget_bytes(2 * 1024 * 1024 * 1024)
      .headroom_bytes(512 * 1024 * 1024)
      .decay_interval_ms(500)
      .decay_rate_percent(25)
      .mode(sefer_alloc::LargeCacheMode::Lazy);

  #[global_allocator]
  static GLOBAL: sefer_alloc::SeferAlloc = sefer_alloc::SeferAlloc::with_config(ALLOCATOR_CONFIG);
  ```

* `crates/shamir-server/benches/{db_handler_rps,duplex_throughput,wire_pipelining,wire_latencies}.rs`
  — все 4 файла используют **defaults**:

  ```rust
  #[global_allocator]
  static GLOBAL: sefer_alloc::SeferAlloc = sefer_alloc::SeferAlloc::new();
  ```

### Тесты (lib) — зелёные

```
./scripts/test.sh -p shamir-server
115 tests run: 115 passed, 0 skipped (2.0s)
```

Lib-тесты однопоточные / низкое allocation pressure — баг **не воспроизводят**.

### Bench `db_handler_rps` — три варианта в `db_handler/*` group

```
db_handler/ping_inprocess                            7.78 µs → 9.49 µs (+22%)
db_handler/execute_read_filter_sort_limit_100records 494.65 µs → 647.15 µs (+31%)
db_handler/execute_full_scan_100records              298.95 µs → 381.95 µs (+28%)
```

(Все эти варианты — single-thread in-process. Все прошли без OOM, только
показали perf-регрессию vs mimalloc, что ожидаемый trade-off в пользу
безопасности native-Rust решения.)

### Bench `duplex_throughput` — крэш

Команда:

```
CARGO_TARGET_DIR=…/.cargo-target-bench \
  cargo bench -p shamir-server --bench duplex_throughput -- \
  --sample-size 10 --measurement-time 2 --warm-up-time 1
```

Прогон:

```
duplex_throughput/lockstep_cap1/10         155.37 ms (64 elem/s)          OK
duplex_throughput/duplex_cap32/10           15.475 ms (646 elem/s)        OK
duplex_throughput/lockstep_cap1/32         495.98 ms (64 elem/s)          OK
duplex_throughput/duplex_cap32/32          memory allocation of 640 bytes failed   ✗
```

Падает в фазе **warm-up / collecting samples** именно на варианте
`duplex_cap32/32`:
* `cap32` — bound channel-capacity 32 на in-flight pipelining,
* `/32` — N = 32 batched request'ов на сессию,
* по логике бенча (см. ниже) — 32 одновременные client-сессии.

Воспроизводится детерминированно с тем же фильтром:

```
cargo bench … --bench duplex_throughput -- 'duplex_cap32/32' …
→ same crash, same warm-up phase, same 640 bytes message
```

### Bench shape

`crates/shamir-server/benches/duplex_throughput.rs` поднимает in-memory
ShamirDb, кладёт его за TCP framer, для каждой итерации spawn'ает
N tokio task'ов (по одной на client-сессию), каждая task:

* читает / пишет фреймы через `tokio::net::TcpStream` half'ы,
* пайплайнит N batched requests через bounded `tokio::sync::mpsc` (cap=32 в `duplex_cap32`),
* ждёт N ответов.

То есть профиль аллокаций:
* буферы под msgpack-encode/decode (~сотни байт каждый),
* bounded MPSC channel slot'ы,
* `Vec<u8>` под frame payload,
* `Arc` под session state.

Все аллокации мелкие (≤ 1 KiB), но **многопоточные** через tokio worker'ы
и cross-thread free (lifetime аллокаций пересекает worker boundaries).
Падающие 640 байт укладываются в `fastbin` диапазон.

---

## Гипотеза

Самый правдоподобный сценарий по сигнатуре краша:

1. `fastbin` (производит 640-байт аллокации) исчерпался / задеградировал,
   и fast-path вернул `null` вместо escalating'а на segment allocator.
2. ИЛИ `alloc-xthread` lock-free cross-thread free выявил race: освободившая
   thread видит освобождение в чужом fastbin'е, но `null` приходит на
   попытке достать новый chunk.
3. ИЛИ под Windows + TCG runtime есть edge-case на `alloc-decommit`:
   освобождённые сегменты возвращаются в OS, и следующая аллокация на тот же
   bin не может grow back (Windows `VirtualAlloc` returned NULL but Rust
   handler не катит exception).

Defaults (`SeferAlloc::new()`) в бенчах — без custom `LargeCacheConfig`.
Возможно дефолтные `headroom_bytes(256 MiB)` + `budget_bytes(None /
unbounded)` на 32 параллельных potok'ах с лёгким cross-thread free дают
worst-case на shard-fragmentation. **Это ещё не проверено** — следующий шаг
эксперимента: применить `with_config(...)` в bench-файлах тоже и пере-прогнать.

---

## Что не воспроизводит баг

* Lib-тесты `shamir-server` (115/115 PASS).
* Bench `db_handler_rps` — single-thread in-process RPS, 3 функции.
* Bench `duplex_throughput` варианты `lockstep_cap1/10`, `duplex_cap32/10`,
  `lockstep_cap1/32` — всё OK.
* Тот же бенч на `mimalloc` (прошлый commit) — все варианты OK.

Поверхность бага: **многопоточная** concurrent-нагрузка с **N≥32 client'ами**
+ default-конфигом sefer-alloc'а.

---

## Reproducer (для upstream)

Минимальный standalone reproducer пока **не извлечён** — баг проявляется
внутри workspace'а с шестью локальными path-deps. План на минимальный repro:

```rust
// Cargo.toml
[dependencies]
tokio        = { version = "1", features = ["full"] }
sefer-alloc  = { version = "0.2.0", features = ["production"] }
bytes        = "1"

// main.rs
#[global_allocator]
static GLOBAL: sefer_alloc::SeferAlloc = sefer_alloc::SeferAlloc::new();

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() {
    let mut handles = Vec::new();
    for _ in 0..32 {
        handles.push(tokio::spawn(async {
            for _ in 0..32 {
                // Mix small + medium allocs across thread boundaries.
                let buf: bytes::Bytes = bytes::Bytes::from(vec![0u8; 640]);
                tokio::task::yield_now().await;
                drop(buf);
                tokio::task::yield_now().await;
            }
        }));
    }
    for h in handles { h.await.unwrap(); }
}
```

(Структурная догадка по форме оригинального бенча; ещё не проверено что это
действительно минимальный repro — может потребоваться более точно
смоделировать tokio-mpsc + TCP буфера.)

---

## Окружение

```
Host:              Windows 10 Pro x86-64
CPU:               (n/a)
RAM:               32 GiB
rustc:             1.93.0 (pinned via rust-toolchain.toml)
sefer-alloc:       0.2.0
   features:       ["production"] (= alloc-global + alloc-xthread + alloc-decommit)
tokio:             1.x (full features)
criterion:         0.5
config in bin:     LargeCacheConfig::new()
                     .budget_bytes(2 GiB)
                     .headroom_bytes(512 MiB)
                     .decay_interval_ms(500)
                     .decay_rate_percent(25)
                     .mode(LargeCacheMode::Lazy)
config in benches: SeferAlloc::new()   (defaults)
```

---

## Следующий шаг

* **(A)** применить `with_config(...)` в bench-файлах тоже, пере-прогнать
  `duplex_throughput/duplex_cap32/32` — если падение исчезнет, root cause
  = defaults, тогда документировать обязательность custom config и
  закрыть. Если падение остаётся — bug в крейте.
* **(B)** если (A) не помогает — minimal reproducer + upstream issue.
* **(C)** до выяснения — НЕ выкатывать sefer-alloc на shamir-db бенчи
  (5 файлов: engine_perf, durability_axis, …) и subscription_* (3 файла),
  т.к. они моделируют похожую concurrent-нагрузку.

Текущее состояние в master: коммит `29a58c37` оставляет sefer-alloc на
shamir-server bin + 4 server-бенчей. Working tree чистый. Решение
сохраняется (per "ничего не откатываем"); расширение покрытия — на hold
до выясния root cause.
