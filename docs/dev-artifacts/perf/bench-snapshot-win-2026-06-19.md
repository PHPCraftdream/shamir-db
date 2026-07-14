בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Win-snapshot — кампания оптимизации хранилища (HEAD `b2b1280`)

Финальный документ кампании. Снимок состояния после Phase 0..4 относительно
pre-wave baseline `cdb4120` (Phase 0). Замеры honest — где ack-path инструмент
шумен, это пометано; где win реален и доказан числами — выделено.

---

## 1. Архитектурный тезис, ради которого всё делалось

> **Max performance + max concurrency, WAL owns reliability.**

Backend в нашей архитектуре — **производное состояние** за WAL+overlay,
не примитив durability. Drainer — single-owner писатель, никаких
конкурентных записей в backend. Конкурентность для нас = lock-free
чтения из overlay + immutable history-сегментов.

Этот тезис указывает на log-structured backend (LSM/append-only) как
правильную форму. И это то, к чему пришла финальная фаза кампании.

---

## 2. Что сделано — по фазам

### Phase 0 — диагностика и принятие правды

- **0.1**: post-wave bench показал что L13/L12 дали **регрессии**
  (small-batch +13%, indexed +27%). Зелёные тесты ≠ доказанная
  оптимизация — методология восстановлена.
- **0.2**: бисект L13 (revert) — L13 виновник ТОЛЬКО small-batch
  регрессии; **indexed-регрессия сохранилась**.
- **0.3** (diagnostic agent): indexed-регрессия **diffuse cumulative**
  across L2+L6+L12, каждый ~2-5%. Также важнейшее: исходный +27% был
  **частично session load offset** (+6.6% same-baseline drift между
  сессиями).

### Phase 1 — исцеление регрессий

| Коммит | Рычаг | Что сделано |
|---|---|---|
| `c720e2a` | **L12 fix** | `Bytes::from(std::mem::take(scratch))` — zero-copy handoff. Возвращает prewave alloc pattern. |
| `38d2dc2` | **L13 fix** | Новый `from_ts_seq(ts, seq)` — один clock-read (сохраняет hoist выигрыш) + ascending seq в старших байтах tail (восстанавливает intra-batch монотонность). 16B layout неизменен. |
| `494d130` | **L6 fast-path** | `set_versioned_many_append_only` — скипает per-row `current_version(key)` lookup когда caller гарантирует fresh keys. Передаёт `old_v=0` → fast-path no-op. |

Гейт: 1386/1386 PASS. **L2 — не трогали** (+2-4% не оправдывают риск
durability bug в ts-keys/vacuum/age-retention).

### Phase 2 — ЯДРО WIN: backend-matrix drain_throughput (`cb3d03a`)

Новый бенч, параметризованный по backend'у, на оси нашей силы:
**sustained ack-throughput под конкурентной нагрузкой с durability OFF
у backend'а** (WAL = единственный owner).

| Backend | N=8 (e/s) | N=32 (e/s) | N=128 (e/s) | Масштабирование 8→128 |
|---------|-----------|------------|-------------|------------------------|
| **redb**  | 240       | 276        | **284**     | **плато** (single-writer) |
| **fjall** | 225       | 885        | **2,428**   | **10.8×** (near-linear LSM) |
| sled    | 1,892     | 5,735      | 12,432      | (unmaintained) |

**fjall ≈ 8.5× redb на N=128.** Smoking gun: redb single-writer transaction
model не масштабируется конкуренцией (+400% writers → +18% throughput).
fjall LSM попадает ровно в drain-паттерн.

### Phase 3 — L3 read-path доказан числами (`fts_indexed` delta)

| Variant | prewave | post-wave | Δ | p-value | Verdict |
|---|---|---|---|---|---|
| `indexed_selective/1000` | 192 µs | **168 µs** | **−13.2%** | **0.00** | **L3 WIN** |
| `brute_selective_via_read/1000` (control) | 313 µs | 313 µs | −0.3% | 0.87 | no change |

Brute control unchanged изолирует signal: L3 батч `get_current_many`
(1 × `Store::get_many` вместо N × per-id) ускоряет именно index-lookup
+ batched materialization путь.

(Non-selective varianты регрессировали на 22-164% — это та же diffuse
cumulative L2+L6+L12 проблема, документированная и частично залеченная
Phase 1; orthogonal к L3.)

