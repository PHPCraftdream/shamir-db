בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-06-29 [pgo-coverage]

## Session summary

Длинная сессия про **captrack-pgo pipeline на shamir-db**. Начиналась как
эксперимент `/opti` с captrack telemetry, выросла в полную перестройку
архитектуры lint'а через несколько фаз A→K + ряд критических фиксов.

**Starting point (resume from `2026-06-29-numa-research.md`):** только что
закрыли perf-campaign ② (3 коммита в shamir-db landed: #292/#303/#304;
#291/#305 closed как ложные кандидаты). NUMA-research-doc написан, ждал
решения про скелет `shamir-numa`. Затем пользователь радикально
переключил трек на captrack-PGO experiment.

**Что сделано в captrack репе (D:/dev/rust/captrack):**
1. **captrack lib** — autodump on exit + periodic background thread
   (atomic write через .tmp + rename, default interval 500ms, SIGKILL-
   resilient). TrackedSmallVec (14-й wrapper). TrackedBytesMut::freeze()
   (E0507 fix, нужен ownership-taking method). `wrap_from()` методы на
   всех 14 TrackedType (Phase K — универсальный wrap для любого ctor).
2. **captrack-pgo (binary)** — новые subcommands `wire`/`unwire`
   автоматически правят/откатывают Cargo.toml целевого workspace.
   Фикс `--allow-dirty` position (должен быть ПОСЛЕ `--` для cargo
   dylint 6.0.1).
3. **captrack-pgo-lint** — backslash escape fix в auto_label (Windows
   `\s`/`\l` ломали generated string literals). Data-flow guard через
   HIR Visitor: `is_safe_instrument_context` + pure `classify_parent_kind`
   classifier. TrackedType enum (6 std + 8 third-party — bytes/scc/
   dashmap/indexmap/smallvec) с path-based recognition через
   `def_path_str` (rustc diagnostic items только для std). vec!/smallvec!
   macro recognition (Phase F — потом disabled non-empty cases #338 как
   correctness fix). Default::default() Strategy B suggestion synthesis
   (11/14 types, BTree-like — warning-only). Phase K **универсальный
   wrap_from** — заменяет старый "replace constructor" подход на "wrap
   original expression", безопасно для любых форм (vec![a,b,c],
   smallvec![...], Vec::from_iter и т.п.).

**Тесты:** 122 lib captrack + 92 lib captrack-pgo-lint + 25 integration
tests `tests/per_type.rs` для apply phase на всех 14 типах + hasher swap
matrix + 1 UI test. Все pass.

**Что landed в shamir-db (D:/dev/rust/shamir-db):**
- `docs(research): close perf-campaign ② — §12 #305 closed by profile`
- `docs(research): NUMA-aware multi-socket design (#287)`
- Никакого production-кода в shamir-db не trogано в этой сессии
  (PGO instrument был временным — wire/instrument/measure/uninstrument/
  unwire каждый раз чисто откатывали Cargo.toml + .rs).

**Реальное измерение PGO coverage на shamir-db (после Phase K):**
- Phase E baseline (только Phase A-D): 16 files / 18 sites / 2 variants
- Phase H (после Phase F+G): 17 files / 20 sites / 2 variants
- **Phase K (universal wrap_from): 19 files / 24 sites** / 2 variants
- Cargo check --workspace exit 0 на каждом шаге.

**Главный insight Phase K:** даже с universal wrap_from coverage растёт
только +4 sites. Bottleneck сейчас — **не recognition** (lint видит всё),
а **data-flow guard**: TrackedX ≠ X в by-value position (return,
struct field, type-ascribed let). Большая часть real-world allocations
в shamir-db escape'ают через эти positions и guard корректно skip'ает
(чтобы не было E0308).

**Открытая идея — Phase L (transparent inspection):** альтернативная
архитектура без TrackedX wrap. Inserts side-effect `record_initial(...)`
сохраняя original тип. Trade-off: только initial capacity (не peak grow),
но работает в ЛЮБОМ context (return, ascription, field init). Это
unlock'нёт реальный coverage gain. Не запущено — ждёт решения user'а.

**Apply phase НЕ запущен на shamir-db.** Только instrument + bench +
uninstrument. Никакая capacity не накатана в production code shamir-db.

**Helpers crate — обсуждали, отложено.** Пользователь предложил
captrack-helpers subcrate для сложных rewrites (типа `hashmap! { … }`
+ hasher swap). Решили — не нужен для shamir-db (большинство уже на
THasher/FxHash, simple inline покрывает 80%+). Opt-in позже если real
demand.

**Timer state:** /babysit cron `11afedf1` (interval 15m, off-minutes
7/22/37/52) активен. Tick'ает на #287 как "still running" — research
doc landed, но task in_progress потому что Phase 1 (skeleton crate
`shamir-numa`) не запущен.

**Active goal:** не установлен.

**Uncommitted работа:** в captrack repo накопилось МНОГО (5 phases:
G/I/F-fix/K) — все Phase A-E уже committed (6 commits раньше), но
Phase F/G/I/K — не. Это серьёзный объём requiring serial commits.
Пользователь спрашивал делать ли commit сейчас vs продолжать debug — мой
последний ответ предлагал (1) commit, (2) apply cycle на shamir-db,
(3) Phase L. Решение от user'а ещё не получено.

## Active goal

none (никаких Stop-hook условий не активировано)

## TaskList

### in_progress
- #287 Исследовать NUMA-aware реализацию работы на нескольких процессорах

### pending
(пусто)

