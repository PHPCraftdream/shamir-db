בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: NUMA N4 — SortedIndexManager ArcSwap → NodeReplicated

## Цель

Зеркало N3 для второй горячей точки: `Arc<ArcSwap<Vec<SortedIndexDefinition>>>` в `crates/shamir-index/src/legacy/sorted_index_manager.rs` → `NodeReplicated<Vec<SortedIndexDefinition>>`.

## Контекст

- История: #304 (commit `acf992cb`) мигрировал DashMap → ArcSwap. Это следующий шаг.
- Тот же файлик-паттерн что в N3 — read-mostly registry с `.load()` / `.load_full()` / `.rcu()`.
- N3 уже сделан и закоммичен на момент этой задачи: shamir-numa уже path-dep `shamir-index` (если не — добавь, как в N3).

## Что делать

Применить **строго ту же** механику миграции что в N3, но к `sorted_index_manager.rs`. Брифа N3 (`03-indexinfo-noderep.md`) хватит как референса для:

- замены типа поля,
- mapping'а `.load()` → `.load_local()`, `.load_full()` → `Arc::clone(&*load_local())`,
- `.rcu(...)` остаётся,
- conструкторы → `NodeReplicated::new(detect(), vec)`,
- Serialize / PartialEq / Default / Clone аналогично.

Если `sorted_index_manager.rs` использует *другие* паттерны (например range queries, sorted insert via binary_search) — сохрани их полностью, миграция касается только storage primitive.

## Gate

```
./scripts/test.sh -p shamir-index
cargo fmt -p shamir-index
cargo clippy -p shamir-index --all-targets -- -D warnings
```

## Discipline

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`. Только редактирование.

- Surgical scope — только `sorted_index_manager.rs`.
- Обнови rustdoc упоминания "ArcSwap" → "NodeReplicated" в шапке.

## Done =

1. `SortedIndexManager`-структура хранит `NodeReplicated<Vec<SortedIndexDefinition>>`.
2. Все callsite'ы внутри файла обновлены.
3. `./scripts/test.sh -p shamir-index` зелёный.
4. clippy + fmt clean.
5. uncommitted.
