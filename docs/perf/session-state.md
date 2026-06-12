# Session State — где остановились, что делать дальше

**Дата паузы**: 2026-06-12 (сессия 80ac797e).

## Что закрыто

Полный план — `docs/perf/remaining-optimizations-plan.md`.

### Этап A — WAL v3 с интернер-дельтой (ЗАВЕРШЁН ✅)

- `0e772ab` — A1+A2+A4 scaffolding (delta в overlay-merge, WalEntryV2 +
  `interner_delta` поле + bump v1→v2, `Interner::touch_with_id` для
  идемпотентного recovery).
- `cb09dfe` — A3 + A4-recovery (delta плумбится в Phase 4 WAL entry,
  recovery применяет через `touch_with_id` ДО replay ops).
- `32e63e1` — A5 (`interner.persist().await` УБРАН из Phase 1;
  background checkpoint каждые 64 коммита; Phase 7 truncation gated на
  `persisted_high_water` per touched table; graceful shutdown flush;
  tunable `INTERNER_CHECKPOINT_INTERVAL`).

**Эффект**: −1 durable write per commit на критическом пути. Главное —
**снят структурный блокер для Стадии B**.

### Этап C — write_set_keys (ЗАВЕРШЁН ✅)

- `2876ec7` — `StagingStore::keys()`, `TxContext::write_set_keys()`,
  `conflicts_with()`. 5 unit-тестов. `#[allow(dead_code)]` до Стадии D.

## Что в работе / прервано

### Этап B — сжать commit_lock (ПРЕРВАН на этапе чтения)

Агент `ao46l` был остановлен (`TaskStop`) до того, как написал
какой-либо код. Был в фазе чтения `commit.rs`, `pre_commit.rs`,
`materialize.rs`, `repo_tx_gate.rs`.

**Working tree чист** — никаких незавершённых правок Stage B нет.

**Что нужно сделать в этапе B** (выписка из плана):

Сжать `commit_lock` до сиквенсорной тройки:
`{SSI-validate, assign_version, record_footprint, publish}`.

Реорганизация `commit_tx_inner` в три секции:

```
PRE-LOCK (concurrent across txs):
  - Phase 1: interner merge + capture delta + rewrite_set_inner
    (persist убран ещё в A5; touch_ind CAS-safe).
  - Phase 2.5: acquire uwl_guards в sorted order (per-table).
  - Phase 2.6: unique re-validation (info_store.get) — I/O вне lock'а.
  - Phase 5a/5c: materialize (parallel join_all уже из Stage 39).

CRITICAL (commit_lock — микросекунды):
  - Phase 4: WAL begin (пока внутри; Stage D вынесет в batch).
  - Phase 2: SSI validate.
  - Phase 2-bis: phantom validate.
  - Phase 3: assign_next_version.
  - Phase 6: record_commit_writes + publish_committed.

POST-LOCK:
  - Phase 6-bis: SSI log.
  - Phase 7: WAL truncation (gated на persisted_hwm — уже из A5).
```

**Критические инварианты**:
1. SSI ordering: predicate-conflict (Phase 2) + record_writes (Phase 6)
   атомарны под lock'ом — оба ВНУТРИ.
2. WAL write order: пока оставить под lock'ом (Stage D вынесет в group
   commit). Если вынести наружу — committer'ы с failed SSI оставят
   ghost-entries в WAL.
3. Materialize до publish: дать же `uwl_guards` per-table защищают;
   reads фильтруют по published version → не видят данных раньше publish.
   ✓
4. uwl_guards таки берём в sorted order по table token (no deadlock).

**Файлы для правки**:
- `crates/shamir-engine/src/tx/commit.rs` (`commit_tx_inner`)
- `crates/shamir-engine/src/tx/pre_commit.rs` (структурирование под
  pre-lock возможность)
- `crates/shamir-engine/src/tx/materialize.rs` (вызов вне lock)
- возможно `crates/shamir-tx/src/repo_tx_gate.rs` (если надо менять
  API для подачи `uwl_guards` снаружи)

**Бенч-цель**: `wire_pipelining` sync/n_32 и n_128 — ждать значимый win
(сейчас lock держит O(rows + index postings)).

**Риск**: высокий (SSI ordering). Тесты обязательно проходят:
- SSI / phantom (`ssi_phantom_tests/*`)
- concurrent-commit
- recovery / rollback

## Что дальше после B

В порядке плана:

### Этап D — group commit (leader/follower)

Требует A ✅ + B + C ✅. `begin_many` уже есть (commit `f2fb99c`).
Логика leader-follower с pending queue. Bench-цель: `wire_pipelining`
n_32/n_128 — главный ×N.

### Этап E — writev fan-out (независимо)

Подписки. `PushSink::try_push_event` → `(&[u8], &Bytes)`, TCP
`write_vectored`. Независим от A/B/C/D.

### Этап F — format bump v1 (перед релизом)

Один координированный bump: positional msgpack, `sub_id: u64`,
framed WAL codec вместо bincode.

## Точка возобновления

При следующей сессии:

```bash
cd D:\dev\rust\shamir-db
git log --oneline -5   # последний коммит должен быть 2876ec7
git status             # должен быть clean
```

Запустить `ao46l` агента на **Этап B** с брифом из плана.

## Общая сводка по совокупности оптимизаций сессии

В master через несколько вложенных goal-сессий ушло **~35+ коммитов**
из playbook'а:

| Этап | Коммиты | Win |
|---|---|---|
| Insert hot path (#5/8/9/10 + ранние) | много | кумулятивный 1.6× на tx/1 |
| Read hot path (#4/5/6 + ранние) | 6 коммитов | −76% scan_1pct, 1000× FTS+LIMIT, 2× SSI reads |
| Commit pipeline (Stage 23/26/27/31a/39/A1-A5) | 9 коммитов | −18% n_128, foundation для B/D |
| Subscriptions (25/29/30/36/40) | 5 коммитов | n_100 −28% (decode+encode cache) |
| WAL (24/31a) | 2 коммита | 2→1 alloc per entry + begin_many |
| Docs + bench fixtures | 4 коммита | playbook + plan + audit |

Структурное состояние: блокер для group commit снят (A complete);
шаг до главного ×N приза (B → D).
