בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Post-wave bench delta — HEAD vs `cdb4120` (Фаза 0 baseline)

**Статус документа:** PHASE-1 CLOSED. Ack-path bench instrument compromised
после нескольких сессий — переход на Фазу 2 (backend-matrix drain бенч).

---

## 1. Что сделано

### Фаза 0 — диагностика (closed)

**0.2 Бисект L13:** revert только L13 показал что L13 виновник **только**
small-batch регрессии (`batch/1_no_result` 456→406µs, −4% within noise).
Indexed-регрессия (+27% на /100) с L13 reverted **сохранилась** — значит
причина indexed-регрессии не в L13.

**0.3 Диагностический бисект (агент #120):** выявил что indexed-регрессия —
**death by a thousand cuts** across L2 + L6 + L12, каждый ~2–5%. Также
важнейшее методологическое открытие: исходный `+27%` был **частично
session load offset** — тот же prewave baseline в свежей сессии измерился
13.78ms вместо 12.93ms (+6.6%). После нормализации регрессия меньше:
`indexed/100` ≈ +5.6%, `indexed/1000` ≈ +17.8% (но /1000 с ±50% noise).

### Фаза 1 — исцеление (closed)

**1.1 L12 fix (`c720e2a`):** `Bytes::from(std::mem::take(scratch))` вместо
`Bytes::copy_from_slice(&scratch)`. Zero-copy handoff (Bytes забирает
Vec'овский heap). Эффективно возвращает prewave alloc pattern.

**1.2 L13 fix (`38d2dc2`):** новый `from_ts_seq(ts, seq)` — один clock-read
(выигрыш L13 hoist'а сохранён) + ascending seq в старших байтах tail'а
(intra-batch монотонность восстановлена). 16-байтный layout неизменен.
`from_ts(ts)` оставлен для single-row путей с полным 8B random tail.

**1.3 L6 fast-path (`494d130`):** `set_versioned_many_append_only` — caller
ГАРАНТИРУЕТ fresh keys, скипает per-row `current_version(key)` lookup
(~50-100ns × N saved на scc::HashMap hash-miss). Caller — `insert_many`
(non-tx batch путь, генерирует fresh RecordId перед вызовом). Передаёт
`old_v=0` в vacuum_key — корректно во всех retention modes.

**1.4 L2 — не трогаем.** Diagnostic-agent recommended ts-keys inlining,
но +2-4% не оправдывают риск durability bug (ts-keys нужны для vacuum/
age-retention/анализа).

Все три фикса прошли гейт: **1386/1386 @oracle PASS**.

### Что НЕ закрыто этим документом

L1 (~30× drain win) **не доказан числами**. `tx_pipeline` использует
`InMemoryRepo` — drain async, off the critical path, его эффект НЕВИДИМ
в этом бенче. Это пробел покрытия, не отсутствие выигрыша.

L3, L14+L5, L6 fast-path — gate-correctness PASS, но perf-доказательство
требует других бенчей (`fts_indexed` для L3, dedicated read для L14+L5).

---

## 2. Bench instrument compromised — почему уходим от ack-path

Серия из 3 контролируемых замеров с одним и тем же baseline `prewave` на
одной системе показала **wildly inconsistent results**:

| Метрика | Замер #1 (initial) | Замер #2 (L13 reverted) | Замер #3 (Phase 1 done) |
|---|---|---|---|
| `single_insert/non_tx` | ~3.24 ms (no change) | not measured | **62 µs (-98.7%!)** |
| `single_insert/tx_staged` | 249 µs (no change) | not measured | 289 µs (+18%) |
| `indexed/non_tx/100` | 16.38 ms (+27%) | 16.66 ms (+29%) | 20.52 ms (+59%) |
| `indexed/non_tx/1000` | 154.7 ms (+26%) | 150.1 ms (+23%) | 180.2 ms (+47%) |
| `non_tx/1_no_result` | 456 µs (+13%) | 406 µs (-4%) | 453 µs (+9%) |

Проблема — НЕ session noise (хотя и он есть). Проблема **systematic**:

1. **`single_insert/non_tx` показал -98.7%** между замерами #1 и #3 без
   кода-изменений на этом пути. Вероятная причина: L9 has_any_index guard
   снимает на single-insert/unindexed путь огромный планнерный overhead.
   Но почему это **проявилось только в третьем замере** — непонятно.
   Что-то изменилось в caching/runtime, не в исходном коде.

2. **`indexed/non_tx/100` дрейфует от +27% к +59%** при том что код не
   менялся (это всё на одном HEAD). Без 2× воспроизводимости regression
   attribution невозможна.

3. **bench design issue (известное):** `indexed_repo` шарится между
   итерациями `iter_custom`. Таблица монотонно растёт — это не steady-
   state measurement, и любые growth-related регрессии будут
   ампифицироваться непредсказуемо.

**Вывод:** Дальнейшие micro-fixes ack-path не имеют смысла, пока этот
инструмент даёт ±50% noise. Time to invest в правильный бенч, а не
тушить пожары в неправильном.

---

## 3. Стратегический разворот — Фаза 2 как главная

Принцип, заземлённый созерцанием: **«max perf + concurrency, WAL owns
reliability»**. Под этот принцип:
- backend в архитектуре — производное состояние за WAL+overlay.
- единственный писатель — single-owner drainer (L1 батч-форма).
- конкурентность для нас = lock-free reads, append-only writes.

Это указывает прямо на **log-structured / LSM** backend (fjall — fav.
кандидат, уже за единым Store-трейтом).

**Фаза 2 — backend-matrix durable-drain бенч** делает три вещи одной
работой:

1. **Доказывает L1** на durable backend (~30× обещанный выигрыш
   наконец проверяется на правильной оси).
2. **Эмпирически выбирает backend** — redb vs fjall (vs опц. sled)
   под наш drain-профиль.
3. **Измеряет ось, на которой архитектура выигрывает по построению** —
   а не side-line ack-path µs где мы случайно теряем 5-10% на каждый
   nice-to-have рывок.

---

## 4. Что НЕ делаем (и почему)

- **НЕ ищем больше виновников в ack-path.** Diffuse cumulative + bench
  noise = ROI≈0.
- **НЕ ревертим L12/L13/L6 fixes.** Они correctness-positive, гейт
  зелёный, и архитектурно правильные (zero-copy handoff, intra-batch
  монотонность, skip useless work).
- **НЕ переделываем L2** (ts-keys inlining). +2-4% не оправдывают риск
  durability bug в vacuum/retention.
- **НЕ повторяем ack-path замер для confidence** — три замера дали три
  разных картины, четвертый ничего не даст.

---

## 5. Команды воспроизведения

```sh
# Pre-wave worktree:
git worktree add --detach .cargo-prewave-tree cdb4120

# Save prewave baseline (already saved):
cd .cargo-prewave-tree && BENCH_FULL=1 \
  CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench-delta \
  cargo bench -p shamir-engine --bench tx_pipeline -- \
  --save-baseline prewave 'tx_overhead/(single_insert|batch_pipeline)'

# Compare HEAD vs prewave:
cd .. && BENCH_FULL=1 \
  CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench-delta \
  cargo bench -p shamir-engine --bench tx_pipeline -- \
  --baseline prewave 'tx_overhead/(single_insert|batch_pipeline)'
```

⚠️ Эти команды дают **wildly inconsistent results** между сессиями.
Не использовать как regression-gate.

---

## 6. Следующий шаг

Фаза 2 — построить `drain_throughput` бенч parametrized по backend
({redb, fjall} ×{8, 32, 128 конкурентных коммиттеров}, durability снята
с backend'а, наш WAL единственный owner). См. таск #117.
