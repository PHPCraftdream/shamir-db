בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-06-29 [numa-research]

## Session summary

Перф-кампания ② закрыта (см. предыдущий чекпоинт
`2026-06-29-perf-campaign-2-close.md`); главные победы #292 (IndexInfo
DashMap→ArcSwap, **−38.84%/1.63×**) и #304 (SortedIndexManager аналогично,
multi-thread **−13.09%**); #291/#305 закрыты как ложные кандидаты по
профилю.

Текущая сессия — переход к #287 (NUMA-aware): пользователь явно попросил
«формально по учебникам» сделать код для multi-socket систем, с
симуляцией и тестами (DI mock / GitHub pipelines). Это исследовательский
трек с конкретным выходом (код + тесты), не /opti цикл.

**Текущее состояние:** написал research-doc
`docs/research/NUMA-DESIGN-2026-06-29.md` (~330 строк) с фундаментом —
**8 формальных источников** (Hennessy & Patterson, Curt Schimmel, Drepper
«What Every Programmer Should Know About Memory» §5, Herlihy/Shavit,
Manegold/Boncz CWI, Porobic OLTP-on-Hardware-Islands VLDB'12, Leis
Morsel-Driven SIGMOD'14, Linux kernel docs). Структура: §1 фундамент →
§2 применимость к нам (9 ArcSwap-точек уже есть, идеальные кандидаты на
per-node replication) → §3 архитектура `shamir-numa` крейта → §4 three
tiers тестирования (DI mock + Linux integration + QEMU NUMA-emulation для
CI) → §5 roadmap 4 фазы → §6 решения зафиксированы → §7 open questions.

**Ключевое архитектурное решение** — `NodeReplicated<T>` примитив с
автоматической деградацией до single-replica на single-socket. Никаких
cfg-gate'ов у потребителей — единый API всегда; на single-socket
поведение идентично сегодняшнему ArcSwap (нулевая регрессия).

**Стопорная точка:** жду решения пользователя — писать ли сейчас скелет
`shamir-numa` крейта (Фаза 1 из §5, ~700 LOC, ~3 дня: Topology trait +
MockTopology + FallbackSingleNodeTopology + NodeReplicated<T> + tier-1
mock-based тесты + CI workflow), либо ревью research-doc сначала.
Production-код пока не трогается; интеграция в существующие ArcSwap'ы —
Фаза 2, отдельный /opti цикл с замером на реальном multi-socket железе
(пока его нет — заявлять выгоду нельзя).

**Uncommitted состояние:** §10–§12 закрытого research-дока перф-кампании
(`WRITE-HOT-PATH-PROFILE-2026-06-28.md`) + SVG flamegraph membuffer'а +
этот чекпоинт + новый NUMA-research-doc. Пользователь сказал «машина
шумная, параллельные бенчи в другом проекте» → отложили коммит §12+SVG
до возможного перезамера. Differential-результаты commit'ов (#292 1.63×,
#304 multi-thread −13%) уже запушены, перепроверять не надо (p=0.00,
ranges non-overlapping, устойчивы к шуму).

**Read-файлы сессии (NUMA-трек):** существующие ArcSwap-сайты
(`shamir-engine/table/{table_manager,interner_manager,table_manager_validators}.rs`,
`shamir-index/legacy/{index_info,sorted_index_manager}.rs`,
`shamir-connect/server/rotation.rs`, `shamir-index/{actor,functional_backend,lib}.rs`).

**Таймеры:** нет активных (`/babysit` снят в прошлой сессии).
**Active goal:** нет (никаких Stop-hook условий).

## Active goal
none

## TaskList

### in_progress
- #287 Исследовать NUMA-aware реализацию работы на нескольких процессорах

### pending
(пусто)

### recently completed (last sessions)
- #292 IndexInfo → ArcSwap (perf-campaign-2)
- #303 Windows WAL TOCTOU race (correctness fix)
- #304 SortedIndexManager → ArcSwap (perf-campaign-2)
- #291 / #305 закрыты как false candidates по профилю
- ранее: #288–#302 (captrack-кампания, часть landed, часть reverted)

Удалённые таски в этой сессии: 5 (#291/#292/#303/#304/#305).

## Decisions

- **#287 — research-doc первым, потом скелет.** Без формального фундамента
  код был бы догадками. Reject: сразу писать `shamir-numa` Cargo.toml.
- **`NodeReplicated<T>` деградирует до single-replica, никаких cfg-gate'ов
  у потребителей.** Reject: оставить старый `Arc<ArcSwap<T>>` для
  single-socket с cfg-switch — приносит cfg-шум во все 9 потребителей.
- **DI Mock — приоритетный testability path, не QEMU.** QEMU остаётся как
  Tier 3 (correctness, не perf), DI mock покрывает всю Topology-логику без
  multi-socket железа. Reject: ставить только QEMU — медленный дев-цикл +
  не работает на Windows-разработчике.
- **Pinning через конфиг (`RepoInstance::config.numa_policy`), не
  hardcoded в конструкторах.** Reject: hardcoded pinning — менее
  тестируемо, меньше гибкости.
- **Cross-socket strong consistency — out of scope первой итерации.**
  `NodeReplicated::rcu` гарантирует eventual consistency (наносекундное
  расхождение между нодами на время COW). Для read-mostly registries
  достаточно. Strong consistency = отдельный design effort. Reject:
  строить barrier-coordination сразу.

## Open questions

- **Hwloc (через `hwlocality` крейт) vs direct `/sys` парсинг?** Hwloc —
  cross-platform, но C-bindings runtime-dep. Direct `/sys` — zero deps,
  Linux-only (наш prod-таргет). Склоняюсь к direct `/sys`; hwloc как
  опциональный feature для macOS-support.
- **Windows-стратегия для `current_node`?** ABI существует
  (`GetCurrentProcessorNumberEx`), но Windows multi-socket в нашем prod
  крайне редок. План: Linux-first, Windows = `FallbackSingleNodeTopology`
  (всегда 0). Подтвердить.
- **Self-hosted CI runner с multi-socket железом?** Платно или
  volunteer-machine. Для OSS первой итерации не обязательно. Tier 1
  (mock) + Tier 3 (QEMU) дают correctness; perf-тесты вне CI на dev
  multi-socket box'е.
- **Коммитить ли §12 + SVG membuffer-flamegraph'а?** Отложено до
  перезамера на чистой машине (шумная). Профиль qualitativно устойчив,
  но осторожность ради честности. Возможный путь — закоммитить только
  §12 (текст) с явным disclaimer про шум; SVG не коммитить.

## Repo state

```
 M docs/research/WRITE-HOT-PATH-PROFILE-2026-06-28.md
?? .flamegraphs/membuffer-pump-frequent-flush-2026-06-29.svg
?? docs/checkpoints/2026-06-29-perf-campaign-2-close.md
?? docs/research/NUMA-DESIGN-2026-06-29.md
```

```
1c382fcf docs(research): §10 #291 closed + §11 анализ #304/#305
acf992cb perf(index): #304 SortedIndexManager DashMap → ArcSwap<Vec<SortedIndexDefinition>>
93e03ffc fix(wal): #303 Windows TOCTOU between SegmentSet::replay snapshot и truncate_below
3f9bcbb6 test(engine): flake mitigation — serialize env-var + multi_thread stress
7bb5d392 perf(index): #292 IndexInfo DashMap → ArcSwap<Vec<IndexDefinition>>
```

master в синке с origin/master (0 коммитов ahead). Working tree —
4 untracked/modified doc-артефакта, кода не тронуто.
