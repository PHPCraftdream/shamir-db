בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-06-26 [campaign-3-planned]

## Session summary

Кампания **② DDL-эволюция** завершена ЦЕЛИКОМ и закоммичена (НЕ запушена —
`master` ahead **4** от origin). Сессия началась с `/resume`
(`2026-06-25-phase-g-won`) после кампании ① (Builder parity), прошла через всю
②, и сейчас на этапе **планирования кампании ③** (таски заведены, не запущены).

**Что сделано за сессию:** (1) Реализована кампания ② целиком через `/crush`
(prompt-first → zero-trust → коммит per-stage): ②.1a/b/c RENAME folder/group/role,
②.2a/b FK ON UPDATE, ②.3a/b unify-uniqueness, ②.4a/b/c DEFAULT — всё запушено
ранее. (2) По `/goal` реализован **②.1d RENAME db** (`a3928add`) — последний хвост
②. Дизайн-прорыв: предпосылка отложения была ЛОЖНОЙ — boot берёт repo-путь из
persisted `path`-поля (`core.rs:154-164`), не из имени db → физ-локация декуплена
→ RENAME db = чистый каталог-rekey (вариант γ), без fs-move/handle-drain/crash-
window. Zero-trust ДВАЖДЫ поймал: envelope соврал «7/7», реально 6/7 — durable-
reopen RED; корень = test-env shared-temp accumulation (не код), фикс
`tempfile::tempdir`. (3) Актуализированы docs/research пофайлово (G2/G3/G5/G6/G7/
G9/G10/G15 → DONE; стереотип «G10 biggest gap» снят — Phase G.4). (4) Пофайловая
сверка ВСЕХ 13 файлов docs/research → собран `CAMPAIGN-3-PLAN.md`.

**Сейчас в полёте:** ничего не исполняется. Заведены **9 тасок кампании ③**
(#277-285), НЕ запущены. Незакоммичены: 4 актуализированных аудита + новый
`CAMPAIGN-3-PLAN.md` (рабочее дерево).

**Гипотезы/находки живые:** (A) mutating-валидаторы — replay-safety УЖЕ решена
архитектурно (`VALIDATORS.md:126-131` — валидаторы не бегут на WAL-replay), что
делает ③.2 tractable. Phase H (репликация) — отложен отдельной кампанией
(`PHASE-H-PLAN.md` + §Отложено в CAMPAIGN-3-PLAN).

**Таймеры:** нет активных (babysit снят по завершении ②; `/goal` 1d достигнут и
auto-clear).

## Active goal
none (goal «реализуй 1d через /crush» достигнут — ②.1d сделан `a3928add`, hook
auto-clear).

## TaskList
### in_progress
(пусто)
### pending — кампания ③ (CAMPAIGN-3-PLAN.md), НЕ запущены
- #277 ③.1a TS unit-тесты 6 FieldBuilder Phase B/C сеттеров (server-free) — READY
- #278 ③.1b TS e2e-добивка (FTS/vector/call/resume, server-gated) — READY
- #279 ③.2a Дизайн transform-валидаторов (оркестратор сам) — READY
- #280 ③.2b Framework mutable-запись (blockedBy #279)
- #281 ③.2c Computed-DEFAULT (blockedBy #280)
- #282 ③.2d Server-stamping created_at/updated_at (blockedBy #280)
- #283 ③.2e Тесты transform-фреймворка (blockedBy #281, #282)
- #284 ③.3a TS-хелперы lit_u64/bin — READY
- #285 ③.3b SelectExpr билдер (engine-gated, развилка A/B) — READY
### recently completed
- #276 ②.1d RENAME db (done, `a3928add`)
### deleted (5) — кампания ② (#272-275) + ранее

## Decisions
- ②.1d → вариант (γ) каталог-rekey, НЕ (α эфемерный fs-move) и НЕ (β logical-id
  миграция): boot уже декуплён через persisted path → fs-move не нужен. Предпосылка
  отложения опровергнута чтением core.rs:154-164.
- Кампания ③ собрана из остатка пофайловой сверки: #1 TS-тесты + #2 mutating-
  валидаторы + #4 мелочи. Phase H (#3) — отложен отдельной кампанией (read/DDL-
  досборка низко-рисковая, репликация — новый пилон со своей дизайн-докой).
- ③.3b SelectExpr — engine-gated; рекомендация (B) задокументировать как future
  (нет use-case; computed-поля покроет ③.2 на write-пути), билдер не строить.
- Zero-trust per-stage НИКОГДА не верит envelope: поймал пустой ②.1a-дифф,
  транзиентные crush-сбои (②.1b/②.1d), и ложный «7/7» ②.1d → реальный durable RED.
- Доки актуализируются пофайлово (чтение целиком), не grep-патчами (по просьбе).

## Open questions
- **Пушить?** `master` ahead 4 (②.1d код `a3928add` + 2 design/brief + docs-
  актуализация `a5b24d5c`). Плюс 5 незакоммиченных docs (аудиты + CAMPAIGN-3-PLAN).
  Push не делал — ждёт явной просьбы.
- **Коммитить** 5 незакоммиченных docs/research (актуализация аудитов +
  CAMPAIGN-3-PLAN.md)? Не коммитил — явной просьбы не было.
- **Старт кампании ③?** «делай ③» → конвейер с #277 (server-free, дёшево). ③.2a-
  дизайн делает оркестратор сам. Либо назвать конкретную таску.
- **③.3b развилка** (A движок+билдер / B документировать-отложить) — решить при старте #285.

## Repo state
```
 M docs/dev-artifacts/research/completeness-oql.md
 M docs/dev-artifacts/research/coverage-rust-query-builder.md
 M docs/dev-artifacts/research/coverage-ts-query-builder.md
 M docs/dev-artifacts/research/coverage-ts-tests.md
?? docs/dev-artifacts/research/CAMPAIGN-3-PLAN.md
master...origin/master [ahead 4]
```
```
a5b24d5c docs(research): актуализация — кампания ② целиком (②.1d done) + G2/G3/G5/G10 сверка
a3928add feat(ddl): ②.1d RENAME db (чистый каталог-rekey, вариант γ)
a844002d docs(prompts): brief for ②.1d RENAME db (каталог-rekey)
0053eade docs(research): ②.1d-a design — RENAME db РЕШЕНО (γ) каталог-rekey без fs-move
bc796114 feat(engine): ②.4c DEFAULT stamp-enforcement на insert
```