### recently completed (last sessions)
- #340 Phase K: universal wrap_from approach для instrument phase
- #338 CRITICAL FIX: Phase F vec![a,b,c] silently drops elements + smallvec! 0/29
- #337 Phase I: integration tests apply phase для всех 14 TrackedType + hasher swap
- #336 Phase H: re-instrument shamir-db + count coverage delta
- #335 Phase G: Default::default() Strategy B suggestion synthesis
- #334 Phase F: vec!/smallvec! macro recognition в lint

Удалённые таски в этой сессии: 22 (от первоначальных Phase A-D + cleanup).

## Decisions

- **Universal wrap_from > replace-constructor.** Phase K принят за
  fundamental pivot — оборачиваем original expression в
  `TrackedX::wrap_from(<original>, ...)` вместо replace на
  `TrackedX::with_capacity_named(0, ...)`. Reject: продолжать
  replace pattern (приводил к correctness regressions, см. #338).
- **Non-empty macro forms — disabled в instrument phase.** Single-span
  rewrite неминуемо теряет элементы. После Phase K reanled через
  wrap_from. Reject: попытка proper HIR-level rewrite в block
  expression (`{ let mut t = ...; t.push(a); t }`) — слишком сложно.
- **Helpers subcrate — отложено.** Простой inline rewrite в apply phase
  покрывает 80%+ shamir-db случаев (THasher уже везде на FxHash).
  Reject: code-gen в `<target>/src/captrack_local/` или separate
  `captrack-helpers` crate — premature design.
- **Apply phase НЕ запущен в session.** Сосредоточились на расширении
  instrument coverage и тестов. Apply cycle (накатить capacity hints в
  production code shamir-db) — отдельная задача. Reject: запустить
  apply на 24-site profile drain_throughput — мало coverage чтобы
  получить значимый perf delta.
- **§12 + SVG flamegraph НЕ коммитить.** Пользователь сообщил про
  шумную машину; differential результаты устойчивы, но absolute
  perf-числа возможно inflated. Отложено до перезамера.

## Open questions

- **Что делать дальше с PGO** — три варианта в моём последнем сообщении:
  (1) commit'ы для Phase F/G/I/K в captrack, (2) apply cycle на
  shamir-db с merged profile, (3) Phase L (transparent inspection,
  type-preserving instrument). Жду решения.
- **Phase L design** — record_initial vs record_creation, ownership
  through block expr — придумать корректный transform который не
  ломает Drop semantics original'а. Не начато.
- **#287 NUMA — следующая phase** — research-doc committed, Phase 1
  (skeleton `shamir-numa` crate) ждёт явного "go". Не начато.
- **smallvec! coverage 0/N** — root cause: multi-statement block
  expansion. Phase K через wrap_from обходит частично (вrap на outer
  macro span). Но shamir-db smallvec! sites не появились в +4 — все
  либо type-annotated либо escape'ают (data-flow guard). Phase L
  закроет.

## Repo state

### shamir-db

```
 M Cargo.lock                         <- auto-regen, не commit
?? .flamegraphs/membuffer-pump-frequent-flush-2026-06-29.svg  <- defer
```

```
bf833347 docs(research): NUMA-aware multi-socket design (#287)
fb60297c docs(research): close perf-campaign ② — §12 #305 closed by profile
1c382fcf docs(research): §10 #291 closed + §11 анализ #304/#305
acf992cb perf(index): #304 SortedIndexManager DashMap → ArcSwap<Vec<SortedIndexDefinition>>
93e03ffc fix(wal): #303 Windows TOCTOU between SegmentSet::replay snapshot и truncate_below
```

master в синке с origin/master (0 коммитов ahead). Working tree
практически чистый — кода не тронуто, остался только Cargo.lock и
старый SVG.

### captrack

```
 M captrack-pgo-lint/Cargo.lock        <- new dev-deps для per_type tests
 M captrack-pgo-lint/Cargo.toml        <- same
 M captrack-pgo-lint/src/instrument.rs <- Phase K (wrap_from), F-fix
 M captrack-pgo-lint/src/lib.rs        <- Phase G (Default Strategy B)
 M captrack-pgo-lint/ui_instrument/*   <- ui fixtures updated
 M src/tests/mod.rs                    <- + wrap_from_tests
 M src/tracked/{14 files}.rs           <- + wrap_from method на каждый
?? captrack-pgo-lint/examples/                  <- (?)
?? captrack-pgo-lint/rustc-ice-2026-06-29.txt  <- compiler ICE? уточнить
?? captrack-pgo-lint/tests/fixtures/            <- per_type fixtures
?? captrack-pgo-lint/tests/per_type.rs          <- 25 integration tests
?? captrack-pgo-lint/ui_per_type/               <- per_type UI blesseds
?? docs/dev-artifacts/checkpoints/2026-06-29-1059.md          <- предыдущий чекпоинт
?? src/tests/wrap_from_tests.rs                 <- 22 wrap_from tests
```

```
c8057ab fix(captrack): TrackedBytesMut::freeze takes self by value (E0507)
035c49b feat(captrack): TrackedSmallVec wrapper (14th tracked type)
57e069a feat(captrack-pgo-lint): path-based recognition for 8 third-party types
786da1c fix(captrack-pgo): pass --allow-dirty after `--` for cargo dylint 6.0.1
e1a972d fix(captrack-pgo-lint): backslash escape + data-flow guard for instrument
4dbb471 feat(captrack): atomic write + periodic autodump (SIGKILL-resilient)
b2a96ce feat: autodump on exit + captrack-pgo wire/unwire subcommands
dfbe57b feat(captrack-pgo): --cap-from / --cap-mul / --cap-round flags (M11)
```

В captrack — 6 коммитов от этой сессии landed (b2a96ce → c8057ab).
Phase F/G/I/K — uncommitted (3-4 logical commits ждут).

**Active timer:** `/babysit` cron `11afedf1` (15m, off-minutes 7/22/37/52)
тикает на #287, signal-absent на сейчас.
