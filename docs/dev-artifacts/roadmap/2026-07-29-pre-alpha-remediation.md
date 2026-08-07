# Pre-alpha remediation plan — волна F-55…F-67

**Дата:** 2026-07-29
**Источник:** `docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md`
(независимое readonly-ревью снапшота `e145b1d3`)
**Вердикт источника:** NO-GO для первого публичного тега.

Этот документ — исполняемая декомпозиция ревью: что чинить, в каком порядке,
с какими зависимостями и что считается «готово». Ревью отвечает на вопрос
«что не так»; этот файл — на вопрос «что мы делаем».

---

## 1. Независимая верификация ключевых утверждений

Ревью не принималось на веру. Перед составлением плана оркестратор
самостоятельно перепроверил release-факты (остальные — code-level — будут
проверены red-then-green в рамках своих задач):

| Утверждение ревью | Статус | Как проверено |
|---|---|---|
| `bench-baseline.json` отсутствует | **подтверждено** | файла нет в дереве |
| tag `v0.1.0-alpha.1` не существует | **подтверждено** | `git tag -l` → только `backup/pre-history-rewrite-2026-07-14`; при этом крейты уже `version = "0.1.0-alpha.1"` |
| `dtolnay/rust-toolchain@1.93.0` — mutable ref | **подтверждено** | 20 вхождений во всех workflow, ни одного 40-char SHA |
| perf-gate не входит в release DAG | **подтверждено** | все `needs:` в `release.yml` = `[fmt, clippy, test, integration, ts-unit, ts-e2e, version-consistency]` |
| `build_reverse_fk_entries` fail-open | **подтверждено** | `fk_reverse_cache.rs:496-499` — `Err(_) => continue`; вызывающий `get_or_build_by_parent:345` делает `build().await?`, т.е. корректно пробросил бы ошибку, если бы она возвращалась |
| `discover_on_update_refs` дублирует паттерн | **подтверждено** | `fk_on_update.rs:743-746` |

Уточнение к ревью: в `release.yml` **уже есть** job `version-consistency`,
входящий в каждый downstream `needs:`. Ревью его не упомянуло. Это снижает
объём работ по P1-R5 — механизм проверки есть, вопрос только в том, какое
состояние считать корректным.

---

## 2. Волна 1 — P0, блокирует тег

Порядок внутри волны определяется зависимостями, не важностью. Все семь
задач обязательны до тега.

| # | ID | Задача | Зависит от | Раздел ревью |
|---|---|---|---|---|
| 1 | #881 | **F-55** fail-closed FK reverse-cache discovery | — | P0-1 |
| 2 | #882 | **F-56** WriterDrainBarrier: доказательство ordering + loom | — | P0-2 |
| 3 | #883 | **F-57** единый online CREATE INDEX lifecycle | **#882** | P0-3 |
| 4 | #884 | **F-58** TOCTOU между high-water и index-seek scan | — | P0-4 |
| 5 | #885 | **F-59** контракт error-atomicity `MirroredStore::transact` | — | P0-5 |
| 6 | #886 | **F-60** fail-closed парсер perf-gate + baseline | — | P0-R1 |
| 7 | #887 | **F-61** изоляция self-hosted runner от недоверенного PR-кода | — | P0-R2 |

**Критический путь:** F-56 → F-57. Остальные пять независимы и могут идти
в любом порядке (сессия исполняет их серийно ради чистого commit-гейта).

**Рекомендуемая очередь исполнения:** F-55 → F-56 → F-57 → F-58 → F-59 →
F-60 → F-61. Сначала самые дешёвые correctness-фиксы (F-55), затем
фундамент (F-56) и то, что на нём стоит (F-57), затем оставшиеся
независимые.

### Ключевые оговорки по волне 1

