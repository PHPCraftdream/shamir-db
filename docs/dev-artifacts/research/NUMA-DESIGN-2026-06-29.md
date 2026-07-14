בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# NUMA-aware S.H.A.M.I.R. — research + design (#287)

> **Дата:** 2026-06-29.
> **Цель:** сформулировать архитектуру для эффективной работы на multi-socket
> (NUMA) системах: какие подсистемы дают выигрыш, как их тестировать без
> доступа к реальному multi-socket железу, как встроить в CI.
> **Метод:** формальные источники (учебники + первичные статьи) → анализ
> применимости к нашему коду → конкретное предложение архитектуры
> (`shamir-numa` крейт) → стратегия тестирования (DI mock + QEMU emulation +
> GitHub Actions).

---

## 1. Академический фундамент

### 1.1 Что такое NUMA (формально)

**NUMA = Non-Uniform Memory Access.** Класс multiprocessor-архитектур, где
доступ к памяти имеет **переменную стоимость** в зависимости от того, на
каком сокете расположена ячейка и с какого ядра идёт обращение. Контраст —
**UMA** (Uniform Memory Access, SMP старого образца с общей шиной), где
все ядра имели одинаковую латентность.

**Каноническая модель (учебник):** Hennessy & Patterson, *Computer
Architecture: A Quantitative Approach*, 6-е изд., §5.2 «Centralized
Shared-Memory Architectures» + §5.3 «Distributed Shared Memory and
Directory-Based Coherence». Архитектура NUMA — частный случай DSM с
аппаратной cache-coherence (ccNUMA). Ключевая характеристика — **memory
locality factor** (NUMA factor): отношение latency удалённой памяти к
локальной. Для современных серверов (Intel Sapphire Rapids, AMD Genoa):
local ~80 ns / remote ~140–200 ns → factor ≈ 1.75–2.5×.

**Bandwidth-асимметрия:** local channel — полная BW канала (≈100 GB/s на
сокет); remote — деление inter-socket interconnect (Intel UPI / AMD
Infinity Fabric, 50–100 GB/s, **делится между всеми cross-socket потоками**).

**Curt Schimmel, *UNIX Systems for Modern Architectures*** (Addison-Wesley,
1994), главы 8–10 — классическое введение в memory consistency, cache
coherence, и NUMA-планирование на уровне ядра ОС.

### 1.2 Что должен знать программист — Drepper

**Ulrich Drepper, «What Every Programmer Should Know About Memory»**
(Red Hat, 2007 — обновлено в LWN). §5 «NUMA Support». Обязательное
чтение. Конкретно для нас релевантно:

- **§5.1** — два режима OS-аллокации: «first-touch» (страница уходит
  на сокет, который первым её записал) vs explicit binding через
  `mbind(2)` / `numa_alloc_onnode`.
- **§5.2** — `sched_setaffinity` для пиннинга потоков, `getcpu(2)` для
  узнавания где мы сейчас.
- **§5.3** — published `/sys/devices/system/node/nodeN/` иерархия как
  source-of-truth для discovery.

### 1.3 Lock-free и cache coherence — Herlihy/Shavit

**Maurice Herlihy, Nir Shavit, *The Art of Multiprocessor Programming***,
2-е изд., главы 7–8 (locks, memory consistency). Ключевое для NUMA:
**false sharing** — две независимые переменные на одной cacheline'е
становятся источником пинг-понга MESI/MOESI протокола между сокетами.
Решение — `#[repr(align(64))]` (или 128 для подавления adjacent-line
prefetch) на конкурирующих полях.

### 1.4 Databases on NUMA — первичные статьи

**Manegold, Boncz, Kersten (CWI Amsterdam):**
- *Database Architecture Optimized for the New Bottleneck: Memory Access*
  (VLDB 1999) — введение проблематики memory hierarchy в DB.
- *Optimizing Database Architecture for the New Bottleneck: Memory
  Access* (VLDB Journal 2000) — расширенная версия.
- *MonetDB/X100: Hyper-Pipelining Query Execution* (CIDR 2005) — Boncz et
  al, демонстрирует per-thread vectorized execution с NUMA-affinity.

