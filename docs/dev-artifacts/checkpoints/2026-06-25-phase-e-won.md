בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-06-25 [phase-e-won + followon-planned]

## Session summary

Кампания **Phase E — Completeness & Operability** реализована ЦЕЛИКОМ (9 фаз
E.1–E.9) и **запушена** (`5a0ad7e..40f66f3` + doc-коммит `dfec7d1`). Стратегия —
commit-per-phase, каждая фаза верифицирована оркестратором (zero-trust: чтение
диффа + перепрогон полного гейта) перед коммитом. Финал: `fmt --all` ✓ ·
`clippy --workspace --all-targets -D warnings` ✓ 0 · Rust **2363/2363** ·
TS-юниты **462/462** · e2e **9/9** против свежесобранного release-сервера.

Сделано по фазам: E.1 if_exists на всех drop-ops (`0449075`); E.2 table-level
cascade на drop_table (`9a30339`); E.3 integration-покрытие DropFunction-guard
(guard был с Phase D.3) (`5c7d51f`); **E.4 RENAME TABLE — только Object 1**, с
честными guard'ами (`a7dcda5`); E.5 RETURNING-симметрия INSERT/DELETE + фикс
латентного update-projection бага (`cd0d00b`); E.6 DESCRIBE TABLE (`daca34d`);
E.7 EXPLAIN/dry-run (`40f66f3`); E.8 e2e FTS/vector/call (`3280e96`); E.9 unit C2
+ doc-fixes F1–F5 (`fef73c8`,`fc14e46`).

**Как шло (важно для контекста)**: пользователь велел задействовать **crush**
(«использовать лимиты, не переключаться пока не исчерпаны, потом @o46l»). crush
оказался нестабильным (серии 429 «overloaded», частые обрывы на tool_call), но
отработал E.5/E.9/E.3 и частично E.4 — затем **жёсткий лимит** «Usage limit
reached for 5 hour (reset 2026-06-25 15:52)». После этого по правилу пользователя
перешёл на **@o46l** (E.8, E.6, E.7) + оркестратор добивал/чинил сам (напр. в E.4
убрал запрещённый `scc::len()`, довёл до гейта). Все агенты — на непересекающихся
файл-сетах; параллельный shared-tree дал coupling (DTO-поле `select` E.5 заставило
править struct-литералы по всему дереву — E.2 их чинил collateral).

**E.4 — архитектурный барьер (вскрыт)**: RENAME таблицы С ДАННЫМИ невозможен
простым store-copy — живые версии в in-memory MVCC-overlay (`cells`/
`VersionedOverlay`), не переносимом copy (history вакуумится при
`Retention::current_only`); `flush_history()` ≠ дренаж overlay→history. Поэтому
E.4 = RenameTable для ПУСТЫХ таблиц + guard `cell_count>0` (отказ populated) +
guard'ы schema-bearing/destination-exists.

**После победы**: обновлены research-доки (DONE.md +раздел Phase E; ACTION-ITEMS
свёрнуты закрытые пункты, остался P0 A2; NEXT-CAMPAIGN баннер ✅) — закоммичено
`dfec7d1` и запушено. Удалены выполненные таски #241–249. Затем по /oxx **описан
поэтапный план follow-on** и записан в `docs/dev-artifacts/research/E4-FOLLOWON-PLAN.md`
(UNTRACKED, НЕ закоммичен) — декомпозиция #250 на F.1/F.2/F.3. babysit-крон
`78fbb47c` удалён (кампания завершена).

## Active goal
`доведи кампанию до победы, друг` — **достигнута** (Phase E 9/9 запушена).
Stop-hook должен был авто-сняться. Активного goal по follow-on НЕТ.

## TaskList
### pending
- #251 F.1 — RENAME INDEX (rekey индекса в таблице)
- #252 F.2 — populated-table rename via overlay drain-migration (снять guard)
- #253 F.3 — RENAME REPO (rekey репо + его таблицы)  (blockedBy: #252)
### recently completed / deleted
- #241–#249 (Phase E E.1–E.9) — completed, затем deleted после доков.
- #250 (E.4-followon umbrella) — deleted, декомпозирована на #251–#253.

## Decisions
- Phase E: commit-per-phase, zero-trust verify оркестратором перед каждым коммитом.
- Агенты: crush до исчерпания лимитов (воля пользователя), затем @o46l; критическое
  добивание/починка — оркестратором лично.
- E.4 ограничен Object 1 (RenameTable пустых таблиц) из-за MVCC-overlay барьера;
  repo/index + populated-миграция вынесены в follow-on (план-файл + #251–#253).
- Коммиты/пуш — только по явной просьбе пользователя (сделаны: Phase E push,
  doc push — оба по явному «пуш»).
- Follow-on план: F.1 RenameIndex (S-M, warmup) → F.2 overlay drain-migration
  (L, ядро; стратегия A force-drain→copy→cold-start предпочтительна) → F.3
  RenameRepo (blockedBy F.2). Тесты до e2e TS на каждой фазе.

## Open questions
- **Коммит план-файла** `E4-FOLLOWON-PLAN.md` (untracked) — жду слова; при
  коммите опц. поправить ссылки `#250`→план-файл в DONE/ACTION-ITEMS.
- **Старт follow-on** — жду «погнали F.1»/`/babygoal`. Реализацию не начинал.
- `.recovery-backup/` (untracked мусор с аварии) — прибрать?
- Прочий untracked: `docs/dev-artifacts/checkpoints/2026-06-24-2230.md` (старый чекпоинт).

## Repo state
```
?? .recovery-backup/
?? docs/dev-artifacts/checkpoints/2026-06-24-2230.md
?? docs/dev-artifacts/research/E4-FOLLOWON-PLAN.md
```
```
dfec7d1 docs(research): отметить кампанию Phase E выполненной (DONE/ACTION-ITEMS/NEXT-CAMPAIGN)
40f66f3 feat(query): Phase E.7 — EXPLAIN / dry-run plan preview (#247)
f75d9b1 docs(prompts): Phase E.7 explain brief (git-ban)
daca34d feat(ddl): Phase E.6 — DESCRIBE TABLE (полная форма объекта) (#246)
a7dcda5 feat(ddl): Phase E.4 — RENAME TABLE (Object 1) с честными guard'ами (#244)
```
