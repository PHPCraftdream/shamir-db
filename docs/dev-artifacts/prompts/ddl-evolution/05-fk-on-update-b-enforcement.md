בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# ②.2b — FK ON UPDATE: enforcement

Кампания **② DDL-эволюция**, этап ②.2, под-этап **b — enforcement** (рисковый).
Источник: `docs/dev-artifacts/research/DDL-EVOLUTION-PLAN.md` §②.2b. Объём: M. Риск средний-
высокий (referential-корректность, новая cascade-propagation логика). Пакет:
`shamir-engine`. **②.2a (surface) уже в дереве** (`on_update: FkAction` есть в
DTO/Ref/builders) — здесь ТОЛЬКО enforcement.

## Задача (одна строка)
Реализовать enforcement `on_update` на UPDATE-пути: когда UPDATE меняет значение
**referenced (parent) поля**, на которое ссылается дочерний FK с
`on_update != NoAction`, фан-аут к зависимым строкам — Restrict (отказ) / Cascade
(переписать FK-значение детей old→new) / SetNull (обнулить FK детей).

## Заземление — образец delete-пути (читай ПЕРВЫМ, ЦЕЛИКОМ)
Phase D ON DELETE — твой шаблон. Зеркаль его структуру для update.
- **`crates/shamir-engine/src/query/batch/fk_restrict.rs`** — `check_fk_restrict`:
  `discover_restrict_refs` (scan repo tables → `child_table.collect_fk_refs()` →
  фильтр `fk.ref_table==parent && fk.on_delete==Restrict`) → `collect_parent_values`
  (scan parent rows по where → значения ref_field) → `child_has_reference`
  (index-fast / scan-fallback). Все хелперы (`collect_parent_values`,
  `child_has_reference`, `scalar_ref_to_qv`, `qv_scalar_to_inner` и пр.) —
  переиспользуй / зеркаль.
- **`crates/shamir-engine/src/query/batch/fk_actions.rs`** — `plan_cascade` /
  `apply_cascade_plan`: `PendingMutation{Delete|SetNull}`, `CascadePlan`,
  pre-tx discovery (resolver НЕ захватывается в HRTB-замыкание
  `run_implicit_batch_tx` — план строится ДО tx, несёт pre-resolved
  TableManager-хэндлы), depth-guard `CASCADE_DEPTH_LIMIT=32`, cycle-guard
  (`visited`-set), TOCTOU-нота.
- **Точка вызова** — `crates/shamir-engine/src/query/batch/query_runner.rs`:
  `BatchOp::Delete`-арм (:544-650) вызывает `check_fk_restrict` (pre-tx) +
  `plan_cascade` (pre-tx) + `apply_cascade_plan` (in-tx, в ОБЕИХ ветках —
  explicit `tx` и non-tx `run_implicit_batch_tx`). `BatchOp::Update`-арм
  (:~460-542) — СЮДА вставляешь зеркальный хук.

## Семантика ON UPDATE — отличия от ON DELETE (КРИТИЧНО)
1. **Триггер — «referenced value changed»**, НЕ «row removed». Фан-аут нужен
   ТОЛЬКО если `op.set` присваивает parent ref_field НОВОЕ значение.
   **Fast no-op gate (ОБЯЗАТЕЛЬНО, для нулевого overhead обычных update):**
   сначала собери множество полей, которые трогает `op.set` (ключи set-документа,
   `op.set` — `QueryValue::Map` поле→значение). Дискаверь child-FK-refs с
   `on_update != NoAction`; если НИ ОДНО их `parent_ref_field` не пересекается с
   set-полями → **верни пустой план немедленно** (большинство update не трогают
   FK-referenced parent-поля → ноль работы).
2. **(old→new) пары.** Для каждого ref_field, который set меняет: `new_value` =
   значение, которое `op.set` присваивает этому полю (литерал-скаляр из set-Map).
   `old_values` = значения ref_field из parent-строк, матчащих `op.where_clause`
   (через `collect_parent_values`). Фан-аут только по тем old, что реально
   меняются (old != new).
   - **MVP-скоуп (как Phase D — single-field, opt-in):** поддержи литерал-скаляр
     new_value. Если set присваивает ref_field вычисляемое ($fn/$ref/$expr) или
     не-скаляр → это вне MVP: **задокументируй и пропусти** (либо консервативно
     отвергни с typed-кодом — выбери Restrict-подобную безопасность; реши в
     сторону наименьшего сюрприза и опиши в финале).
