בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# RecordView / MessagePack-pass-through — текущая картина

Живой обзор кампании. Дополняет: `record-view-migration.md` (исходный спек),
`endgame-msgpack-passthrough.md` (north-star wire-pass-through + legacy text elimination),
`r5-deintern-plan.md`, `wave2-*-plan.md`, `wave2-autonomy-decisions.md`.

---

## 1. Суть (north-star)

**Сервер = линза над id-ключевым MessagePack.** Дерево `InnerValue` и legacy text encoding —
прочь с горячих путей; интернинг/де-интернинг на **клиенте**; **storage-байты =
wire-байты** (как получили → провалидировали линзой → записали verbatim; как
считали с диска → отдали). API остаётся имя-based; v1-совместимость сохранена.

**Конечная цель (#61):** ноль `InnerValue` + ноль legacy text encoding во всём проекте.
`InnerValue` выживает ТОЛЬКО за оправданными холодными границами (recovery-кодек,
byte-identity хеш индекса); legacy text encoding — ТОЛЬКО как v1-inbound `QueryRecord::Legacy` +
control-plane (admin/system).

---

## 2. СДЕЛАНО (на master, всё green-gated: clippy --workspace 0 · test.sh · @e2e 543/543)

### Чтение tree-free (ранее)
| Шаг | Коммит | Суть |
|---|---|---|
| R5 | `21569aa` | де-интернинг результата прямо с линзы (O(N²)+дерево прочь) |
| AGG | `aef7968` | прунинг декода агрегаций до референсируемых полей |

### Запись tree-free (W3 эпик) — insert/update/delete/set
| Шаг | Коммит | Суть |
|---|---|---|
| byte-merge | `a432707` | `merge_storage_bytes` (byte-identity 22/22) — фундамент |
| W3a | `5b90799` | удалён мёртвый non-tx CRUD trio + фикс latent update_tx new-field |
| W3b | `beca2f9` | delete_tx через линзу + фикс latent tx-commit posting-cache |
| W3c | `42b5fc4` | update_tx merge через byte-merge |
| W3d | `4033463` | set_tx (upsert) через byte-merge |

### Wire pass-through — ЖИВОЙ end-to-end
| Шаг | Коммит | Суть |
|---|---|---|
| types-W1 | `fcdca96` | `validate_keys_resolve` (security-спина) + `record_view_to_id_msgpack` |
| qt-dto | `8bd81f8` | wire-DTO: records_idmsgpack / QueryRecord::IdBytes / result_encoding |
| S-legacy | `eaae672` | мёртвый legacy text ingest-кодек удалён |
| S-write | `44f4ddb` | сервер принимает id-keyed запись + validate + verbatim |
| S-read | `41f7fb5` | сервер отдаёт id-keyed чтение (SELECT* verbatim / проекция; fallback Name) |
| version-neg | `416a98a` | протокол v2 (сервер анонсирует, клиент пишет) + фикс latent positional-msgpack |
| S-client | `df285dd` | клиент интернит на send + де-интернит на recv → **pass-through ЖИВОЙ** |

### Legacy-text-elimination read-result (#60, идёт)
| Шаг | Коммит | Суть |
|---|---|---|
| #60 A+B+C | `56ad49b` | paginate/distinct/order_by → QueryValue (canonical-key byte-identity) |
| #60 D+E | `8958fd9` | Path B scans + aggregate/HAVING → QueryValue (Direct lazy-legacy cache) |
| #60 F+G | `09eff50` | MIN/MAX/COUNT shortcut + temporal _version → QueryValue → **read-result production legacy-free** |
| W3d-2 (#62) | `0da6873` | non-tx execute_set reroute → implicit-tx → execute_set_tx (tree-merge + dead non-tx changefeed прочь) |

### Supply-chain
| Шаг | Коммит | Суть |
|---|---|---|
| cooldown 30d | `a1bc6ca` | cargo-cooldown + dependabot + CI `cargo cooldown check` + `--locked` |

**Бонусом — 4 реальных латентных прод-бага найдены+починены:** update_tx
new-field result; tx-commit posting-cache stale; skip_serializing_if/positional-
msgpack хендшейк; read-only-batch repo-collect.

---

## 3. ОСТАЛОСЬ

| Приоритет | Задача |
|---|---|
| бэклог | #82 (DX value-API над QueryValue для процедур — после #61); #55 (X-remap холодный); #41 (Stage 6 спекулятивно); #72 (INSERT…RETURNING ResultEncoding::Id); #83 (test.sh .exe fallback) |

**Терминал #61 ДОСТИГНУТ честной формой** (`8649de0`): движение `E1–E6` +
`I1` + `S9b` + `S10`. Финальная серия точечных устранений валюты увела
оставшуюся id-материализацию с горячего, а неустранимый пол **задокументирован**
(`docs/dev-artifacts/perf/innervalue-floor.md`): production-`InnerValue` ~654 (с ~1004) сведён
к 4 категориям — библиотека типа, byte-identity index-якоря, recovery-якоря,
engine owned-value §5b-границы. **De-generify отвергнут** (zero-cost generic).
S9b закрыл V1→V2 разрыв index-hash (rebuild-on-open). Метод закрытия #61 —
**документация прибытия**, не вынужденный ноль.

**#60 ЗАКРЫТ** (`09eff50`): read-result production-путь legacy-free. legacy-twins
(`hashable_legacy` бэкит production canonical-key; `apply_select`/`project`-twin —
bench/example-вызовы) остаются, но НЕ на production-пути. #62 ЗАКРЫТ (`0da6873`).

**Честный предел legacy text encoding:** `QueryRecord::Legacy` variant + `inner_to_legacy_value`/
`legacy_value_to_inner` остаются (v1-inbound + control-plane/computed) — не
удаляются. Достижимо: read-result *production* эмитит только `Direct(QueryValue)`,
ноль legacy `Value` строится для результатов.

---

## 4. Где на дуге

**Кампания завершена честной формой.** Pass-through полный и живой, запись
tree-free на всех путях, read-result production legacy-free, горячие пути
(read/filter/aggregate/stream/temporal) линз-нативны (RecordView/ScalarRef/
bytes-проекция), index V1→V2 авто-мигрирует на открытии, а неустранимый
InnerValue-пол **назван и задокументирован** (`innervalue-floor.md`). Дуга
сошлась: широкое (веер по крейтам) → узкое (точечные устранения) → точка
(прибытие #61). Дальше — за порогом: #82 (DX над устоявшимся QueryValue).

**Метод (весь путь):** design-pass `@aoh` перед крупным/рисковым → byte-identity
golden-тест на каждом персист/wire-шаге → коммит между этапами → агенты пишут
код (без гейтов/git), оркестратор гоняет один авторитетный гейт (**`cargo fmt
--all -- --check`** + clippy --workspace --all-targets + test.sh + @e2e) и
коммитит → контекст оркестратора лёгкий.

⚠️ **Дисциплина (урок ревью 17 июня):** `fmt --check` — ОБЯЗАТЕЛЬНАЯ первая
проверка гейта (агенты не умеют fmt → дрейф копится молча). В кампании он выпал
из гейта на ~30 коммитов → 93 файла дрейфа; восстановлено `style:`-sweep'ом
(`28ffe8a`, в `.git-blame-ignore-revs`). Никогда не писать «Gate green», не
прогнав ВСЕ четыре.
