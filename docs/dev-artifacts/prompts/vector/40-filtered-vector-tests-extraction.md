בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# VR-9 — style: вынести inline-тесты filtered_vector.rs в tests/ (К-1) (#431)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #431.
> Чисто механическая правка стиля, БЕЗ изменения поведения тестов.

## Задача

`crates/shamir-engine/src/table/filtered_vector.rs:130-211` содержит
`#[cfg(test)] mod tests { ... }` ПРЯМО в файле реализации — нарушает
конвенцию репо (CLAUDE.md: "Never embed `#[cfg(test)] mod tests { ... }`
inline inside implementation files. Move them to the `tests/` directory").

1. Создай `crates/shamir-engine/src/table/tests/filtered_vector_tests.rs` —
   перенеси ВСЕ 8 тестов из inline-модуля 1:1 (bare_vector_returns_none,
   and_without_vector_returns_none, and_with_two_vectors_returns_none,
   and_with_one_vector_and_one_pred_extracts,
   and_with_one_vector_and_two_preds_packs_residual_as_and,
   oversample_default_is_two, oversample_clamped_to_min_one,
   oversample_explicit_preserved) + вспомогательные fn `vec_sim()`/`eq()`.
2. Замени `use super::*;` на явные импорты из
   `shamir_engine::table::filtered_vector::*` (или правильный путь модуля —
   грепни, как другие файлы в `tests/` импортируют из `table::`, следуй
   тому же паттерну) + `use shamir_query_types::filter::{Filter, FilterValue};`.
3. Удали inline `#[cfg(test)] mod tests { ... }` из
   `filtered_vector.rs` целиком (строки ~130-211).
4. Добавь `pub mod filtered_vector_tests;` в
   `crates/shamir-engine/src/table/tests/mod.rs` (в алфавитном порядке
   среди существующих `pub mod ..._tests;`).
5. Убедись, что `filtered_vector.rs` объявляет `#[cfg(test)] mod tests;`
   через родительский `mod.rs` (грепни как это сделано для других модулей
   с тестами в `tests/`, например `filtered_ann_tests.rs` уже существует —
   найди, как связан её родительский implementation-файл, повтори паттерн).

## Гейт

- `./scripts/test.sh -p shamir-engine` (lib) 1×, плюс явный фильтр
  `./scripts/test.sh -p shamir-engine -- filtered_vector` чтобы убедиться,
  что все 8 тестов нашлись и прошли под новым путём;
- `cargo clippy -p shamir-engine --all-targets -- -D warnings`;
- `cargo fmt -p shamir-engine -- --check`.

## Дисциплина

Тесты ТОЛЬКО через ./scripts/test.sh. Хирургично: только эти 2 файла +
новый файл тестов + mod.rs манифест. Не меняй логику
`try_extract_filtered_vector_query`/`build_residual`/`resolve_oversample` —
это чистое перемещение файлов, 0 изменений поведения. stray-логи отметь,
не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

Все 8 тестов перенесены 1:1 в `tests/filtered_vector_tests.rs`, inline-модуль
удалён из `filtered_vector.rs`, `tests/mod.rs` обновлён, гейт зелёный (все 8
тестов проходят под новым путём, ни один не потерян/не переименован
случайно). Финал: точный список перенесённых тестов, вывод гейта.
