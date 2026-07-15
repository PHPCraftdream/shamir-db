# OQL Research 02 — Sequencing / dependencies in batches (`after` + `$query`)

Read-only исследование, 2026-07-15. Вопрос: насколько ЯВНЫМ и ПРОСТЫМ является
механизм зависимостей между запросами батча, где грабли, что несогласовано.
(Общий контекст структуры батча — см. `01-nested-batch-recursion.md`; здесь не дублируется.)

---

## 1. Два механизма, один граф

`BatchPlanner::plan` (`crates/shamir-query-types/src/batch/planner.rs:89-161`) строит
ЕДИНЫЙ dependency-граф из двух источников:

1. **Неявный**: рекурсивный обход op'а на `$query`-ссылки —
   `extract_dependencies` (planner.rs:164-212) сканирует `where`-фильтры,
   `set`/`values`/`key` значения, параметры `Call`, `bind` суб-батча.
   Каждый найденный `FilterValue::QueryRef { alias }` / `{"$query": "..."}`
   даёт ребро "этот op зависит от alias".
2. **Явный**: поле `QueryEntry.after: Vec<String>`
   (`crates/shamir-query-types/src/batch/query_entry.rs:46-50`), вливаемое в тот же
   набор deps (planner.rs:120-124):

   ```rust
   // Merge explicit ordering dependencies from `after`.
   for raw in &entry.after {
       let base = Self::extract_base_alias(raw);
       deps.insert(base);
   }
   ```

**Ответ на вопрос 1:** да, `after` может ссылаться на алиас, от которого нет
никакой `$query`-связи — это чистое "исполнить после X, значение не читаю".
Задокументированный кейс — DDL→DML: `insert after create_table`
(query_entry.rs:48, docstring `Batch::after` в
`crates/shamir-query-builder/src/batch/batch.rs:730-731`). После merge оба
механизма неразличимы: `BatchPlan.dependencies` не помнит, какое ребро явное,
какое авто-извлечённое.