**ScyllaDB shared-nothing model** (Avi Kivity et al, ScyllaDB engineering
blog 2014+): «один shard на одно ядро, никаких блокировок между shard'ами,
NUMA-aware распределение данных по сокетам». Модель применима к
shard-per-table или shard-per-key-range архитектурам. Документация
ScyllaDB остаётся канонической для DB-инженеров.

**Porobic et al, *OLTP on Hardware Islands*** (VLDB 2012) — измеренный
эффект NUMA-affinity на OLTP workloads: до 2–3× throughput от правильной
привязки txn-coordinator'ов к сокетам.

**Leis et al, *Morsel-Driven Parallelism***, (SIGMOD 2014, TU Munich) —
NUMA-aware work-stealing для аналитических запросов. Идея «morsel-of-work»
с per-socket task pool.

### 1.5 Сводка — что забирать из учебников

| Принцип | Источник | Применимо к нам |
|---|---|---|
| First-touch allocation | Drepper §5.1 | Worker-init time matters — где первая запись |
| Thread pinning через `sched_setaffinity` | Drepper §5.2 | Pin критических thread'ов (WAL writer, Drainer) к одному узлу |
| Per-node memory binding (`mbind`) | Drepper §5.1 | Per-node Arc-replicas read-mostly state |
| False sharing — align to cache line | Herlihy/Shavit §8 | `#[repr(align(64))]` на конкурирующих счётчиках |
| Shared-nothing per-core sharding | ScyllaDB | Sharding model для долгосрочной цели |
| Morsel-driven NUMA-local pools | Leis et al | Pattern для query execution |
| Topology discovery через `/sys` | Linux ABI | Базовая инфраструктура |

---

## 2. Применимость к S.H.A.M.I.R.

### 2.1 Где NUMA-stride реально болит у нас сейчас

Анализ по hot-path'ам кампании ②:

**(A) Read-mostly registries — 9 ArcSwap-точек уже в коде.** Это
идеальные кандидаты per-node replication: один Arc на сокет, читатель
берёт свой локальный → нулевая cross-socket latency на read-path,
synchronized COW на write-path:

