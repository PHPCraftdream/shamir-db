בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Meta-Review — корпус исследований `docs/research/`

**Рецензент:** Claude Opus (главный цикл), zero-trust сверка с исходным кодом
**своими руками** — не пересказ цитат отчётов, а прямое чтение
`schema_validator.rs`, `access.rs`, `lib.rs`, `filter_enum.rs`, `schema.rs`,
`batch.rs`, прогоны `vitest` как ground-truth для счётчиков тестов.
**Предмет:** шесть документов — пять отчётов агентов + адверсариальное
`REVIEW.md` (crush-агент).

---

## 0. TL;DR

Корпус **достоверен и полезен**. Из шести документов пять — фактически точны
по сути; ошибки сосредоточены в **арифметике сводок и счётчиках**, не в
качественных вердиктах. Адверсариальное `REVIEW.md` поймало эти ошибки
корректно (я перепроверил каждую его находку — все опорные подтвердились), но
само повторило ровно тот грех, за который штрафовало: его заголовочный счётчик
«4 фактические ошибки» не сходится с телом (там 3 ❌-секции).

**Единственная содержательная находка ранга P0** во всём корпусе:
FK/unique молча не срабатывают под autocommit. Я подтвердил её прямым чтением
кода — это **реальная correctness-дыра**, не доковый нюанс.

---

## 1. Что я проверил своими глазами (ground truth)