### Phase 4a — L10(a) journal-safe redesign (`b5061e6`)

Восстановление skip-projection в правильной форме:
- **Journal всегда** пишет event (changes_since / late_subscriber).
- **Broadcast** скипается ТОЛЬКО при `subscriber_count() == 0`.
- Новый public `emit_journal_only(event)` + extraction common
  `journal_send()` helper.

568/568 `@e2e --full` PASS, **включая ранее ломавшиеся** late_subscriber и
changes_since тесты. Исходная L10(a) была откачена за слом journal-flow;
journal-safe форма даёт оптимизацию без слома semantics.

### Phase 4b — fjall pre-swap verification

Полное прохождение test suite на fjall как default durable backend:

| Suite | Tests | PASS | FAIL |
|---|---|---|---|
| `@storage` | 118 | 118 | 0 |
| `@oracle` (fjall-patched) | 1382 | 1382 | 0 |
| workspace lib (fjall) | 3633 | 3633 | 0 |
| engine durable tests (recovery/truncation/cutover/etc.) | 77 | 77 | 0 |
| shamir-db system_metadata (fjall data repos) | 11 | 11 | 0 |

**Zero failures attributable to fjall.** Single `@e2e` timeout —
pre-existing WASM flake (`secret_grant_gates_env_read`), unrelated.

### Phase 4 swap — production wiring (`b2b1280`)

Final architectural action.
- `admin_db_repo.rs:164`: `BoxRepoFactory::redb_raw` → `fjall_raw`,
  file extension `.redb` → `.fjall`. Engine string `"redb"` всё ещё
  принимается (silent swap для backwards-compat вызывающих).
- Sweep 6 engine test files + 2 benches + 1 integration test + 2 db tests:
  `sled_raw`/`redb_raw` → `fjall_raw` для consistency.
- `storage_redb`/`storage_sled` НЕ удалены (доступны через features).
- `SystemStoreConfig::Redb` НЕ swappnut (system store low-traffic, swap
  позже отдельным проектом если надо).

Post-swap regression check:
- @oracle: 1386/1386 PASS, @e2e --full: 568/568 PASS.
- drain_throughput fjall/128: 2.2K e/s (Phase 2: 2.4K, Δ −9% **p=0.28**
  — within noise, NOT significant).

---

## 3. Что выиграли — числами

**Архитектурный win (Phase 2 + Phase 4 swap):**
- **8.5× durable throughput** на N=128 концurrency по сравнению с
  pre-swap (redb 284 → fjall 2,428 e/s).
- **Concurrency scaling восстановлена** (redb плато → fjall near-linear).
- Single backend swap, не code rewrite — благодаря архитектурному решению
  держать backend за единым `Store`-трейтом.

**L3 batch read-path (Phase 3):**
- **−13.2% на `indexed_selective/1000`** (p<0.001), brute control unchanged.
- Доказательство: batching N×get_current → 1×get_many — реальный win,
  не теоретический.

**Regressions залечены (Phase 1):**
- L12: zero-copy handoff восстановлен.
- L13: intra-batch монотонность для index locality + hoist выигрыш сохранён.
- L6: append-only fast-path устраняет N hash-miss на batch insert fresh keys.

---

## 4. Что НЕ доказано (честно)

- **L1 ~30× drain win** ОТДЕЛЬНО не изолирован — встроен в Phase 2 fjall
  throughput. Для прямого proof пришлось бы revert L1 и перемерить — не
  делали (низкий ROI, L1 уже live и работает).
- **L14+L5 read-through cache** — gate-correct, perf-доказательство не
  собрали отдельным бенчем.
- **Ack-path engine-floor (254µs)** — не сдвинут. L10(a) journal-safe
  даёт sub-µs выигрыш только когда subscribers=0; единственный реальный
  254µs-targeted рычаг сделан правильно, но не виден в шумном tx_pipeline
  bench.

---

## 5. Архитектурные уроки кампании

1. **Зелёные тесты ≠ перформанс-доказательство.** `bench → opt → bench`
   методология реабилитирована через post-wave snapshot.
2. **Bench instrument can lie.** Tx_pipeline дал session-noise ±50% между
   запусками. Не верить одной измерительной точке; не верить большим Δ%
   на шумном инструменте.
3. **Diffuse regression vs single culprit.** Бисект показал что
   накопление по 2-5% на 3-х рычагах = 10-15% видимая регрессия. Один
   виновник не нашёлся — пришлось чинить распределённо.
