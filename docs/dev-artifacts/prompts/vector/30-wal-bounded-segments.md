בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# fail-hunt — bounded_segment_count_under_append_truncate_loop (#422)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #422:
> `./scripts/test.sh -p shamir-wal --full` — тест
> `bounded_segment_count_under_append_truncate_loop` падает СТАБИЛЬНО
> (3/3 прогона, зафиксировано при #419). Это может быть прод-дефект
> bounded-segment инварианта WAL (сегменты не отсекаются → неограниченный
> рост диска) — вес высокий. Принцип: найти корень, НЕ маскировать.

## Шаги

1. Воспроизведи: `./scripts/test.sh -p shamir-wal --full --
   bounded_segment_count` (лог в файл, читай полный assert-вывод).
2. Пойми инвариант теста (что значит bounded: cap на число сегментов при
   append+truncate лупе) и найди корень: прод-дефект (truncate_below не
   отсекает / сегменты клонируются / off-by-one cap) или дрейф теста
   после недавних изменений WAL (git log -- crates/shamir-wal за
   последние недели — что менялось).
3. Почини корень. Если прод — это критичный фикс (диск-рост);
   если тест-дрейф после интенционального изменения — обнови тест и
   назови коммит, который изменил контракт.
4. Регресс: инвариант должен остаться закреплён детерминированным тестом.

## Гейт

- `./scripts/test.sh -p shamir-wal --full` полностью зелёный, 2 прогона.
- `./scripts/test.sh -p shamir-engine` (лib — потребитель WAL).
- `cargo clippy -p shamir-wal --all-targets -- -D warnings`; fmt тронутых.

## Дисциплина

Тесты только через ./scripts/test.sh; вывод в файл → grep файла.
Хирургично, скоуп: shamir-wal (+ engine только если корень там).
stray-логи отметь, не удаляй. НЕ трогать tests/e2e/** (там работает
другой агент).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

Корень назван с доказательством (assert-вывод до фикса), фикс хирургичен,
гейт зелёный 2×. Финал: корень (прод/тест-дрейф + виновный коммит),
изменённые файлы, вывод гейта.
