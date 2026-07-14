בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: NUMA N1 — LinuxTopology

## Цель

Реализовать `LinuxTopology` в `crates/shamir-numa/src/linux.rs` — продакшен-impl `Topology` trait, использующий `/sys/devices/system/node/*` и `libc` syscalls. Подключить ветку в `detect()`. Добавить cfg-gated integration-тест.

## Контекст

Фаза 1 завершена (commit `cd9e3004`): `Topology` trait, `MockTopology`, `FallbackSingleNodeTopology`, `NodeReplicated<T>`, `CachePadded`, `parse_cpulist`, `detect()`. Сейчас `detect()` всегда возвращает fallback. Источники — `crates/shamir-numa/README.md` + `docs/dev-artifacts/research/NUMA-DESIGN-2026-06-29.md` (раздел §3.3).

Cargo `rust-toolchain.toml` пинит 1.93.0. Windows dev-хост — Linux-код компилится **только** в CI ubuntu-latest. Это часть verification model: ты пишешь, CI проверяет компиляцию `--locked`.

## Что делать

### 1. Cargo deps — target-cfg

В `crates/shamir-numa/Cargo.toml` добавить:

```toml
[target.'cfg(target_os = "linux")'.dependencies]
libc = "0.2"
```

(Версия `0.2` — последняя стабильная семейство; точную минорную не пинить, пусть resolves к свежему.)

### 2. `src/linux.rs` (новый файл)

```rust
//! `LinuxTopology` — продакшен-impl поверх `/sys/devices/system/node/`
//! и libc::sched_*. Включается под cfg(target_os = "linux").
```

Структура:

```rust
pub struct LinuxTopology {
    cores: Vec<Vec<CpuId>>,             // cores[node] = CPUs на ноде
    cpu_to_node: TMap<CpuId, NodeId>,   // обратный индекс для current_node()
}
```

`shamir_collections::TMap` — IndexMap + FxHasher. Если не подходит / не хочется тянуть зависимость — обычный `HashMap` с `BuildHasherDefault<rustc_hash::FxHasher>` или просто `std::collections::HashMap`.

API:

```rust
impl LinuxTopology {
    /// Probe /sys for NUMA topology. Returns Err если /sys/devices/system/node
    /// отсутствует (не Linux / контейнер без NUMA).
    pub fn probe() -> Result<Self, AffinityError>;
}

impl Topology for LinuxTopology {
    fn num_nodes(&self) -> usize;
    fn cores_on_node(&self, node: NodeId) -> &[CpuId];
    fn current_node(&self) -> NodeId;
    fn pin_current_thread_to_node(&self, node: NodeId) -> Result<(), AffinityError>;
}
```

Probe-логика:

1. Прочитать `/sys/devices/system/node/online` (формат тот же что cpulist — переиспользуй `parse_cpulist`!). Это даёт список нод. Если файла нет — вернуть `AffinityError::Unsupported`.
2. Для каждой ноды N прочитать `/sys/devices/system/node/nodeN/cpulist`, распарсить через `parse_cpulist`.
3. Построить `cpu_to_node` обратный map.

`current_node`:
- Вызвать `libc::sched_getcpu()` → `i32` (CPU id).
- Lookup в `cpu_to_node`. Если miss — вернуть `NodeId(0)` (fallback вместо паники).

`pin_current_thread_to_node`:
- Проверить `node.0 < self.cores.len()` → иначе `NodeOutOfRange`.
- Построить `libc::cpu_set_t` (zero-initialized), добавить все CPUs из `cores_on_node(node)` через `libc::CPU_SET`.
- Вызвать `libc::sched_setaffinity(0 /* current thread */, mem::size_of::<libc::cpu_set_t>(), &cpu_set as *const _)`.
- Если ret != 0 → `AffinityError::Syscall(io::Error::last_os_error())`.

### 3. `src/lib.rs` — cfg-gated re-export

```rust
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::LinuxTopology;
```

### 4. `src/detect.rs` — Linux branch

```rust
#[cfg(target_os = "linux")]
pub fn detect() -> Arc<dyn Topology> {
    if let Ok(topo) = crate::linux::LinuxTopology::probe() {
        if topo.num_nodes() > 0 {
            return Arc::new(topo);
        }
    }
    Arc::new(FallbackSingleNodeTopology::detect())
}

#[cfg(not(target_os = "linux"))]
pub fn detect() -> Arc<dyn Topology> {
    Arc::new(FallbackSingleNodeTopology::detect())
}
```

(Сохрани форму существующей `detect()` и просто разветви.)

### 5. Тесты

`src/linux.rs` snizu:

```rust
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn probe_on_real_linux_host_succeeds() {
        let topo = LinuxTopology::probe().expect("Linux host must expose /sys/devices/system/node");
        assert!(topo.num_nodes() >= 1);
        let node0 = NodeId(0);
        assert!(!topo.cores_on_node(node0).is_empty(), "node 0 must have at least one CPU");
    }

    #[test]
    fn current_node_is_in_range() {
        let topo = LinuxTopology::probe().unwrap();
        let n = topo.current_node();
        assert!(n.0 < topo.num_nodes());
    }
}
```

Integration test `tests/linux_topology.rs` (cfg-gated):

```rust
#![cfg(target_os = "linux")]

use shamir_numa::detect;

#[test]
fn detect_returns_non_empty_topology() {
    let topo = detect();
    assert!(topo.num_nodes() >= 1);
}

#[test]
fn pin_to_node_zero_succeeds() {
    let topo = detect();
    topo.pin_current_thread_to_node(shamir_numa::NodeId(0))
        .expect("pin to node 0 should succeed on any Linux host");
}
```

### 6. Verify

Локально (Windows):

```
./scripts/test.sh -p shamir-numa
```

Должно зелено (Linux-код cfg-gated, не компилится тут).

В CI (matrix включает ubuntu-latest) `numa.yml` tier1-mock проверит компиляцию + run. Это — твой ground truth для Linux-сборки.

## Discipline

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, или любую git-команду, мутирующую working tree или индекс. Только редактирование файлов; orchestrator коммитит.

- One file = one primary export — `linux.rs` владеет `LinuxTopology` (и его inline `tests` модулем, если оставляешь).
- Imports at the top of file.
- `unsafe { libc::sched_setaffinity(...) }` — обязательный `// SAFETY: ...` комментарий с инвариантами (size_of cpu_set_t матчится с C ABI; pointer валиден на длительности вызова).
- `Result<T, AffinityError>` — никаких panic'ов в hot path. Используй существующие варианты enum'а; добавь новый только если genuinely нужен.
- Сохрани кодстайл существующих файлов в `shamir-numa/src/*.rs` (rustdoc-стиль, имена, тон).
- `cargo fmt -p shamir-numa` после правок.

## Done = 

1. `crates/shamir-numa/src/linux.rs` существует и имеет `pub struct LinuxTopology` + impl блоки.
2. `Cargo.toml` имеет target-cfg libc dep.
3. `lib.rs` cfg-gated re-export.
4. `detect.rs` ветвится по cfg(target_os).
5. `tests/linux_topology.rs` существует.
6. `./scripts/test.sh -p shamir-numa` зелёный на Windows (mock + fallback + cpulist + cache_padded + node_replicated).
7. `cargo fmt --all -- --check` clean.
8. Файлы оставлены uncommitted (твоя зона — реализация; коммитит orchestrator).
