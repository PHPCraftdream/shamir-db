בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Кампания ② — DDL-эволюция & корректность (твин Phase E)

> **Статус:** ✅ **РЕАЛИЗОВАНА И ЗАПУШЕНА** (②.1a/b/c, ②.2a/b, ②.3a/b, ②.4a/b/c).
> Остаток — только осознанно отложенный **②.1d RENAME db** (отдельная мини-таска,
> см. §②.1d). Преемник `NEXT-CAMPAIGN.md` (Phase E) / `PHASE-G-PLAN.md` (Phase G).
> Источник — остаток `docs/research/` после кампании ① (Builder parity). Четыре
> движковые задачи, одна когерентная многоэтапная кампания, commit-per-stage,
> порядок по возрастанию риска. Реализация — через `/crush` (prompt-first →
> zero-trust → коммит per-stage); дизайн-проходы (②.3a, ②.4a) сделал оркестратор
> сам. Факт реализации (коммиты, тесты, развилки) — в `DONE.md`.

---

## 0. Рамка

Прямое продолжение **Phase E** (Completeness & Operability: if_exists / cascade /
DropFn-guard / RENAME-table / RETURNING / DESCRIBE / EXPLAIN). Та же мышца —
эволюция DDL + referential-корректность. Закрывает последние in-scope хвосты
`completeness-ddl.md`: G6-остаток (RENAME), G7-остаток (FK ON UPDATE), G15 (две
дороги uniqueness), G9 (DEFAULT). **Out-of-scope by design не трогаем.**

Порядок — по возрастанию риска: ②.1 механика (зелёный старт) → ②.2 referential
(паттерн Phase D готов) → ②.3 correctness-сверка (write-path race) → ②.4 DEFAULT
(design-gated на mutating-валидаторы).

---

## ②.1 — RENAME-остаток (db / role / group / folder) · S · риск низкий

**Заземление.** Паттерн готов: 5 RENAME-ops уже есть —
`RenameFunction`/`RenameValidator`/`RenameTable`/`RenameIndex`/`RenameRepo`
(`batch_op.rs:57-103`). На каждый: wire `RenameXOp` (query-types admin) + dispatch
(`batch_op.rs`) + handler `handle_rename_x` (`execute/admin_*.rs`) + Rust-билдер
(`ddl/rename_x.rs`, one-file-one-export) + TS-билдер (`builders/ddl.ts`). Недостаёт
**db / role / group / folder** (+column — N/A для schemaless).

**Под-этапы (commit-per-object, как E.4-followon F.1/F.2/F.3), порядок по сцеплению:**
- **②.1a RENAME folder** — function-folder path-rekey + ancestor-Execute access.
  Ближе всего к готовому `rename_function`/`rename_validator`.
- **②.1b RENAME group** — access-registry. ⚠ Проверить: grants/members ссылаются
  на группу по **id** (тогда rename = только display-name, тривиально) или по
  **имени** (тогда rekey ссылок). Заземлить при импле.
- **②.1c RENAME role** — RBAC-registry; grants ссылаются на роль. Та же развилка
  id-vs-name, что у группы.
