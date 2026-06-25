בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Кампания ② — DDL-эволюция & корректность (твин Phase E)

> **Статус:** план сформирован, **ждёт слова о старте**. Преемник `NEXT-CAMPAIGN.md`
> (Phase E) / `PHASE-G-PLAN.md` (Phase G). Источник — остаток `docs/research/`
> после кампании ① (Builder parity). Четыре движковые задачи, одна когерентная
> многоэтапная кампания, commit-per-stage, порядок по возрастанию риска.
> Реализация — через `/crush` (prompt-first → zero-trust → коммит per-stage);
> дизайн-проходы (②.3a, ②.4a) делает оркестратор сам.

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
  > **✅ РЕШЕНО — ОТЛОЖЕНО отдельной мини-таской (Phase ②.1d, оркестратор).**
  > Заземление: db **name-keyed**, и db_name входит в **физический on-disk путь**
  > (`db_dir = root.join(&db_name); path = db_dir.join(&repo)`,
  > `admin_db_repo.rs:182,190`). В отличие от чистых каталожных folder/group/role,
  > RENAME db требует: (1) in-memory `dbs: DashMap<String,DbInstance>` rekey;
  > (2) registry-запись (`save_database`/`remove_database`); (3) каскад ВСЕХ
  > дочерних каталожных строк (repos/tables/indexes/access-meta по db_name);
  > (4) **переименование on-disk каталога `root/<old>`→`root/<new>` с открытыми
  > file-handle** — на Windows нельзя rename каталог с живыми хэндлами стора:
  > нужно quiesce/drain/close всех repo-сторов внутри db → rename → reopen →
  > rebind путей; (5) crash-атомарность (крэш посреди оставляет полу-
  > переименованный каталог + рассинхрон каталога). Это полноценный lifecycle-
  > под-проект с file-handle-draining и crash-recovery, категорически тяжелее
  > трёх предыдущих RENAME (чистый каталог-rekey без физ-связи). Решение в сторону
  > совершенства: **НЕ вкатывать crash-небезопасный half-baked перенос файлов** в
  > кампанию ②; вынести в отдельную мини-таску с собственным дизайном (atomicity,
  > handle-drain, recovery). ②.1 закрывается на folder+group+role (3/4).

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

## Решение (ждёт пользователя)
Старт ②.1 по слову. ②.4 (E2) — решить на ②.4a: в составе ② (узкий) или отдельной
мини-кампанией (полный mutating). Таски заведутся при «делай ②».
