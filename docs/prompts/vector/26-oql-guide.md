בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V6.3 — OQL-поверхность векторных фич + guide 06-search

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #415
> (последний лист V6 кампании, roadmap
> `docs/roadmap/VECTOR_PRODUCTION_EXECUTION.md` §6.3): сверка OQL-форм
> векторных фич + актуализация `docs/guide/06-search.md`.

## Часть 1 — OQL-поверхность (сверка)

1. Найди OQL-парсер/поверхность (см. `docs/prompts/oql-surface/`, grep по
   `oql` в crates). Выясни: есть ли текстовый OQL-синтаксис для
   vector_similarity, и поддерживает ли он новые поля кампании:
   `ef_search`, `oversample` (per-query, #399), `vector_quantization` в
   DDL (#411), filtered ANN (and(vector_similarity, residual)).
2. Если поверхность есть, но новых полей НЕТ — добавь их (парсер +
   парсер-юниты по образцу существующих; тесты через ./scripts/test.sh).
   Если текстового OQL-синтаксиса для векторов нет вовсе — зафиксируй это
   явно в финальном отчёте (что именно есть, чего нет) и НЕ выдумывай новый
   язык — только guide тогда.

## Часть 2 — docs/guide/06-search.md

Актуализировать векторную секцию до полного покрытия кампании:
- per-query `ef_search` + `oversample` (что делают, дефолты, clamp
  MAX_EF_SEARCH, когда крутить);
- filtered ANN: and(vectorSimilarity, предикат), пути post/pre/co-filter и
  cost-based выбор (кратко, со ссылкой на design-доки), stats.index_used
  метки (`index2_ranked` / `filtered_vector_scan`);
- quantization `sq8`: DDL-опция, порог fit 256, dequant-rescore, ~4×
  экономия памяти (цифры из docs/benchmarks/vector/2026-07-05-quantization.md:
  sq8 ≈ 25–44% от f32, recall@10 ≈ 0.978);
- персистентность: снапшот (v2 с QuantMeta) + delta-log + crash-recovery,
  cold-start поведение;
- компакция tombstone: когда срабатывает, что происходит (кратко).
- ВСЕ примеры кода — ТОЛЬКО через билдеры (TS-билдер как в текущем guide;
  Rust-билдер где уместно). Raw JSON в guide ЗАПРЕЩЁН (CLAUDE.md).
- Точные wire/builder-имена бери из кода и свежих тестов:
  `tests/e2e/tests/18-vectors.test.js`,
  `crates/shamir-client-ts/src/__tests__/e2e-vector.test.ts`.

## Гейт

- Если менялся Rust (OQL-парсер): `./scripts/test.sh -p <crate>` +
  `cargo clippy -p <crate> --all-targets -- -D warnings` + fmt тронутых.
- Если менялись TS-билдеры/тесты: npx vitest run в shamir-client-ts.
- Доки: проверь, что примеры синтаксически согласованы с реальными API
  (имена методов из билдеров, не выдуманные).

## Дисциплина

- Менять только: OQL-парсер (если есть и требует), его тесты, guide
  06-search.md. Ничего вне задачи. stray-логи отметь, не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

- Отчёт о состоянии OQL-поверхности (есть/нет, что добавлено, парсер-юниты
  зелёные) + 06-search.md покрывает ef_search/oversample/filtered/
  quantization/персистентность/компакцию, примеры builder-only.
- Финал: список изменённых файлов, состояние OQL, вывод гейта.
