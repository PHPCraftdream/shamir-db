בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase E.9 (#249) — unit на Phase B/C constraints (C2) + doc-fixes F1–F5

Кампания **Phase E**, Track C (verification). Независима. Дешёвая страховка
билдер-слоя + гигиена отчётов.

## ⛔ Git-запрет (НАРУШЕНИЕ = катастрофа)
NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, или
любую git-команду, мутирующую дерево/индекс. Только редактируй файлы. НЕ коммить.

## ЧАСТЬ 1 — C2: wire-shape unit-тесты констрейнтов поля
Заземлено (coverage-ts-tests §3.4): констрейнты поля scalar / oneOf / format /
compare / foreignKey / unique покрыты ТОЛЬКО server-gated e2e → в дефолтном
`npx vitest run` (без бинаря сервера) нулевое покрытие билдер-слоя.

СДЕЛАТЬ: добавить wire-shape unit-тесты на эти 6 сеттеров в
`crates/shamir-client-ts/src/core/builders/__tests__/ddl.test.ts` (по образцу уже
существующих в этом файле тестов на другие констрейнты — посмотри, как они
ассертят wire-форму поля). Цель: слой билдера покрыт независимо от сервера.
- Изучи актуальные имена сеттеров в `crates/shamir-client-ts/src/core/builders/`
  (field/constraint builder). НЕ выдумывай — проверь реальный API.
- Если какого-то сеттера реально нет (напр. one_of/B2 ещё открыт — см. F5 ниже),
  НЕ пиши тест на несуществующий API; зафиксируй это в отчёте.

## ЧАСТЬ 2 — doc-fixes F1–F5 (ACTION-ITEMS §F)
ВНИМАНИЕ: часть могла быть уже поправлена при выпиле выполненного (DONE.md) —
СНАЧАЛА прочти текущее состояние каждого файла, правь только если расхождение есть.
- **F1**: `docs/research/coverage-rust-query-builder.md` — «5»/«7» количество →
  привести к одному числу 10 (если ещё не приведено).
- **F2**: `docs/research/coverage-ts-tests.md` — it()-счётчики занижены; взять
  актуальные из `npx vitest run` (общее число тестов) и поправить.
- **F3**: `docs/research/completeness-oql.md` §1.6 — «12 folders» → 11.
- **F4**: `docs/research/completeness-ddl.md` §1.5 — парентеза про SCRAM
  (challenge/response существует) — уточнить формулировку.
- **F5**: `docs/research/coverage-ts-query-builder.md` #180 — Rust `one_of`
  помечен ✅, реально сеттера нет (B2 открыт) — снять ✅ / пометить как gap.

## Дисциплина проекта (ОБЯЗАТЕЛЬНО)
- Запросы — ТОЛЬКО query-builder; никакого raw JSON / as any.
- TS-тесты: `cd crates/shamir-client-ts && npx vitest run src/core/builders/__tests__/ddl.test.ts`.
  Вывод при нужде в файл.
- В тестах JSON-литералы многострочные и с отступами.
- НЕ используй tool под-агентов — пиши/правь сам.
- Платформа Windows, shell bash.

## Файл-сет (параллельно работает другой агент над admin_function.rs — НЕ трогай Rust)
ТОЛЬКО: `crates/shamir-client-ts/src/core/builders/__tests__/ddl.test.ts` +
`docs/research/*.md` (F1–F5). НЕ трогай Rust-крейты, ddl.ts билдер/тип, write.*,
e2e-файлы.

## Гейт
- `npx vitest run src/core/builders/__tests__/ddl.test.ts` зелёный (приложи числа).
- Doc-правки — только текст, проверь что не сломал markdown.

## Что вернуть
(1) изменённые файлы; (2) какие 6 сеттеров покрыл (и какие отсутствуют —
напр. one_of); (3) какие F1–F5 реально правил vs уже-исправлено; (4) вывод
vitest с числами. НЕ КОММИТЬ. Финальный текст — отчёт оркестратору.