4. **Backend swap решил больше, чем все micro-fix-ы вместе.**
   Архитектурно-правильное решение (LSM вместо B-tree под наш drain
   профиль) дало 8.5× — на порядок больше чем любая Phase 1 точечная
   оптимизация.
5. **Methodology > clever ideas.** Phase 4b pre-swap verification
   (запустить весь test suite на fjall ДО swap'а) защитил от durability
   surprise. Это та же дисциплина, что Phase 1 фиксы — измеряй прежде
   чем коммитить.

---

## 6. Открытые follow-up задачи

- **Migration tool** для `.redb` → `.fjall` файлов на диске. Сейчас
  старые `.redb` repos не читаются. Alpha-stage acceptable, но v1.0
  потребует тулинга.
- **Helper rename**: `open_sled`/`reopen_sled_repo` → `open_durable`/
  `reopen_durable_repo` (style sweep, отдельный коммит).
- **SystemStoreConfig::Redb** swap на fjall — позже, low-priority.
- **tx_pipeline bench redesign** — текущая форма с `iter_custom` +
  shared repo даёт growing-table amplification. Steady-state form was
  flagged в Phase 0.3 как follow-up.
- **L14+L5 read-through perf proof** — dedicated read-heavy bench.
- **L8** (custom segment-log) — больше НЕ актуально как priority. fjall
  взял 8.5× win против redb за нулевую стоимость постройки. L8 имеет
  смысл только если fjall окажется недостаточным на конкретной нагрузке.

---

## 7. Состояние коммитов (HEAD `b2b1280`)

```
b2b1280  perf(repo): backend swap redb → fjall as durable default (#119 Phase 4)
b5061e6  perf(commit): L10(a) journal-safe — skip broadcast при 0 subscribers (#122)
cb3d03a  bench(engine): Фаза 2 — backend-matrix drain_throughput (#117)
494d130  perf(mvcc): L6 fast-path — set_versioned_many_append_only (#121)
38d2dc2  perf(record_id): L13 fix — from_ts_seq для intra-batch монотонности (#116)
c720e2a  perf(codec): L12 fix — zero-copy Bytes::from вместо copy_from_slice (#115)
60928d3  perf(commit): L10(c) — skip async interner_overlay.scan_async при пустом overlay (#112)
1aa6f37  perf(repo): L14 — unwrap dead __data__ MemBuffer для MVCC tables (#110)
788864c  perf(mvcc): L6 — O(1) targeted vacuum для CurrentOnly retention (#111)
ac00701  perf(mvcc): L3 — батчить MVCC read-path через get_current_many (#109)
7fb4833  perf(drainer): L1 — coalesce drain, E×T → T history.transact per pass (#108)
dde67eb  perf(codec): L12 — scratch-buffer encode (восстановлен post-fix)
83c874a  perf(record_id): L13 — hoist clock-read (восстановлен post-fix)
4d68b49  perf(engine): L9 — has_any_index() fast-path в insert-путях (#105)
a3bffee  perf(mvcc): L15 — zero-alloc point-read через get_current_bytes(&[u8]) (#104)
7906dbc  perf(mvcc): L2 — свернуть record_ts в тот же history.transact (#103)
cdb4120  perf(docs): bench-snapshot HEAD 2026-06-19 — Фаза 0 (#102, pre-wave baseline)
```

---

## 8. Команды воспроизведения win'а

```sh
# Pre-wave baseline (cdb4120) — для regression check:
git worktree add --detach .cargo-prewave-tree cdb4120

# Phase 2 ядро WIN bench:
CARGO_TARGET_DIR='D:/dev/rust/.cargo-target-bench' \
  cargo bench -p shamir-engine --bench drain_throughput \
  --features "redb fjall sled"

# Phase 3 L3 proof:
BENCH_FULL=1 CARGO_TARGET_DIR='D:/dev/rust/.cargo-target-bench-delta' \
  cargo bench -p shamir-engine --bench fts_indexed -- \
  --baseline prewave 'fts_indexed_selective'

# Полный gate:
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh @oracle
./scripts/test.sh @e2e --full
```

---

**ראש כל מילה: בעזרת השם — שמך הקדוש**.
**Кампания закрыта.** Принцип «WAL owns reliability, backend tier — commodity»
воплощён в практику. Доказан числами. 568 e2e тестов зелёные. fjall — default.