- **F-56 нельзя «починить» заменой `Relaxed` → `Release`.** Release без
  synchronizes-with на стороне читателя не создаёт нужный happens-before.
  Требуется выбранный и доказанный протокол (SeqCst с proof / seqlock с
  обязательным re-check / gate-объект), проверенный loom-моделью.
- **F-57 допускает урезанный вариант**: если полный `OnlineIndexBuildGuard`
  не влезает в один проход — разрешить `CREATE INDEX` только на пустой
  таблице или в offline maintenance mode. Это приемлемо для alpha,
  «оставить как есть» — нет.
- **F-58 допускает feature-disable**: отключить AsOf index seek
  (`PaginationMode::IndexSeek`, F-53b) и вернуться к корректному full-scan
  курсору — валидный исход задачи, если seqlock/epoch не успевает.
- **F-60 не включает фактический захват baseline** — это ручной
  operator-шаг на зарегистрированной self-hosted машине
  (`docs/guide-docs/CI_PERF_GATE_RUNBOOK.md`), недоступный из этой среды.
  В скоупе задачи — только hardening парсера (fail-closed на
  missing/duplicate/renamed cell).
- **F-61 содержит policy-решение**, а не только код: выбор между ephemeral
  runner / maintainer-approval gating / ограничением на internal branches
  до перехода в public. Требует согласования с пользователем.

---

## 3. Волна 2 — release-консистентность, обязательна до тега

| # | ID | Задача | Раздел ревью |
|---|---|---|---|
| 8 | #888 | **F-62** включить perf-gate в release DAG | P1-R3 |
| 9 | #889 | **F-63** пин `dtolnay/rust-toolchain` на commit SHA | P1-R4 |
| 10 | #890 | **F-64** согласовать version / CHANGELOG / tag | P1-R5 |

**F-64 требует решения пользователя, а не выбора агента.** Два взаимно
исключающих варианта:

1. **alpha.1 ещё не выпускалась** → слить текущий `[Unreleased]` в раздел
   `[0.1.0-alpha.1]`, поставить реальную дату, тег ставить после закрытия
   волны 1.
2. **alpha.1 считается выпущенной вне Git** → восстановить immutable tag на
   правильном старом SHA, поднять workspace/README/CHANGELOG до alpha.2.

Нельзя тегировать текущий SHA как alpha.1, оставив его изменения в
`[Unreleased]` — опубликованный артефакт и changelog разойдутся.

---

## 4. Волна 3 — P1-инженерные долги (после волны 1, до широкой функциональности)

| # | ID | Задача | Зависит от | Раздел ревью |
|---|---|---|---|---|
| 11 | #891 | **F-65** FK indexed action не должен глотать read-errors | — | P1-6 |
| 12 | #892 | **F-66** `ri_barrier_tokens`: убрать `std::sync::Mutex` | — | P1-2 |
| 13 | #893 | **F-67** per-index mutation epoch вместо manager-wide | **#884** | P1-4 |

F-65 — прямой родственник F-55 (тот же класс «`_ => continue` вместо
пробрасывания ошибки», только в fast-path FK-действий). F-66 — нарушение
пятого столпа идеологии проекта (`std::sync::Mutex` в engine runtime с
poisoning-семантикой). F-67 без F-58 бессмыслен: сначала нужно закрыть
гонку, потом сужать её радиус.

---

## 5. Волна 4 — задачи НЕ заводятся сознательно

Следующие пункты ревью зафиксированы, но **тасок под них нет намеренно** —
их нельзя корректно оценить до закрытия волны 1, а часть прямо вредна
раньше времени.

**P1, отложенные:**

- **P1-1** — coarse FK commit-lock. Оптимизация поверх F-46; трогать до
  закрытия correctness-волны нельзя, иначе будем оптимизировать код,
  который ещё меняется. Нужен бенчмарк-набор (1/2/4/8 writers, hot vs
  distinct child tables, p50/p95/p99, abort rate) прежде чем менять.
