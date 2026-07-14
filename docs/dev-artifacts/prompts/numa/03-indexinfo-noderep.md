בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: NUMA N3 — IndexInfo ArcSwap → NodeReplicated

## Цель

Заменить `ArcSwap<Vec<IndexDefinition>>` на `NodeReplicated<Vec<IndexDefinition>>` в `crates/shamir-index/src/legacy/index_info.rs`. На single-socket — zero overhead (одна реплика, identical to bare ArcSwap, см. shamir-numa README). На multi-socket — каждая нода читает свою реплику без cross-socket latency.

## Контекст

- История: #292 мигрировал DashMap → ArcSwap (commit `7bb5d392`). Теперь шаг к NodeReplicated.
- Файл — `crates/shamir-index/src/legacy/index_info.rs` (≈250 LOC). Подразумевается одна горячая структура `IndexInfo`.
- `shamir-numa::NodeReplicated<T>` API:
  - `pub fn new(topology: Arc<dyn Topology>, initial: T) -> Self`
  - `pub fn load_local(&self) -> Guard<Arc<T>>` — Guard&lt;Arc&lt;T&gt;&gt;, как у ArcSwap
  - `pub fn load_node(&self, node: NodeId) -> Guard<Arc<T>>` — для тестов
  - `pub fn store(&self, value: T)` — публикует на все реплики
  - `pub fn rcu(&self, f: impl FnMut(&T) -> T)` — CAS-loop, mirror на остальные ноды
  - `pub fn num_replicas(&self) -> usize`
- `shamir-numa::detect()` -> `Arc<dyn Topology>` — фабрика топологии.

## Что делать

### 1. Cargo deps

В `crates/shamir-index/Cargo.toml` добавить:

```toml
shamir-numa = { path = "../shamir-numa" }
```

### 2. `index_info.rs` — поле

Замени:

```rust
indexes: ArcSwap<Vec<IndexDefinition>>,
```

на:

```rust
indexes: shamir_numa::NodeReplicated<Vec<IndexDefinition>>,
```

### 3. Конструкторы

`new()`, `from_definitions()`, `Deserialize`, `Clone` — везде `ArcSwap::from_pointee(vec)` / `ArcSwap::from(arc)` замени на:

```rust
shamir_numa::NodeReplicated::new(shamir_numa::detect(), vec)
```

`detect()` дёшев, но если хочется кэша — один раз `lazy_static`/`OnceLock<Arc<dyn Topology>>` на уровне крейта. Реши по месту, но **без преждевременной оптимизации** — `detect()` per `IndexInfo::new()` ок для Фазы 2.

### 4. Reads — `load()` / `load_full()` mapping

- `self.indexes.load()` → `self.indexes.load_local()`. Возвращает `Guard<Arc<Vec<IndexDefinition>>>` — Deref to `Arc<Vec<…>>`, чейн `.iter()` / `.len()` / `.is_empty()` работают как раньше.
- `self.indexes.load_full()` → нет прямого аналога. Два варианта:
  - **(a)** `Arc::clone(&*self.indexes.load_local())` — клон Arc-снапшота, эквивалент `load_full()`.
  - **(b)** Добавить в `shamir-numa::NodeReplicated` метод `load_full_local()` → `Arc<T>`. Если используешь — также добавь короткий unit-test там.

Сохрани семантику: snapshot Arc должен жить столько, сколько iterator его держит (иначе `iter()` сломает invariant). Вариант (a) полностью эквивалентен ArcSwap-форме; рекомендация — (a), без расширения API. Если выберешь (b) — оставь чистый patch в shamir-numa.

### 5. Writes — `rcu()`

`self.indexes.rcu(|cur| { ... })` остаётся **по форме** — у `NodeReplicated` метод `rcu` с той же сигнатурой `(impl FnMut(&T) -> T)`. Замена one-to-one.

### 6. Serialize / PartialEq / Default / Clone

- `Serialize` через `load_full` — используй (a)-стиль `Arc::clone(&*self.indexes.load_local())`.
- `PartialEq` — тоже через локальные guard'ы; load_local оба, deref, compare.
- `Default` → `Self::new()`.
- `Clone` — взять snapshot Arc и обернуть в новый `NodeReplicated::new(detect(), (*snap).clone())`. **Не** клонируй сам `NodeReplicated` напрямую (он не Clone и не должен быть — это владеющий ресурс с per-node replicas).

### 7. Gate

```
./scripts/test.sh -p shamir-index
cargo fmt -p shamir-index
cargo clippy -p shamir-index --all-targets -- -D warnings
```

Все тесты `shamir-index` должны остаться зелёные (миграция семантически прозрачна).

## Discipline

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`. Только редактирование.

- Surgical scope — только `index_info.rs` + `Cargo.toml`. Никаких попутных рефакторов.
- Сохрани все rustdoc-комментарии существующей структуры; обнови их где упоминается "ArcSwap" → "NodeReplicated", и добавь упоминание per-node replication в `# Storage` блоке вверху.
- Imports at the top.
- Если вариант (b) выбран — отдельным изменением расширь `shamir-numa::NodeReplicated`, добавь unit-test для `load_full_local()` в `crates/shamir-numa/src/tests/node_replicated_tests.rs`.

## Done =

1. `IndexInfo::indexes` имеет тип `NodeReplicated<Vec<IndexDefinition>>`.
2. Все callsite'ы внутри файла обновлены, семантика сохранена.
3. `Cargo.toml` имеет `shamir-numa` path-dep.
4. `./scripts/test.sh -p shamir-index` зелёный.
5. `clippy -p shamir-index --all-targets -- -D warnings` clean.
6. `fmt --check` clean.
7. uncommitted.