- `shamir-engine/src/table/table_manager_validators.rs` — validator_bindings (#289)
- `shamir-engine/src/table/interner_manager.rs` — interner snapshots
- `shamir-engine/src/table/table_manager.rs` — общие table-config Arc'и
- `shamir-index/src/legacy/index_info.rs` — IndexInfo (#292)
- `shamir-index/src/legacy/sorted_index_manager.rs` — SortedIndexManager (#304)
- `shamir-index/src/actor.rs`, `functional_backend.rs`, `lib.rs` — backend registries
- `shamir-connect/src/server/rotation.rs` — TLS rotation

После кампании ② весь read-mostly state уже в ArcSwap. **Шаг к NUMA
тривиален**: завернуть в `NodeReplicated<T>` (см. §4).

**(B) Per-thread worker pools — tokio multi-thread runtime.** Tokio по
дефолту запускает W worker'ов и parking-стратегией ходит по ним. **Не**
NUMA-aware: worker thread может стартовать на сокете 0, потом мигрировать
на сокет 1 ОС-планировщиком.

Tokio имеет experimental `LocalRuntime` (per-thread), его можно по одному
на сокет с pinned worker'ами — морсель-driven pattern.

**(C) WAL writer — single-threaded leader.** `WalSegment` имеет один
write-handle под `Mutex<File>`, append-only. Должен быть pinned на ОДНОМ
конкретном сокете — там, где живут его буферы и куда привязан
fsync-thread пула.

**(D) Drainer — long-running task с тяжёлым I/O.** Drainer'у выгодно
быть pinned на тот же сокет, где работает inner storage backend
(sled/fjall имеют свои thread-pool'ы для compaction/flush).

**(E) MvccStore — read-mostly cells map + write-on-commit.** Это самая
сложная структура. Per-node replication неприменим (есть mutations при
каждом write). Можно sharding-by-key-range per node (как ScyllaDB), но
это **большой redesign**, требует separate effort.

### 2.2 Где NUMA НЕ поможет

- Очень коротко-живущие транзакции (микросекунды) — overhead pinning'а
  больше выгоды локальности.
- Single-socket развёртывание (большинство dev-машин, многие prod
  серверы средних размеров) — feature должна быть **no-op**, не тормозить.
- WASM-host (CPU-bound user code) — там доминируют JIT-затраты, не memory.

### 2.3 Концептуальная иерархия выигрыша

```
ROI убывает сверху вниз:

  high  ║  per-node Arc-replicas всех read-mostly registries (9 точек) ◄ старт
        ║  pinned WAL writer + drainer
        ║  cache-line padding конкурирующих atomics (#289-style mirror counters)
        ║  per-node memory pools (custom allocator gating)
  low   ║  per-node MvccStore sharding (большой redesign)
```

---

## 3. Архитектурное предложение

### 3.1 Новый крейт `shamir-numa`

Изолирует ВСЮ NUMA-логику. Зависимость одного направления:
`shamir-numa → shamir-types` (если нужны общие type aliases) либо вообще
без зависимостей внутри workspace'а. Cargo-feature `numa` опционален в
потребителях.

```
crates/shamir-numa/
├── Cargo.toml
├── src/
│   ├── lib.rs               — re-exports
│   ├── topology.rs          — Topology trait + impls
│   ├── node_replicated.rs   — NodeReplicated<T> primitive
│   ├── affinity.rs          — pin_thread_to_node, current_node
│   └── allocator.rs         — (опц.) NodeAlloc — будущее расширение
└── tests/
    ├── topology_mock.rs     — DI mock-based unit tests
    └── replicated.rs        — semantic tests on NodeReplicated
```

### 3.2 Topology trait (фундамент DI)

```rust
pub trait Topology: Send + Sync {
    /// Количество NUMA-нод. На single-socket системах = 1.
    fn num_nodes(&self) -> usize;

    /// Какие CPU-cores принадлежат node'у. Возвращает CpuId per node.
    fn cores_on_node(&self, node: NodeId) -> &[CpuId];

    /// Текущий node вызывающего потока. Best-effort; на single-socket
    /// или non-Linux всегда возвращает NodeId(0).
    fn current_node(&self) -> NodeId;

    /// Привязать вызывающий поток к node'у. Идемпотентно. Возвращает
    /// Err если не поддерживается на платформе.
    fn pin_current_thread_to_node(&self, node: NodeId) -> Result<(), AffinityError>;
}
```

**Реальные impl'ы (за feature gates):**
- `LinuxTopology` — gated `cfg(target_os = "linux")`, читает
  `/sys/devices/system/node/*` и использует `sched_setaffinity`.
- `FallbackSingleNodeTopology` — для Windows/macOS/single-socket Linux.
  Всегда `num_nodes = 1`, no-op pin.

**DI Mock impl (всегда доступен, под `pub mod mock`):**
- `MockTopology { nodes: Vec<MockNode> }` + `MockNode { cpus: Vec<CpuId> }`.
- `pin_*` — записывает вызов в `Vec<(ThreadId, NodeId)>` для assertions.

### 3.3 NodeReplicated<T> — ключевой примитив

```rust
/// Per-NUMA-node реплика read-mostly данных T.
/// Каждый node имеет свой ArcSwap<T> — читатель node N достаёт реплику
/// без обращения к удалённым cacheline'ам.
///
/// Write-path: COW по всем нодам через rcu (та же модель что и
/// шаблон #292/#304).
pub struct NodeReplicated<T> {
    topology: Arc<dyn Topology>,
    replicas: Vec<ArcSwap<T>>,  // длина = num_nodes
}

impl<T: Clone + Send + Sync + 'static> NodeReplicated<T> {
    pub fn new(topology: Arc<dyn Topology>, initial: T) -> Self { … }

    /// Загрузить snapshot для текущего node'а вызывающего потока.
    /// O(1) под обложкой = ArcSwap::load + Arc::clone.
    pub fn load_local(&self) -> arc_swap::Guard<Arc<T>> {
        let node = self.topology.current_node();
        self.replicas[node.0].load()
    }

    /// Загрузить snapshot конкретного node'а (для cross-node debugging).
    pub fn load_node(&self, node: NodeId) -> arc_swap::Guard<Arc<T>> { … }

    /// COW обновление на всех node'ах через rcu CAS-loop (зеркало #292/#304).
    pub fn rcu(&self, mut f: impl FnMut(&T) -> T) {
        let next = f(&*self.replicas[0].load_full());
        let next = Arc::new(next);
        for replica in &self.replicas { replica.store(next.clone()); }
        // ⚠ ВНИМАНИЕ: cross-node store последовательный; ослабленная
        // консистентность между нодами в течение наносекунд. Acceptable
        // для read-mostly registries (eventual consistency). Для strong
        // consistency нужен extra coordination — out of scope первой
        // итерации.
    }
}
```

**Single-node случай** (Windows dev, single-socket Linux): `num_nodes=1`,
`load_local` идёт по 0-индексу, COW обновляет одну реплику → **поведение
идентично сегодняшнему ArcSwap**. Не регрессирует.

**Multi-node случай**: `num_nodes=N`, каждый node-локальный читатель
не вылезает за пределы своего сокета → memory locality.

### 3.4 Интеграция с существующими ArcSwap'ами

После того как `shamir-numa` существует, миграция из ArcSwap →
NodeReplicated точечная:

**Было:**
```rust
indexes: Arc<ArcSwap<Vec<SortedIndexDefinition>>>,
```

**Стало (за feature gate `numa`):**
```rust
#[cfg(feature = "numa")]
indexes: shamir_numa::NodeReplicated<Vec<SortedIndexDefinition>>,
#[cfg(not(feature = "numa"))]
indexes: Arc<ArcSwap<Vec<SortedIndexDefinition>>>,
```

Или **унифицировано**: NodeReplicated сам деградирует до одной реплики
при `num_nodes=1` → миграция без cfg-gate'ов, всегда NodeReplicated, на
single-socket бесплатно. Это **предпочтительный путь** — меньше cfg-шума.

### 3.5 Pinning критических thread'ов

```rust
pub fn pin_wal_writer_to_node(topo: &dyn Topology, node: NodeId) { … }
pub fn pin_drainer_to_node(topo: &dyn Topology, node: NodeId) { … }
```

Конфигурация: `RepoInstance::config()` получает поле `numa_policy:
NumaPolicy`, дефолт = `NumaPolicy::None` (no-op). Активация в проде —
через env / config.

---

## 4. Стратегия тестирования

### 4.1 Three tiers

**Tier 1 — DI mock unit tests (всегда выполняются).**

`shamir-numa/tests/topology_mock.rs`:
```rust
#[test]
fn replicated_load_uses_current_node() {
    let mock = Arc::new(MockTopology::with_nodes(2)
        .pin_self_to_node(NodeId(1)));  // тест-помощник
    let r = NodeReplicated::new(mock.clone(), vec![42]);
    let snap = r.load_local();
    assert_eq!(&**snap, &vec![42]);
    // Mock записал вызов load на ноду 1 — assert:
    assert_eq!(mock.load_log(), vec![NodeId(1)]);
}

#[test]
fn rcu_updates_all_replicas() {
    let mock = Arc::new(MockTopology::with_nodes(4));
    let r = NodeReplicated::new(mock.clone(), 0u64);
    r.rcu(|v| v + 1);
    for n in 0..4 {
        assert_eq!(&**r.load_node(NodeId(n)), &1u64);
    }
}
```

Покрывает: семантика `NodeReplicated`, корректность `pin_*` API,
diff-detection вкл/выкл NUMA.

**Tier 2 — Linux-only integration tests (опционально, `cfg(target_os =
"linux")`).**

Тестируют `LinuxTopology` против реального `/sys` — на multi-socket
машине проходит реальный кейс, на single-socket даёт num_nodes=1
(тривиальный).

**Tier 3 — QEMU NUMA emulation (для CI).**

QEMU умеет симулировать произвольную NUMA-топологию:
```
qemu-system-x86_64 \
  -smp cpus=4,sockets=2,cores=2 \
  -m 4G \
  -object memory-backend-ram,id=ram0,size=2G \
  -object memory-backend-ram,id=ram1,size=2G \
  -numa node,memdev=ram0,cpus=0-1,nodeid=0 \
  -numa node,memdev=ram1,cpus=2-3,nodeid=1
```

В таком VM `/sys/devices/system/node/` показывает 2 ноды; `numactl
--hardware` подтверждает топологию. `LinuxTopology::probe()` находит
обе, тесты идут по реальному пути pinning'а.

Замечание: QEMU **не моделирует latency-асимметрию** (memory backend
один и тот же RAM хоста). Меряем поведение, не perf. Для perf-тестов
нужно реальное железо.

### 4.2 GitHub Actions CI стратегия

`.github/workflows/numa.yml`:

```yaml
name: numa
on: [push, pull_request]

jobs:
  unit-mock:
    # Бесплатные стандартные runners → DI mock tests
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo test -p shamir-numa --lib

  qemu-numa-emulation:
    # Опционально, медленный (~5 мин boot QEMU + тесты)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install QEMU + KVM
        run: sudo apt-get install -y qemu-system-x86 qemu-utils numactl
      - name: Build test binary
        run: cargo test -p shamir-numa --test linux_topology --no-run
      - name: Boot 2-node QEMU + run integration test
        run: ./scripts/ci-qemu-numa-test.sh

  self-hosted-multi-socket:
    # Опциональный self-hosted runner с реальным multi-socket железом.
    # Off by default; включается label'ом self-hosted.
    runs-on: [self-hosted, numa-2socket]
    if: github.event_name == 'push' && contains(github.event.head_commit.message, '[numa-ci]')
    steps:
      - uses: actions/checkout@v4
      - run: cargo test -p shamir-numa --features numa-real
```

**Realistic положение для open-source:** Tier 1 + Tier 3 (QEMU)
покрывают correctness — DI mock'и проверяют логику работы с топологией,
QEMU подтверждает что реальные syscalls работают на realistic Linux.
Tier 2 self-hosted hardware — bonus.

**Perf-тесты NUMA effect** требуют реального multi-socket железа и
**вне CI**. Это разовые benchmarks локально на dev-машине с двумя
сокетами, не gate.

### 4.3 Risk register тестирования

| Риск | Mitigation |
|---|---|
| QEMU-NUMA не моделирует latency → false positives | Документировать что Tier 3 — correctness, не perf |
| `sched_setaffinity` требует CAP_SYS_NICE на некоторых Docker'ах | CI: добавить `--cap-add SYS_NICE` или skip-on-eperm |
| `/sys/devices/system/node/` отсутствует в контейнере | Fallback to `FallbackSingleNodeTopology`, log warning |
| MockTopology divergence от реальной семантики | Test-double для каждого реального impl'а: один и тот же набор тестов прогонять через Mock И через Linux на CI |

---

## 5. Roadmap (поэтапно)

### Фаза 1 — Фундамент (≈3 дня, ~700 LOC)

1. `shamir-numa` крейт: `Topology` trait + `MockTopology` + `FallbackSingleNodeTopology`.
2. `NodeReplicated<T>` с тестами через MockTopology.
3. `LinuxTopology` за `cfg(target_os = "linux")`.
4. CI workflow (Tier 1 unit-mock + Tier 3 QEMU).

**Deliverable:** библиотека готова к подключению, тесты зелёные.
**Риск low** — изолированная новая инфраструктура.

### Фаза 2 — Интеграция existing ArcSwap'ов (≈2 дня, ~200 LOC)

Замена `Arc<ArcSwap<T>>` на `NodeReplicated<T>` в 2–3 самых горячих
точках:
1. `IndexInfo` (#292) — главный win прошлой кампании.
2. `SortedIndexManager::indexes` (#304).
3. `validator_bindings` (#289).

Multi-socket bench (на реальном железе, не CI) ожидает дополнительные
**~10–20% throughput** под concurrent read-heavy нагрузкой на 2-socket
машине. На single-socket — null delta (что и хотим).

### Фаза 3 — Thread pinning (≈1 день, ~150 LOC)

`pin_wal_writer_to_node`, `pin_drainer_to_node`. Активация через
конфиг. Default off.

### Фаза 4 — (опционально, не сейчас) — Per-node allocator pools

`NodeAlloc`-обёртка над mimalloc/jemalloc с per-arena-per-node config.
Большая работа, отдельный спринт. Не часть #287.

---

## 6. Решения, зафиксированные сейчас

- **`shamir-numa` — отдельный крейт**, не модуль внутри shamir-types/etc.
  Изоляция NUMA-specific code; легко удалить если не пригодится; чёткая
  feature-граница.
- **NodeReplicated деградирует до single-replica на single-socket**.
  Никаких cfg-gate'ов у потребителей — `NodeReplicated<T>` универсален.
  Reject: оставить старый ArcSwap для single-socket и cfg-switch — это
  привносит cfg-шум во все потребители.
- **DI Mock — приоритетный testability path**, не QEMU. Mock покрывает
  логику Topology consumer'ов; QEMU подтверждает интеграцию с Linux ABI.
  Reject: ставить только QEMU — медленно для дев-цикла, не работает
  на Windows.
- **Pinning через конфиг, не constructor**. `RepoInstance::config` получает
  `numa_policy` поле; реальный pinning происходит в worker-init.
  Reject: hardcoded pinning в конструкторах — менее тестируемо, меньше
  гибкости.
- **Cross-socket strong consistency — out of scope первой итерации**.
  `NodeReplicated::rcu` обеспечивает eventual consistency между нодами
  (наносекундное расхождение). Подходит для read-mostly registries; для
  ситуаций требующих strong cross-node consistency нужен extra
  coordination — отдельный design effort.

## 7. Open questions

- **Какой ABI для `current_node`?** На Linux — `getcpu(2)`. На Windows
  ABI существует (`GetCurrentProcessorNumberEx`), но Windows NUMA крайне
  редко конфигурируется в наших prod-сценариях. Решение: Linux-first,
  Windows = `FallbackSingleNodeTopology` (всегда 0). Подтвердить.
- **`hwloc` (через `hwlocality` крейт) или прямой `/sys` парсинг?**
  Hwloc — battle-tested, кроссплатформенный (включая Mac/Win/BSD), но
  C-bindings и runtime-dep. Прямой `/sys` — zero deps, Linux-only, наш
  prod-таргет. Я склоняюсь к direct `/sys` для start; hwloc можно
  опциональным feature для тех кто хочет macOS-support.
- **Self-hosted CI runner с multi-socket железом — стоит организовывать?**
  Платно или volunteer-machine. Для OSS-проекта первой итерации не
  обязательно. Решить когда дойдёт.

## 8. Источники (обязательное чтение для контрибутора)

1. Hennessy & Patterson, *Computer Architecture: A Quantitative Approach*,
   6th ed., Morgan Kaufmann 2019. §5.2–5.3.
2. Curt Schimmel, *UNIX Systems for Modern Architectures*, Addison-Wesley
   1994. Главы 8–10.
3. Ulrich Drepper, *What Every Programmer Should Know About Memory*,
   2007 (LWN), §5.
4. Herlihy & Shavit, *The Art of Multiprocessor Programming*, 2nd ed.,
   2020. §7–8.
5. Manegold/Boncz/Kersten, *Database Architecture Optimized for the New
   Bottleneck*, VLDB 1999/2000.
6. Porobic, Pandis, Branco, Tözün, Ailamaki, *OLTP on Hardware Islands*,
   VLDB 2012.
7. Leis et al, *Morsel-Driven Parallelism: A NUMA-Aware Query Evaluation
   Framework for the Many-Core Age*, SIGMOD 2014.
8. Linux kernel docs: `Documentation/admin-guide/mm/numa_memory_policy.rst`,
   `Documentation/ABI/stable/sysfs-devices-node`.
9. QEMU NUMA emulation: `docs/system/i386/numa.rst` в QEMU source tree.

---

## 9. Что делать сейчас

Этот документ — **fundament**. Следующий шаг — **скелет крейта
`shamir-numa`** (Фаза 1 из §5), который:
- Заводит `Topology` trait + `MockTopology` + `FallbackSingleNodeTopology`.
- Заводит `NodeReplicated<T>`.
- Прогоняет тесты (Tier 1 unit-mock).
- Заводит CI-workflow Tier 1 + Tier 3-skeleton (без QEMU-script — это
  отдельный шаг).

`LinuxTopology` и реальная QEMU-интеграция — следующая итерация.
Интеграция в существующие ArcSwap'ы (Фаза 2) — отдельный /opti цикл
после Фазы 1.

Стоп-точка: после §5 Фаза 1. Перед тем как трогать production-код
(Фаза 2) — мерить на реальном multi-socket железе. Без замера — нет
основания заявлять выгоду.