- **P1-3** — DDL error/cancellation semantics index2. Поглощается F-57;
  отдельная задача появится, только если F-57 закроет lifecycle, но
  оставит неоднозначность `Building` → error → recovery.
- **P1-5** — top-K всё ещё O(N) scan. Настоящее решение — planner-путь
  `ORDER BY indexed_field LIMIT/OFFSET` с ordered index walk и early-stop.
  Зависит от того, чем закончится F-57/F-58 (какие индексные гарантии
  вообще останутся).
- **P1-7** — гигиена change-history комментариев. Делается отдельным
  `docs:`/`chore:` коммитом **после** волны 1, иначе перемешает cleanup с
  correctness-диффами. Часть комментариев (`drain_writers` doc, `backfill_index2_backend`
  Step 2/3) станет верной сама собой после F-56/F-57.

**Breadth (§7-§10 ревью) — не раньше alpha:**

DDL (`ALTER TABLE`, partial/composite indexes, `VALIDATE CONSTRAINT`,
generated columns), OQL (JOIN/EXISTS/semi-join, set operations, subqueries,
window functions, MERGE), query-builders (typed enums вместо строк,
`build() -> Result`, typed cursor bookmark, parity-gate Rust↔TS),
производительность (covering indexes, cursor-depth benchmarks, DDL
benchmarks под нагрузкой).

Прямая цитата ревью, с которой план согласен: *«Не стоит добавлять больше
DDL surface, пока CREATE INDEX не имеет единой семантики: breadth поверх
ненадёжного lifecycle увеличит число failure modes»*.

---

## 6. Протокол исполнения

Единый для всей волны, соответствует `CLAUDE.md`:

1. **Prompt-first.** Бриф в `docs/dev-artifacts/prompts/post-alpha/<NN>-<name>.md`,
   закоммичен **до** запуска агента. Каждый бриф содержит дословный запрет
   git-мутаций.
2. **Делегирование** — `/crush` (`--role smart`), fallback `@sh` при отказе
   провайдера.
3. **Zero-trust верификация оркестратором** — не отчёт агента, а личная
   проверка: полный diff, `cargo fmt --check`, `cargo clippy --workspace
   --all-targets -- -D warnings`, `./scripts/test.sh`, плюс личное
   red-then-green воспроизведение самого механизма (сломать → убедиться,
   что падает → восстановить → убедиться, что зелено).
4. **Commit-гейт между задачами.** Проверенный код коммитится до старта
   следующей задачи. Не копить несколько стадий в рабочем дереве.
5. **TDD** внутри задачи: сначала падающий тест, воспроизводящий баг.

---

## 7. Definition of done для первого тега

Тег не ставится, пока не выполнено всё:

- [ ] #881–#887 (волна 1) закрыты и смёржены.
- [ ] #888–#890 (волна 2) закрыты; решение по F-64 принято пользователем.
- [ ] Заморожен release-candidate SHA.
- [ ] `cargo fmt --all -- --check` чисто.
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings` чисто.
- [ ] `./scripts/test.sh --full --locked` зелено.
- [ ] Adversarial race-тесты из P0-2/P0-3/P0-4 зелены.
- [ ] version / CHANGELOG / tag согласованы.
- [ ] secret/history scan перед переключением репозитория в public.
- [ ] Проверены checksums, SBOM, cosign по инструкции runbook.
- [ ] Disaster-recovery drill: backup → повреждение → restore → сверка digest.

---

## 8. Статус на момент написания

- Волна 1 начата: **#881 (F-55) в работе**, бриф закоммичен
  (`0a4838f3`), исполнитель — `/crush`, сессия `shamir-f55-fk-discovery`.
- Волны 2 и 3 заведены как таски, не начаты.
- Волна 4 сознательно не заведена (см. §5).
- CI на `e145b1d3` полностью зелёный; `ts-e2e-nightly` красный по
  расписанию — предсуществующий, не связанный с этой волной.