- **②.1d RENAME db** — system-store db-registry. **Самый тяжёлый**: все дочерние
  пути (repo/table/index/access-meta) ссылаются на имя db. Либо db id-keyed
  (rename дёшев), либо каскадный rekey. Делать ПОСЛЕДНЕЙ; если каскад тяжёл —
  вынести/отложить.
  > **✅ РЕШЕНО (②.1d-a, оркестратор) — ВАРИАНТ (γ): чистый каталог-rekey, БЕЗ
  > переноса файлов. Реализуется (ранее ошибочно отложено).**
  >
  > **Пересмотр предпосылки отложения.** Прежнее решение откладывало ②.1d, считая,
  > что boot реконструирует repo-путь ИЗ ИМЕНИ db (`root/<db_name>/<repo>`) → rename
  > требует fs-переноса + handle-drain + reopen + crash-recovery. **Это неверно.**
  > Заземление (`core.rs:154-164`): boot re-attach берёт физический путь из
  > **persisted `path`-поля записи repo** (`record["path"]`), НЕ из имени db.
  > Физическая локация УЖЕ декуплена от логического имени db в каталоге.
  >
  > **Следствие:** RENAME db = **чистый каталог-rekey, структурно как RENAME role**:
  > (1) in-memory `dbs: DashMap` move old→new (DbInstance с открытыми handle —
  > НЕ трогаем, путь в записях repo неизменен); (2) db-registry
  > `save_database(new)`+`remove_database(old)`; (3) rekey ВСЕХ дочерних каталожных
  > строк, несущих `db_name` (repositories/tables/indexes/schemas/retention/
  > buffer-config/access-meta) old→new — `path`-поля НЕ трогаются; (4) на reboot
  > load_databases даёт новое имя, load_repositories даёт repos с `db_name=new` и
  > неизменным `path` → re-attach под новым именем у того же физ-каталога. **НЕТ**
  > fs-move, handle-drain, reopen, crash-window, Windows-lock. «Самый тяжёлый
  > случай» оказался лёгким — предпосылка была ложной.
  >
  > **Точка риска — ПОЛНОТА** (как RENAME role «rekey ссылок в users», но шире):
  > пропуск каталога, несущего `db_name`, осиротит метаданные. Бриф обязан
  > grep-перечислить КАЖДУЮ system_store-таблицу с колонкой `db_name` и rekey-ить
  > каждую; тест — db с repo+table+index+schema+access-meta → rename → ВСЕ
  > переехали + readback под новым именем + durable reboot.
  >
  > **Косметика (не баг):** repo, созданный ПОСЛЕ rename, ляжет в `root/<new>/`
  > (admin_db_repo.rs:182 строит путь из текущего имени), тогда как старые — в
  > `root/<old>/`. Каждая запись repo хранит свой `path` → boot корректен для
  > обоих; одна db в двух физ-каталогах — косметически, не data-loss.
  > Задокументировать; будущая консолидация опц. Скоуп: SYSTEM_DB переименовывать
  > нельзя (как `remove_db` его защищает).

**Точка риска (урок E.4):** каталог-rekey + reverse-index. Для id-keyed реестров
rename — это смена display-name (дёшево); для name-keyed — rekey всех ссылок.
**Тесты:** e2e rename + readback на каждый объект; для name-keyed — проверка, что
ссылки (grants/members/child-paths) переключились.

---

## ②.2 — FK ON UPDATE · M · риск средний (паттерн Phase D готов)

**Заземление.** Phase D ON DELETE полностью реализован: enum
`FkAction{NoAction/Restrict/Cascade/SetNull}` (`admin/types/fk_action.rs`,
snake_case wire, serde-default `NoAction` для backward-compat, builder-default
`Restrict`); `ForeignKeyRef{ref_table, ref_field, on_delete}`
(`validator/schema/foreign_key.rs`) + `new()`/`with_on_delete()`; enforcement —
`query/batch/fk_actions.rs` + `fk_restrict.rs` на delete-пути. ON UPDATE — зеркало
на update-пути.

**Под-этапы:**
- **②.2a wire + DTO.** Добавить `on_update: FkAction` в `ForeignKeyRef` +
  `ForeignKeyDto` — **тот же serde-default-`NoAction` split** (legacy-схемы без
  поля → NoAction, байты не меняются; `skip_serializing_if = is_no_action`).
  `with_on_update()` + комбинированный конструктор. Билдеры:
  Rust `.foreign_key_on_update(...)` (зеркало `.foreign_key_on_delete()`),
  TS `.foreignKeyOnUpdate(...)`.
- **②.2b enforcement.** Хук на UPDATE-пути: детектировать, что UPDATE сменил
  значение **referenced** (parent) поля → фан-аут к зависимым строкам.
  `Restrict` (отказ, если зависимые есть), `Cascade` (**обновить FK-значение
  зависимых на новое** — новый бит против delete-CASCADE), `SetNull` (обнулить
  FK зависимых). Зеркалить `fk_actions.rs`, новый триггер — «referenced value
  changed».
- **②.2c тесты.** e2e + unit на ON UPDATE RESTRICT/CASCADE/SET NULL; back-compat
  (legacy-схема без on_update → NoAction).

**Риск:** referential-корректность; Cascade-propagation нового значения — новая
логика (delete её не имел), покрыть тщательно.

> `ON UPDATE` для составных ключей и циклов FK — если всплывёт, ограничить
> single-field FK (как Phase D) и задокументировать.

---

## ②.3 — E5 unify-uniqueness · M · риск correctness (design-gated)

**Заземление — ДВА активных механизма с разными ролями:**
- **Путь A — validator-probe** (`schema_validator.rs:142`): `if rule.constraints.unique`
  → `db.exists_in_self(field_name, &field_qv, None)` — read-probe self-table,
  с UPDATE-skip-if-unchanged и NULL-bypass, фаерит на tx+autocommit (implicit tx).