| Утверждение (источник) | Прямая проверка | Итог |
|---|---|---|
| FK/unique fail-open под autocommit (completeness-ddl §1.2, REVIEW R5) | `schema_validator.rs:106` — `// ctx.db()==None → FK check silently skipped`; `:160-164` — то же для unique. Обе за `if let Some(db)=ctx.db()`. | ✅ **реальный P0** |
| Открытые access-дефолты `0o777`/owner=System (completeness-ddl G10) | `crates/shamir-types/src/access.rs:104` `pub const OPEN: u16 = 0o777`; `:172` «Open default: owner = System, group = None, mode = 0o777» | ✅ реально |
| Rust `FieldBuilder` без `one_of` (coverage-rust #26) | греп по всему `shamir-query-builder/src`: 0 совпадений `one_of`; `schema.rs` имеет `scalar/format/unique/foreign_key`, не `one_of` | ✅ верно |
| TS имеет `oneOf` (coverage-ts #180, coverage-ts-tests) | `builders/ddl.ts:621` `oneOf(values){ this._constraints.one_of = values }` | ✅ верно — **TS превосходит Rust здесь** |
| Rust `Batch` без сеттера `result_encoding` (coverage-rust #37) | `batch/batch.rs:628` хардкод `result_encoding: ResultEncoding::default()`, сеттера нет | ✅ верно |
| «12 folders» → реально 11 (REVIEW против completeness-oql §1.6) | `funclib/src/lib.rs:59-60` — две `in_folder("crypto",…)` подряд (crypto + canonical под crypto). 11 различных имён. | ✅ REVIEW прав, отчёт ошибся |
| `MAX_FILTER_DEPTH = 64` (completeness-oql §1.2) | `filter_enum.rs:9` `pub const MAX_FILTER_DEPTH: usize = 64;` | ✅ точно |
| `it()`-счётчики занижены (REVIEW против coverage-ts-tests) | vitest (мои прогоны): select=28, ddl=75, filter=40, admin=42, e2e=74 — совпадает с таблицей REVIEW; отчёт писал 18/45/30/30/55 | ✅ REVIEW прав |
| Report 1 «10 ❌», не 5/7 (REVIEW) | пересчёт ❌-строк Table 1: #8,26,27,30,46,48,49,50,51,52 = 10 | ✅ REVIEW прав |
| TS делает SCRAM-handshake (REVIEW против completeness-ddl §1.5) | я сам писал `protocol.test.ts` в этой сессии — `runHandshake` = 4 сообщения SCRAM-Argon2id | ✅ парентеза «(no challenge/response)» вводит в заблуждение |

Ни одно опорное утверждение проверки не развалилось. Доверие к корпусу — высокое.

---

## 2. Поабзацная оценка (мои оценки vs crush REVIEW)

| Документ | Оценка crush | Моя оценка | Комментарий |
|---|---|---|---|
| `coverage-rust-query-builder.md` | B+ | **B+** | Согласен. Построчные вердикты точны; сводка врёт в числах (5/7 vs реальные 10 ❌). |
| `coverage-ts-query-builder.md` | A− | **A−** | Самый аккуратный из пяти. Каждый TS-метод подтверждён по file:line. Единственная заусеница — рассинхрон рейтинга `one_of` с Report 1 (разные критерии: wire-поле vs builder-сеттер). |
| `coverage-ts-tests.md` | B | **B** | Качественная картина верна и честна (e2e-гейтинг — ценное наблюдение). Минус — систематический недосчёт `it()` на 15–40%. Реальная картина покрытия **лучше**, чем отчёт рисует. |
| `completeness-oql.md` | A− | **A−** | Лучшая аналитика. Одна фактическая ошибка (12 vs 11 folders). Находка «keyset: engine готов, surface нет» (H3) — самый ценный actionable-инсайт корпуса. |
| `completeness-ddl.md` | A | **A** | Лучший отчёт. FK/unique fail-open — важнейшая correctness-находка. G10 (открытые дефолты) — корректный ship-blocker. Единственный промах — формулировка про SCRAM. |
| `REVIEW.md` (crush) | — | **A−** | Острое, дисциплинированное (claim про DropValidator честно помечен «could not verify»). Но см. §3. |

---

## 3. Где сам ревьюер оступился (мои находки на REVIEW.md)

1. **Ирония — его заголовочный счётчик завышен.** Tally пишет «❌ Factual errors:
   4», но ❌-заголовков в теле ровно **3**: Report 1 (арифметика), Report 3
   (недосчёт `it()`), Report 4 (folders). SCRAM и `one_of` он сам отнёс к ⚠, не
   ❌. То есть REVIEW повторил тот же класс рассогласования «число-в-шапке ≠
   тело», за который заминусовал Report 1. Мелочь, но показательная — рецензент
   не вычитал собственную сводку.
2. **`one_of` он назвал «contradiction», хотя это, по сути, ошибка Report 2.**
   REVIEW аккуратно пишет «оба индивидуально корректны при разных критериях».
   Но Report 2 (#180) пометил **Rust** `one_of` как ✅ — а Rust-сеттера нет
   вовсе. Это не просто «разные критерии рейтинга»: для билдер-паритета (а
   именно его меряет Report 2) Rust-значение должно быть ❌/🟡, и тогда это
   ещё один кейс «TS превосходит Rust», который Report 2 пропустил. REVIEW
   смягчил это до «ambiguity», хотя тянет на фактическую неточность Report 2.
3. **Дисциплина — в плюс.** «DropValidator refuses if bound_in non-empty» помечен
   «could not verify» вместо угадывания. Это правильный zero-trust-рефлекс.

---

## 4. Чего во всём корпусе не хватает

- **Нет приоритизации сквозь все пять отчётов.** Каждый даёт свой gap-list, но
  никто не свёл их в единый ранжированный план «что чинить первым». Это
  закрывает соседний файл `ACTION-ITEMS.md`.
- **Счётчики не из ground-truth.** Отчёт по тестам считал `it()` на глаз вместо
  `vitest run` — отсюда систематический недосчёт. Урок: количественные claim'ы
  надо брать из раннера, не из чтения.
- **«Что есть» местами раздуто, «что делать» — сжато.** Инвентаризации
  возможностей подробны; конкретные следующие шаги (особенно дешёвые победы
  вроде keyset-DTO) тонут в общем объёме.

---

## 5. Вердикт о доверии

**Принять корпус как достоверную базу для планирования**, с тремя поправками,
которые я подтвердил кодом:

1. Реальное число ❌ в `coverage-rust-query-builder.md` — **10** (а не 5/7).
2. `it()`-счётчики в `coverage-ts-tests.md` занижены на 15–40%; покрытие
   фактически **шире** заявленного (но качественные дыры — FTS/vector/call e2e,
   Phase B/C без unit-тестов — реальны и не меняются).
3. `completeness-oql.md`: **11** folders funclib, не 12 (canonical живёт под
   `crypto`).
4. `completeness-ddl.md`: парентезу «(no challenge/response)» читать как
   относящуюся к at-rest хешированию (Argon2id), **не** к протоколу — SCRAM-
   handshake существует (`protocol.ts`/`scram.ts`).

Содержательные выводы корпуса — особенно FK/unique fail-open (P0) и открытые
access-дефолты (ship-blocker) — **подтверждены и должны попасть в трекинг**.
Список реальной работы вынесен в `docs/research/ACTION-ITEMS.md`.
