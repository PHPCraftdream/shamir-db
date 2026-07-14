בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V-fix — потеря второго последовательного tx vector-промоута (#420)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #420:
> найден при #416 ПРЕ-СУЩЕСТВУЮЩИЙ баг — потеря данных на обычном пути.

## Репро (доказано диагностическим тестом при #416)
tx1: insert vector-строки A ([1,0,0]) + commit → промоут A ок, A ищется.
tx2: insert vector-строки B ([0,1,0]) + commit → B появляется в графе, НО
search([0,1,0]) даёт score 0.0 для B — т.е. в граф лёг НЕ ТОТ вектор
(staging был верен: staged_new_vec==[0,1,0] проверялось). Второй
последовательный Phase 5d промоут кладёт искажённый/чужой вектор.
Воспроизводилось на `create_index_v2` (HnswAdapter-путь) в
`crates/shamir-engine/src/tx/tests/` окружении (см. tx_vector_delete_tests.rs
— make_repo/vec_record/poll_vector_hits хелперы; тест «два последовательных
insert-tx» из этих хелперов).

## Задача
1. Напиши регресс-тест `sequential_tx_vector_promotes_both_searchable` (в
   tx_vector_delete_tests.rs или новый файл): tx1 insert A + commit → poll A
   ищется; tx2 insert B + commit → poll B ищется с ВЕРНЫМ вектором (top-hit
   по своему вектору, score ~1.0/дистанция ~0 — с разумным допуском);
   A всё ещё ищется. Убедись, что тест КРАСНЕЕТ на текущем коде.
2. Найди корень. Подозрения (проверь по коду): (а) Phase 5d
   `apply_staged_vectors`/`apply_committed_vectors` — второй промоут против
   устаревшего снапшота/ArcSwap-слота; (б) гонка spawn_blocking инсертов
   двух промоутов; (в) BRUTE_FORCE_MAX-путь: маленький индекс (2 вектора)
   идёт brute-force — возможно `vectors`-map не пополняется вторым промоутом
   (extract/staging кладёт, а brute-force скан читает старый снапшот);
   (г) interner: field-id второй записи резолвится иначе (extract_vec по
   ipath) — вектор извлекается из НЕ ТОГО поля/пустой. Отладь реально — что
   именно лежит в adapter.vectors/graph после второго промоута.
3. Почини КОРЕНЬ (не симптом). Хирургично. Регресс-тест зеленеет.

## Гейт
- Тесты ТОЛЬКО через ./scripts/test.sh. `./scripts/test.sh @vector @engine
  --full` (2+ раз); `cargo clippy -p shamir-index -p shamir-engine
  --all-targets -- -D warnings`; `cargo fmt` тронутых `-- --check`.
- Пиллары. Импорты в шапке. НЕ трогать вне задачи. stray-логи — отметь.

⛔ NEVER run git reset/checkout/clean/stash/restore/rm. Только редактируй;
коммитит оркестратор.

## Финал
Корень (точный механизм), фикс, доказательство красный→зелёный регресс-теста,
вывод гейта.
