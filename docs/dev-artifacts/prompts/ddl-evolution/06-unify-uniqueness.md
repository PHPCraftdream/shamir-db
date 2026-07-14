בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# ②.3b — E5 unify-uniqueness: нормативный контракт + coherence-тест

Кампания **② DDL-эволюция**, этап ②.3, под-этап **b — импл** (по решению ②.3a).
Источник: `docs/dev-artifacts/research/DDL-EVOLUTION-PLAN.md` §②.3 (читай блок «✅ РЕШЕНО (②.3a)»
ПЕРВЫМ — там полный контракт). Объём: S. Риск **низкий** (документация + тесты;
**БЕЗ write-path хирургии** — гонку/HIGH-A не трогать). Пакеты: `shamir-engine`,
`shamir-db` (тесты).

## Решение ②.3a (кратко) — ВАРИАНТ (B) Defense-in-depth
Два слоя uniqueness КОМПЛЕМЕНТАРНЫ (не дубль-по-ошибке), уже связаны
DDL-инвариантом. Задача ②.3b — **зафиксировать контракт в коде (doc) + закрыть
coherence-тестом**. НЕ снимать probe, НЕ менять write-path.

## Контракт (нормативный — то, что документируешь)
1. **DDL-invariant** (`crates/shamir-db/src/shamir_db/execute/admin_schema.rs:103-150`
   `validate_unique_indexes`): `unique` schema-rule ⟹ single-field unique index,
   иначе отказ `unique_requires_index`. Обратное НЕ требуется (голый
   `create_index{unique}` легитимен).
2. **Write-path two-layer:**
   - **probe** (`crates/shamir-engine/src/validator/schema/schema_validator.rs:134-184`):
     логический fail-fast → чистая field-scoped `unique_violation`; на tx И
     autocommit; NULL-bypass; UPDATE-skip-if-unchanged. O(1) через обязательный
     индекс (`exists_in_self` → `find_single_field_index` →`lookup_by_index`,
     `validator_db.rs:299-306`).
   - **index-guard** (`crates/shamir-engine/src/table/table_manager.rs:426-438`
     `unique_write_lock` + posting, `table_manager_crud.rs:85,184`): физическая
     атомарность, закрытие non-tx↔tx гонки (HIGH-A) + within-batch dedup.
3. **Источник физической истины — индекс.** Probe = ранний отказ + диагностика
   (pre-tx, TOCTOU-окно), НЕ авторитет атомарности.

## Что сделать
### 1. Doc-контракт в коде (нормативные комментарии, surgical)
- В `schema_validator.rs` unique-секции (~:134-184) — расширь doc-комментарий:
  явно укажи, что probe — fail-fast/диагностика поверх index-guard (HIGH-A —
  авторитет атомарности), и что DDL-инвариант (`validate_unique_indexes`)
  гарантирует индекс ⇒ probe O(1). Перекрёстная ссылка на `unique_write_lock` и
  `validate_unique_indexes`.
- В `table_manager.rs` `unique_write_lock` (~:426-438) — добавь в doc обратную
  ссылку: probe (schema_validator) — логический слой поверх; этот lock —
  физический авторитет.
- НЕ меняй логику — только нормативные doc-комментарии, фиксирующие контракт
  (чтобы будущий читатель не принял «два пути» за баг-дубль).

### 2. Coherence-тест (закрывает непокрытое)
Уже ЕСТЬ (НЕ дублируй): `unique_without_index_rejected_at_ddl`,
`unique_accept_new_reject_duplicate`, `unique_batch_duplicate_within_tx`,
`update_unique_violation_rejects` (в
`crates/shamir-db/tests/declarative_schema_unique_e2e.rs`). Добавь рядом ТОЛЬКО
непокрытое:
- **NULL-bypass**: два insert с `unique`-полем = NULL → ОБА проходят (unique не
  применяется к NULL). Если такой тест уже есть — не дублируй, отметь.
- **UPDATE-skip-if-unchanged**: update строки, НЕ меняющий unique-значение
  (меняющий другое поле) → НЕ ложный `unique_violation`.
- **autocommit-enforcement (explicit)**: non-tx (autocommit) insert дубля →
  `unique_violation` (подтверждает, что probe фаерит на autocommit, не только
  внутри явного tx). Если покрыто — отметь.
- **bare-index-without-rule**: создать unique-ИНДЕКС через `create_index{unique}`
  БЕЗ schema-`unique`-rule → вставка дубля всё равно отвергается (index-guard
  enforce-ит физически). Подтверждает «обратное направление» контракта.
- (опц., если стабильно) **concurrent-insert race**: две параллельные
  non-tx-вставки одного значения → ровно одна проходит. Если детерминированно не
  выходит на Windows-nextest — НЕ добавляй флейк, отметь, что HIGH-A покрыт
  существующими tx-тестами.

## Гейт (прогони сам, всё зелёное — НЕ коммить)
- `./scripts/test.sh -p shamir-engine -p shamir-db --full -- unique`
  (вкл. существующие + новые; не сломай).
- `cargo fmt -p shamir-engine -p shamir-db -- --check` + `cargo clippy --workspace
  --all-targets -- -D warnings`.

## ЖЁСТКИЕ ПРАВИЛА
- ⛔ НЕ используй инструмент agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER `git reset`/`checkout`/`clean`/`stash`/`restore`/`rm` или мутирующую
  git-команду (НЕ удаляй `run.log`/отслеживаемые; scratch-логи — в /tmp). НЕ коммить.
- ⛔ НЕ трогай write-path / гонку / unique_write_lock логику — ТОЛЬКО doc-комментарии
  + тесты. Снятие/демотирование probe ЗАПРЕЩЕНО (решение ②.3a = сохранить оба слоя).
- Surgical. Запросы — через билдер. Тесты — только через `./scripts/test.sh`,
  JSON-литералы многострочные и с отступами.
- Заверши финальным текстом: изменённые файлы (file:line) + что из coherence-тестов
  было НОВЫМ vs уже-покрытым + вывод гейта.
