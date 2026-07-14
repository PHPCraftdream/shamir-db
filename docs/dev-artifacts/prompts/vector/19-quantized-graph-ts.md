בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V5.2 Фаза B — quantization в TS-билдере + vitest + parity

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Реализуешь
> ФАЗУ B листа #411 (V5.2). ТОЛЬКО TypeScript-клиент: провести опцию
> `quantization` векторного индекса через TS query-builder + vitest-юниты +
> parity с Rust wire. Фаза A (Rust core) УЖЕ закоммичена — НЕ трогай Rust.

## Контекст (проверенные факты)
- Rust wire-op УЖЕ несёт поле: `crates/shamir-query-types/src/admin/types/index_ops.rs:70`
  `pub vector_quantization: Option<String>` (значение `"sq8"` для SQ8; None =
  без квантизации). Это форма, которую TS должен воспроизвести на проводе.
- TS-билдер: `crates/shamir-client-ts/src/core/builders/ddl.ts` — функция
  создания индекса. Сейчас opts несёт `vector_dim?: number`,
  `vector_metric?: string` (строки ~167-168), и проставляет
  `op.vector_dim`/`op.vector_metric` (строки ~190-192). Тип `CreateIndexOp`
  (найди его определение — вероятно в `../types/…`) несёт эти поля.
- vitest-юниты билдера: `crates/shamir-client-ts/src/core/builders/__tests__/ddl.test.ts`.
- parity-тест: `crates/shamir-client-ts/src/core/builders/__tests__/vector_filter_parity.test.ts`
  (образец сверки TS-формы с Rust wire-формой).

## Задача
1. **`CreateIndexOp` TS-тип**: добавь `vector_quantization?: string` (рядом с
   `vector_metric`). Значение — строка метода квантизации (`"sq8"`).
2. **`ddl.ts` builder**: добавь в `opts` поле `vector_quantization?: string`;
   в теле — `if (opts?.vector_quantization !== undefined)
   op.vector_quantization = opts.vector_quantization;` (по образцу
   vector_metric). Строго opt-in: не задано → поле в op отсутствует (wire
   back-compat, как Rust `#[serde(default/skip_serializing_if)]`).
3. **vitest-юнит** (`ddl.test.ts`): создание vector-индекса с
   `quantization: "sq8"` кладёт `vector_quantization: "sq8"` в op; без опции —
   поле отсутствует (undefined), не `null`. Тест значений metric+dim+quantization
   вместе.
4. **parity**: в `vector_filter_parity.test.ts` (или рядом) добавь сверку, что
   TS-op с `vector_quantization: "sq8"` сериализуется в форму, совпадающую с
   Rust wire-контрактом (поле `vector_quantization` строкой). Если в проекте
   есть механизм генерации/сверки wire-схемы Rust↔TS — используй его; иначе —
   явная проверка ключа+значения по документированному контракту (см.
   index_ops.rs). НЕ хардкодь сырой JSON запроса помимо parity-round-trip
   (правило CLAUDE.md — но parity-тесты серилизации это разрешённое исключение).

## Дисциплина + гейт
- Гейт TS: из `crates/shamir-client-ts/` запусти vitest на тронутых сьютах
  (`npx vitest run src/core/builders/__tests__/ddl.test.ts
  src/core/builders/__tests__/vector_filter_parity.test.ts` или проектный
  скрипт — найди в package.json `test`/`vitest`). ВСЕ зелёные. Также
  typecheck (`npx tsc --noEmit` или проектный `build`/`typecheck` скрипт),
  чтобы CreateIndexOp-изменение типа не сломало компиляцию.
- Rust НЕ трогай (Фаза A закоммичена). `dist/`-артефакты не редактируй руками
  (это билд-выход); меняй только `src/`.
- Если правишь генерируемые типы — правь ИСТОЧНИК, не dist.
- stray-файлы/логи — ОТМЕТЬ, НЕ удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done
- `CreateIndexOp` + `ddl.ts` opts несут `vector_quantization?: string`
  (opt-in, отсутствует когда не задано); vitest-юнит + parity зелёные;
  typecheck чист.
- Финал: тронутые файлы (только src/, TS), вывод vitest тронутых сьютов,
  вывод typecheck, подтверждение opt-in (без опции — поле отсутствует),
  что это закрывает Фазу B и #411 целиком.
