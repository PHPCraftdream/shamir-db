בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-06-25 [phase-g-won]

## Session summary

Кампания **Phase G** реализована ЦЕЛИКОМ и закоммичена (НЕ запушена — `master`
ahead 14 от `origin/master`). Phase G = досборка билдеров (P1) + закрытие
последнего P0 (A2 access-enforcement). Все 4 таски (#254–257), декомпозированные
в 7 фаз, выполнены, zero-trust-верифицированы оркестратором, **вся работа строго
делегирована crush** (по воле пользователя; agent-tool запрещён — падает
context-canceled; git-мутации в брифах запрещены; commit-per-phase; prompt-first
— каждый бриф закоммичен ДО делегирования).

Реализовано: **G.1** `FieldBuilder::one_of()` (`3753cbb`); **G.2**
`Insert::row_idmsgpack()` + реэкспорт `ByteBuf` (`f32ed0c`); **G.3** e2e
commit-migration/dropUser-dropRole/chgrp (`09eeeed`); **G.4a** owner-on-create —
оказалось уже сделано прежними слайсами (верифицировано, кода не требовало);
**G.4b** единообразный `Action::Create` гейт на create_db/repo/table (`7ef8860`,
аддитивно под OPEN); **G.4c** P0-флип дефолта `open 0o777 → enforced 0o700` для
новых объектов (Strategy A) + `ResourceMeta::owned_enforced` + 7 create-сайтов +
починка 51 фикстуры (`e9769b4`); **G.4d** group-path access-grant e2e (`356aaf0`).

**Zero-trust поймал 2 реальных пробела crush в G.4c:** (1) crush сделал движок,
но НЕ починил 51 упавшую фикстуру (его гейт оборвался до прогона) → ре-делегация
в ту же crush-сессию `g4c-enforced-default` с классификацией провалов A/B/C/D
(traversal-тесты опирались на open-предков → открыть предков; default-assertion
тесты → обновить на enforced + явный chmod-0o777 путь; owner_on_create →
mode 0o700; инфраструктурные → chmod-open в setup). (2) Параллельный запуск
`rust --full` + e2e дал Windows file-lock на `shamir-server.exe` (cargo не смог
релинковать) → перезапуск rust-suite отдельно. **Финальная верификация
(оркестратор сам):** rust `--full` 4501/4501, e2e 709/709, clippy `--workspace`
чисто. Спека доступа НЕ ослаблена (target-проверки сохранены, open-путь покрыт
явным chmod). Ключевая находка: superuser→`Actor::System`→байпасит гейт
(`handler.rs:117`), поэтому churn ограничился тестами с реальными не-superuser
юзерами; create-ops `is_admin` (superuser-only на wire), поэтому G.4b create-гейт
не выразим в e2e (покрыт Rust-интеграцией).

После кампании по просьбе пользователя обновлены research-доки (рабочее дерево,
НЕ закоммичено): `DONE.md` (секция Phase G + сводная карта B2/B4/C3/A2 ✅),
`ACTION-ITEMS.md` (A2/B2/B4/C3 → done-ссылки; P0/P1 пусты; остался P2 E5 + P3
B5–B7/E2), `PHASE-G-PLAN.md` (баннер ✅ ВЫПОЛНЕНО).

**Тех-состояние:** TaskList пуст; babysit-cron `71b088bf` снят (вручную после
закрытия всех тасок); /goal «реализуй все задачи» — достигнут, авто-снялся.
Активных таймеров/фонов нет.

## Active goal
`реализуй все задачи, друг. Используй /crush` — **ДОСТИГНУТ** (вся Phase G done).
Stop-hook авто-снялся. Активного goal нет.

## TaskList
(пусто — все Phase G таски завершены и закрыты)
### recently completed
- #254 G.1 (B2) one_of · #255 G.2 (B4) row_idmsgpack · #256 G.3 (C3) e2e
- #257 G.4 (A2) umbrella · #258 G.4a owner-on-create · #259 G.4b create-гейт
- #260 G.4c enforced-дефолт · #261 G.4d group-path e2e

## Decisions
- A2/G.4c: дефолт `open→enforced`, **Strategy A** (enforced только для новых
  объектов; legacy без поля `mode` → OPEN через `from_record`). Пользователь
  выбрал «делать сейчас, Strategy A» (vs отдельная ветка / отложить).
- Порядок Phase G: P1 (G.1→G.2→G.3) первыми и чисто, A2 (G.4) последней и
  изолированно (на зелёной базе churn локализуется). G.4 под-фазы по риску:
  G.4a/b аддитивны (зелёные) → G.4c вся churn → G.4d e2e.
- Вся реализация — строго crush; zero-trust оркестратором перед каждым коммитом
  (поймал 2 пробела в G.4c). agent-tool запрещён.
- Doc-апдейты сделаны напрямую (не через crush) — контекст-ёмкая разметка
  завершённого, прецедент прошлой «обнови доки».

## Open questions
- **Пушить?** `master` ahead 14 от `origin/master` (вся Phase G: код + план +
  брифы). Doc-апдейты research (3 файла) — НЕ закоммичены, ждут слова о коммите.
- (фон) Validator bindings dangling-refs при rename — известная limitation, вне
  scope. P2/P3 остаток (E5 unify-uniqueness, B5–B7 DX-билдеры, E2 DEFAULT) — не
  блокеры, будущие кампании.

## Repo state
```
 M docs/research/ACTION-ITEMS.md
 M docs/research/DONE.md
 M docs/research/PHASE-G-PLAN.md
?? docs/checkpoints/2026-06-25-phase-g-planned.md  (+ этот файл)
master...origin/master [ahead 14]
```
```
356aaf0 test(e2e): G.4d — group-membership access-grant path (chgrp + group bits)
b1b874a docs(prompts): G.4d brief — group-path access-grant e2e
e9769b4 feat(access): G.4c — enforced (owner-rwx) default for new objects (Strategy A)
7841773 docs(prompts): G.4c Part-2 addendum — 51 fixture failures categorized (A/B/C/D)
878f7d8 docs(prompts): G.4c brief — enforced default flip (Strategy A)
7ef8860 feat(access): G.4b — uniform Action::Create gate on create_db/repo/table
09eeeed test(e2e): G.3 C3 — commit-migration / dropUser-dropRole / chgrp
f32ed0c feat(query-builder): G.2 B4 — Insert::row_idmsgpack
3753cbb feat(query-builder): G.1 B2 — FieldBuilder::one_of
```
