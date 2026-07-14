בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-06-26 [campaign-2-ddl-evolution-won]

## Session summary

Кампания **② DDL-эволюция & корректность** реализована ЦЕЛИКОМ и **запушена**
(`master` синхрон с `origin/master`). Происхождение: сессия восстановлена через
`/resume` после кампании ① (Builder parity); пользователь запушил ① + doc-планы,
попросил завести таски строго по `docs/dev-artifacts/research/DDL-EVOLUTION-PLAN.md` и
реализовать через `/babygoal` (crush-агенты, коммиты между этапами, все развилки
решать самому «в сторону красоты и совершенства»). 4 движковые задачи ②.1–②.4
взяты последовательной цепочкой blockedBy (по возрастанию риска).

Конвейер на каждый этап: **prompt-first** (бриф в `docs/dev-artifacts/prompts/ddl-evolution/`
закоммичен ДО агента) → **/crush** (`crush run --role smart --session <id>
--timeout 60m`, без inner `&`, `run_in_background:true`) → **zero-trust verify**
(оркестратор сам читал ВЕСЬ дифф + прогонял гейт `fmt --check` +
`./scripts/test.sh` + `clippy --workspace --all-targets -D warnings` (+TS
`vitest`/`tsc`), НЕ верил envelope) → **коммит per-stage**. Дизайн-проходы
②.3a/②.4a (и решение по ②.1d) — оркестратор делал САМ (как прецедент ①.4
boundary), записывал в `DDL-EVOLUTION-PLAN.md`.

Реализовано и закоммичено: **②.1a** RENAME folder (`1bdb1e39`), **②.1b** RENAME
group (`8ec5889e`, id-keyed → display-name), **②.1c** RENAME role (`eba3d562`,
name-keyed → rekey ссылок в users), **②.2a** FK ON UPDATE surface (`b45ea51e`),
**②.2b** FK ON UPDATE enforcement (`214b15bb`, новый модуль `fk_on_update.rs`),
**②.3a/b** unify-uniqueness (`816e1484` design + `6c98c029` контракт+тесты),
**②.4a/b/c** DEFAULT (`1f8eddd7` design + `da0e1f9e` surface + `bc796114` stamp).

**Развилки, решённые оркестратором:** ②.1d RENAME db → ОТЛОЖЕНО (#276) —
db_name в физическом on-disk пути → каскад + file-handle-drain + crash-atomicity
= отдельный под-проект, не вкатывать crash-небезопасный half-baked. ②.3a → (B)
defense-in-depth (DDL-инвариант unique-rule⟹index уже есть, probe O(1) →
комплементарны, probe не снимать). ②.4a → (B) узкий литерал-DEFAULT (replay-safe
by-construction, не нужен mutating-фреймворк; computed `now()` → будущая
(A)-мини-кампания). ②.2b → computed new-value REJECT (не тихий skip), depth=1
уровень (исключает FK-cycle).

**Механика по ходу:** ②.1b crush упал на транзиентном API-сбое (`finished:
error`, пустое дерево) → перезапущен в ТУ ЖЕ сессию `d21b-rename-group`, контекст
исследования сохранён. ②.1a агент удалил отслеживаемый `run.log` → оркестратор
поймал и `git checkout` восстановил. Несколько агентов оставляли scratch-логи в
корне — оркестратор чистил перед коммитом. babysit `33aede9f` (15m) армлен на
старте, СНЯТ вручную по завершении ② (иначе следующий тик подхватил бы
отложенный #276).

## Active goal
none (/babygoal не использует /goal; babysit снят вручную — кампания ② done,
#276 намеренно отложен и НЕ должен авто-стартовать).

## TaskList
### in_progress
(пусто)
### pending
- #276 ②.1d RENAME db (deferred — on-disk каскад + crash-safety) — отдельная
  мини-таска, НЕ блокирует ничего, ждёт явной отмашки «делай ②.1d». Заземление
  в описании таски + `DDL-EVOLUTION-PLAN.md §②.1d`.
### recently completed
(нет — #272–#275 удалены по просьбе пользователя после завершения)
### deleted (4)
- #272 ②.1 RENAME · #273 ②.2 FK ON UPDATE · #274 ②.3 unify-uniqueness ·
  #275 ②.4 DEFAULT (все завершены и запушены до удаления)

## Decisions
- Кампания ② = 4 движковые задачи ②.1–②.4 строго из `DDL-EVOLUTION-PLAN.md`,
  последовательная цепочка по возрастанию риска; ②.1d вынесен отдельно.
- ②.1d RENAME db ОТЛОЖЕН (#276): name-keyed + физический on-disk путь →
  каскад/file-handle-drain/crash-atomicity = под-проект; не half-baked в ②.
- ②.3a unify-uniqueness → (B) defense-in-depth: probe (fail-fast чистая ошибка,
  O(1) через обязательный индекс) + index-guard (HIGH-A атомарность), связаны
  DDL-инвариантом; probe НЕ снимать (потеря UX/семантики ради мнимого выигрыша).
- ②.4a DEFAULT → (B) узкий литерал: константный default replay-safe
  by-construction → без mutating-фреймворка; computed → отдельная (A)-кампания.
- Zero-trust per-stage: оркестратор сам читал дифф + прогонял гейт; поймал
  удаление run.log, транзиентный crush-сбой, scratch-логи.

## Open questions
- **#276 ②.1d RENAME db** — делать сейчас (своя мини-кампания: дизайн
  atomicity/handle-drain/recovery → crush)? Ждёт «делай ②.1d».
- **Будущая (A)-мини-кампания** — mutating/transform-валидаторы (computed
  server-stamping `created_at`/`updated_at`), на которую опирается computed-DEFAULT.
  Заводить отдельной кампанией ③?
- 4 pre-existing tsc-ошибки в `e2e-schema-validators.test.ts` (WriteValue) — не
  росли в ②; мелкий test-type фикс отдельной задачей при желании.

## Repo state
```
(git status --short пуст — рабочее дерево чистое)
master...origin/master  (синхрон, всё запушено)
```
```
bc796114 feat(engine): ②.4c DEFAULT stamp-enforcement на insert
459dd95d docs(prompts): brief for ②.4c DEFAULT stamp-enforcement
da0e1f9e feat(ddl): ②.4b DEFAULT surface — default: Option<QueryValue> сквозь слои
8549020d docs(prompts): brief for ②.4b DEFAULT surface
1f8eddd7 docs(research): ②.4a design — E2 DEFAULT РЕШЕНО (B) узкий литерал-default
```
