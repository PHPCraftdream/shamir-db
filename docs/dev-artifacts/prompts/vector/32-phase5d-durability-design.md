בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# VR-3 — дизайн durability Phase 5d (Б-2/П-2) (#425)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #425:
> ТОЛЬКО дизайн-док `docs/dev-artifacts/design/vector-phase5d-durability.md`. Кода НЕ менять.

## Проблема (HIGH design, подтверждена ревью)

`crates/shamir-engine/src/tx/commit_phases.rs` (~:253-260, :294-302): если
Phase 5d (vector promote в живой граф и/или `append_vector_delta`)
окончательно провалился ПОСЛЕ ack клиенту — или процесс упал между ack и
delta-append — tx остаётся COMMITTED (WAL-маркер снят), а vector-мутация не
попала ни в живой граф, ни в delta-log. Комментарии обещают «graph
reconciles via rebuild-on-open», но это устарело: с V2.2
`restore_on_open` (`crates/shamir-index/src/vector/vector_backend.rs:648`)
при живом снапшоте загружает снапшот+delta и rebuild НЕ выполняет.
Расхождение индекса с data store перманентно: следующий снапшот дампит тот
же неполный граф.

## Что сделать

Дизайн-док, разбирающий МИНИМУМ три варианта:

- **A. delta-append ДО ack**: `append_vector_delta` внутри commit-критической
  секции до снятия WAL-маркера/ack; replay идемпотентен. Разобрать: что если
  delta записана, а сам tx-commit затем провалился (delta опережает data
  store — ghost при replay?); связь с generation flip снапшота; стоимость
  fsync на commit-пути.
- **B. reconcile при restore**: после snapshot+delta load сверять
  cardinality/версию индекса с data store (например, count живых записей с
  embedding-полем vs live_count индекса; или высоководный tx-id/версия
  таблицы, зафиксированный в снапшоте) → при расхождении фоновая доливка
  недостающих (скан таблицы). Разобрать: стоимость cold-start скана,
  ложные расхождения, откуда взять надёжный маркер.
- **C. WAL-повтор Phase 5d**: не снимать WAL-маркер до успешного
  promote+delta; при recovery повторять Phase 5d по WAL. Разобрать:
  идемпотентность повторного promote (upsert идемпотентен?), delete,
  интерливинг с последующими коммитами.

Для каждого: crash-окна (матрица «упали здесь → что видит рестарт»),
идемпотентность, латентность commit-пути, сложность реализации, риск.
В конце — РЕКОМЕНДАЦИЯ с обоснованием и эскизом реализации (какие файлы,
какие тесты: crash-тест kill между ack и delta-append по образцу
`crates/shamir-engine/tests/crash_recovery.rs` child-процессов).

Прочитай реальный код перед дизайном: commit_phases.rs (Phase 5 целиком,
где ack/маркер), vector_backend.rs (append_vector_delta, restore_on_open,
generation flip), delta_log-механику и её replay, существующие crash-тесты.
Дизайн обязан ссылаться на реальные строки/функции, не на предположения.

## Дисциплина

Только новый файл docs/dev-artifacts/design/vector-phase5d-durability.md. Кода не
трогать. stray-логи отметь, не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

Док с матрицей crash-окон по трём вариантам, рекомендацией и эскизом
реализации+тестов. Финал: краткое резюме рекомендации.