- **Путь B — index-level** (`table_manager.rs`): unique-индекс + `unique_write_lock`
  — атомарный write-path guard, **закрывает non-tx↔tx unique-race** («HIGH-A»,
  `table_manager.rs:426-432`).

Это **не дубликат по ошибке** — это два слоя: probe (логическая проверка,
fail-fast, понятная ошибка `unique_violation`) и index-guard (физическая
атомарность, закрытие гонки). G15 называет это «coherence risk»: schema-rule без
индекса vs индекс без rule enforce-ятся по-разному.

**Под-этапы:**
- **②.3a дизайн-нота (оркестратор сам).** Прочитать оба пути целиком, решить
  **единый источник истины**, СОХРАНИВ две гарантии: (1) non-tx↔tx race-closing
  (HIGH-A), (2) autocommit-enforcement + NULL-bypass + UPDATE-skip. Кандидаты:
  - **(A) Делегирование:** schema-rule `unique` формально ТРЕБУЕТ unique-индекс
    (билдер уже fail-closed) и enforce-ится ЧЕРЕЗ него атомарно; validator-probe
    либо снимается, либо демотируется до fast-fail-precheck (индекс — авторитет).
    Чище, один источник. ⚠ Снятие probe не должно ослабить ошибку/гонку.
  - **(B) Defense-in-depth формализованный:** оба слоя остаются, но DDL-time
    инвариант `unique schema-rule ⟺ unique index` + один документированный
    семантический контракт. Ниже риск, но «два пути» остаются.
  Записать решение в этот файл (как ①.4 boundary).

  > **✅ РЕШЕНО (②.3a, оркестратор) — ВАРИАНТ (B), Defense-in-depth формализованный.**
  > Заземление (прочитаны оба пути целиком):
  > - **DDL-инвариант УЖЕ существует.** `validate_unique_indexes`
  >   (`admin_schema.rs:103-150`) при `set_table_schema`/`add_schema_rule`
  >   ОТВЕРГАЕТ `unique` schema-rule без single-field индекса с
  >   `unique_requires_index`. То есть половина G15-«coherence risk» («rule без
  >   индекса») уже закрыта by-construction: unique-rule ⟹ unique-index гарантирован.
  > - **Probe — O(1), не pathological double-cost.** `exists_in_self`
  >   (`validator_db.rs:299-306`) идёт через `find_single_field_index` →
  >   `lookup_by_index` (тот самый индекс, что инвариант требует); full-scan лишь
  >   в редком exclude-случае. Два слоя = два O(1)-обращения, не O(n).
  > - **Слои комплементарны, не дубль-по-ошибке:**
  >   probe (`schema_validator.rs:142`) = логический fail-fast → чистая
  >   field-scoped `unique_violation`, на tx И autocommit, NULL-bypass + UPDATE-skip;
  >   index-guard (`unique_write_lock`, `table_manager_crud.rs:85,184`) = физическая
  >   атомарность, закрытие non-tx↔tx гонки (HIGH-A) + within-batch dedup.
  >
  > **Почему НЕ (A) Делегирование:** снятие probe потеряло бы (1) чистую
  > field-scoped `unique_violation` (index-violation даёт другое, менее точное
  > сообщение `"Unique index '{}' violated"`), (2) пришлось бы переэнфорсить
  > NULL-bypass + UPDATE-skip на index-слое. Выигрыш (один read) мнимый — probe и
  > так O(1) через обязательный индекс. Снятие ослабило бы UX/семантику ради нуля.
  >
  > **Контракт (нормативный, для ②.3b в код):**
  > 1. **DDL-invariant:** `unique` schema-rule ⟹ single-field unique index
  >    (enforced `validate_unique_indexes`). Обратное НЕ требуется: голый
  >    unique-index (через `create_index{unique}`) легитимен — он нижнеуровневый
  >    примитив, enforce-ит физически, отдаёт index-level ошибку; schema-`unique`
  >    — декларативный слой поверх, добавляющий чистый `unique_violation` + DDL-инвариант.
  > 2. **Write-path two-layer:** probe (fail-fast, чистая ошибка, tx+autocommit,
  >    NULL-bypass, UPDATE-skip) НАД index-guard (атомарность HIGH-A). Probe
  >    O(1) через обязательный индекс — не лишняя стоимость, а ранний выход с
  >    лучшей диагностикой.
  > 3. **Единый источник физической истины — индекс.** Probe НЕ авторитет
  >    атомарности (он pre-tx, TOCTOU-окно); авторитет — `unique_write_lock` +
  >    index posting. Probe — диагностика и ранний отказ.
  >
  > **②.3b (импл) = ЛЁГКИЙ, без write-path хирургии:** нормативный doc-контракт в
  > коде (модуль-док `schema_validator` unique-секция + `table_manager`
  > unique_write_lock + перекрёстные ссылки) + coherence-тест, фиксирующий: оба
  > слоя активны; DDL-инвариант держит rule⟹index; голый index без rule
  > enforce-ит; probe O(1). HIGH-A / autocommit / NULL-bypass / UPDATE-skip —
  > СОХРАНЕНЫ (не трогаем гонку). Риск снят: «coherence risk» закрывается
  > документацией + тестом, а не рефактором write-path.