Важное следствие merge'а: `execute_plan_impl` передаёт каждому запросу
`resolved_refs` только его ЗАДЕКЛАРИРОВАННЫХ deps
(`batch_execute.rs:293-295`, комментарий "Each query's FilterContext gets only
the resolved_refs from its declared dependencies"). Так как `after`-ребро тоже
попадает в deps, `after`-зависимость дополнительно открывает доступ к
результату X через `$query` — хотя семантически заявлялась только как ordering.

## 2. Параллельность — заявлена, но не реализована

Планировщик группирует независимые запросы в "параллельные" стадии
(topological sort, Kahn, planner.rs:460-523; doc-пример planner.rs:38-43:
"Stage 1 runs `users` and `products` in parallel"). Но исполнитель — честно
задокументированная ложь по отношению к doc'ам планировщика
(`crates/shamir-engine/src/query/batch/batch_execute.rs:254-269`):

> "For each stage, executes all queries **sequentially** within a stage. ...
> `futures::future::try_join_all` was tried and measured as a no-op on
> in-memory CPU-bound workloads ... Real parallelism needs
> `tokio::spawn`-per-query ... kept out of scope for now and tracked as a
> future opt."

Т.е. фактически ВСЁ исполняется последовательно: стадия за стадией, внутри
стадии — в порядке insertion order (tie-break planner.rs:501-503). Порядок
детерминирован, но пользовательская модель "независимые запросы бегут
параллельно" сегодня не соответствует рантайму — а doc-комментарии planner'а
её продолжают обещать.

## 3. Ошибки конфигурации — покрыты хорошо

- **Цикл**: `BatchError::CircularDependency { cycle }` с полным путём цикла —
  planner.rs:139-141, детекция DFS white-gray-black planner.rs:336-389.
  Цикл ловится и для `$query`, и для `after`, и для их смеси (общий граф).
- **Неизвестный алиас**: `BatchError::UnknownAlias { alias, referenced_by }` —
  строгая валидация на этапе плана, planner.rs:126-134 (rationale в
  module-doc planner.rs:13-25: "no way to lie about dependencies").
- **Глубина цепочки**: `BatchError::TooDeep` (planner.rs:144-151).
- **Self-reference**: планировщик отдельно не проверяет — самоссылка проявится
  как `CircularDependency` из одного узла; билдеры (Rust `try_build`
  batch.rs:709-716, TS `tryBuild`) ловят её раньше как `SelfReference` /
  Error.

Диагностика в целом сильная сторона механизма: fail-fast, именованные ошибки.

## 4. Билдеры: Rust vs TS — асимметрия выражения `after`

**Достаточность:** один механизм достаточен — если значение читается через
`$query`, ребро создаётся автоматически и `after` писать НЕ нужно; `after`
нужен только там, где данных не течёт (DDL→DML, side-effect ordering). Это
нигде не сформулировано одной фразой в пользовательских doc'ах билдеров —
выводится из чтения planner.rs.

**Rust** (`shamir-query-builder/src/batch/batch.rs:730-737`):

```rust
/// Declare that `dependent` must execute AFTER `on` (ordering edge).
pub fn after(&mut self, dependent: &Handle, on: &Handle) -> &mut Self {
```

- Отдельный вызов на билдере, НЕ на entry: `b.after(&rows, &mk);` — читается
  как "rows after mk", но оба аргумента одного типа `&Handle`, перепутать
  порядок легко, компилятор не спасёт.
- Не fluent: op регистрируется одним вызовом, ordering — другим, в другой
  строке; зависимость визуально оторвана от запроса.
- Типизировано хендлами (защита от опечаток в алиасе).

**TS** (`shamir-client-ts/src/core/builders/batch.ts:81-103`):

```ts
add(alias, op, opts?: { returnResult?: boolean; after?: string[] })
```

- `after` — сырые строки-алиасы в opts прямо при регистрации op'а
  (плюс к каждому add). Локальнее, чем в Rust, но строки вместо хендлов —
  опечатка ловится только `tryBuild()`/сервером, `build()` не проверяет
  (batch.ts:240: "`build()` itself remains unchecked for backward
  compatibility").
- Симметрии между клиентами нет: Rust — пост-фактум метод с двумя хендлами,
  TS — массив строк в опциях. Один и тот же wire-формат, два разных ментальных
  API.

**`@`/path-нормализация несогласована.** Планировщик и Rust `try_build`
прогоняют `after`-строки через `extract_base_alias` (стрип `@`, срез по
`[`/`.` — planner.rs:121, batch.rs:711), т.е. сервер примет
`after: ["@mk"]` и даже `after: ["mk[0].id"]`. TS `tryBuild` сравнивает
строку буквально (`declared.has(dep)`, batch.ts:258-266) — `"@mk"` там
упадёт. Мелочь, но это ровно та несогласованность, которая всплывает при
переносе примера между клиентами.

## 5. TODO/FIXME

Grep по `TODO|FIXME` в `shamir-query-types/src/batch` и
`shamir-engine/src/query/batch` — попаданий про planner/after нет. Единственный
"tracked as a future opt" — комментарий о параллелизме в
batch_execute.rs:266-269 (без issue-номера, только прозой). Т.е. болевые точки
ниже в коде НЕ зафиксированы как долги.

## 6. Читаемость типичной цепочки

Реальный chained-пример (query_refs_tests.rs:40-45):

```rust
let active = b.query("active", Query::from("users").where_eq("status", "active"));
b.query(
    "first_active",
    Query::from("users").where_eq("name", active.first().field("name")),
);
```

Data-flow цепочка через `Handle::column/first/field` читается ХОРОШО:
зависимость видна прямо в месте использования значения, алиас не
дублируется строкой. Это лучший случай.

Худший случай — смешанная цепочка (e2e `ddl_wire_e2e/error_codes.rs:121`,
`changes_since.rs:216`): op'ы регистрируются подряд, а `b.after(&chmod_h,
&chown_h);` стоит отдельной строкой ниже — чтобы восстановить фактический
порядок исполнения, нужно собрать в голове (a) все `Handle`-использования в
фильтрах/значениях, (b) все `b.after(...)` строки, (c) знание, что всё
остальное — insertion order внутри одной стадии. Для батча из 5+ op'ов это
уже нетривиальное упражнение: порядок исполнения НЕ виден из порядка строк
кода.

## Честная оценка

**Корректность:** механизм работает правильно. Единый граф, строгая
валидация (UnknownAlias / CircularDependency / TooDeep / NestingTooDeep),
детерминированный Kahn с insertion-order tie-break, deps-scoped result
visibility. Багов в семантике при исследовании не найдено.

**Явность:** смешанная. `$query` через `Handle` — это НЕ совсем "spooky
action at a distance", потому что зависимость видна в точке использования
значения; но она видна только тому, кто ЗНАЕТ, что `$query` = ребро DAG'а.
Разработчик, читающий батч как "набор именованных запросов", не увидит
порядка исполнения нигде — ни в структуре кода, ни в типе: порядок — это
эмерджентное свойство (auto-refs ∪ after ∪ insertion order), вычисляемое
планировщиком. `after` явнее, но в Rust он синтаксически оторван от entry.

## Болевые точки (без решений)

1. **Два источника рёбер сливаются без следа** — `BatchPlan.dependencies` не
   различает явное `after` и авто-`$query`; в отладке/ошибках нельзя сказать,
   откуда взялось ребро.
2. **`after` даёт больше, чем обещает**: ordering-ребро заодно открывает
   доступ к результату X через resolved_refs (batch_execute.rs:293-295), хотя
   семантика заявлена как "просто после".
3. **Doc'и планировщика обещают параллелизм стадий, исполнитель всё гоняет
   последовательно** (planner.rs:38-43 vs batch_execute.rs:256-269) —
   пользовательская модель и рантайм расходятся; "future opt" не оформлен
   TODO/issue.
4. **Асимметрия API между клиентами**: Rust — пост-фактум
   `b.after(&dep, &on)` (два одинаковых `&Handle`, порядок аргументов легко
   перепутать), TS — `opts.after: string[]` inline при `add()`.
5. **`@`/path-нормализация `after`-строк несогласована**: сервер и Rust
   `try_build` стрипают `@` и path-хвост, TS `tryBuild` сравнивает буквально.
6. **Сервер молча принимает мусорный path в `after`** (`after: ["mk[0].id"]`
   нормализуется до `mk`) — валидно, но выглядит как ссылка на значение,
   которой нет.
7. **Нет одной фразы в пользовательских doc'ах**: "если используешь `$query` —
   `after` не нужен; `after` только для op'ов без потока данных". Правило
   выводится только чтением planner.rs.
8. **Порядок исполнения невидим в коде билдера**: он складывается из трёх
   источников (auto-refs, after, insertion-order tie-break) и нигде не
   доступен клиенту до исполнения (план не возвращается билдером; на wire
   `execution_plan` отдаётся только в ответе).
9. **TS `build()` не валидирует ничего** (легаси-совместимость) — dangling
   `after`/`$query` уезжает на сервер, если разработчик не знает про
   `tryBuild()`.
10. **Терминологическая коллизия `after`**: в `ReadQuery`/TS `Query` есть
    keyset-pagination `.after(key, limit)` (`shamir-query-builder/src/query/query.rs:199`),
    а в `Batch` — dependency `.after(dep, on)`. Два несвязанных смысла одного
    имени в одном SDK.
