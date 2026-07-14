בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# VR-10 — валидация мульти-vector-index на таблицу (П-3) + докнота fit-порога (П-4) (#432)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #432.

## П-3: два vector-индекса на таблицу ломают промоут

Факты ревью: `staged_vectors` в TxContext ключуются token'ом ТАБЛИЦЫ (не
индекса); `promote_vectors`/`apply_vector_batch`
(`crates/shamir-engine/src/tx/commit_phases.rs`) гонят один и тот же батч
векторов через ВСЕ vector-backends таблицы. Два vector-индекса с разными
dim → DimMismatch и провал промоута. Нигде не задокументировано и не
провалидировано на DDL.

Задача (минимум, полная поддержка мульти-индексов ВНЕ скоупа):
1. Проверь фактическое поведение: создай в тесте таблицу с двумя
   vector-индексами (разные поля/dim) — что происходит на insert/commit?
2. DDL-валидация: `create_index` с `index_type: vector`, когда на таблице
   УЖЕ есть vector-индекс → явная понятная ошибка (подбери существующий
   код ошибки — index_exists/unsupported/invalid_config — по образцу
   соседних валидаций в admin/DDL-обработчике сервера/engine; найди где
   валидируются index-опции, напр. vector_dim). Ошибка должна доходить до
   клиента с внятным сообщением.
3. Тесты: Rust-юнит на валидацию (красный до фикса: второй индекс
   принимался) + негативный e2e-пункт НЕ нужен, достаточно Rust-уровня.
4. Зафиксируй ограничение: (а) заметка в `docs/guide-docs/guide/06-search.md`
   («одна таблица — один vector-индекс; снятие ограничения — backlog»),
   (б) строка в `docs/BACKLOG.md` (формат таблицы соблюдай) про полную
   поддержку мульти-vector-index.

## П-4: докнота fit-порога

В `docs/guide-docs/guide/06-search.md` (секция SQ8): квантайзер обучается ОДИН раз на
первых 256 векторах (FIT_THRESHOLD) и не переобучается при дрейфе
распределения; практическое следствие и когда это ок. 2-4 предложения,
builder-only примеры не нужны.

## Гейт

- `./scripts/test.sh -p shamir-engine` (+ крейт, где валидация — напр.
  `-p shamir-index`/`-p shamir-server`, по месту фикса) 1×;
- `cargo clippy -p <тронутые> --all-targets -- -D warnings`; fmt тронутых.

## Дисциплина

Тесты только через ./scripts/test.sh. Хирургично. НЕ трогать
`crates/shamir-index/src/vector/hnsw_adapter.rs` (там работает другой
агент). stray-логи отметь, не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

Второй vector-индекс на таблицу отвергается DDL с внятной ошибкой,
регресс-тест зелёный, guide+BACKLOG обновлены, докнота fit-порога на месте,
гейт зелёный. Финал: где валидация, код ошибки, вывод гейта.
