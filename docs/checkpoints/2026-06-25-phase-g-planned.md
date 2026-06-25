בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-06-25 [phase-g-planned]

## Session summary

Кампания **E.4-followon** реализована ЦЕЛИКОМ и **запушена** (`dfec7d1..3193da8`):
F.1 RENAME INDEX (`1ac6b91`), F.2 populated-table RENAME со снятием MVCC-overlay-
барьера через `MvccStore::drain_to_history` (`722f05d`), F.3 RENAME REPO
(`5e3ea60`) + обновлённые research-доки (`3193da8`). Трек **E1 (RENAME) закрыт
полностью** (table+index+populated+repo). Вся работа делегирована **crush**
(строго, по воле пользователя; при падении — рестарт в той же сессии; agent-tool
запрещён — он падал context-canceled), commit-per-phase, каждая фаза
zero-trust-верифицирована оркестратором. Поймано и починено **3 реальных
дефекта**, которые crush не заметил: rekey-баг hash-индекса в F.1 (физический ключ
хэширует name_interned → drop+rebuild вместо byte-swap); 2 тест-бага + латентный
клиентский interner-баг `extractRepo` в F.3 (array-form `from` → возвращал 'main'
вместо реального репо → de-intern промах для non-default репо).

Затем по запросу пользователя изучён `docs/research/` на остаток работы и
**составлен подробный план следующей кампании «Phase G»** — записан в
`docs/research/PHASE-G-PLAN.md` и закоммичен (`4df8115`, **НЕ запушен** — ahead 1).
Phase G = P1 билдеры/тесты (G.1 B2 one_of, G.2 B4 row_idmsgpack, G.3 C3 e2e)
ПЕРВЫМИ и чисто, затем A2 access enforcement (G.4, под-фазы owner-on-create →
uniform gate → open→enforced default → negative e2e) ПОСЛЕДНЕЙ и изолированно (на
зелёной базе, чтобы смена дефолта доступа не маскировала регрессии). План заземлён
file:line. **Заведены 4 таски** (#254-257) по плану. Реализацию Phase G НЕ
начинал — жду «погнали G.1»/`/babygoal`.

**Тех-долг сессии:** я (оркестратор) несколько раз ломал формат tool-call'ов —
лез стрэй-токен `court`/`coourt` перед открывающим тегом, вызовы не исполнялись.
Это мой output-глюк, не поломка сессии; пользователь указал, решено компактом.
Бенчи/`/loop`/babysit таймеры — нет активных (babysit-cron `4cd14df1` снят после
завершения F-кампании).

## Active goal
`довди задачи до конца с помощью /crush` — **достигнута** (F.1/F.2/F.3 done).
Stop-hook авто-снялся (условие выполнено). Активного goal по Phase G НЕТ.

## TaskList
### pending
- #254 G.1 (B2) — Rust FieldBuilder::one_of()  (S)
- #255 G.2 (B4) — Rust Insert::row_idmsgpack()  (M)
- #256 G.3 (C3) — e2e commit-migration / dropUser-dropRole / chgrp  (S-M)
- #257 G.4 (A2) — Access enforcement (owner-on-create → enforced default → uniform gate)  (L, blockedBy: #256)
### recently completed / deleted
- #251 F.1 RENAME INDEX, #252 F.2 populated-rename, #253 F.3 RENAME REPO — completed (E.4-followon, запушены).

## Decisions
- Phase G: P1 (G.1→G.2→G.3) первыми и чисто, A2 (G.4) последней и изолированно —
  т.к. A2 меняет дефолт доступа (open→enforced) и потянет массовую правку
  фикстур; на зелёной базе регрессии локализуются, не маскируются.
- A2 декомпозирован по риску: G.4a owner-on-create (аддитивно) → G.4b uniform gate
  (аддитивно) → G.4c open→enforced (вся churn здесь, Strategy A: enforced только
  для новых объектов) → G.4d negative e2e.
- Вся реализация — строго через crush (воля пользователя), agent-tool запрещён;
  zero-trust верификация оркестратором перед каждым коммитом (поймала 3 дефекта).
- e2e — через `SHAMIR_SERVER_BIN` override на debug-сервер (быстро) вместо 25-мин
  release; override добавлен в e2e-harness.ts в F.2.
- Коммиты/пуш — только по явной просьбе; коммиты per-phase разрешены пользователем
  для кампании, пуш — отдельным словом.

## Open questions
- **Пушить `4df8115`** (план Phase G) — ahead 1, жду слова.
- **Старт Phase G** — жду «погнали G.1»/`/babygoal`. Реализацию не начинал.
- (фон) `extractRepo`-фикс — латентный баг был только для non-default репо; теперь
  закрыт. Validator-bindings dangling-refs при rename (db/repo/table в table_ref)
  — известная limitation, вне scope, потенциальный отдельный follow-on.

## Repo state
```
(дерево чисто — git status --short пуст)
master...origin/master [ahead 1]   ← 4df8115 (план Phase G) не запушен
```
```
4df8115 docs(research): Phase G plan — Builders finish (B2/B4/C3) + Access enforcement (A2)
3193da8 docs(research): отметить E.4-followon (E1) выполненным — F.1/F.2/F.3
5e3ea60 feat(ddl): E.4-followon F.3 — RENAME REPO (#253)
4f88398 docs(prompts): Phase F.3 rename-repo brief (git-ban, no agent-tool, debug e2e)
722f05d feat(engine,tx): E.4-followon F.2 — populated-table RENAME via overlay drain (#252)
```
