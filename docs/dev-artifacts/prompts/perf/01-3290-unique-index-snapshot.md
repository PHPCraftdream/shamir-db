בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# perf-③.290 — снять unique-index снэпшот раз на батч

> **Target:** убрать 2×N DashMap-iter на batch insert с unique-индексом.
> Корень и фикс найдены чтением кода + flamegraph профиля
> (`docs/dev-artifacts/research/WRITE-HOT-PATH-PROFILE-2026-06-28.md`,
> `.flamegraphs/shamir-engine-tx_pipeline-symbols.svg`). Подпись в профиле:
> `dashmap::Iter::next 3.2%` + `dashmap::lock_shared 0.89%` + часть
> `malloc/free` (IndexDefinition clone) + часть атомиков. Ожидаемый
> выигрыш на `tx_overhead/batch_pipeline/indexed/tx/1000` — ~5-7%.

## ⛔ Запреты
- НЕ `git reset/checkout/clean/stash/restore/rm` и любая git-мутация дерева/индекса.
  Только редактируй; коммитит оркестратор. НЕ удаляй отслеживаемые. НЕ sub-agent.
- Тесты — ТОЛЬКО `./scripts/test.sh` (raw `cargo test` заблокирован perimeter-guard).
- Хирургически; не трогай несвязанный код/комменты. НЕ менять поведение, только
  устранить redundant DashMap-iter.

## Прочитанная реальность (точные file:line)

### Корневой call-site — `crates/shamir-engine/src/table/table_manager_crud.rs:167-191`

Batch insert с unique-индексом делает ДВА полных обхода `indexes_unique`
DashMap на КАЖДУЮ строку батча:

```rust
// Текущая версия (table_manager_crud.rs:167-191):
if self.index_manager.has_unique_indexes() {
    let mut batch_seen: TFxSet<(u64, Vec<u8>)> = TFxSet::default();
    for (i, v) in values.iter().enumerate() {
        // (А) Per-row DashMap iter #1: внутри validate_unique_for_create
        //     → indexes_unique.iter().collect::<Vec<IndexDefinition>>()
        self.index_manager.validate_unique_for_create(v).await?;
        // (B) Per-row DashMap iter #2: ещё один iter() прямо здесь
        for def in self.index_manager.iter_unique_indexes() {
            if let Some(vs) = crate::index::index_keys::extract_index_leaves(v, &def.paths) {
                let key = bincode::serialize(&vs).map_err(...)?;
                if !batch_seen.insert((def.name_interned, key)) {
                    return Err(...);
                }
            }
        }
    }
}
```

Для 1000 строк = **2000 обходов DashMap** + 1000 clone'ов `Vec<IndexDefinition>`
+ 1000×def `IndexDefinition.clone()` (а это ещё аллокация `Vec<IndexInfoItem>`).

### Связанные точки

- `crates/shamir-index/src/legacy/index_manager_unique.rs:32-56`
  (`validate_unique_for_create`) — внутри `let defs: Vec<IndexDefinition> =
  self.indexes_unique.iter().collect();` (строка 40), затем `for def in defs { ... }`.
  Цикл нужен; iter()+collect() — НЕ нужны, если defs передать снаружи.
- `crates/shamir-index/src/legacy/index_manager.rs:707` — `pub fn iter_indexes(...)`,
  есть аналог `iter_unique_indexes` (используется в crud.rs:177). Эти геттеры
  оставить — они для НЕ-горячих callers (DDL, doctor).
- `crates/shamir-index/src/legacy/index_definition.rs:6-13` — `IndexDefinition {
  name_interned: u64, paths: Vec<IndexInfoItem> }`. Clone = аллокация Vec.

### Почему def'ы константны в течение батча

`indexes_unique` мутируется ТОЛЬКО на DDL (create_unique_index, drop_index).
Batch insert — это не DDL; во время батча def'ы не меняются. (Если бы хотели
строгости — можно brief'ом для будущей кампании добавить epoch-снимок;
сейчас это не нужно: и текущая версия с iter()+collect не закрывает гонку с
конкурентным DDL — то же значение видит обе iter'ы или ни одной, по lock-shared
DashMap-семантике. Снимок «раз на батч» эквивалентен.)

## Задача (хирургическая)

### 1. Новый метод `validate_unique_for_create_with_defs` в
   `crates/shamir-index/src/legacy/index_manager_unique.rs` рядом с
   существующим `validate_unique_for_create`:

```rust
/// Variant of [`validate_unique_for_create`] that accepts pre-collected
/// unique-index definitions, avoiding a per-call DashMap iteration when the
/// caller already has a batch-scope snapshot.
///
/// Use this from batch insert paths where definitions are stable for the
/// duration of the batch. Standalone callers should keep using
/// [`validate_unique_for_create`].
pub async fn validate_unique_for_create_with_defs(
    &self,
    value: &(impl RecordRef + ?Sized),
    defs: &[IndexDefinition],
) -> DbResult<()> {
    if defs.is_empty() {
        return Ok(());
    }
    for def in defs {
        if let Some(irk) =
            build_index_key_from_record(true, def.name_interned, value, &def.paths)
        {
            let index_key = irk.to_bytes();
            if let Some(existing_id) = self.check_unique_key(&index_key).await? {
                return Err(shamir_storage::error::DbError::DuplicateKey(format!(
                    "Unique index '{}' violated: value already exists for record {:?}",
                    def.name_interned, existing_id
                )));
            }
        }
    }
    Ok(())
}
```

Существующий `validate_unique_for_create` оставь как есть — он делегирует на
новый: `self.validate_unique_for_create_with_defs(value, &self.indexes_unique.iter().collect::<Vec<_>>()).await`.
Это сохраняет API для standalone-callers (`table_manager_crud.rs:91`,
`:355` — это НЕ batch-цикл, OK как есть).

### 2. Снять снэпшот в batch-call-site
   `crates/shamir-engine/src/table/table_manager_crud.rs:167-191`:

```rust
if self.index_manager.has_unique_indexes() {
    // Snapshot unique-index defs ONCE per batch — they are stable for the
    // duration of insert_many (mutated only by DDL). Eliminates 2×N
    // DashMap-iter + N×IndexDefinition::clone seen on the hot path
    // (flamegraph: dashmap::Iter::next 3.2% + lock_shared 0.89%).
    let unique_defs: Vec<IndexDefinition> =
        self.index_manager.iter_unique_indexes().collect();
    let mut batch_seen: TFxSet<(u64, Vec<u8>)> = TFxSet::default();
    for (i, v) in values.iter().enumerate() {
        self.index_manager
            .validate_unique_for_create_with_defs(v, &unique_defs)
            .await?;
        for def in &unique_defs {
            if let Some(vs) =
                crate::index::index_keys::extract_index_leaves(v, &def.paths)
            {
                let key = bincode::serialize(&vs)
                    .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                if !batch_seen.insert((def.name_interned, key)) {
                    return Err(shamir_storage::error::DbError::DuplicateKey(format!(
                        "Unique index '{}' violated within batch (row {} duplicates an earlier row)",
                        def.name_interned, i
                    )));
                }
            }
        }
    }
}
```

⚠ Изменения МИНИМАЛЬНЫ: добавлен `let unique_defs = ...`, замена
`validate_unique_for_create(v)` → `validate_unique_for_create_with_defs(v, &unique_defs)`,
замена `for def in self.index_manager.iter_unique_indexes()` → `for def in &unique_defs`.
Семантика идентична; никакие диагностики/ошибки не меняются.

### 3. Импорт IndexDefinition в crud.rs

Если ещё не импортирован — добавить `use shamir_index::legacy::index_definition::IndexDefinition;`
в шапку файла (use-у-топа правило). Свериться с уже существующими импортами.

## НЕ ТРОГАЙ (вне скоупа)

- `validate_unique_for_update` (`index_manager_unique.rs:69-108`) — аналогичная
  pattern, но update-путь НЕ в горячем профиле бенча insert. Отдельный фикс
  при необходимости.
- Standalone callers `validate_unique_for_create` (`table_manager_crud.rs:91`,
  `:355`) — они НЕ в цикле, лишний DashMap-iter раз-в-write терпим.
- Другие `*.iter().collect()` в index_manager_unique.rs:217/248/291 — не в hot path.
- regular index (`on_records_created_batch`) — УЖЕ batch-эффективен.

## Тесты (`./scripts/test.sh`, НЕ raw cargo)

- РЕГРЕССИЯ: все существующие unique-тесты зелёные. Главные точки:
  - `./scripts/test.sh -p shamir-index -p shamir-engine -- unique`
  - `./scripts/test.sh @engine` — полный engine-suite.
- Новый unit (`index_manager_unique` тесты): `validate_unique_for_create_with_defs`
  на пустом `defs` (ранний выход), с одним def (accept/reject), с несколькими
  def (один из них trips). Если тесты для `validate_unique_for_create` уже
  есть — параллельные тесты для новой версии достаточны.

## Гейт (прогони сам, приложи ПОЛНЫЙ вывод)

```
./scripts/test.sh @engine
./scripts/test.sh -p shamir-index --full
cargo clippy -p shamir-engine -p shamir-index --all-targets -- -D warnings
cargo fmt -p shamir-engine -p shamir-index -- --check
```

Всё зелёное. Если падает — НЕ ослабляй тест, сообщи с диагнозом.

## Финальный отчёт

Изменённые файлы; новый метод сигнатура; «было/стало» для горячего цикла в
`table_manager_crud.rs:167-191`; вывод гейта.

Отдельный bench-compare (criterion / flamegraph до/после) — делает оркестратор
сам после твоей сдачи.
