# WAL Refactor — один файловый движок

## 1. Текущее состояние

Три параллельных WAL-синка для одного протокола: (a) **per-table** `WalManager`
(KV `info_store`, non-tx операции); (b) **repo-level** `RepoWalManager`
(KV `__tx__`, транзакционный путь); (c) **файловый** `WalSegment` +
`WalGroupCommit` — готов, но не подключён. KV-синки дают только уровень 1
(MemBuffer) или уровень 3 (KV flush = write+fsync) — уровень 2 (OS page cache)
недостижим без file-WAL. Это ломает контракт «default = ур.2».

| Синк | Режим | Уровень | Статус |
|---|---|---|---|
| `WalManager` (per-table KV) | non-tx | 1 / 3 | в prod |
| `RepoWalManager` (KV `__tx__`) | tx | 1 / 3 | в prod |
| `WalSegment` + `WalGroupCommit` | — | 2 / 3 | готов, не подключён |

---

## 2. Целевое состояние

Один append-only WAL-файл на уровне репозитория (`wal/NNNNNN.log`).

```
WalSink { File(WalSegment), Noop }
```

`WalGroupCommit` работает поверх `WalSink`. Два тира:

| Тир | Ack-точка | Уровень |
|---|---|---|
| `Buffered` (default) | после `write()` в OS page cache | 2 |
| `Synced` | после `fsync` | 3 |

**InMemory-репо** → durability бессмысленна; file I/O недопустим.
**Транзиторно (F3):** in-memory держит KV-путь (`group=None`), чтобы in-process
recovery-тесты на `list_inflight` остались зелёными; инвариант
`group.is_some()` ⟺ file-режим. **F5-решение:** при удалении KV-стока in-memory
переходит на `WalSink::Noop` (и in-memory recovery-тесты переезжают на disk
tempdir) ЛИБО вводится `WalSink::Mem` (Vec-backed) для in-process replay-паритета.

Все операции (non-tx и tx) эмитируют `WalEntryV2`. V1 выводится из строя;
bulk-insert без WAL-redo — явный компромисс, флажок в точке вызова.

Recovery: `replay()` → применить committed ops идемпотентно (`touch_with_id`
+ ops → data-store). Рваный хвост (крах во время write) — отброс, не ошибка.

До релиза — атомарный перевод. Мост двойного recovery не строится.

---

## 3. Архитектурные решения

1. **Один repo-WAL, не per-table.** Порядок ops глобально монотонен; per-table
   WAL не даёт этого без межфайлового упорядочивания. Recovery тривиален:
   один файл, один проход.

2. **V2 для всех.** V1 retire'd. non-tx путь переводится на полный `WalEntryV2`
   redo. Bulk-insert без WAL — задокументированный trade-off (throughput vs
   crash-recoverability); вызывающий ставит `#[allow(...)]`-аналог комментария.

3. **`WalSink` — enum, не trait.** Нет `dyn` на горячем пути append. Два
   варианта известны статически; `Noop` ветка оптимизируется до noop в release.

4. **Pre-release = атомарный flip.** Новый путь включается одним feature-флагом
   или переименованием; старый удаляется в том же PR. Dual-recovery bridge
   невозможен — он маскирует баги и держит технический долг.

5. **`txn_id` / version-floor — синк-независимы.** Назначение и публикация
   версий не зависят от выбранного синка; инвариант сохраняется при любом
   `WalSink`.

6. **Truncation = checkpoint после durable-materialize.** Сегмент усекается
   только после того, как все записи из него materialized **и** fsync'd в
   data-store (F6, зависит от D2). Преждевременная ротация — потеря данных.

---

## 4. План (F0–F6)

| Этап | Что | Тип | Риск | Gate |
|---|---|---|---|---|
| **F0** | Этот спек-документ | docs | — | — |
| **F1** | `WalSink` enum {`File`,`Noop`} + `WalGroupCommit` поверх него; noop-тесты | additive | низкий | `@storage` |
| **F2** | `RepoWalManager`: файловый сток (File/Noop), `begin_grouped`, `recover`; аддитив, commit не подключён | wire | средний | `@oracle` |
| **F3** | Cutover **tx** + recovery: `commit`/`pre_commit`/`group_commit` → `begin_grouped`; `recovery` → `replay`; tiers Buffered/Synced; фоновый fsync | breaking | высокий | `@oracle @e2e` + crash |
| **F4** | Cutover **non-tx**: `table_manager_crud`/`write_exec` эмитят `WalEntryV2` в repo-WAL; per-table `WalManager` отвязан от write-пути | breaking | высокий | `@e2e` + crash |
| **F5** | Удалить KV-WAL: KV-методы `RepoWalManager` + `WalManager` V1 + V1-кодек + magic-sniff + `info_store_for_test` | cleanup | средний | `cargo check --workspace` + `@oracle @e2e` |
| **F6** | Truncation / ротация сегментов после durable-materialize | additive | средний | crash + рост-лимит |

**Зависимости:** F0 → F1 → F2 → F3 → F4 → F5; F6 — после D2.
**Безопасность:** F3 и F4 — два независимых high-risk cutover'а, НЕ сливать; каждый отдельным проходом с crash-инъекцией.

---

## 5. Что поглощается из старого плана

- **W4 / W5** → F3 (tx commit ladder + recovery replay).
- **W6 / W7** → F4 (non-tx cutover) + F5 (удаление старых синков).
- **D3** (tiers: default/Synced) → растворяется в F3; отдельного этапа нет.
- **D2** остаётся как materialize-батчер; F6 от него зависит.
- **D4** (crash-injection) → единые crash-тесты на F3 / F4 / F6.

---

## 6. Инварианты (STOP при сомнении)

1. WAL — единственный источник истины; data-store, индексы, маркеры —
   производный кэш, восстановимый из WAL.
2. `replay()` идемпотентен; рваный хвост (torn tail) — отброс, не ошибка,
   не потеря подтверждённых данных.
3. Truncation только после durable-materialize в data-store (fsync
   подтверждён).
4. `InMemory` = `WalSink::Noop`; никакого file I/O для in-memory репо.
5. Тесты только через `./scripts/test.sh`; gate = fmt + clippy + `@oracle`
   / `@e2e` на каждом этапе.
