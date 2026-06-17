בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# RecordView / MessagePack-pass-through — текущая картина

Живой обзор кампании. Дополняет: `record-view-migration.md` (исходный спек),
`endgame-msgpack-passthrough.md` (north-star wire-pass-through + JSON),
`r5-deintern-plan.md`, `wave2-*-plan.md`, `wave2-autonomy-decisions.md`.

---

## 1. Суть (north-star)

**Сервер = линза над id-ключевым MessagePack.** Дерево `InnerValue` и JSON —
прочь с горячих путей; интернинг/де-интернинг на **клиенте**; **storage-байты =
wire-байты** (как получили → провалидировали линзой → записали verbatim; как
считали с диска → отдали). API остаётся имя-based; v1-совместимость сохранена.

**Конечная цель (#61):** ноль `InnerValue` + ноль JSON во всём проекте.
`InnerValue` выживает ТОЛЬКО за оправданными холодными границами (recovery-кодек,
byte-identity хеш индекса); JSON — ТОЛЬКО как v1-inbound `QueryRecord::Json` +
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
| S-json | `eaae672` | мёртвый JSON ingest-кодек удалён |
| S-write | `44f4ddb` | сервер принимает id-keyed запись + validate + verbatim |
| S-read | `41f7fb5` | сервер отдаёт id-keyed чтение (SELECT* verbatim / проекция; fallback Name) |
| version-neg | `416a98a` | протокол v2 (сервер анонсирует, клиент пишет) + фикс latent positional-msgpack |
| S-client | `df285dd` | клиент интернит на send + де-интернит на recv → **pass-through ЖИВОЙ** |

### JSON-elimination read-result (#60, идёт)
| Шаг | Коммит | Суть |
|---|---|---|
| #60 A+B+C | `56ad49b` | paginate/distinct/order_by → QueryValue (canonical-key byte-identity) |
| #60 D+E | `8958fd9` | Path B scans + aggregate/HAVING → QueryValue (Direct lazy-json cache) |

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
| сейчас | **#60 F+G** — MIN/MAX/count shortcut-строки + удалить `hashable_json`/`apply_select`(json)/`project`-twin → read-result production-путь **json-free** |
| далее | **3 холодных якоря InnerValue** (терминал): recovery/doctor-кодек; funclib (почти не-стена — осталась обёртка inner_to_json_value, в осн. мигрирована); **index-hash leaf** (самый трудный — persisted byte-identity, нужна discriminant-стабильная схема ИЛИ index-format rebuild-миграция, ИЛИ принятый предел) |
| хвост | #62 (non-tx execute_set детри/reroute), #55 (X-remap холодный), #41 (Stage 6 спекулятивно) |

**Честный предел JSON:** `QueryRecord::Json` variant + `inner_to_json_value`/
`json_value_to_inner` остаются (v1-inbound + control-plane/computed) — не
удаляются. Достижимо: read-result *production* эмитит только `Direct(QueryValue)`,
ноль `json::Value` строится для результатов.

---

## 4. Где на дуге

**Сердце кампании — сделано и доказано:** pass-through полный и живой, запись
tree-free на всех путях, read-result почти json-free. Остаток — **долизывание**
(#60 F+G — мелко) и **холодные якоря** (funclib/recovery — средне; index-hash —
трудно/возможный принятый предел).

**Метод (весь путь):** design-pass `@aoh` перед крупным/рисковым → byte-identity
golden-тест на каждом персист/wire-шаге → коммит между этапами → агенты пишут
код (без гейтов/git), оркестратор гоняет один авторитетный гейт (clippy
--workspace + test.sh + @e2e) и коммитит → контекст оркестратора лёгкий.