- **②.3b импл** по решению ②.3a. Без двойного enforce / без ослабления гонки.
- **②.3c тесты.** unique сохранён на tx+autocommit; concurrent-insert race-тест
  (две вставки одного значения → ровно одна проходит); NULL-bypass; UPDATE-skip.

**Риск:** самый высокий из не-E2 — трогает write-path race. Поэтому design-pass
первым, отдельным коммитом-нотой.

---

## ②.4 — E2 DEFAULT-значения полей · M-L · **design-gated, кандидат на отдельную мини-кампанию**

**Заземление.** `VALIDATORS.md:154` («Future extensions»): **mutating / transform
validators** (BEFORE-trigger, возвращают модифицированную запись — set defaults,
normalise, compute) — **явно вне MVP**. `VALIDATORS.md:160` — инвариант «replay
must not re-transform». `:225` — «ideally transform-validators for server-side
stamping». То есть E2 зависит от подсистемы, которой ещё нет.

**Под-этапы:**
- **②.4a дизайн-проход (оркестратор сам) — РАЗВИЛКА.**
  - **(A) Полный:** построить mutating/transform-валидаторы (общий фреймворк:
    валидатор возвращает изменённую запись; «replay-must-not-re-transform»
    инвариант; порядок vs CHECK-валидаторы) → DEFAULT + server-side `created_at`
    едут поверх. Это **сам по себе под-проект** → вероятно отдельная мини-кампания.
  - **(B) Узкий:** выделенный DEFAULT-механизм на schema-слое — штамповать
    литерал (и, опц., computed-через-scalar) default на insert ДО валидации, без
    общего mutating-фреймворка. Меньше скоуп, быстрее.
  Решить здесь. Рекомендация: **(B)** для скоупа кампании ②; **(A)** — если
  mutating-валидаторы нужны широко → вынести E2 в отдельную мини-кампанию.

  > **✅ РЕШЕНО (②.4a, оркестратор) — ВАРИАНТ (B), узкий литерал-DEFAULT.**
  > Заземление (прочитаны schema-DTO + validator-pipeline):
  > - **`ConstraintsDto`** (`schema_ops.rs:48-96`) — естественное место для
  >   `default: Option<QueryValue>` рядом с `required`/`unique`/etc (тот же
  >   serde-Option-skip pattern).
  > - **Валидаторы ЧИСТЫЕ** (`table_manager_validators.rs:291-321`
  >   `run_validators_loop`): `validate(new, old, ctx)` берёт `new` по ССЫЛКЕ,
  >   возвращает ошибки — НЕ мутируют запись. DEFAULT требует pre-validation
  >   мутации (штамп до того, как `required` проверит поле).
  > - **КЛЮЧЕВОЙ ИНСАЙТ — DEFAULT тривиально replay-safe** (снимает зависимость
  >   от mutating-фреймворка, которую VALIDATORS.md:160 ставил блокером):
  >   литерал-default штампует ТОЛЬКО отсутствующее поле константой; после первой
  >   записи поле ПРИСУТСТВУЕТ → replay/reload никогда не пере-штампует. В отличие
  >   от `updated_at=now()` (меняется на replay → нужен «replay-must-not-re-
  >   transform» инвариант + общий фреймворк), константный DEFAULT идемпотентен
  >   by-construction. Поэтому (B) НЕ требует mutating/transform-подсистемы.
  >
  > **Почему НЕ (A):** общий mutating/transform-фреймворк (валидатор возвращает
  > изменённую запись, replay-инвариант, порядок vs CHECK) — отдельный под-проект,
  > нужен для computed-stamping (`created_at`/`updated_at`/derive). Константный
  > DEFAULT в нём НЕ нуждается. Строить фреймворк ради литерал-дефолта — over-
  > engineering; (A) остаётся отдельной будущей мини-кампанией для computed-
  > stamping.
  >
  > **Скоуп (B):** `default: Option<QueryValue>` (литерал-константа) в
  > Constraints; на INSERT (не update) ДО валидации — для каждого field-rule с
  > default, отсутствующего во входящей записи, заштамповать литерал; явное
  > значение НЕ перетирать; NULL явный — не отсутствие (не штамповать поверх
  > явного NULL); defaulted-поле затем удовлетворяет `required`. Computed-default
  > (scalar/now()) — ВНЕ скоупа (вынести в (A)-мини-кампанию), т.к. не replay-safe.
  > Под-этапы: **②.4b** surface (DTO/Constraints/builder/TS, аддитивно) →
  > **②.4c** stamp-enforcement (pre-validation на insert-пути) → **②.4d** тесты
  > (default на insert; не перетирает явное; явный NULL не перетёрт; required+default;
  > replay-идемпотентность — повторная загрузка не меняет; default отсутствует →
  > поведение как раньше).