3. **Действия:**
   - **Restrict** — если какой-то дочерний row ссылается на изменяемое old
     значение → отказ `BatchError::query_coded(alias, "fk_restrict",
     "cannot update '<parent>': value still referenced by '<child>.<field>'")`.
     (Зеркаль `check_fk_restrict`, но триггер — на изменяемые old.)
   - **Cascade** — **НОВЫЙ бит**: для каждого дочернего row с FK==old →
     **UPDATE его FK-поля на new** (НЕ delete!). Новый `PendingMutation`-вариант
     `UpdateField { table, id, field, new_value }`. Применяется в tx через тот
     же механизм, что update children (найди, как `fk_actions` мутирует —
     `update_tx_bytes` / аналог; для setnull он ставит Null, тебе нужен
     произвольный new скаляр).
   - **SetNull** — для каждого дочернего row с FK==old → FK-поле = Null
     (зеркаль существующий `PendingMutation::SetNull`; child-поле должно быть
     nullable — переиспользуй `set_null_requires_nullable`-проверку из
     fk_actions, если она применима).
4. **Атомарность / порядок.** Cascade/setnull детей применяй в ТОЙ ЖЕ
   implicit/tx-batch, что и parent update, чтобы коммит был атомарным (как
   delete-cascade). Дети ссылаются на ЗНАЧЕНИЯ, не на существование строки —
   обнови детей в том же tx (порядок parent-vs-child некритичен для value-rekey,
   но держи в одном tx). Pre-tx discovery (resolver вне HRTB-замыкания), как у
   delete.
5. **Cycle/depth.** Update-cascade может цепляться (если FK-поле ребёнка само
   referenced). Для MVP ограничь ОДНИМ уровнем ИЛИ переиспользуй depth-guard;
   реши и задокументируй (как Phase D — single-field, циклы документируй, не
   строй).

## Реализация (предложенная структура — выбери чище, если видишь)
- Новый модуль `crates/shamir-engine/src/query/batch/fk_on_update.rs`
  (зарегистрируй в `batch/mod.rs` рядом с `mod fk_actions; mod fk_restrict;`).
  Экспортируй `plan_fk_on_update(resolver, parent_table_ref, parent_table,
  update_op, ctx, alias) -> Result<FkUpdatePlan, BatchError>` (включает и
  restrict-проверку, и cascade/setnull-мутации; restrict → ранний Err) +
  `apply_fk_update_plan(plan, tx, alias)`. one-file-one-export — если делишь
  на restrict/actions, держи симметрию с delete-парой.
- Хук в `query_runner.rs` `BatchOp::Update`-арм: ДО `execute_update_tx` собери
  `subst_op` (он уже строится для $param) и вызови `plan_fk_on_update`; затем в
  ОБЕИХ ветках (explicit tx / non-tx implicit) примени план в tx ПЕРЕД (или
  совместно с) parent update — зеркаль delete-арм :590-650.

## Тесты (обязательно, shamir-engine)
Рядом с `fk_restrict_tests` / `fk_actions_tests` (найди:
`grep -rln "fk_restrict\|fk_actions\|on_delete" crates/shamir-engine --include=*.rs
| grep test`). **Используй УНИКАЛЬНЫЙ validator_id** (НЕ 9001 — он переиспользуется
в fk_actions/fk_restrict тестах и даёт test-isolation флейк при параллельном
прогоне; возьми, напр., 9301+). Покрой:
- ON UPDATE RESTRICT: update parent ref_field, на который ссылается ребёнок →
  отказ `fk_restrict`; нет ссылок → проходит.
- ON UPDATE CASCADE: update parent id 5→7 → дочерний FK 5 становится 7 (readback).
- ON UPDATE SET NULL: → дочерний FK становится Null.
- **No-op gate**: update, НЕ трогающий ref_field → дети не тронуты, ноль фан-аута.
- back-compat: FK с on_update=NoAction (legacy) → update parent не фанаутит.
- несколько детей / несколько строк → все перексрешены.

## Гейт (прогони сам, всё зелёное — НЕ коммить)
- `./scripts/test.sh -p shamir-engine -- fk` (вкл. новые on_update + существующие
  fk_restrict/fk_actions — НЕ сломай delete-путь). **Прогоняй per-suite**, не
  широким параллельным фильтром (есть pre-existing validator-id-9001 isolation
  флейк в delete-тестах — не твой; твои тесты бери с уникальным id).
- `cargo fmt -p shamir-engine -- --check` + `cargo clippy --workspace
  --all-targets -- -D warnings`.

## ЖЁСТКИЕ ПРАВИЛА
- ⛔ НЕ используй инструмент agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER `git reset`/`checkout`/`clean`/`stash`/`restore`/`rm` или мутирующую
  git-команду (НЕ удаляй `run.log`/отслеживаемые; scratch-логи — в /tmp). Только
  редактируй; коммитит оркестратор.
- Surgical: новый модуль + хук в Update-арм + тесты. НЕ трогай delete-путь
  (fk_restrict/fk_actions) кроме переиспользования общих хелперов. Если выносишь
  общий хелпер в shared-место — минимально и симметрично. Импорты в шапку.
- Билдер-only где строишь запросы. Без raw `serde_json::Value`. Тесты — только
  через `./scripts/test.sh`.
- Заверши финальным текстом: изменённые/новые файлы (file:line) + семантические
  решения (computed new_value? depth?) + вывод гейта (engine fk PASS включая
  delete-путь, fmt/clippy чисто).