- **②.4b импл** по решению. **②.4c тесты** (default на insert; не перетирает
  явное значение; computed-default если выбрано; replay-идемпотентность если (A)).

**Развилка кампании:** ②.1–②.3 — чистая когерентная кампания DDL-эволюции. **②.4
(E2) — design-gated**; если ②.4a выберет (A), E2 становится отдельной мини-
кампанией (mutating-валидаторы), и кампания ② закрывается на ②.3. Решаем на ②.4a.

---

## Порядок, зависимости, стратегия

```
②.1 RENAME (a→b→c→d)  →  ②.2 FK ON UPDATE (a→b→c)  →  ②.3 E5 (a-design→b→c)  →  ②.4 E2 (a-design→ fork)
   зелёный старт            Phase-D-твин                correctness/race            design-gated, м.б. отдельно
```
- В основном независимы (берутся по порядку риска), кроме внутренних a→b→c.
- **Дизайн-проходы ②.3a и ②.4a — оркестратор делает сам** (correctness/архитектура,
  не механика; прецедент — ①.4 boundary). Импл-этапы — через `/crush`.
- Стратегия: prompt-first (бриф в `docs/prompts/ddl-evolution/` ДО агента, запрет
  git-мутаций) → zero-trust verify (дифф + `./scripts/test.sh` + бенчи в
  изолированном `CARGO_TARGET_DIR`, оркестратор сам) → отдельный чистый коммит.
- Гейт каждого этапа: `./scripts/test.sh` на тронутые крейты + fmt + clippy;
  e2e через release-сервер где нужно (НЕ параллелить rust `--full` с e2e —
  Windows file-lock на `shamir-server.exe`).

## Скоуп-границы (не раздуваем)
- FK ON UPDATE — single-field (как Phase D); составные/циклы — задокументировать, не строить.
- RENAME db — если каскадный rekey тяжёл, отложить (②.1d опционально).
- E2 — по умолчанию узкий (B); полный mutating-фреймворк (A) — отдельная кампания.

## Итог (реализовано)
✅ Кампания выполнена и запушена: **②.1a/b/c** (RENAME folder/group/role),
**②.2a/b** (FK `ON UPDATE`), **②.3a/b** (unify-uniqueness, defense-in-depth),
**②.4a/b/c** (литерал-`DEFAULT`). Развилки решены: ②.4 — узкий (B), в составе ②
(кампания закрылась на ②.4, НЕ на ②.3, т.к. (B) не потребовал mutating-фреймворка).
Коммиты/тесты/развилки — в `DONE.md`. Закрытые action-items: G6/G7/G9/G15
(`ACTION-ITEMS.md`, `completeness-ddl.md`).

**Осталось (осознанно отложено, не блокеры):**
- **②.1d RENAME db** — отдельная мини-таска (on-disk каскад + file-handle-drain +
  crash-atomicity; свой дизайн). Ждёт явной отмашки.
- **(A)-мини-кампания** — mutating/transform-валидаторы для computed-`DEFAULT` /
  server-stamping (`created_at`/`updated_at`). Будущая кампания.
